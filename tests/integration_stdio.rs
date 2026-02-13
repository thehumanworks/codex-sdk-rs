use std::time::{Duration, Instant};

use codex_app_server_sdk::client::StdioConfig;
use codex_app_server_sdk::events::{ServerEvent, ServerNotification};
use codex_app_server_sdk::protocol::requests::{
    ClientInfo, InitializeParams, ModelListParams, ThreadStartParams, TurnStartParams,
};
use codex_app_server_sdk::{ClientError, CodexClient};
use serde_json::json;

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

async fn spawn_initialized_client() -> Result<CodexClient, Box<dyn std::error::Error>> {
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;
    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "integration_test",
            "Integration Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;
    Ok(client)
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn initialize_over_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;

    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "integration_test",
            "Integration Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;

    client.initialized().await?;

    Ok(())
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn rejects_requests_before_initialized_notification() -> Result<(), Box<dyn std::error::Error>>
{
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;
    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "integration_test",
            "Integration Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;

    let err = client
        .model_list(ModelListParams::default())
        .await
        .expect_err("request should fail before initialized() is sent");

    match err {
        ClientError::NotReady { method } => {
            assert_eq!(method, "model/list");
        }
        other => panic!("unexpected error: {other}"),
    }

    Ok(())
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn model_list_typed_matches_raw() -> Result<(), Box<dyn std::error::Error>> {
    let client = spawn_initialized_client().await?;

    let typed = client.model_list(ModelListParams::default()).await?;
    assert!(
        !typed.data.is_empty(),
        "typed model list should not be empty"
    );

    let raw = client
        .send_raw_request("model/list", json!({}), Some(Duration::from_secs(30)))
        .await?;

    let raw_data = raw
        .get("data")
        .and_then(|v| v.as_array())
        .expect("raw model list must include array data");

    assert_eq!(typed.data.len(), raw_data.len());

    let typed_first = &typed.data[0].id;
    let raw_first = raw_data[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("raw model entry must include id");

    assert_eq!(typed_first, raw_first);
    Ok(())
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn thread_and_turn_complete_over_event_stream() -> Result<(), Box<dyn std::error::Error>> {
    let client = spawn_initialized_client().await?;

    let thread = client.thread_start(ThreadStartParams::default()).await?;
    let turn = client
        .turn_start(TurnStartParams::text(
            thread.thread.id.clone(),
            "Reply with exactly: ok",
        ))
        .await?;

    let target_turn_id = turn.turn.id;
    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut saw_agent_delta = false;
    let mut saw_agent_item = false;

    while Instant::now() < deadline {
        let event = match tokio::time::timeout(Duration::from_secs(2), client.next_event()).await {
            Ok(Ok(event)) => event,
            Ok(Err(err)) => return Err(format!("event stream error: {err}").into()),
            Err(_) => continue,
        };

        match event {
            ServerEvent::Notification(ServerNotification::ItemAgentMessageDelta(delta)) => {
                let text = delta.delta.or(delta.text).unwrap_or_default();
                if !text.is_empty() {
                    saw_agent_delta = true;
                }
            }
            ServerEvent::Notification(ServerNotification::ItemCompleted(item)) => {
                if item
                    .item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|kind| kind == "agentMessage")
                {
                    saw_agent_item = true;
                }
            }
            ServerEvent::Notification(ServerNotification::TurnCompleted(done)) => {
                if done.turn.id == target_turn_id {
                    let status = done.turn.status.unwrap_or_default();
                    assert!(
                        status == "completed" || status == "interrupted",
                        "unexpected final turn status: {status}"
                    );
                    assert!(
                        saw_agent_delta || saw_agent_item,
                        "expected agent output before completion"
                    );
                    return Ok(());
                }
            }
            _ => {}
        }
    }

    Err("timed out waiting for turn/completed".into())
}
