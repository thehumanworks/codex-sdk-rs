//! Managed `codex app-server --listen ws://...` process lifecycle: probing,
//! starting (daemon or owned/blocking child), readiness, and shutdown.
//!
//! All process and port management for websocket servers lives here; the
//! `client` module only re-exports [`WsServerHandle`] and [`WsStartMode`].

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::net::Ipv4Addr;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Error as WsError;
use url::{Host, Url};

use crate::client::{ClientOptions, WsConfig, WsStartConfig};
use crate::error::ClientError;

const DAEMON_LOG_DIR_NAME: &str = "codex-app-server-sdk";
const MAX_DAEMON_LOG_BYTES: u64 = 10 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_INTERVAL: Duration = Duration::from_millis(150);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SHUTDOWN_POLL_ATTEMPTS: u32 = 20;

/// Per-target startup locks, keyed by the normalized `(host, port)` listen
/// target so startups against different servers do not serialize each other.
static STARTUP_LOCKS: OnceLock<StdMutex<HashMap<(String, u16), Arc<Mutex<()>>>>> = OnceLock::new();

fn startup_lock(target: &WsTarget) -> Arc<Mutex<()>> {
    let locks = STARTUP_LOCKS.get_or_init(Default::default);
    let mut locks = locks.lock().expect("startup lock map poisoned");
    locks.entry(target.lock_key()).or_default().clone()
}

/// A validated `ws://host:port` endpoint: the single parse and single
/// host-formatting path for everything in this module (listen URLs, log file
/// names, startup-lock keys).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WsTarget {
    host: Host<String>,
    port: u16,
}

impl WsTarget {
    /// Parses an explicitly configured websocket URL. Requires the `ws`
    /// scheme, a host, and an explicit port. `label` names the offending
    /// config field in error messages (e.g. `listen_url`).
    fn parse(label: &str, url: &str) -> Result<Self, ClientError> {
        let parsed = Url::parse(url)
            .map_err(|err| ClientError::Config(format!("invalid websocket {label}: {err}")))?;
        if parsed.scheme() != "ws" {
            return Err(ClientError::Config(format!(
                "websocket {label} must use the `ws` scheme: `{url}`"
            )));
        }
        let host = parsed
            .host()
            .ok_or_else(|| {
                ClientError::Config(format!(
                    "invalid websocket {label}: missing host in `{url}`"
                ))
            })?
            .to_owned();
        let port = parsed.port().ok_or_else(|| {
            ClientError::Config(format!(
                "websocket {label} must include explicit port: `{url}`"
            ))
        })?;
        Ok(Self { host, port })
    }

    fn is_loopback(&self) -> bool {
        match &self.host {
            Host::Ipv4(ip) => ip.is_loopback(),
            Host::Ipv6(ip) => ip.is_loopback(),
            Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        }
    }

    /// The host as it appears in a URL authority (IPv6 bracketed).
    fn host_authority(&self) -> String {
        match &self.host {
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => format!("[{ip}]"),
            Host::Domain(domain) => domain.clone(),
        }
    }

    fn url(&self) -> String {
        format!("ws://{}:{}", self.host_authority(), self.port)
    }

    /// `localhost` normalized to `127.0.0.1`; every other host unchanged.
    fn normalized(&self) -> Self {
        match &self.host {
            Host::Domain(domain) if domain.eq_ignore_ascii_case("localhost") => Self {
                host: Host::Ipv4(Ipv4Addr::LOCALHOST),
                port: self.port,
            },
            _ => self.clone(),
        }
    }

    /// Log file for a daemon serving this target, derived from the host as
    /// written (no `localhost` normalization) with non-alphanumeric
    /// characters replaced by `_`.
    fn log_path(&self) -> PathBuf {
        let host_label = match &self.host {
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => ip.to_string(),
            Host::Domain(domain) => domain.to_lowercase(),
        };
        let safe_host_label: String = host_label
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect();

        daemon_log_dir().join(format!("app-server-{safe_host_label}-{}.log", self.port))
    }

