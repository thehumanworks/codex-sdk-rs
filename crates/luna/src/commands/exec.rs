use std::io;
use std::process::ExitCode;

use codex_app_server_sdk::{ModelReasoningEffort, ThreadOptions, TurnOptions};

use crate::cli::{
    CliArgs, ResumeTarget, TransportMode, resolve_current_working_directory, resolve_prompt,
};
use crate::config::{
    build_thread_config, load_agent_profile, parse_optional_dynamic_tools,
    parse_optional_json_object, parse_optional_json_value, resolve_output_schema,
};
use crate::connection::{connect_ws_codex, ensure_authenticated, spawn_stdio_codex};
use crate::error::LunaError;
use crate::output;

const MODEL: &str = "gpt-5.6-luna";

pub(super) async fn run(cli: CliArgs) -> Result<ExitCode, LunaError> {
    let resolved_websocket_url = super::websocket_url_for(&cli)?;
    let CliArgs {
        agent,
        working_directory,
        model,
        model_provider,
        reasoning_effort,
        reasoning_summary,
        model_verbosity,
        config_profile,
        approval_policy,
        sandbox_mode,
        sandbox_policy_json,
        sandbox_network_access_enabled,
        sandbox_writable_roots,
        web_search_mode,
        dynamic_tools_json,
        personality,
        base_instructions,
        developer_instructions,
        ephemeral,
        experimental_raw_events,
        persist_extended_history,
        config_entries,
        config_json,
        output_schema_json,
        output_schema_file,
        turn_extra_json,
        resume_target,
        final_response_only,
        json_output,
        transport_mode,
        prompt_parts,
        no_daemon,
        ..
    } = cli;

    let websocket_url_for_connect = resolved_websocket_url.url.clone();
    let manage_daemon_for_connect = resolved_websocket_url.manage_daemon() && !no_daemon;
    let connect_task = tokio::spawn(async move {
        match transport_mode {
            TransportMode::WebSocket => {
                connect_ws_codex(&websocket_url_for_connect, manage_daemon_for_connect).await
            }
            TransportMode::Stdio => spawn_stdio_codex().await,
        }
    });

    let prompt = match resolve_prompt(prompt_parts) {
        Ok(prompt) => prompt,
        Err(err) => {
            connect_task.abort();
            return Err(err);
        }
    };
    let working_directory = match working_directory {
        Some(path) => path,
        None => match resolve_current_working_directory() {
            Ok(path) => path,
            Err(err) => {
                connect_task.abort();
                return Err(err);
            }
        },
    };
    let thread_config = match build_thread_config(
        config_json,
        config_entries,
        web_search_mode,
        config_profile,
        model_verbosity,
        sandbox_network_access_enabled,
        sandbox_writable_roots,
    ) {
        Ok(config) => config,
        Err(err) => {
            connect_task.abort();
            return Err(err);
        }
    };
    let sandbox_policy =
        match parse_optional_json_value(sandbox_policy_json, "--sandbox-policy-json") {
            Ok(value) => value,
            Err(err) => {
                connect_task.abort();
                return Err(err);
            }
        };
    let output_schema = match resolve_output_schema(output_schema_json, output_schema_file) {
        Ok(schema) => schema,
        Err(err) => {
            connect_task.abort();
            return Err(err);
        }
    };
    let turn_extra = match parse_optional_json_object(turn_extra_json, "--turn-extra-json") {
        Ok(extra) => extra,
        Err(err) => {
            connect_task.abort();
            return Err(err);
        }
    };
    let dynamic_tools = match parse_optional_dynamic_tools(dynamic_tools_json) {
        Ok(dynamic_tools) => dynamic_tools,
        Err(err) => {
            connect_task.abort();
            return Err(err);
        }
    };

    let mut thread_options = ThreadOptions::builder()
        .model(model.unwrap_or_else(|| MODEL.to_string()))
        .working_directory(working_directory);
    if let Some(model_provider) = model_provider {
        thread_options = thread_options.model_provider(model_provider);
    }
    if let Some(approval_policy) = approval_policy {
        thread_options = thread_options.approval_policy(approval_policy);
    }
    if let Some(sandbox_mode) = sandbox_mode {
        thread_options = thread_options.sandbox_mode(sandbox_mode);
    }
    if let Some(personality) = personality {
        thread_options = thread_options.personality(personality);
    }
    if let Some(base_instructions) = base_instructions {
        thread_options = thread_options.base_instructions(base_instructions);
    }
    if let Some(ephemeral) = ephemeral {
        thread_options = thread_options.ephemeral(ephemeral);
    }
    if let Some(config) = thread_config {
        thread_options = thread_options.config(config);
    }
    if let Some(dynamic_tools) = dynamic_tools {
        thread_options = thread_options.dynamic_tools(dynamic_tools);
    }
    if let Some(experimental_raw_events) = experimental_raw_events {
        thread_options = thread_options.experimental_raw_events(experimental_raw_events);
    }
    if let Some(persist_extended_history) = persist_extended_history {
        thread_options = thread_options.persist_extended_history(persist_extended_history);
    }
    if let Some(agent_name) = agent {
        let agent = match load_agent_profile(&agent_name) {
            Ok(agent) => agent,
            Err(err) => {
                connect_task.abort();
                return Err(err);
            }
        };
        thread_options = thread_options.developer_instructions(agent.instructions);
    }
    if let Some(developer_instructions) = developer_instructions {
        thread_options = thread_options.developer_instructions(developer_instructions);
    }

    let mut turn_options = TurnOptions::builder();
    turn_options =
        turn_options.model_reasoning_effort(reasoning_effort.unwrap_or(ModelReasoningEffort::Max));
    if let Some(reasoning_summary) = reasoning_summary {
        turn_options = turn_options.model_reasoning_summary(reasoning_summary);
    }
    if let Some(sandbox_policy) = sandbox_policy {
        turn_options = turn_options.sandbox_policy(sandbox_policy);
    }
    if let Some(output_schema) = output_schema {
        turn_options = turn_options.output_schema(output_schema);
    }
    if let Some(turn_extra) = turn_extra {
        turn_options = turn_options.extra(turn_extra);
    }

    let codex = connect_task.await.map_err(|error| {
        LunaError::Transport(format!("transport connect task failed: {error}"))
    })??;

    ensure_authenticated(&codex).await?;

    let options = thread_options.build();
    let turn_options = turn_options.build();
    let mut thread = match resume_target {
        Some(ResumeTarget::Last) => codex.resume_latest_thread(options),
        Some(ResumeTarget::SessionId(session_id)) => codex.resume_thread_by_id(session_id, options),
        None => codex.start_thread(options),
    };

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
