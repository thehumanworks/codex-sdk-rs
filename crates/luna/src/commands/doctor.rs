use std::process::ExitCode;

use crate::cli::CliArgs;
use crate::doctor::{DoctorOptions, run_doctor};
use crate::error::LunaError;

pub(super) async fn run(cli: CliArgs) -> Result<ExitCode, LunaError> {
    let resolved = super::websocket_url_for(&cli)?;
    let manage_daemon = resolved.manage_daemon() && !cli.no_daemon;
    let passed = run_doctor(DoctorOptions {
        json: cli.json_output,
        live: cli.doctor_live,
        transport: cli.transport_mode,
        websocket_url: resolved.url,
        manage_daemon,
    })
    .await?;
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
