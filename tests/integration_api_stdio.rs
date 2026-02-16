use std::time::{Duration, Instant};

use codex_app_server_sdk::api::{Codex, ThreadEvent, ThreadOptions, TurnOptions};
use codex_app_server_sdk::{CodexClient, StdioConfig};

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn run_collects_typed_items_and_response() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let mut thread = codex.start_thread(ThreadOptions::default());

    let result = thread
        .run("Reply with exactly: ok", TurnOptions::default())
        .await?;

    assert!(thread.id().is_some(), "thread id should be populated");
    assert!(
        !result.items.is_empty(),
        "turn should contain completed items"
    );
    assert!(
        !result.final_response.trim().is_empty(),
        "final response should not be empty"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn run_streamed_emits_turn_lifecycle_events() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let mut thread = codex.start_thread(ThreadOptions::default());

    let mut streamed = thread
        .run_streamed("Reply with exactly: ok", TurnOptions::default())
        .await?;

    let deadline = Instant::now() + TEST_TIMEOUT;
    let mut saw_started = false;
    let mut saw_completed = false;

    while Instant::now() < deadline {
        let next = tokio::time::timeout(Duration::from_secs(2), streamed.next_event()).await;
        let Some(event) = (match next {
            Ok(event) => event,
            Err(_) => continue,
        }) else {
            break;
        };

        let event = event?;
        match event {
            ThreadEvent::ThreadStarted { .. } | ThreadEvent::TurnStarted => {
                saw_started = true;
            }
            ThreadEvent::TurnCompleted { .. } => {
                saw_completed = true;
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                return Err(format!("turn failed unexpectedly: {}", error.message).into());
            }
            ThreadEvent::ItemStarted { .. }
            | ThreadEvent::ItemUpdated { .. }
            | ThreadEvent::ItemCompleted { .. }
            | ThreadEvent::Error { .. } => {}
        }
    }

    assert!(saw_started, "expected started events");
    assert!(saw_completed, "expected turn completion event");

    Ok(())
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn codex_client_start_thread_runs_typed_api() -> Result<(), Box<dyn std::error::Error>> {
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;
    let mut thread = client.start_thread(ThreadOptions::default());

    let result = thread
        .run("Reply with exactly: ok", TurnOptions::default())
        .await?;

    assert!(thread.id().is_some(), "thread id should be populated");
    assert!(
        !result.final_response.trim().is_empty(),
        "final response should not be empty"
    );

    Ok(())
}
