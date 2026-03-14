use codex_app_server_sdk::api::{Codex, ThreadOptions, TurnOptions};
use codex_app_server_sdk::{JsonSchema, OpenAiSerializable, StdioConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema, OpenAiSerializable)]
struct SchemaConstrainedReply {
    answer: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(StdioConfig::default()).await?;
    let mut thread = codex.start_thread(ThreadOptions::default());

    let turn_options = TurnOptions::builder()
        .output_schema_for::<SchemaConstrainedReply>()
        .build();

    let turn = thread
        .run(
            "Respond with strict JSON only, using one field named answer with value ok.",
            turn_options,
        )
        .await?;

    let value: serde_json::Value = serde_json::from_str(&turn.final_response)?;
    let reply = SchemaConstrainedReply::from_openai_value(value)?;
    println!("answer: {}", reply.answer);

    Ok(())
}
