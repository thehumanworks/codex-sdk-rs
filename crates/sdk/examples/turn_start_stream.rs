use std::time::{Duration, Instant};

use codex_app_server_sdk::CodexClient;
use codex_app_server_sdk::client::StdioConfig;
use codex_app_server_sdk::events::{ServerEvent, ServerNotification};
use codex_app_server_sdk::protocol::requests::{
    ClientInfo, InitializeParams, ThreadStartParams, TurnStartParams,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;

    let init = InitializeParams::new(ClientInfo::new(
        "example_turn_stream",
        "Example Turn Stream",
        env!("CARGO_PKG_VERSION"),
    ));
    client.initialize(init).await?;
    client.initialized().await?;

    let thread = client.thread_start(ThreadStartParams::default()).await?;
    let turn = client
        .turn_start(TurnStartParams::text(
            thread.thread.id.clone(),
            "Reply with exactly: ok",
        ))
        .await?;

    let target_turn_id = turn.turn.id;
    println!("started turn {target_turn_id}");

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut saw_output = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), client.next_event()).await {
            Ok(Ok(ServerEvent::Notification(ServerNotification::ItemAgentMessageDelta(delta)))) => {
                if let Some(text) = delta.delta.or(delta.text) {
                    saw_output = true;
                    print!("{text}");
                }
            }
            Ok(Ok(ServerEvent::Notification(ServerNotification::ItemCompleted(item)))) => {
                if item
                    .item
                    .get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|kind| kind == "agentMessage")
                {
                    saw_output = true;
                }
            }
            Ok(Ok(ServerEvent::Notification(ServerNotification::TurnCompleted(done)))) => {
                if done.turn.id == target_turn_id {
                    println!("\nturn status: {:?}", done.turn.status);
                    if !saw_output {
                        eprintln!("warning: no agent output was observed before completion");
                    }
                    break;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => {
                eprintln!("event error: {err}");
                break;
            }
            Err(_) => {}
        }
    }

    Ok(())
}
