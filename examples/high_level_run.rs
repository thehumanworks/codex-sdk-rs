use codex_app_server_sdk::StdioConfig;
use codex_app_server_sdk::api::{
    Codex, ModelReasoningEffort, SandboxMode, ThreadOptions, TurnOptions, WebSearchMode,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let options = ThreadOptions::builder()
        .sandbox_mode(SandboxMode::WorkspaceWrite)
        .model_reasoning_effort(ModelReasoningEffort::Medium)
        .web_search_mode(WebSearchMode::Live)
        .skip_git_repo_check(true)
        .build();
    let mut thread = codex.start_thread(options);

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
