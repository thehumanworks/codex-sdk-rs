use std::process::ExitCode;

use crate::cli::{CliArgs, TransportMode, resolve_current_working_directory};
use crate::connection::{connect_ws_codex, spawn_stdio_codex};
use crate::error::LunaError;
use crate::session::list_sessions;

pub(super) async fn run(cli: CliArgs) -> Result<ExitCode, LunaError> {
    let resolved = super::websocket_url_for(&cli)?;
    let cwd_filter = if cli.sessions_all {
        None
    } else {
        Some(
            cli.working_directory
                .clone()
                .map(Ok)
                .unwrap_or_else(resolve_current_working_directory)?,
        )
    };
    let codex = match cli.transport_mode {
        TransportMode::WebSocket => {
            connect_ws_codex(&resolved.url, resolved.manage_daemon() && !cli.no_daemon).await?
        }
        TransportMode::Stdio => spawn_stdio_codex().await?,
    };
    list_sessions(&codex, cwd_filter.as_deref()).await?;
    Ok(ExitCode::SUCCESS)
}
