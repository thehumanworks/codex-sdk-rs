use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Error as WsError;
use url::{Host, Url};

use crate::client::{WsServerHandle, WsStartConfig, WsStartMode};
use crate::error::ClientError;

const DAEMON_LOG_DIR_NAME: &str = "codex-app-server-sdk";
const MAX_DAEMON_LOG_BYTES: u64 = 10 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_INTERVAL: Duration = Duration::from_millis(150);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_ACK_TIMEOUT: Duration = Duration::from_secs(2);

static STARTUP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone)]
struct WsStartTarget {
    connect_url: String,
    listen_url: String,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    Reachable,
    Unavailable,
}

pub async fn ensure_local_ws_app_server(
    url: &str,
    env: &std::collections::HashMap<String, String>,
) -> Result<(), ClientError> {
    let Some(target) = parse_managed_ws_target(url)? else {
        return Ok(());
    };

    let _handle = start_ws_server_internal(target, env, true, WsStartMode::Daemon).await?;
    Ok(())
}

pub async fn start_ws_server(
    config: &WsStartConfig,
    mode: WsStartMode,
) -> Result<WsServerHandle, ClientError> {
    let target = build_start_target(config)?;
    start_ws_server_internal(target, &config.env, config.reuse_existing, mode).await
}

fn startup_lock() -> &'static Mutex<()> {
    STARTUP_LOCK.get_or_init(|| Mutex::new(()))
}

async fn start_ws_server_internal(
    target: WsStartTarget,
    env: &std::collections::HashMap<String, String>,
    reuse_existing: bool,
    mode: WsStartMode,
) -> Result<WsServerHandle, ClientError> {
    if let Some(handle) = existing_server_handle(&target, reuse_existing, mode).await? {
        return Ok(handle);
    }

    let _guard = startup_lock().lock().await;

    if let Some(handle) = existing_server_handle(&target, reuse_existing, mode).await? {
        return Ok(handle);
    }

    match mode {
        WsStartMode::Daemon => {
            spawn_daemon_launcher_thread(&target, env)?;
            wait_for_ready(&target, None).await?;
            Ok(WsServerHandle::daemon_started(
                target.listen_url,
                target.connect_url,
                target.log_path,
            ))
        }
        WsStartMode::Blocking => {
            let mut child = spawn_owned_process(&target, env)?;
            wait_for_ready(&target, Some(&mut child)).await?;
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
) -> Result<Option<WsServerHandle>, ClientError> {
    match probe_app_server(&target.connect_url).await {
        Ok(ProbeState::Reachable) => {
            if reuse_existing {
                Ok(Some(WsServerHandle::from_reused_existing(
                    target.listen_url.clone(),
                    target.connect_url.clone(),
                    mode,
                    Some(target.log_path.clone()),
                )))
            } else {
                Err(ClientError::TransportSend(format!(
                    "websocket app-server already running at `{}` and reuse_existing is disabled",
                    target.connect_url
                )))
            }
        }
        Ok(ProbeState::Unavailable) => Ok(None),
        Err(err) => Err(conflict_error(&target.connect_url, err)),
    }
}

fn parse_managed_ws_target(url: &str) -> Result<Option<WsStartTarget>, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::TransportSend(format!("invalid websocket URL: {err}")))?;

    if parsed.scheme() != "ws" {
        return Ok(None);
    }

    let host = parsed.host().ok_or_else(|| {
        ClientError::TransportSend(format!("invalid websocket URL: missing host in `{url}`"))
    })?;

    if !is_loopback_host(&host) {
        return Ok(None);
    }

    let port = parsed.port().ok_or_else(|| {
        ClientError::TransportSend(format!(
            "loopback websocket URL must include explicit port: `{url}`"
        ))
    })?;

    let listen_host = normalized_listen_host(&host);
    let listen_url = format!("ws://{listen_host}:{port}");
    let log_path = log_path_for(&host, port);

    Ok(Some(WsStartTarget {
        connect_url: url.to_string(),
        listen_url,
        log_path,
    }))
}

fn build_start_target(config: &WsStartConfig) -> Result<WsStartTarget, ClientError> {
    let listen = parse_ws_url("listen_url", &config.listen_url)?;
    parse_ws_url("connect_url", &config.connect_url)?;
    let listen_host = listen.host().ok_or_else(|| {
        ClientError::TransportSend(format!(
            "invalid websocket listen_url: missing host in `{}`",
            config.listen_url
        ))
    })?;
    let listen_port = listen.port().ok_or_else(|| {
        ClientError::TransportSend(format!(
            "websocket listen_url must include explicit port: `{}`",
            config.listen_url
        ))
    })?;

    Ok(WsStartTarget {
        listen_url: config.listen_url.clone(),
        connect_url: config.connect_url.clone(),
        log_path: log_path_for(&listen_host, listen_port),
    })
}

fn parse_ws_url(label: &str, url: &str) -> Result<Url, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::TransportSend(format!("invalid websocket {label}: {err}")))?;
    if parsed.scheme() != "ws" {
        return Err(ClientError::TransportSend(format!(
            "websocket {label} must use the `ws` scheme: `{url}`"
        )));
    }
    if parsed.host().is_none() {
        return Err(ClientError::TransportSend(format!(
            "invalid websocket {label}: missing host in `{url}`"
        )));
    }
    if parsed.port().is_none() {
        return Err(ClientError::TransportSend(format!(
            "websocket {label} must include explicit port: `{url}`"
        )));
    }
    Ok(parsed)
}

