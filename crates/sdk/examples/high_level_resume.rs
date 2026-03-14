use std::env;

use codex_app_server_sdk::StdioConfig;
use codex_app_server_sdk::api::{
    ApprovalMode, Codex, ResumeThread, SandboxMode, ThreadOptions, TurnOptions,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let working_directory = env::current_dir()?.to_string_lossy().to_string();

    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let options = ThreadOptions::builder()
        .working_directory(working_directory)
        .approval_policy(ApprovalMode::OnRequest)
        .sandbox_mode(SandboxMode::WorkspaceWrite)
        .build();

    let resume_target = match env::var("CODEX_THREAD_ID") {
        Ok(thread_id) => ResumeThread::ById(thread_id),
        Err(_) => ResumeThread::Latest,
    };
    let mut thread = codex.resume_thread(resume_target, options);
    let response = thread
        .ask("Give me a one-line status update.", TurnOptions::default())
        .await?;
    println!("{response}");

    Ok(())
}
