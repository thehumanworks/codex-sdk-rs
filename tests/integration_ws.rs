#![cfg(feature = "ws")]

use std::net::TcpListener;
use std::time::Duration;

use codex_app_server_sdk::protocol::requests::{ClientInfo, InitializeParams};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};

fn reserve_local_ws_url() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);

    Ok(format!("ws://127.0.0.1:{}", addr.port()))
}

async fn connect_initialized_ws_client(
    url: &str,
) -> Result<CodexClient, Box<dyn std::error::Error>> {
    let client = CodexClient::connect_ws(WsConfig {
        url: url.to_string(),
        options: ClientOptions::default(),
    })
    .await?;

    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "integration_ws_test",
            "Integration WS Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;

    Ok(client)
}

#[tokio::test]
#[ignore = "requires local codex app-server runtime with ws transport plus auth/network"]
async fn auto_starts_and_reuses_persistent_loopback_websocket_server()
-> Result<(), Box<dyn std::error::Error>> {
    let url = reserve_local_ws_url()?;

    let first_client = connect_initialized_ws_client(&url).await?;
    drop(first_client);

    tokio::time::sleep(Duration::from_millis(200)).await;

    let second_client = connect_initialized_ws_client(&url).await?;
    drop(second_client);

    Ok(())
}
