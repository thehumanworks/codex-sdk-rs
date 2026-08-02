mod cli;
mod commands;
mod config;
mod connection;
mod doctor;
mod environment;
mod error;
mod output;
mod session;
mod websocket;

use std::env;
use std::process::ExitCode;

use clap::CommandFactory;
use cli::*;
use error::LunaError;

const APP_NAME: &str = "luna";
const MODEL: &str = "gpt-5.6-luna";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(exit_code) => exit_code,
        Err(LunaError::Usage(message)) => {
            let error = LunaError::Usage(message);
            if error.to_string().starts_with("error:") {
                eprint!("{}", output::format_error(&error.to_string()));
            } else {
                eprintln!(
                    "{}\n",
                    output::format_error(&format!("{} [{}]", error, error.code()))
                );
                eprintln!("{}", CliParser::command().render_help());
            }
            ExitCode::from(error.exit_code())
        }
        Err(error) => {
            eprintln!(
                "{}",
                output::format_error(&format!("{APP_NAME} [{}]: {error}", error.code()))
            );
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run() -> Result<ExitCode, LunaError> {
    commands::dispatch(parse_cli_args(env::args().skip(1))?).await
}
