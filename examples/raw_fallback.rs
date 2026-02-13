use serde_json::json;

use codex_app_server_sdk::CodexClient;
use codex_app_server_sdk::client::StdioConfig;
use codex_app_server_sdk::protocol::requests::{ClientInfo, InitializeParams};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;

    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "example_raw",
            "Example Raw",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;

    let result = client
        .send_raw_request("model/list", json!({ "limit": 5 }), None)
        .await?;

    println!("raw result: {result}");
    Ok(())
}