    fn lock_key(&self) -> (String, u16) {
        let normalized = self.normalized();
        (normalized.host_authority().to_lowercase(), normalized.port)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsStartMode {
    Daemon,
    Blocking,
}

#[derive(Debug)]
pub struct WsServerHandle {
    listen_url: String,
    connect_url: String,
    mode: WsStartMode,
    reused_existing: bool,
    log_path: Option<PathBuf>,
    process_group_id: Option<u32>,
    child: Option<Child>,
}

impl WsServerHandle {
    pub fn listen_url(&self) -> &str {
        &self.listen_url
    }

    pub fn connect_url(&self) -> &str {
        &self.connect_url
    }

    pub fn mode(&self) -> WsStartMode {
        self.mode
    }

    pub fn reused_existing(&self) -> bool {
        self.reused_existing
    }

    pub fn started_new_process(&self) -> bool {
        !self.reused_existing
    }

    pub fn owns_process(&self) -> bool {
        self.child.is_some()
    }

    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    pub fn connect_config(&self, options: ClientOptions) -> WsConfig {
        WsConfig::new(self.connect_url.clone(), options)
    }

    /// Terminates an owned server process and waits (async) until the server
    /// no longer answers websocket handshakes on the connect URL.
    ///
    /// Process-group termination (SIGTERM, escalating to SIGKILL) is
    /// unix-only. On Windows shutdown is best-effort `child.kill()` of the
    /// direct child; grandchildren are not terminated.
    pub async fn shutdown(&mut self) -> Result<(), ClientError> {
        let process_group_id = self.process_group_id.take();
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };

        if let Some(process_group_id) = process_group_id {
            let _ = terminate_process_group(process_group_id);
        }
        #[cfg(not(unix))]
        let _ = child.kill();

        let mut child_exited = false;
        for attempt in 0..SHUTDOWN_POLL_ATTEMPTS {
            if !child_exited && child.try_wait()?.is_some() {
                child_exited = true;
            }
            if child_exited && self.connect_target_released().await {
                return Ok(());
            }
            if attempt == 5
                && let Some(process_group_id) = process_group_id
            {
                let _ = terminate_process_group(process_group_id);
            }
            tokio::time::sleep(SHUTDOWN_POLL_INTERVAL).await;
        }

        if let Some(process_group_id) = process_group_id {
            let _ = kill_process_group(process_group_id);
        }
        if !child_exited {
            let _ = child.kill();
            let _ = child.wait()?;
        }

        for _ in 0..SHUTDOWN_POLL_ATTEMPTS {
            if self.connect_target_released().await {
                return Ok(());
            }
            tokio::time::sleep(SHUTDOWN_POLL_INTERVAL).await;
        }

        Err(ClientError::Startup {
            message: format!(
                "websocket app-server did not release `{}` during shutdown",
                self.connect_url
            ),
            log_path: self.log_path.clone(),
        })
    }

    /// True once nothing answers a websocket handshake on the connect URL —
    /// the same liveness predicate used by startup ([`probe_app_server`]).
    /// A probe protocol error means something other than our server answered,
    /// so the target counts as released.
    async fn connect_target_released(&self) -> bool {
        !matches!(
            probe_app_server(&self.connect_url, None).await,
            Ok(ProbeState::Reachable)
        )
    }

    pub(crate) fn from_reused_existing(
        listen_url: String,
        connect_url: String,
        mode: WsStartMode,
        log_path: Option<PathBuf>,
    ) -> Self {
        Self {
            listen_url,
            connect_url,
            mode,
            reused_existing: true,
            log_path,
            process_group_id: None,
            child: None,
        }
    }

    pub(crate) fn daemon_started(
        listen_url: String,
        connect_url: String,
        log_path: PathBuf,
    ) -> Self {
        Self {
            listen_url,
            connect_url,
            mode: WsStartMode::Daemon,
            reused_existing: false,
            log_path: Some(log_path),
            process_group_id: None,
            child: None,
        }
    }

    pub(crate) fn blocking_started(listen_url: String, connect_url: String, child: Child) -> Self {
        let process_group_id = Some(child.id());
        Self {
            listen_url,
            connect_url,
            mode: WsStartMode::Blocking,
            reused_existing: false,
            log_path: None,
            process_group_id,
            child: Some(child),
        }
    }
}

impl Drop for WsServerHandle {
    /// Best-effort only: SIGTERM the process group (unix) / kill the child
    /// (elsewhere) and reap it if it has already exited. Never sleeps or
    /// waits — call [`WsServerHandle::shutdown`] for a confirmed shutdown.
    fn drop(&mut self) {
        if let Some(process_group_id) = self.process_group_id.take() {
            let _ = terminate_process_group(process_group_id);
        }
        if let Some(mut child) = self.child.take() {
            #[cfg(not(unix))]
            let _ = child.kill();
            let _ = child.try_wait();
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
fn signal_process_group(process_group_id: u32, signal: i32) -> std::io::Result<()> {
    let process_group_id = i32::try_from(process_group_id)
        .map_err(|_| std::io::Error::other("process group id is too large"))?;
    let result = unsafe { kill(-process_group_id, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group_id: u32) -> std::io::Result<()> {
    signal_process_group(process_group_id, 15)
}

#[cfg(unix)]
fn kill_process_group(process_group_id: u32) -> std::io::Result<()> {
    signal_process_group(process_group_id, 9)
}

#[cfg(not(unix))]
fn terminate_process_group(_process_group_id: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn kill_process_group(_process_group_id: u32) -> std::io::Result<()> {
    Ok(())
}

#[derive(Debug, Clone)]
struct WsStartTarget {
    connect_url: String,
    listen_url: String,
    /// Parsed listen endpoint (startup-lock key).
    listen: WsTarget,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    Reachable,
    Unavailable,
}

pub async fn ensure_local_ws_app_server(
    url: &str,
    env: &HashMap<String, String>,
    auth_token: Option<&str>,
) -> Result<(), ClientError> {
    let Some(target) = parse_managed_ws_target(url)? else {
        return Ok(());
    };

    let _handle =
        start_ws_server_internal(target, env, true, WsStartMode::Daemon, auth_token).await?;
    Ok(())
}

pub async fn start_ws_server(
    config: &WsStartConfig,
    mode: WsStartMode,
) -> Result<WsServerHandle, ClientError> {
    let target = build_start_target(config)?;
    start_ws_server_internal(target, &config.env, config.reuse_existing, mode, None).await
}

async fn start_ws_server_internal(
    target: WsStartTarget,
    env: &HashMap<String, String>,
    reuse_existing: bool,
    mode: WsStartMode,
    auth_token: Option<&str>,
) -> Result<WsServerHandle, ClientError> {
    if let Some(handle) = existing_server_handle(&target, reuse_existing, mode, auth_token).await? {
        return Ok(handle);
    }

    let lock = startup_lock(&target.listen);
    let _guard = lock.lock().await;

    if let Some(handle) = existing_server_handle(&target, reuse_existing, mode, auth_token).await? {
        return Ok(handle);
    }

    match mode {
        WsStartMode::Daemon => {
            spawn_daemon(&target, env).await?;
            wait_for_ready(&target, None, auth_token).await?;
            Ok(WsServerHandle::daemon_started(
                target.listen_url,
                target.connect_url,
                target.log_path,
            ))
        }
        WsStartMode::Blocking => {
            let mut child = spawn_owned_process(&target, env)?;
            wait_for_ready(&target, Some(&mut child), auth_token).await?;
            Ok(WsServerHandle::blocking_started(
                target.listen_url,
                target.connect_url,
                child,
            ))
        }
    }
}

async fn existing_server_handle(
    target: &WsStartTarget,
    reuse_existing: bool,
    mode: WsStartMode,
    auth_token: Option<&str>,
) -> Result<Option<WsServerHandle>, ClientError> {
    match probe_app_server(&target.connect_url, auth_token).await {
        Ok(ProbeState::Reachable) => {
            if reuse_existing {
                Ok(Some(WsServerHandle::from_reused_existing(
                    target.listen_url.clone(),
                    target.connect_url.clone(),
                    mode,
                    Some(target.log_path.clone()),
                )))
            } else {
                Err(ClientError::Startup {
                    message: format!(
                        "websocket app-server already running at `{}` and reuse_existing is disabled",
                        target.connect_url
                    ),
                    log_path: Some(target.log_path.clone()),
                })
            }
        }
        Ok(ProbeState::Unavailable) => Ok(None),
        Err(probe_failure) => Err(ClientError::Startup {
            message: format!(
                "websocket startup conflict at `{}`: {probe_failure}",
                target.connect_url
            ),
            log_path: Some(target.log_path.clone()),
        }),
    }
}

/// Classifies `url` for `start_and_connect_ws`: `Some(target)` when the URL
/// is a loopback `ws://` endpoint the SDK manages (may auto-start a daemon),
/// `None` when it is connect-only (`wss://`, non-loopback), and an error when
/// it cannot be a managed target at all (unparseable, missing host/port).
fn parse_managed_ws_target(url: &str) -> Result<Option<WsStartTarget>, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::Config(format!("invalid websocket URL: {err}")))?;

    if parsed.scheme() != "ws" {
        return Ok(None);
    }

    let host = parsed
        .host()
        .ok_or_else(|| {
            ClientError::Config(format!("invalid websocket URL: missing host in `{url}`"))
        })?
        .to_owned();

    let mut connect = WsTarget { host, port: 0 };
    if !connect.is_loopback() {
        return Ok(None);
    }

    connect.port = parsed.port().ok_or_else(|| {
        ClientError::Config(format!(
            "loopback websocket URL must include explicit port: `{url}`"
        ))
    })?;

    let listen = connect.normalized();

    Ok(Some(WsStartTarget {
        connect_url: url.to_string(),
        listen_url: listen.url(),
        log_path: connect.log_path(),
        listen,
    }))
}

fn build_start_target(config: &WsStartConfig) -> Result<WsStartTarget, ClientError> {
    let listen = WsTarget::parse("listen_url", &config.listen_url)?;
    WsTarget::parse("connect_url", &config.connect_url)?;

    Ok(WsStartTarget {
        listen_url: config.listen_url.clone(),
        connect_url: config.connect_url.clone(),
        log_path: listen.log_path(),
        listen,
    })
}

fn daemon_log_dir() -> PathBuf {
    std::env::temp_dir().join(DAEMON_LOG_DIR_NAME)
}

fn prepare_daemon_log_dir() -> std::io::Result<PathBuf> {
    let directory = daemon_log_dir();
    if let Ok(metadata) = fs::symlink_metadata(&directory) {
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other(format!(
                "refusing symlinked app-server log directory `{}`",
                directory.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(std::io::Error::other(format!(
                "app-server log path is not a directory: `{}`",
                directory.display()
            )));
        }
    } else {
        fs::create_dir_all(&directory)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

fn open_daemon_log(path: &Path) -> std::io::Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(std::io::Error::other(format!(
            "refusing symlinked app-server log file `{}`",
            path.display()
        )));
    }
    let rotate = path
        .metadata()
        .map(|metadata| metadata.len() >= MAX_DAEMON_LOG_BYTES)
        .unwrap_or(false);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(!rotate)
        .truncate(rotate)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// The single liveness predicate for websocket app-servers: attempts a real
/// websocket handshake. `Ok(Reachable)` when a server accepted the handshake,
/// `Ok(Unavailable)` when nothing (or nothing yet) is listening, and
/// `Err(description)` when the port is occupied by something that is not a
/// websocket app-server.
async fn probe_app_server(url: &str, auth_token: Option<&str>) -> Result<ProbeState, String> {
    let request =
        crate::transport::ws::build_ws_request(url, auth_token).map_err(|err| err.to_string())?;
    let connect = tokio_tungstenite::connect_async(request);
    match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
        Ok(Ok((mut stream, _))) => {
            let _ = stream.close(None).await;
            Ok(ProbeState::Reachable)
        }
        Ok(Err(err)) => classify_probe_error(url, err),
        Err(_) => Ok(ProbeState::Unavailable),
    }
}

fn classify_probe_error(url: &str, err: WsError) -> Result<ProbeState, String> {
    match err {
        WsError::Io(io_err) if is_retryable_connect_error(io_err.kind()) => {
            Ok(ProbeState::Unavailable)
        }
        WsError::ConnectionClosed | WsError::AlreadyClosed => Ok(ProbeState::Unavailable),
        WsError::Http(response) => Err(format!(
            "websocket probe failed for `{url}`: unexpected HTTP status {}",
            response.status()
        )),
        other => Err(format!("websocket probe failed for `{url}`: {other}")),
    }
}

fn is_retryable_connect_error(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::ConnectionRefused
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::NotConnected
            | ErrorKind::TimedOut
            | ErrorKind::AddrNotAvailable
            | ErrorKind::BrokenPipe
    )
}

/// Spawns the detached daemon via `spawn_blocking` (the spawn itself does
/// blocking filesystem work). Uses `std::process::Command`, not
/// `tokio::process`, deliberately: the daemon must outlive us and must not be
/// reaped by the runtime.
async fn spawn_daemon(
    target: &WsStartTarget,
    env: &HashMap<String, String>,
) -> Result<(), ClientError> {
    let target_for_task = target.clone();
    let env_for_task = env.clone();
    let spawn_result =
        tokio::task::spawn_blocking(move || spawn_daemon_process(&target_for_task, &env_for_task))
            .await;

    let startup_error = |detail: String| ClientError::Startup {
        message: format!(
            "failed to start websocket app-server daemon for `{}`: {detail}",
            target.connect_url
        ),
        log_path: Some(target.log_path.clone()),
    };

    match spawn_result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(startup_error(err.to_string())),
        Err(join_error) => Err(startup_error(join_error.to_string())),
    }
}

fn spawn_daemon_process(
    target: &WsStartTarget,
    env: &HashMap<String, String>,
) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let _ = (target, env);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "managed websocket daemons are not supported on Windows; use stdio or connect to an explicitly managed app-server URL",
        ));
    }

