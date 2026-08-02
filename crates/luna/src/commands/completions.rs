use std::io;
use std::process::ExitCode;

use clap::CommandFactory;
use clap_complete::{Shell, generate};

use crate::APP_NAME;
use crate::cli::CliParser;
use crate::error::LunaError;

pub(super) fn run(shell: Shell) -> Result<ExitCode, LunaError> {
    generate(
        shell,
        &mut CliParser::command(),
        APP_NAME,
        &mut io::stdout(),
    );
    Ok(ExitCode::SUCCESS)
}
