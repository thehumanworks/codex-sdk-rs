use codex_app_server_sdk::{
    CodexClient, OpenAiSerializable, ThreadEvent, ThreadItem, ThreadOptions, TurnOptions, WsConfig,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema, OpenAiSerializable)]
struct SearchResult {
    url: String,
    summary: String,
    relevance_score: f64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, OpenAiSerializable)]
struct SearchResults {
    results: Vec<SearchResult>,
    total_results: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client =
        CodexClient::connect_ws(WsConfig::default().with_url("ws://192.168.1.201:4222")).await?;
    let mut thread = client.start_thread(
        ThreadOptions::builder()
            .model("gpt-5.3-codex-spark")
            .model_reasoning_effort(codex_app_server_sdk::ModelReasoningEffort::High)
            .developer_instructions(r"ROLE
            You are a meticulous and detailed web search agent.

            PROCESS
            Take time to reason over the user's query, the implicit meanings it may have. Then run the following steps in order, in a loop over a maximum of 3 iterations (more complex research queries may require more iterations):
            1. break down the user's query into 2-4 web search queries that are likely to yield the most relevant results
            2. perform each web search query in parallel using your native web search tool
            3. review the results: if the results feel incomplete, identify the knowledge gaps
            Go back to step 1 if necessary.

            OUTPUT
            Follow the output schema exactly.")
            .build(),
    );
    let mut output_stream = thread
        .run_streamed(
            "Search for modern rust libraries for styling CLIs.",
            TurnOptions::builder()
                .output_schema_for::<SearchResults>()
                .build(),
        )
        .await?;
    let mut last_agent_message: Option<String> = None;
    while let Some(Ok(event)) = output_stream.next_event().await {
        if let ThreadEvent::ItemCompleted { item } = &event {
            if let ThreadItem::AgentMessage(agent_message) = item {
                last_agent_message = Some(agent_message.text.clone());
            }
        }
        if let ThreadEvent::TurnCompleted { .. } = &event {
            break;
        }
    }
    println!(
        "{}",
        last_agent_message
            .as_ref()
            .unwrap_or(&String::from("No agent message"))
    );

    Ok(())
}
