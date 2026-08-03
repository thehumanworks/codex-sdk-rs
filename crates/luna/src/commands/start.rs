use std::process::ExitCode;

use crate::cli::CliArgs;
use crate::connection::start_ws_server;
use crate::error::LunaError;

pub(super) async fn run(cli: CliArgs) -> Result<ExitCode, LunaError> {
    let resolved = super::websocket_url_for(&cli)?;
    start_ws_server(&resolved.url, cli.ws_auth_token.as_deref()).await?;
    println!("WebSocket server ready at {}", resolved.url);
    Ok(ExitCode::SUCCESS)
}
