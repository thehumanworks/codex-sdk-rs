use codex_app_server_sdk::StdioConfig;
use codex_app_server_sdk::abstractions::{Codex, ThreadOptions, TurnOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let mut thread = codex.start_thread(ThreadOptions::default());

    let turn = thread
        .run(
            "Summarize this repository in two bullet points.",
            TurnOptions::default(),
        )
        .await?;

    println!("thread: {}", thread.id().unwrap_or("<unknown>"));
    println!("response: {}", turn.final_response);
    println!("items: {}", turn.items.len());

    Ok(())
}