fn is_loopback_host(host: &Host<&str>) -> bool {
    match host {
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
        Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
    }
}

fn normalized_listen_host(host: &Host<&str>) -> String {
    match host {
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => format!("[{ip}]"),
        Host::Domain(domain) if domain.eq_ignore_ascii_case("localhost") => "127.0.0.1".to_string(),
        Host::Domain(domain) => domain.to_string(),
    }
}

fn log_path_for(host: &Host<&str>, port: u16) -> PathBuf {
    let host_label = match host {
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
        Host::Domain(domain) => domain.to_lowercase(),
    };
    let safe_host_label: String = host_label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();

    daemon_log_dir().join(format!("app-server-{safe_host_label}-{port}.log"))
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

async fn probe_app_server(url: &str) -> Result<ProbeState, ClientError> {
    let connect = tokio_tungstenite::connect_async(url);
    match tokio::time::timeout(PROBE_TIMEOUT, connect).await {
        Ok(Ok((mut stream, _))) => {
            let _ = stream.close(None).await;
            Ok(ProbeState::Reachable)
        }
        Ok(Err(err)) => classify_probe_error(url, err),
        Err(_) => Ok(ProbeState::Unavailable),
    }
}

fn classify_probe_error(url: &str, err: WsError) -> Result<ProbeState, ClientError> {
    match err {
        WsError::Io(io_err) if is_retryable_connect_error(io_err.kind()) => {
            Ok(ProbeState::Unavailable)
        }
        WsError::ConnectionClosed | WsError::AlreadyClosed => Ok(ProbeState::Unavailable),
        WsError::Http(response) => Err(ClientError::TransportSend(format!(
            "websocket probe failed for `{url}`: unexpected HTTP status {}",
            response.status()
        ))),
        other => Err(ClientError::TransportSend(format!(
            "websocket probe failed for `{url}`: {other}"
        ))),
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

fn spawn_daemon_launcher_thread(
    target: &WsStartTarget,
    env: &std::collections::HashMap<String, String>,
) -> Result<(), ClientError> {
    let target_for_thread = target.clone();
    let target_for_error = target.clone();
    let env_for_thread = env.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), std::io::Error>>();

    thread::spawn(move || {
        let result = spawn_daemon_process(&target_for_thread, &env_for_thread);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(STARTUP_ACK_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(ClientError::TransportSend(format!(
            "failed to start websocket app-server daemon for `{}`: {err}; logs: {}",
            target_for_error.connect_url,
            target_for_error.log_path.display()
        ))),
        Err(err) => Err(ClientError::TransportSend(format!(
            "failed to confirm websocket app-server daemon startup for `{}`: {err}; logs: {}",
            target_for_error.connect_url,
            target_for_error.log_path.display()
        ))),
    }
}

fn spawn_daemon_process(
    target: &WsStartTarget,
    env: &std::collections::HashMap<String, String>,
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
    env: &std::collections::HashMap<String, String>,
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

fn codex_binary(env: &std::collections::HashMap<String, String>) -> String {
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
) -> Result<(), ClientError> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
        if let Some(child) = child.as_deref_mut()
            && let Some(status) = child.try_wait()?
        {
            return Err(ClientError::TransportSend(format!(
                "websocket app-server for `{}` exited before becoming ready with status {status}",
                target.connect_url
            )));
        }

        match probe_app_server(&target.connect_url).await {
            Ok(ProbeState::Reachable) => return Ok(()),
            Ok(ProbeState::Unavailable) => {}
            Err(err) => {
                return Err(ClientError::TransportSend(format!(
                    "websocket app-server readiness probe failed for `{}`: {err}; logs: {}",
                    target.connect_url,
                    target.log_path.display()
                )));
            }
        }

        if Instant::now() >= deadline {
            return Err(ClientError::TransportSend(format!(
                "timed out waiting for websocket app-server at `{}`; logs: {}",
                target.connect_url,
                target.log_path.display()
            )));
        }

        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

fn conflict_error(connect_url: &str, err: ClientError) -> ClientError {
    match err {
        ClientError::TransportSend(message) => ClientError::TransportSend(format!(
            "websocket startup conflict at `{connect_url}`: {message}"
        )),
        other => other,
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
        match missing_port {
            ClientError::TransportSend(message) => {
                assert!(
                    message.contains("explicit port"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("unexpected error variant: {other}"),
        }

        let invalid = parse_managed_ws_target("not-a-url").expect_err("invalid URL should fail");
        match invalid {
            ClientError::TransportSend(message) => {
                assert!(
                    message.contains("invalid websocket URL"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("unexpected error variant: {other}"),
        }
    }

    #[test]
    fn explicit_start_config_supports_separate_listen_and_connect_urls() {
        let target = build_start_target(&WsStartConfig::new(
            "ws://0.0.0.0:4222",
            "ws://127.0.0.1:4222",
            std::collections::HashMap::new(),
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
    fn codex_binary_prefers_explicit_environment_map() {
        let env = std::collections::HashMap::from([(
            "CODEX_BINARY".to_string(),
            "/opt/codex/bin/codex".to_string(),
        )]);
        assert_eq!(codex_binary(&env), "/opt/codex/bin/codex");
    }
}
