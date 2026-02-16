#[cfg(feature = "ws")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use codex_app_server_sdk::api::{ThreadEvent, ThreadOptions, TurnOptions};
    use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};

    let client = CodexClient::connect_ws(WsConfig {
        url: "ws://127.0.0.1:4222".to_string(),
        options: ClientOptions::default(),
    })
    .await?;

    let mut thread = client.start_thread(ThreadOptions::default());
    let turn = thread
        .run("Reply with exactly: ok", TurnOptions::default())
        .await?;
    println!("response: {}", turn.final_response);

    let mut streamed = thread
        .run_streamed("Reply with exactly: ok", TurnOptions::default())
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
            | ThreadEvent::ItemCompleted { .. }
            | ThreadEvent::Error { .. } => {}
        }
    }

    Ok(())
}

#[cfg(not(feature = "ws"))]
fn main() {
    eprintln!("enable the `ws` feature to run this example");
}
