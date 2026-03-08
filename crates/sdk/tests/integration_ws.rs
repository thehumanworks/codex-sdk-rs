use std::collections::HashMap;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use codex_app_server_sdk::api::{ThreadEvent, ThreadOptions, TurnOptions};
use codex_app_server_sdk::protocol::requests::{ClientInfo, InitializeParams};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};

fn reserve_local_ws_url() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);

    Ok(format!("ws://127.0.0.1:{}", addr.port()))
}

fn isolated_codex_home() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "codex-sdk-rs-ws-integration-home-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(path.join(".codex")).expect("create isolated codex home");
    path
}

fn isolate_home_enabled() -> bool {
    std::env::var("CODEX_SDK_TEST_ISOLATE_HOME")
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            normalized == "1" || normalized == "true" || normalized == "yes"
        })
        .unwrap_or(false)
}

fn isolated_ws_env() -> HashMap<String, String> {
    let mut env = HashMap::new();
    if isolate_home_enabled() {
        let isolated_home = isolated_codex_home();
        env.insert(
            "HOME".to_string(),
            isolated_home.to_string_lossy().to_string(),
        );
        env.insert(
            "CODEX_HOME".to_string(),
            isolated_home.to_string_lossy().to_string(),
        );
    }
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        env.insert("OPENAI_API_KEY".to_string(), api_key);
    }
    env
}

async fn connect_initialized_ws_client(
    url: &str,
) -> Result<CodexClient, Box<dyn std::error::Error>> {
    let client = CodexClient::start_and_connect_ws(WsConfig {
        url: url.to_string(),
        env: isolated_ws_env(),
        options: ClientOptions::default(),
    })
    .await?;

    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "integration_ws_test",
            "Integration WS Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;

    Ok(client)
}

#[tokio::test]
async fn auto_starts_and_reuses_persistent_loopback_websocket_server()
-> Result<(), Box<dyn std::error::Error>> {
    let url = reserve_local_ws_url()?;

    let first_client = connect_initialized_ws_client(&url).await?;
    drop(first_client);

    tokio::time::sleep(Duration::from_millis(200)).await;

    let second_client = connect_initialized_ws_client(&url).await?;
    drop(second_client);

    Ok(())
}

#[tokio::test]
async fn ws_client_start_thread_runs_and_streams() -> Result<(), Box<dyn std::error::Error>> {
    let url = reserve_local_ws_url()?;
    let client = connect_initialized_ws_client(&url).await?;

    let mut thread = client.start_thread(ThreadOptions::default());
    let turn = thread
        .run("Reply with exactly: ok", TurnOptions::default())
        .await?;

    assert!(thread.id().is_some(), "thread id should be populated");
    assert!(
        !turn.final_response.trim().is_empty(),
        "final response should not be empty"
    );

    let mut streamed = thread
        .run_streamed("Reply with exactly: ok", TurnOptions::default())
        .await?;
    let mut saw_terminal = false;

    while let Some(next) =
        tokio::time::timeout(Duration::from_secs(2), streamed.next_event()).await?
    {
        let event = next?;
        match event {
            ThreadEvent::TurnCompleted { .. } => {
                saw_terminal = true;
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                return Err(format!("turn failed unexpectedly: {}", error.message).into());
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. }
            | ThreadEvent::ItemUpdated { .. }
            | ThreadEvent::ItemCompleted { .. }
            | ThreadEvent::Error { .. } => {}
        }
    }

    assert!(saw_terminal, "expected streamed turn completion event");

    Ok(())
}

#[tokio::test]
async fn test_connect_ws_does_not_start_daemon() {
    let url = "ws://127.0.0.1:4234"; // Random unused port
    let config = WsConfig {
        url: url.to_string(),
        env: HashMap::new(),
        options: ClientOptions::default(),
    };
    let result = CodexClient::connect_ws(config).await;
    assert!(
        result.is_err(),
        "Expected connect_ws to fail without start_and_connect_ws"
    );
}
