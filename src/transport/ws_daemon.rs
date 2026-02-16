use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Error as WsError;
use url::{Host, Url};

use crate::error::ClientError;

const DAEMON_LOG_DIR: &str = "/tmp/codex-app-server-sdk";
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_INTERVAL: Duration = Duration::from_millis(150);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_ACK_TIMEOUT: Duration = Duration::from_secs(2);

static STARTUP_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone)]
struct ManagedWsTarget {
    connect_url: String,
    listen_url: String,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    Reachable,
    Unavailable,
}

pub async fn ensure_local_ws_app_server(url: &str) -> Result<(), ClientError> {
    let Some(target) = parse_managed_ws_target(url)? else {
        return Ok(());
    };

    if probe_app_server(&target.connect_url).await? == ProbeState::Reachable {
        return Ok(());
    }

    let _guard = startup_lock().lock().await;

    if probe_app_server(&target.connect_url).await? == ProbeState::Reachable {
        return Ok(());
    }

    spawn_daemon_launcher_thread(&target)?;
    wait_for_ready(&target).await
}

fn startup_lock() -> &'static Mutex<()> {
    STARTUP_LOCK.get_or_init(|| Mutex::new(()))
}

fn parse_managed_ws_target(url: &str) -> Result<Option<ManagedWsTarget>, ClientError> {
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

    Ok(Some(ManagedWsTarget {
        connect_url: url.to_string(),
        listen_url,
        log_path,
    }))
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

    PathBuf::from(DAEMON_LOG_DIR).join(format!("app-server-{safe_host_label}-{port}.log"))
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

fn spawn_daemon_launcher_thread(target: &ManagedWsTarget) -> Result<(), ClientError> {
    let target_for_thread = target.clone();
    let target_for_error = target.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), std::io::Error>>();

    thread::spawn(move || {
        let result = spawn_daemon_process(&target_for_thread);
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

fn spawn_daemon_process(target: &ManagedWsTarget) -> std::io::Result<()> {
    fs::create_dir_all(DAEMON_LOG_DIR)?;

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target.log_path)?;
    let log_file_stderr = log_file.try_clone()?;

    let _child = Command::new("codex")
        .arg("app-server")
        .arg("--listen")
        .arg(&target.listen_url)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_stderr))
        .spawn()?;

    Ok(())
}

async fn wait_for_ready(target: &ManagedWsTarget) -> Result<(), ClientError> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;

    loop {
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
            PathBuf::from("/tmp/codex-app-server-sdk/app-server-localhost-4555.log")
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
}
