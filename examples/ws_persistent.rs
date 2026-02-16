#[cfg(feature = "ws")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use codex_app_server_sdk::protocol::requests::{ClientInfo, InitializeParams};
    use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};

    let client = CodexClient::connect_ws(WsConfig {
        url: "ws://127.0.0.1:4222".to_string(),
        options: ClientOptions::default(),
    })
    .await?;

    let init = InitializeParams::new(ClientInfo::new(
        "ws_persistent_example",
        "WS Persistent Example",
        env!("CARGO_PKG_VERSION"),
    ));

    let _ = client.initialize(init).await?;
    client.initialized().await?;

    println!("connected to websocket app-server");
    Ok(())
}

#[cfg(not(feature = "ws"))]
fn main() {
    eprintln!("enable the `ws` feature to run this example");
}
