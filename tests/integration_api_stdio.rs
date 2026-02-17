use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codex_app_server_sdk::api::{Codex, ThreadEvent, ThreadOptions, TurnOptions};
use codex_app_server_sdk::{CodexClient, OpenAiSerializable, StdioConfig};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const TEST_TIMEOUT: Duration = Duration::from_secs(90);

fn isolated_codex_home() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "codex-sdk-rs-api-integration-home-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(path.join(".codex")).expect("create isolated codex home");
    path
}

fn isolated_stdio_config() -> StdioConfig {
    let mut config = StdioConfig::default();
    let mut env = HashMap::new();
    let isolated_home = isolated_codex_home();
    env.insert(
        "HOME".to_string(),
        isolated_home.to_string_lossy().to_string(),
    );
    env.insert(
        "CODEX_HOME".to_string(),
        isolated_home.to_string_lossy().to_string(),
    );
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        env.insert("OPENAI_API_KEY".to_string(), api_key);
    }
    config.env = env;
    config
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, OpenAiSerializable)]
struct SchemaConstrainedResponse {
    answer: String,
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn run_collects_typed_items_and_response() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(isolated_stdio_config()).await?;
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
    let codex = Codex::spawn_stdio(isolated_stdio_config()).await?;
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
    let client = CodexClient::spawn_stdio(isolated_stdio_config()).await?;
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

#[tokio::test]
#[ignore = "requires local codex app-server runtime"]
async fn run_respects_output_schema_for_typed_deserialization()
-> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(isolated_stdio_config()).await?;
    let mut thread = codex.start_thread(ThreadOptions::default());

    let turn_options = TurnOptions::builder()
        .output_schema_for::<SchemaConstrainedResponse>()
        .build();

    let result = thread
        .run(
            "Respond with strict JSON only using one field named answer with value ok.",
            turn_options,
        )
        .await?;

    let value: serde_json::Value = serde_json::from_str(&result.final_response)?;
    let parsed = SchemaConstrainedResponse::from_openai_value(value)?;
    assert!(
        !parsed.answer.trim().is_empty(),
        "expected non-empty answer in schema constrained output"
    );

    Ok(())
}