    #[cfg(not(windows))]
    {
        prepare_daemon_log_dir()?;

        let log_file = open_daemon_log(&target.log_path)?;
        let log_file_stderr = log_file.try_clone()?;

        let mut command = Command::new(codex_binary(env));
        command
            .arg("app-server")
            .arg("--listen")
            .arg(&target.listen_url)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_file_stderr));

        for (key, value) in env {
            command.env(key, value);
        }

        let _child = command.spawn()?;

        Ok(())
    }
}

fn spawn_owned_process(
    target: &WsStartTarget,
    env: &HashMap<String, String>,
) -> std::io::Result<Child> {
    let mut command = Command::new(codex_binary(env));
    #[cfg(unix)]
    command.process_group(0);
    command
        .arg("app-server")
        .arg("--listen")
        .arg(&target.listen_url)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for (key, value) in env {
        command.env(key, value);
    }

    command.spawn()
}

fn codex_binary(env: &HashMap<String, String>) -> String {
    env.get("CODEX_BINARY")
        .filter(|value| !value.is_empty())
        .cloned()
        .or_else(|| {
            std::env::var("CODEX_BINARY")
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "codex".to_string())
}

async fn wait_for_ready(
    target: &WsStartTarget,
    mut child: Option<&mut Child>,
    auth_token: Option<&str>,
) -> Result<(), ClientError> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
        if let Some(child) = child.as_deref_mut()
            && let Some(status) = child.try_wait()?
        {
            return Err(ClientError::Startup {
                message: format!(
                    "websocket app-server for `{}` exited before becoming ready with status {status}",
                    target.connect_url
                ),
                log_path: None,
            });
        }

        match probe_app_server(&target.connect_url, auth_token).await {
            Ok(ProbeState::Reachable) => return Ok(()),
            Ok(ProbeState::Unavailable) => {}
            Err(probe_failure) => {
                return Err(ClientError::Startup {
                    message: format!(
                        "websocket app-server readiness probe failed for `{}`: {probe_failure}",
                        target.connect_url
                    ),
                    log_path: Some(target.log_path.clone()),
                });
            }
        }

        if Instant::now() >= deadline {
            return Err(ClientError::Startup {
                message: format!(
                    "timed out waiting for websocket app-server at `{}`",
                    target.connect_url
                ),
                log_path: Some(target.log_path.clone()),
            });
        }

        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_ws_urls_are_managed() {
        let ipv4 = parse_managed_ws_target("ws://127.0.0.1:4222")
            .expect("url should parse")
            .expect("loopback IPv4 should be managed");
        assert_eq!(ipv4.listen_url, "ws://127.0.0.1:4222");

        let ipv6 = parse_managed_ws_target("ws://[::1]:4222")
            .expect("url should parse")
            .expect("loopback IPv6 should be managed");
        assert_eq!(ipv6.listen_url, "ws://[::1]:4222");
    }

    #[test]
    fn remote_urls_are_connect_only() {
        let remote = parse_managed_ws_target("ws://203.0.113.10:4222").expect("url should parse");
        assert!(
            remote.is_none(),
            "non-loopback ws URL should not be managed"
        );

        let secure = parse_managed_ws_target("wss://127.0.0.1:4222").expect("url should parse");
        assert!(secure.is_none(), "wss URL should not be managed");
    }

    #[test]
    fn localhost_listen_url_is_normalized() {
        let target = parse_managed_ws_target("ws://localhost:4555")
            .expect("url should parse")
            .expect("localhost should be managed");

        assert_eq!(target.listen_url, "ws://127.0.0.1:4555");
        assert_eq!(
            target.log_path,
            std::env::temp_dir()
                .join("codex-app-server-sdk")
                .join("app-server-localhost-4555.log")
        );
    }

    #[test]
    fn invalid_and_missing_port_urls_are_rejected() {
        let missing_port =
            parse_managed_ws_target("ws://127.0.0.1").expect_err("missing port should be rejected");
        assert!(
            matches!(&missing_port, ClientError::Config(message) if message.contains("explicit port")),
            "unexpected error: {missing_port:?}"
        );

        let invalid = parse_managed_ws_target("not-a-url").expect_err("invalid URL should fail");
        assert!(
            matches!(&invalid, ClientError::Config(message) if message.contains("invalid websocket URL")),
            "unexpected error: {invalid:?}"
        );
    }

    #[test]
    fn explicit_start_config_supports_separate_listen_and_connect_urls() {
        let target = build_start_target(&WsStartConfig::new(
            "ws://0.0.0.0:4222",
            "ws://127.0.0.1:4222",
            HashMap::new(),
        ))
        .expect("config should be valid");

        assert_eq!(target.listen_url, "ws://0.0.0.0:4222");
        assert_eq!(target.connect_url, "ws://127.0.0.1:4222");
        assert_eq!(
            target.log_path,
            std::env::temp_dir()
                .join("codex-app-server-sdk")
                .join("app-server-0_0_0_0-4222.log")
        );
    }

    #[test]
    fn ws_target_parse_rejects_bad_urls_with_config_errors() {
        let bad_scheme = WsTarget::parse("listen_url", "wss://127.0.0.1:4222")
            .expect_err("wss should be rejected for listen_url");
        assert!(
            matches!(&bad_scheme, ClientError::Config(message) if message.contains("`ws` scheme")),
            "unexpected error: {bad_scheme:?}"
        );

        let missing_port = WsTarget::parse("connect_url", "ws://127.0.0.1")
            .expect_err("missing port should be rejected");
        assert!(
            matches!(&missing_port, ClientError::Config(message) if message.contains("explicit port")),
            "unexpected error: {missing_port:?}"
        );
    }

    #[test]
    fn ws_target_formats_ipv6_and_derives_log_paths() {
        let target = WsTarget::parse("listen_url", "ws://[::1]:4222").expect("valid IPv6 url");
        assert_eq!(target.url(), "ws://[::1]:4222");
        assert_eq!(
            target.log_path(),
            std::env::temp_dir()
                .join("codex-app-server-sdk")
                .join("app-server-__1-4222.log")
        );
    }

    #[test]
    fn startup_locks_are_keyed_by_normalized_host_and_port() {
        let localhost = WsTarget::parse("connect_url", "ws://localhost:49222").expect("valid url");
        let loopback = WsTarget::parse("connect_url", "ws://127.0.0.1:49222").expect("valid url");
        let other_port = WsTarget::parse("connect_url", "ws://127.0.0.1:49223").expect("valid url");

        assert_eq!(localhost.lock_key(), loopback.lock_key());
        assert_ne!(localhost.lock_key(), other_port.lock_key());
        assert!(Arc::ptr_eq(
            &startup_lock(&localhost),
            &startup_lock(&loopback)
        ));
        assert!(!Arc::ptr_eq(
            &startup_lock(&localhost),
            &startup_lock(&other_port)
        ));
    }

    #[test]
    fn codex_binary_prefers_explicit_environment_map() {
        let env = HashMap::from([(
            "CODEX_BINARY".to_string(),
            "/opt/codex/bin/codex".to_string(),
        )]);
        assert_eq!(codex_binary(&env), "/opt/codex/bin/codex");
    }
}
