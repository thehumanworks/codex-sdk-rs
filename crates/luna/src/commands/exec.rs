use std::io;
use std::process::ExitCode;

use super::turn;
use crate::cli::{CliArgs, resolve_prompt};
use crate::error::LunaError;
use crate::output;

pub(super) async fn run(mut cli: CliArgs) -> Result<ExitCode, LunaError> {
    let prompt = resolve_prompt(std::mem::take(&mut cli.prompt_parts))?;
    let final_response_only = cli.final_response_only;
    let json_output = cli.json_output;
    let turn::PreparedTurn {
        mut thread,
        turn_options,
        ..
    } = turn::prepare(cli).await?;

    if final_response_only {
        let final_response = thread.ask(prompt, turn_options).await?;
        let mut stdout = io::stdout();
        output::print_final_response(&mut stdout, &final_response)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut streamed = thread.run_streamed(prompt, turn_options).await?;

    if json_output {
        output::stream_json_events(&mut streamed).await?;
    } else {
        output::stream_human_events(&mut streamed).await?;
    }

    Ok(ExitCode::SUCCESS)
}
