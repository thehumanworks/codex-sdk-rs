#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use codex_app_server_sdk::api::{ThreadEvent, ThreadOptions, TurnOptions};
    use codex_app_server_sdk::{CodexClient, WsConfig, WsStartConfig};

    let _server = CodexClient::start_ws_daemon(WsStartConfig::default()).await?;
    // `connect_ws` accepts both `ws://` and `wss://` URLs; the daemon helper above remains ws-only.
    let client = CodexClient::connect_ws(WsConfig::default()).await?;

    let mut thread = client.start_thread(
        ThreadOptions::builder()
            .model("gpt-5.3-codex-spark")
            .skip_git_repo_check(true)
            .build(),
    );
    let turn = thread
        .run(
            "Write a haiku about Codex Spark, and its speed!",
            TurnOptions::default(),
        )
        .await?;
    println!("response: {}", turn.final_response);

    let mut streamed = thread
        .run_streamed(
            "Write a haiku about Codex Spark, and its speed!",
            TurnOptions::default(),
        )
        .await?;
    while let Some(next) = streamed.next_event().await {
        match next? {
            ThreadEvent::TurnCompleted { .. } => {
                println!("streamed turn completed");
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                eprintln!("streamed turn failed: {}", error.message);
                break;
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. }
            | ThreadEvent::ItemUpdated { .. }
            | ThreadEvent::Error { .. } => {}
            ThreadEvent::ItemCompleted { item } => {
                use codex_app_server_sdk::ThreadItem;

                if let ThreadItem::AgentMessage(message) = item {
                    println!("Agent Message: {}", message.text);
                }
            }
        }
    }

    Ok(())
}
