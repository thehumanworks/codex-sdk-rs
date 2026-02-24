use std::env;

use codex_app_server_sdk::StdioConfig;
use codex_app_server_sdk::api::{ApprovalMode, Codex, SandboxMode, ThreadOptions, TurnOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let thread_id =
        env::var("CODEX_THREAD_ID").expect("set CODEX_THREAD_ID to a recorded thread/session id");

    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let options = ThreadOptions::builder()
        .approval_policy(ApprovalMode::OnRequest)
        .sandbox_mode(SandboxMode::WorkspaceWrite)
        .build();

    let mut thread = codex.resume_thread(thread_id, options);
    let response = thread
        .ask("Give me a one-line status update.", TurnOptions::default())
        .await?;
    println!("{response}");

    Ok(())
}
