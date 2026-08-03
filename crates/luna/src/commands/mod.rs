mod chat;
mod completions;
mod doctor;
mod exec;
mod sessions;
mod start;
mod turn;

use std::env;
use std::process::ExitCode;

use crate::cli::{CliArgs, CommandKind, ParsedCommand, TransportMode};
use crate::error::LunaError;
use crate::websocket::{
    CODEX_APP_SERVER_WS_URL_ENV, CODEX_WEB_SERVER_URL_ENV, ResolvedWebsocketUrl,
    resolve_websocket_url,
};

pub(crate) async fn dispatch(command: ParsedCommand) -> Result<ExitCode, LunaError> {
    match command {
        ParsedCommand::Help(help) => {
            print!("{help}");
            Ok(ExitCode::SUCCESS)
        }
        ParsedCommand::Version(version) => {
            print!("{version}");
            Ok(ExitCode::SUCCESS)
        }
        ParsedCommand::Completions(shell) => completions::run(shell),
        ParsedCommand::Run(cli) => match cli.command_kind {
            CommandKind::Exec => exec::run(*cli).await,
            CommandKind::Chat => chat::run(*cli).await,
            CommandKind::Start => start::run(*cli).await,
            CommandKind::Sessions => sessions::run(*cli).await,
            CommandKind::Doctor => doctor::run(*cli).await,
        },
    }
}

fn websocket_url_for(cli: &CliArgs) -> Result<ResolvedWebsocketUrl, LunaError> {
    if cli.transport_mode == TransportMode::Stdio {
        return Ok(ResolvedWebsocketUrl::default());
    }
    let primary = env::var(CODEX_APP_SERVER_WS_URL_ENV).ok();
    let legacy = env::var(CODEX_WEB_SERVER_URL_ENV).ok();
    resolve_websocket_url(
        cli.websocket_url.as_deref(),
        primary.as_deref(),
        legacy.as_deref(),
    )
}
