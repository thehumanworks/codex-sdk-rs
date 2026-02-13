use codex_app_server_sdk::CodexClient;
use codex_app_server_sdk::client::StdioConfig;
use codex_app_server_sdk::protocol::requests::{
    ClientInfo, GetAccountParams, InitializeParams, LoginAccountParams,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| "set OPENAI_API_KEY before running this example")?;

    let client = CodexClient::spawn_stdio(StdioConfig::default()).await?;
    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "example_auth",
            "Example Auth",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;

    client
        .account_login_start(LoginAccountParams::api_key(api_key))
        .await?;

    let account = client.account_read(GetAccountParams::default()).await?;
    println!("account result: {account:?}");

    Ok(())
}
