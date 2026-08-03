use std::env;
use std::io::{self, IsTerminal, Read};

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use codex_app_server_sdk::{
    ApprovalMode, ModelReasoningEffort, ModelReasoningSummary, ModelVerbosity, Personality,
    SandboxMode, ServiceTier, WebSearchMode,
};

use crate::APP_NAME;
use crate::config::normalize_agent_name;
use crate::error::LunaError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumeTarget {
    Last,
    SessionId(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportMode {
    WebSocket,
    Stdio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Exec,
    Chat,
    Start,
    Sessions,
    Doctor,
}

#[derive(Debug, PartialEq)]
pub(crate) struct CliArgs {
    pub(crate) command_kind: CommandKind,
    pub(crate) no_daemon: bool,
    pub(crate) agent: Option<String>,
    pub(crate) working_directory: Option<String>,
    pub(crate) websocket_url: Option<String>,
    /// Resolved websocket bearer credential (never printed).
    pub(crate) ws_auth_token: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) model_provider: Option<String>,
    pub(crate) reasoning_effort: Option<ModelReasoningEffort>,
    pub(crate) reasoning_summary: Option<ModelReasoningSummary>,
    pub(crate) model_verbosity: Option<ModelVerbosity>,
    pub(crate) service_tier: ServiceTier,
    pub(crate) config_profile: Option<String>,
    pub(crate) approval_policy: Option<ApprovalMode>,
    pub(crate) sandbox_mode: Option<SandboxMode>,
    pub(crate) sandbox_policy_json: Option<String>,
    pub(crate) sandbox_network_access_enabled: Option<bool>,
    pub(crate) sandbox_writable_roots: Vec<String>,
    pub(crate) web_search_mode: Option<WebSearchMode>,
    pub(crate) dynamic_tools_json: Option<String>,
    pub(crate) personality: Option<Personality>,
    pub(crate) base_instructions: Option<String>,
    pub(crate) developer_instructions: Option<String>,
    pub(crate) ephemeral: Option<bool>,
    pub(crate) experimental_raw_events: Option<bool>,
    pub(crate) persist_extended_history: Option<bool>,
    pub(crate) config_entries: Vec<String>,
    pub(crate) config_json: Option<String>,
    pub(crate) output_schema_json: Option<String>,
    pub(crate) output_schema_file: Option<String>,
    pub(crate) turn_extra_json: Option<String>,
    pub(crate) resume_target: Option<ResumeTarget>,
    pub(crate) sessions_all: bool,
    pub(crate) final_response_only: bool,
    pub(crate) json_output: bool,
    pub(crate) doctor_live: bool,
    pub(crate) transport_mode: TransportMode,
    pub(crate) prompt_parts: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum ParsedCommand {
    Help(String),
    Version(String),
    Completions(Shell),
    Run(Box<CliArgs>),
}

#[derive(Debug, Parser)]
#[command(
    name = "luna",
    version,
    about = "Opinionated Codex app-server CLI",
    disable_help_subcommand = true
)]
pub(crate) struct CliParser {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run one Codex turn from an argument or stdin
    #[command(alias = "x")]
    Exec(Box<TurnCliArgs>),
    /// Start an interactive multi-turn Codex chat
    Chat(Box<TurnCliArgs>),
    /// Reuse or start a loopback WebSocket app-server, then exit
    Start(StartCliArgs),
    /// List recorded sessions ordered by last activity
    Sessions(SessionsCliArgs),
    /// Diagnose Luna, Codex, authentication, and transport readiness
    Doctor(DoctorCliArgs),
    /// Generate a shell completion script
    Completions(CompletionsCliArgs),
}

#[derive(Debug, Args)]
struct TransportCliArgs {
    /// WebSocket app-server URL
    #[arg(long, value_name = "URL", conflicts_with = "stdio")]
    ws_url: Option<String>,
    /// Bearer token for WebSocket app-server auth (`Authorization: Bearer`)
    #[arg(
        long,
        value_name = "TOKEN",
        conflicts_with_all = ["stdio", "ws_auth_token_file"]
    )]
    ws_auth_token: Option<String>,
    /// Read the WebSocket auth bearer token from a file
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["stdio", "ws_auth_token"]
    )]
    ws_auth_token_file: Option<String>,
    /// Connect without managing a local WebSocket daemon
    #[arg(long)]
    no_daemon: bool,
    /// Spawn an app-server over stdio instead of WebSocket
    #[arg(long, conflicts_with = "ws_url")]
    stdio: bool,
}

#[derive(Debug, Args)]
struct TurnCliArgs {
    #[command(flatten)]
    transport: TransportCliArgs,
    /// Load ~/.codex/config.toml [agents.NAME]
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,
    /// Set the Codex working directory (default: current directory)
    #[arg(long, value_name = "PATH")]
    cwd: Option<String>,
    /// Override the model (default: gpt-5.6-luna)
    #[arg(long)]
    model: Option<String>,
    /// Override the model provider
    #[arg(long)]
    model_provider: Option<String>,
    /// Override reasoning effort
    #[arg(long, value_parser = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"])]
    reasoning_effort: Option<String>,
    /// Override reasoning summary mode
    #[arg(long, value_parser = ["none", "auto", "concise", "detailed"])]
    reasoning_summary: Option<String>,
    /// Set model verbosity through Codex config
    #[arg(long, value_parser = ["low", "medium", "high"])]
    model_verbosity: Option<String>,
    /// Use the fast service tier
    #[arg(long)]
    fast: bool,
    /// Set a Codex config profile override
    #[arg(long)]
    config_profile: Option<String>,
    /// Set the approval policy
    #[arg(long, value_parser = ["never", "on-request", "on-failure", "untrusted"])]
    approval_policy: Option<String>,
    /// Set the sandbox mode
    #[arg(long, value_parser = ["read-only", "workspace-write", "danger-full-access"])]
    sandbox: Option<String>,
    /// Set a raw sandbox policy JSON payload
    #[arg(long)]
    sandbox_policy_json: Option<String>,
    /// Enable workspace-write network access
    #[arg(long, conflicts_with = "sandbox_network_access_disabled")]
    sandbox_network_access_enabled: bool,
    /// Disable workspace-write network access
    #[arg(long, conflicts_with = "sandbox_network_access_enabled")]
    sandbox_network_access_disabled: bool,
    /// Add a workspace-write writable root (repeatable)
    #[arg(long, value_name = "PATH")]
    sandbox_writable_root: Vec<String>,
    /// Set web search mode
    #[arg(long, value_parser = ["disabled", "cached", "live"])]
    web_search_mode: Option<String>,
    /// Set thread/start dynamicTools as a JSON array
    #[arg(long)]
    dynamic_tools_json: Option<String>,
    /// Set model personality
    #[arg(long, value_parser = ["none", "friendly", "pragmatic"])]
    personality: Option<String>,
    /// Set base instructions
    #[arg(long)]
    base_instructions: Option<String>,
    /// Set developer instructions (overrides --agent instructions)
    #[arg(long)]
    developer_instructions: Option<String>,
    /// Run without persisting session files
    #[arg(long)]
    ephemeral: bool,
    /// Enable raw response item events
    #[arg(long)]
    experimental_raw_events: bool,
    /// Persist extended history for resume/fork/read
    #[arg(long)]
    persist_extended_history: bool,
    /// Resume the most recent recorded session
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    continue_last: bool,
    /// Resume a specific session ID
    #[arg(short = 'r', long, value_name = "ID", conflicts_with = "continue_last")]
    resume: Option<String>,
    /// Set a Codex config override (repeatable)
    #[arg(long, value_name = "KEY=VALUE")]
    config: Vec<String>,
    /// Merge a JSON object into thread config
    #[arg(long)]
    config_json: Option<String>,
    /// Set the turn output schema as JSON
    #[arg(long)]
    output_schema_json: Option<String>,
    /// Load the turn output schema from a file
    #[arg(
        long,
        visible_alias = "output-schema",
        conflicts_with = "output_schema_json"
    )]
    output_schema_file: Option<String>,
    /// Merge a JSON object into raw turn/start extras
    #[arg(long)]
    turn_extra_json: Option<String>,
    /// Show only final agent message content
    #[arg(long, conflicts_with = "json")]
    final_response: bool,
    /// Show turn events as JSONL
    #[arg(long, conflicts_with = "final_response")]
    json: bool,
    /// Prompt text; exec reads stdin when omitted, chat opens the composer
    #[arg(value_name = "PROMPT")]
    prompt: Vec<String>,
}

#[derive(Debug, Args)]
struct StartCliArgs {
    /// Loopback WebSocket URL to reuse or start
    #[arg(long, value_name = "URL")]
    ws_url: Option<String>,
    /// Bearer token for WebSocket app-server auth (`Authorization: Bearer`)
    #[arg(long, value_name = "TOKEN", conflicts_with = "ws_auth_token_file")]
    ws_auth_token: Option<String>,
    /// Read the WebSocket auth bearer token from a file
    #[arg(long, value_name = "PATH", conflicts_with = "ws_auth_token")]
    ws_auth_token_file: Option<String>,
}

#[derive(Debug, Args)]
struct SessionsCliArgs {
    #[command(flatten)]
    transport: TransportCliArgs,
    /// Include sessions from every working directory
    #[arg(long)]
    all: bool,
    /// Filter sessions to this working directory
    #[arg(long, value_name = "PATH")]
    cwd: Option<String>,
}

#[derive(Debug, Args)]
struct DoctorCliArgs {
    #[command(flatten)]
    transport: TransportCliArgs,
    /// Print grouped human-readable checks (default)
    #[arg(long, conflicts_with = "json")]
    summary: bool,
    /// Print a versioned redacted JSON report
    #[arg(long, conflicts_with = "summary")]
    json: bool,
    /// Run upstream network diagnostics and app-server readiness checks
    #[arg(long)]
    live: bool,
    /// Disable color (doctor output is currently plain by default)
    #[arg(long)]
    no_color: bool,
    /// Use ASCII labels (doctor output is currently ASCII by default)
    #[arg(long)]
    ascii: bool,
}

#[derive(Debug, Args)]
struct CompletionsCliArgs {
    /// Shell to generate completions for
    shell: Shell,
}

pub(crate) fn parse_cli_args(
    args: impl IntoIterator<Item = String>,
) -> Result<ParsedCommand, LunaError> {
    let args = normalize_legacy_cli_args(args.into_iter().collect())?;
    let parsed = match CliParser::try_parse_from(std::iter::once(APP_NAME.to_string()).chain(args))
    {
        Ok(parsed) => parsed,
        Err(error) => {
            return match error.kind() {
                clap::error::ErrorKind::DisplayHelp => Ok(ParsedCommand::Help(error.to_string())),
                clap::error::ErrorKind::DisplayVersion => {
                    Ok(ParsedCommand::Version(error.to_string()))
                }
                _ => Err(LunaError::Usage(error.to_string())),
            };
        }
    };

    match parsed.command {
        CliCommand::Exec(args) => Ok(ParsedCommand::Run(Box::new(turn_cli_args(
            CommandKind::Exec,
            *args,
        )?))),
        CliCommand::Chat(args) => Ok(ParsedCommand::Run(Box::new(turn_cli_args(
            CommandKind::Chat,
            *args,
        )?))),
        CliCommand::Start(args) => Ok(ParsedCommand::Run(Box::new(start_cli_args(args)?))),
        CliCommand::Sessions(args) => Ok(ParsedCommand::Run(Box::new(sessions_cli_args(args)?))),
        CliCommand::Doctor(args) => Ok(ParsedCommand::Run(Box::new(doctor_cli_args(args)?))),
        CliCommand::Completions(args) => Ok(ParsedCommand::Completions(args.shell)),
    }
}

fn normalize_legacy_cli_args(mut args: Vec<String>) -> Result<Vec<String>, LunaError> {
    let sessions_positions: Vec<usize> = args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| (arg == "--sessions").then_some(index))
        .collect();
    match sessions_positions.as_slice() {
        [] => {}
        [position] => {
            let position = *position;
            args.remove(position);
            if args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "exec" | "x" | "chat" | "start" | "sessions" | "doctor" | "completions"
                )
            }) {
                return Err(LunaError::Usage(
                    "--sessions cannot be combined with another command".to_string(),
                ));
            }
            args.insert(0, "sessions".to_string());
        }
        _ => {
            return Err(LunaError::Usage(
                "--sessions may only be provided once".to_string(),
            ));
        }
    }
    Ok(args)
}

fn turn_cli_args(command_kind: CommandKind, args: TurnCliArgs) -> Result<CliArgs, LunaError> {
    let transport_mode = transport_mode(args.transport.stdio);
    let mut cli = empty_cli_args(command_kind, transport_mode);
    apply_transport_args(&mut cli, args.transport)?;
    cli.agent = args
        .agent
        .as_deref()
        .map(normalize_agent_name)
        .transpose()?;
    cli.working_directory = args
        .cwd
        .as_deref()
        .map(normalize_working_directory)
        .transpose()?;
    cli.model = normalize_optional_string(args.model, "--model")?;
    cli.model_provider = normalize_optional_string(args.model_provider, "--model-provider")?;
    cli.reasoning_effort = args
        .reasoning_effort
        .as_deref()
        .map(|raw| {
            parse_wire_enum::<ModelReasoningEffort>(
                raw,
                "--reasoning-effort",
                ModelReasoningEffort::VARIANTS,
            )
        })
        .transpose()?;
    cli.reasoning_summary = args
        .reasoning_summary
        .as_deref()
        .map(|raw| {
            parse_wire_enum::<ModelReasoningSummary>(
                raw,
                "--reasoning-summary",
                ModelReasoningSummary::VARIANTS,
            )
        })
        .transpose()?;
    cli.model_verbosity = args
        .model_verbosity
        .as_deref()
        .map(|raw| {
            parse_wire_enum::<ModelVerbosity>(raw, "--model-verbosity", ModelVerbosity::VARIANTS)
        })
        .transpose()?;
    cli.service_tier = if args.fast {
        ServiceTier::Fast
    } else {
        ServiceTier::Default
    };
    cli.config_profile = normalize_optional_string(args.config_profile, "--config-profile")?;
    cli.approval_policy = args
        .approval_policy
        .as_deref()
        .map(|raw| {
            parse_wire_enum::<ApprovalMode>(raw, "--approval-policy", ApprovalMode::VARIANTS)
        })
        .transpose()?;
    cli.sandbox_mode = args
        .sandbox
        .as_deref()
        .map(|raw| parse_wire_enum::<SandboxMode>(raw, "--sandbox", SandboxMode::VARIANTS))
        .transpose()?;
    cli.sandbox_policy_json =
        normalize_optional_string(args.sandbox_policy_json, "--sandbox-policy-json")?;
    cli.sandbox_network_access_enabled = match (
        args.sandbox_network_access_enabled,
        args.sandbox_network_access_disabled,
    ) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    };
    cli.sandbox_writable_roots = args
        .sandbox_writable_root
        .iter()
        .map(|root| normalize_working_directory(root))
        .collect::<Result<Vec<_>, _>>()?;
    cli.web_search_mode = args
        .web_search_mode
        .as_deref()
        .map(|raw| {
            parse_wire_enum::<WebSearchMode>(raw, "--web-search-mode", WebSearchMode::VARIANTS)
        })
        .transpose()?;
    cli.dynamic_tools_json =
        normalize_optional_string(args.dynamic_tools_json, "--dynamic-tools-json")?;
    cli.personality = args
        .personality
        .as_deref()
        .map(|raw| parse_wire_enum::<Personality>(raw, "--personality", Personality::VARIANTS))
        .transpose()?;
    cli.base_instructions =
        normalize_optional_string(args.base_instructions, "--base-instructions")?;
    cli.developer_instructions =
        normalize_optional_string(args.developer_instructions, "--developer-instructions")?;
    cli.ephemeral = args.ephemeral.then_some(true);
    cli.experimental_raw_events = args.experimental_raw_events.then_some(true);
    cli.persist_extended_history = args.persist_extended_history.then_some(true);
    cli.config_entries = args.config;
    cli.config_json = normalize_optional_string(args.config_json, "--config-json")?;
    cli.output_schema_json =
        normalize_optional_string(args.output_schema_json, "--output-schema-json")?;
    cli.output_schema_file =
        normalize_optional_string(args.output_schema_file, "--output-schema-file")?;
    cli.turn_extra_json = normalize_optional_string(args.turn_extra_json, "--turn-extra-json")?;
    cli.resume_target = if args.continue_last {
        Some(ResumeTarget::Last)
    } else {
        args.resume
            .as_deref()
            .map(normalize_session_id)
            .transpose()?
            .map(ResumeTarget::SessionId)
    };
    cli.final_response_only = args.final_response;
    cli.json_output = args.json;
    cli.prompt_parts = args.prompt;
    Ok(cli)
}

fn start_cli_args(args: StartCliArgs) -> Result<CliArgs, LunaError> {
    let mut cli = empty_cli_args(CommandKind::Start, TransportMode::WebSocket);
    cli.websocket_url = normalize_optional_string(args.ws_url, "--ws-url")?;
    cli.ws_auth_token = crate::websocket::resolve_ws_auth_token(
        args.ws_auth_token.as_deref(),
        args.ws_auth_token_file.as_deref(),
    )?;
    Ok(cli)
}

fn sessions_cli_args(args: SessionsCliArgs) -> Result<CliArgs, LunaError> {
    let mut cli = empty_cli_args(CommandKind::Sessions, transport_mode(args.transport.stdio));
    apply_transport_args(&mut cli, args.transport)?;
    cli.working_directory = args
        .cwd
        .as_deref()
        .map(normalize_working_directory)
        .transpose()?;
    cli.sessions_all = args.all;
    Ok(cli)
}

fn doctor_cli_args(args: DoctorCliArgs) -> Result<CliArgs, LunaError> {
    let mut cli = empty_cli_args(CommandKind::Doctor, transport_mode(args.transport.stdio));
    apply_transport_args(&mut cli, args.transport)?;
    cli.json_output = args.json;
    cli.doctor_live = args.live;
    let _ = (args.summary, args.no_color, args.ascii);
    Ok(cli)
}

fn apply_transport_args(cli: &mut CliArgs, transport: TransportCliArgs) -> Result<(), LunaError> {
    cli.no_daemon = transport.no_daemon;
    cli.websocket_url = normalize_optional_string(transport.ws_url, "--ws-url")?;
    cli.ws_auth_token = crate::websocket::resolve_ws_auth_token(
        transport.ws_auth_token.as_deref(),
        transport.ws_auth_token_file.as_deref(),
    )?;
    if cli.transport_mode == TransportMode::Stdio && cli.ws_auth_token.is_some() {
        return Err(LunaError::Usage(
            "--ws-auth-token/--ws-auth-token-file cannot be combined with --stdio".to_string(),
        ));
    }
    Ok(())
}

fn transport_mode(stdio: bool) -> TransportMode {
    if stdio {
        TransportMode::Stdio
    } else {
        TransportMode::WebSocket
    }
}

fn normalize_optional_string(
    value: Option<String>,
    flag: &str,
) -> Result<Option<String>, LunaError> {
    value
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(LunaError::Usage(format!(
                    "value for {flag} cannot be empty"
                )))
            } else {
                Ok(trimmed.to_string())
            }
        })
        .transpose()
}

fn empty_cli_args(command_kind: CommandKind, transport_mode: TransportMode) -> CliArgs {
    CliArgs {
        command_kind,
        no_daemon: false,
        agent: None,
        working_directory: None,
        websocket_url: None,
        ws_auth_token: None,
        model: None,
        model_provider: None,
        reasoning_effort: None,
        reasoning_summary: None,
        model_verbosity: None,
        service_tier: ServiceTier::Default,
        config_profile: None,
        approval_policy: None,
        sandbox_mode: None,
        sandbox_policy_json: None,
        sandbox_network_access_enabled: None,
        sandbox_writable_roots: Vec::new(),
        web_search_mode: None,
        dynamic_tools_json: None,
        personality: None,
        base_instructions: None,
        developer_instructions: None,
        ephemeral: None,
        experimental_raw_events: None,
        persist_extended_history: None,
        config_entries: Vec::new(),
        config_json: None,
        output_schema_json: None,
        output_schema_file: None,
        turn_extra_json: None,
        resume_target: None,
        sessions_all: false,
        final_response_only: false,
        json_output: false,
        doctor_live: false,
        transport_mode,
        prompt_parts: Vec::new(),
    }
}

fn parse_wire_enum<T: std::str::FromStr>(
    raw: &str,
    flag: &str,
    variants: &[&str],
) -> Result<T, LunaError> {
    T::from_str(raw.trim()).map_err(|_| {
        LunaError::Usage(format!(
            "invalid {flag} '{raw}'; expected one of: {}",
            variants.join(", ")
        ))
    })
}

fn normalize_session_id(raw: &str) -> Result<String, LunaError> {
    let session_id = raw.trim();
    if session_id.is_empty() {
        return Err(LunaError::Usage(
            "session id for --resume cannot be empty".to_string(),
        ));
    }
    Ok(session_id.to_string())
}

fn normalize_working_directory(raw: &str) -> Result<String, LunaError> {
    if raw.trim().is_empty() {
        return Err(LunaError::Usage(
            "working directory for --cwd cannot be empty".to_string(),
        ));
    }
    Ok(raw.to_string())
}

pub(crate) fn resolve_current_working_directory() -> Result<String, LunaError> {
    let cwd = env::current_dir().map_err(|error| {
        LunaError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    Ok(cwd.to_string_lossy().to_string())
}

pub(crate) fn resolve_prompt(prompt_parts: Vec<String>) -> Result<String, LunaError> {
    if !prompt_parts.is_empty() {
        return Ok(prompt_parts.join(" "));
    }

    if io::stdin().is_terminal() {
        return Err(LunaError::Usage(
            "missing prompt; pass text args or pipe stdin".to_string(),
        ));
    }

    let mut piped = String::new();
    io::stdin().read_to_string(&mut piped)?;
    let prompt = piped.trim();
    if prompt.is_empty() {
        return Err(LunaError::Usage("stdin prompt is empty".to_string()));
    }
    Ok(prompt.to_string())
}

#[cfg(test)]
#[allow(clippy::useless_conversion)]
mod tests {
    use super::*;
    use crate::config::{build_thread_config, load_agent_profile_from_config};
    use crate::connection::account_is_authenticated;
    use crate::output::thread_item_to_json;
    use crate::session::crop_preview_text;
    use crate::websocket::{DEFAULT_WS_URL, WebsocketUrlSource, resolve_websocket_url};
    use crate::*;
    use codex_app_server_sdk::ThreadItem;
    use serde_json::{Map, Value};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn parse_cli_args_supports_agent_flag_and_prompt() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--agent".to_string(),
                "writer".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.agent.as_deref(), Some("writer"));
        assert!(cli.working_directory.is_none());
        assert!(cli.resume_target.is_none());
        assert!(!cli.final_response_only);
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn chat_and_exec_share_the_exact_argument_contract() {
        let command = CliParser::command();
        let exec = command.find_subcommand("exec").expect("exec command");
        let chat = command.find_subcommand("chat").expect("chat command");
        let signature = |command: &clap::Command| {
            command
                .get_arguments()
                .map(|argument| {
                    (
                        argument.get_id().as_str().to_string(),
                        argument.get_long().map(str::to_string),
                        argument.get_short(),
                        format!("{:?}", argument.get_action()),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(signature(exec), signature(chat));

        let flags = [
            "--stdio",
            "--agent=writer",
            "--cwd=/tmp/project",
            "--model=gpt-custom",
            "--reasoning-effort=high",
            "--fast",
            "--sandbox=workspace-write",
            "--web-search-mode=live",
            "--final-response",
            "hello",
        ];
        let parse = |command: &str| {
            let ParsedCommand::Run(cli) = parse_cli_args(
                std::iter::once(command.to_string()).chain(flags.iter().map(|arg| arg.to_string())),
            )
            .expect("shared turn args") else {
                panic!("expected run command");
            };
            *cli
        };
        let mut exec = parse("exec");
        let chat = parse("chat");
        exec.command_kind = CommandKind::Chat;
        assert_eq!(exec, chat);
    }

    #[test]
    fn parse_cli_args_supports_equals_form() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--agent=writer".to_string(),
                "hi".to_string(),
                "there".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.agent.as_deref(), Some("writer"));
        assert!(cli.working_directory.is_none());
        assert!(cli.resume_target.is_none());
        assert!(!cli.final_response_only);
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hi", "there"]);
    }

    #[test]
    fn parse_cli_args_supports_final_response_flag() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--final-response".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert!(cli.resume_target.is_none());
        assert!(cli.working_directory.is_none());
        assert!(cli.final_response_only);
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello", "world"]);
    }

    #[test]
    fn parse_cli_args_supports_cwd_flag() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--cwd".to_string(),
                "/tmp/project".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.working_directory.as_deref(), Some("/tmp/project"));
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_cwd_equals_form() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--cwd=/tmp/project".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.working_directory.as_deref(), Some("/tmp/project"));
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_continue_short_flag() {
        let parsed = parse_cli_args(
            vec!["exec".to_string(), "-c".to_string(), "hello".to_string()].into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.resume_target, Some(ResumeTarget::Last));
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_continue_long_flag() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--continue".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.resume_target, Some(ResumeTarget::Last));
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_resume_short_flag() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "-r".to_string(),
                "session_123".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(
            cli.resume_target,
            Some(ResumeTarget::SessionId("session_123".to_string()))
        );
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_luna_reasoning_efforts() {
        for (raw, expected) in [
            ("max", ModelReasoningEffort::Max),
            ("ultra", ModelReasoningEffort::Ultra),
        ] {
            let parsed = parse_cli_args(
                vec![
                    "exec".to_string(),
                    format!("--reasoning-effort={raw}"),
                    "hello".to_string(),
                ]
                .into_iter(),
            )
            .expect("parse reasoning effort");
            let ParsedCommand::Run(cli) = parsed else {
                panic!("expected run command");
            };

            assert_eq!(cli.reasoning_effort, Some(expected));
        }
    }

    #[test]
    fn parse_cli_args_supports_sessions_command() {
        let parsed =
            parse_cli_args(vec!["sessions".to_string()].into_iter()).expect("parse sessions");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.command_kind, CommandKind::Sessions);
        assert!(!cli.sessions_all);
        assert!(cli.prompt_parts.is_empty());
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
    }

    #[test]
    fn parse_cli_args_supports_sessions_flag() {
        let parsed =
            parse_cli_args(vec!["--sessions".to_string()].into_iter()).expect("parse --sessions");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.command_kind, CommandKind::Sessions);
        assert!(!cli.sessions_all);
        assert!(cli.prompt_parts.is_empty());
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
    }

    #[test]
    fn parse_cli_args_supports_sessions_all_flag() {
        let parsed = parse_cli_args(vec!["sessions".to_string(), "--all".to_string()].into_iter())
            .expect("parse sessions --all");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.command_kind, CommandKind::Sessions);
        assert!(cli.sessions_all);
        assert!(cli.prompt_parts.is_empty());
    }

    #[test]
    fn parse_cli_args_supports_exec_command_and_strips_keyword() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--final-response".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse exec");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert!(cli.final_response_only);
        assert_eq!(cli.service_tier, ServiceTier::Default);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_fast_selects_fast_service_tier() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--fast".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse --fast");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.service_tier, ServiceTier::Fast);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_x_alias_for_exec() {
        let parsed = parse_cli_args(
            vec![
                "x".to_string(),
                "--final-response".to_string(),
                "solve".to_string(),
                "for".to_string(),
                "x".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse x alias");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.command_kind, CommandKind::Exec);
        assert!(cli.final_response_only);
        assert_eq!(cli.prompt_parts, vec!["solve", "for", "x"]);
    }

    #[test]
    fn parse_cli_args_supports_start_command_without_prompt() {
        let parsed = parse_cli_args(vec!["start".to_string()].into_iter()).expect("parse start");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert!(cli.prompt_parts.is_empty());
        assert_eq!(cli.command_kind, CommandKind::Start);
    }

    #[test]
    fn parse_cli_args_rejects_start_with_stdio() {
        let error = parse_cli_args(vec!["start".to_string(), "--stdio".to_string()].into_iter())
            .expect_err("start should reject stdio");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_start_with_no_daemon() {
        let error =
            parse_cli_args(vec!["start".to_string(), "--no-daemon".to_string()].into_iter())
                .expect_err("start should reject no-daemon");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_bare_prompt_without_exec() {
        let error = parse_cli_args(vec!["hello".to_string()].into_iter())
            .expect_err("bare prompt should be rejected");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_start_with_prompt_arguments() {
        let error = parse_cli_args(vec!["start".to_string(), "hello".to_string()].into_iter())
            .expect_err("start should reject prompt args");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_all_without_sessions() {
        let error = parse_cli_args(vec!["--all".to_string(), "hello".to_string()].into_iter())
            .expect_err("--all without sessions");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_sessions_with_prompt() {
        let error = parse_cli_args(vec!["sessions".to_string(), "hello".to_string()].into_iter())
            .expect_err("sessions prompt conflict");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn crop_preview_text_truncates_and_normalizes_whitespace() {
        let preview = crop_preview_text("hello   world from   luna", 12);
        assert_eq!(preview, "hello world ...");
    }

    #[test]
    fn resolve_websocket_url_prefers_explicit_value() {
        let resolved = resolve_websocket_url(
            Some("ws://127.0.0.1:5555"),
            Some("ws://127.0.0.1:4444"),
            Some("ws://127.0.0.1:3333"),
        )
        .expect("resolve explicit websocket url");
        assert_eq!(resolved.url, "ws://127.0.0.1:5555");
        assert_eq!(resolved.source, WebsocketUrlSource::Flag);
        assert!(!resolved.manage_daemon());
    }

    #[test]
    fn resolve_websocket_url_uses_new_env_before_legacy_env() {
        let resolved = resolve_websocket_url(
            None,
            Some("ws://127.0.0.1:4444"),
            Some("ws://127.0.0.1:3333"),
        )
        .expect("resolve env url");
        assert_eq!(resolved.url, "ws://127.0.0.1:4444");
        assert_eq!(resolved.source, WebsocketUrlSource::Environment);
        assert!(!resolved.manage_daemon());
    }

    #[test]
    fn resolve_websocket_url_keeps_legacy_env_as_fallback() {
        let resolved = resolve_websocket_url(None, None, Some("ws://127.0.0.1:3333"))
            .expect("resolve legacy env url");
        assert_eq!(resolved.url, "ws://127.0.0.1:3333");
        assert_eq!(resolved.source, WebsocketUrlSource::LegacyEnvironment);
        assert!(!resolved.manage_daemon());
    }

    #[test]
    fn resolve_websocket_url_falls_back_to_managed_default() {
        let resolved =
            resolve_websocket_url(None, None, None).expect("resolve default websocket url");
        assert_eq!(resolved.url, DEFAULT_WS_URL);
        assert_eq!(resolved.source, WebsocketUrlSource::Default);
        assert!(resolved.manage_daemon());
    }

    #[test]
    fn parse_cli_args_supports_doctor_json_and_live() {
        let parsed = parse_cli_args(
            vec![
                "doctor".to_string(),
                "--json".to_string(),
                "--live".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse doctor");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected doctor command");
        };
        assert_eq!(cli.command_kind, CommandKind::Doctor);
        assert!(cli.json_output);
        assert!(cli.doctor_live);
    }

    #[test]
    fn parse_cli_args_supports_generated_shell_completions() {
        let parsed = parse_cli_args(vec!["completions".to_string(), "zsh".to_string()].into_iter())
            .expect("parse completions");
        assert!(matches!(parsed, ParsedCommand::Completions(Shell::Zsh)));
    }

    #[test]
    fn declarative_help_includes_portable_first_run_commands() {
        let help = CliParser::command().render_long_help().to_string();
        assert!(help.contains("doctor"));
        assert!(help.contains("completions"));
        assert!(help.contains("Run one Codex turn"));

        let mut command = CliParser::command();
        let exec_help = command
            .find_subcommand_mut("exec")
            .expect("exec subcommand")
            .render_long_help()
            .to_string();
        assert!(exec_help.contains("--fast"));
    }

    #[test]
    fn parse_cli_args_supports_resume_equals_form() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--resume=session_123".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(
            cli.resume_target,
            Some(ResumeTarget::SessionId("session_123".to_string()))
        );
        assert_eq!(cli.transport_mode, TransportMode::WebSocket);
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_stdio_flag() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--stdio".to_string(),
                "hello".to_string(),
                "world".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.transport_mode, TransportMode::Stdio);
        assert_eq!(cli.prompt_parts, vec!["hello", "world"]);
    }

    #[test]
    fn parse_cli_args_supports_extended_configuration_flags() {
        let parsed = parse_cli_args(
            vec![
                "exec".to_string(),
                "--ws-url=ws://127.0.0.1:9999".to_string(),
                "--model".to_string(),
                "gpt-5-custom".to_string(),
                "--model-provider".to_string(),
                "sandboxed-provider".to_string(),
                "--reasoning-effort=high".to_string(),
                "--reasoning-summary=detailed".to_string(),
                "--model-verbosity=high".to_string(),
                "--config-profile=work".to_string(),
                "--approval-policy=on-failure".to_string(),
                "--sandbox=workspace-write".to_string(),
                "--sandbox-policy-json".to_string(),
                "{\"type\":\"workspaceWrite\"}".to_string(),
                "--sandbox-network-access-disabled".to_string(),
                "--sandbox-writable-root".to_string(),
                "/tmp/one".to_string(),
                "--sandbox-writable-root=/tmp/two".to_string(),
                "--web-search-mode=live".to_string(),
                "--dynamic-tools-json=[{\"name\":\"lookup\",\"description\":\"Lookup docs\",\"inputSchema\":{\"type\":\"object\"}}]".to_string(),
                "--personality=pragmatic".to_string(),
                "--base-instructions".to_string(),
                "base rules".to_string(),
                "--developer-instructions".to_string(),
                "dev rules".to_string(),
                "--ephemeral".to_string(),
                "--experimental-raw-events".to_string(),
                "--persist-extended-history".to_string(),
                "--config".to_string(),
                "feature.enabled=true".to_string(),
                "--config-json={\"provider\":\"local\"}".to_string(),
                "--output-schema-json={\"type\":\"object\"}".to_string(),
                "--turn-extra-json={\"customTurnFlag\":true}".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.websocket_url.as_deref(), Some("ws://127.0.0.1:9999"));
        assert_eq!(cli.model.as_deref(), Some("gpt-5-custom"));
        assert_eq!(cli.model_provider.as_deref(), Some("sandboxed-provider"));
        assert_eq!(cli.reasoning_effort, Some(ModelReasoningEffort::High));
        assert_eq!(cli.reasoning_summary, Some(ModelReasoningSummary::Detailed));
        assert_eq!(cli.model_verbosity, Some(ModelVerbosity::High));
        assert_eq!(cli.config_profile.as_deref(), Some("work"));
        assert_eq!(cli.approval_policy, Some(ApprovalMode::OnFailure));
        assert_eq!(cli.sandbox_mode, Some(SandboxMode::WorkspaceWrite));
        assert_eq!(
            cli.sandbox_policy_json.as_deref(),
            Some("{\"type\":\"workspaceWrite\"}")
        );
        assert_eq!(cli.sandbox_network_access_enabled, Some(false));
        assert_eq!(cli.sandbox_writable_roots, vec!["/tmp/one", "/tmp/two"]);
        assert_eq!(cli.web_search_mode, Some(WebSearchMode::Live));
        assert_eq!(
            cli.dynamic_tools_json.as_deref(),
            Some(
                "[{\"name\":\"lookup\",\"description\":\"Lookup docs\",\"inputSchema\":{\"type\":\"object\"}}]"
            )
        );
        assert_eq!(cli.personality, Some(Personality::Pragmatic));
        assert_eq!(cli.base_instructions.as_deref(), Some("base rules"));
        assert_eq!(cli.developer_instructions.as_deref(), Some("dev rules"));
        assert_eq!(cli.ephemeral, Some(true));
        assert_eq!(cli.experimental_raw_events, Some(true));
        assert_eq!(cli.persist_extended_history, Some(true));
        assert_eq!(cli.config_entries, vec!["feature.enabled=true"]);
        assert_eq!(cli.config_json.as_deref(), Some("{\"provider\":\"local\"}"));
        assert_eq!(
            cli.output_schema_json.as_deref(),
            Some("{\"type\":\"object\"}")
        );
        assert_eq!(
            cli.turn_extra_json.as_deref(),
            Some("{\"customTurnFlag\":true}")
        );
    }

    #[test]
    fn parse_cli_args_rejects_removed_network_access_flag() {
        let error = parse_cli_args(vec!["--network-access-enabled".to_string()].into_iter())
            .expect_err("removed network access flag");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn build_thread_config_rejects_silent_precedence_conflicts() {
        let error = build_thread_config(
            Some(r#"{"web_search":"cached"}"#.to_string()),
            Vec::new(),
            Some(WebSearchMode::Live),
            None,
            None,
            None,
            Vec::new(),
        )
        .expect_err("conflicting sources should fail");
        assert!(matches!(error, LunaError::Usage(_)));
        assert!(format!("{error}").contains("web_search"));
    }

    #[test]
    fn build_thread_config_allows_identical_values_from_multiple_sources() {
        let config = build_thread_config(
            Some(r#"{"web_search":"live"}"#.to_string()),
            Vec::new(),
            Some(WebSearchMode::Live),
            None,
            None,
            None,
            Vec::new(),
        )
        .expect("identical values should be accepted")
        .expect("config should exist");
        assert_eq!(config["web_search"], "live");
    }

    #[test]
    fn account_readiness_supports_current_and_legacy_app_server_shapes() {
        assert!(account_is_authenticated(&Map::from_iter([(
            "account".to_string(),
            serde_json::json!({ "type": "chatgpt" }),
        )])));
        assert!(account_is_authenticated(&Map::from_iter([(
            "isLoggedIn".to_string(),
            Value::Bool(true),
        )])));
        assert!(account_is_authenticated(&Map::from_iter([(
            "requiresOpenaiAuth".to_string(),
            Value::Bool(false),
        )])));
        assert!(!account_is_authenticated(&Map::from_iter([
            ("account".to_string(), Value::Null),
            ("requiresOpenaiAuth".to_string(), Value::Bool(true)),
        ])));
    }

    #[test]
    fn parse_cli_args_rejects_missing_resume_value() {
        let error = parse_cli_args(vec!["--resume".to_string()].into_iter())
            .expect_err("missing --resume value");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_conflicting_resume_flags() {
        let error = parse_cli_args(
            vec![
                "--continue".to_string(),
                "--resume".to_string(),
                "session_123".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("conflicting resume flags");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_duplicate_final_response_flag() {
        let error = parse_cli_args(
            vec![
                "--final-response".to_string(),
                "--final-response".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("duplicate final-response");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_duplicate_stdio_flag() {
        let error = parse_cli_args(vec!["--stdio".to_string(), "--stdio".to_string()].into_iter())
            .expect_err("duplicate stdio");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_ws_url_with_stdio() {
        let error = parse_cli_args(
            vec![
                "exec".to_string(),
                "--stdio".to_string(),
                "--ws-url".to_string(),
                "ws://127.0.0.1:9000".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("ws-url should conflict with stdio");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_accepts_ws_auth_token() {
        let ParsedCommand::Run(cli) = parse_cli_args(
            vec![
                "exec".to_string(),
                "--ws-auth-token".to_string(),
                "secret".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse auth token") else {
            panic!("expected run command");
        };
        assert_eq!(cli.ws_auth_token.as_deref(), Some("secret"));
    }

    #[test]
    fn parse_cli_args_rejects_ws_auth_token_with_stdio() {
        let error = parse_cli_args(
            vec![
                "exec".to_string(),
                "--stdio".to_string(),
                "--ws-auth-token".to_string(),
                "secret".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("auth token should conflict with stdio");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_multiple_output_schema_sources() {
        let error = parse_cli_args(
            vec![
                "exec".to_string(),
                "--output-schema-json={\"type\":\"object\"}".to_string(),
                "--output-schema-file".to_string(),
                "/tmp/schema.json".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("output schema source conflict");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_missing_cwd_value() {
        let error =
            parse_cli_args(vec!["--cwd".to_string()].into_iter()).expect_err("missing --cwd value");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_empty_cwd_value() {
        let error =
            parse_cli_args(vec!["--cwd=".to_string()].into_iter()).expect_err("empty --cwd value");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_duplicate_cwd_flag() {
        let error = parse_cli_args(
            vec![
                "--cwd".to_string(),
                "/tmp/a".to_string(),
                "--cwd=/tmp/b".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("duplicate cwd");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_unknown_option() {
        let error = parse_cli_args(vec!["--nope".to_string()].into_iter()).expect_err("invalid");
        assert!(matches!(error, LunaError::Usage(_)));
    }

    #[test]
    fn normalize_agent_name_trims_toml_extension() {
        let normalized = normalize_agent_name("reviewer.toml").expect("normalized");
        assert_eq!(normalized, "reviewer");
    }

    #[test]
    fn thread_item_to_json_includes_agent_message_phase() {
        let value = thread_item_to_json(&ThreadItem::AgentMessage(
            codex_app_server_sdk::AgentMessageItem {
                id: "msg_1".to_string(),
                text: "done".to_string(),
                phase: Some(codex_app_server_sdk::AgentMessagePhase::FinalAnswer),
            },
        ));

        assert_eq!(
            value,
            serde_json::json!({
                "type": "agentMessage",
                "id": "msg_1",
                "text": "done",
                "phase": "final_answer",
            })
        );
    }

    #[test]
    fn load_agent_profile_from_config_uses_developer_instructions() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let roles_dir = dir.join("roles");
        fs::create_dir_all(&roles_dir).expect("create roles dir");
        fs::write(
            &config_path,
            "\
[agents.reviewer]
description = \"Review code thoroughly\"
config_file = \"roles/reviewer.toml\"
",
        )
        .expect("write config");
        fs::write(
            roles_dir.join("reviewer.toml"),
            "\
model = \"gpt-5.6-luna\"
developer_instructions = \"Focus on correctness and tests.\"
",
        )
        .expect("write role config");

        let profile = load_agent_profile_from_config("reviewer", &config_path).expect("load");
        assert_eq!(profile.instructions, "Focus on correctness and tests.");

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_uses_model_instructions_file_when_needed() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let roles_dir = dir.join("roles");
        fs::create_dir_all(&roles_dir).expect("create roles dir");
        fs::write(
            &config_path,
            "\
[agents.reviewer]
config_file = \"roles/reviewer.toml\"
",
        )
        .expect("write config");
        fs::write(
            roles_dir.join("reviewer.toml"),
            "model_instructions_file = \"reviewer.md\"\n",
        )
        .expect("write role config");
        fs::write(
            roles_dir.join("reviewer.md"),
            "\nBe strict about regressions.\n",
        )
        .expect("write instructions file");

        let profile = load_agent_profile_from_config("reviewer", &config_path).expect("load");
        assert_eq!(profile.instructions, "Be strict about regressions.");

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_prefers_developer_instructions_over_model_file() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let roles_dir = dir.join("roles");
        fs::create_dir_all(&roles_dir).expect("create roles dir");
        fs::write(
            &config_path,
            "\
[agents.reviewer]
config_file = \"roles/reviewer.toml\"
",
        )
        .expect("write config");
        fs::write(
            roles_dir.join("reviewer.toml"),
            "\
developer_instructions = \"Prefer inline instructions\"
model_instructions_file = \"reviewer.md\"
",
        )
        .expect("write role config");
        fs::write(
            roles_dir.join("reviewer.md"),
            "Use file instructions instead.\n",
        )
        .expect("write instructions file");

        let profile = load_agent_profile_from_config("reviewer", &config_path).expect("load");
        assert_eq!(profile.instructions, "Prefer inline instructions");

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_falls_back_to_description() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let roles_dir = dir.join("roles");
        fs::create_dir_all(&roles_dir).expect("create roles dir");
        fs::write(
            &config_path,
            "\
[agents.reviewer]
description = \"Review code thoroughly\"
config_file = \"roles/reviewer.toml\"
",
        )
        .expect("write config");
        fs::write(
            roles_dir.join("reviewer.toml"),
            "model = \"gpt-5.6-luna\"\n",
        )
        .expect("write role config");

        let profile = load_agent_profile_from_config("reviewer", &config_path).expect("load");
        assert_eq!(profile.instructions, "Review code thoroughly");

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_rejects_missing_role() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        fs::write(
            &config_path,
            "[agents.default]\nconfig_file = \"roles/default.toml\"\n",
        )
        .expect("write config");

        let error =
            load_agent_profile_from_config("reviewer", &config_path).expect_err("missing role");
        assert!(matches!(error, LunaError::Config(_)));
        let message = format!("{error}");
        assert!(message.contains("agents.reviewer"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_rejects_missing_config_file_and_description() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        fs::write(&config_path, "[agents.reviewer]\n").expect("write config");

        let error = load_agent_profile_from_config("reviewer", &config_path)
            .expect_err("missing config_file and description");
        assert!(matches!(error, LunaError::Config(_)));
        let message = format!("{error}");
        assert!(message.contains("must set config_file or description"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_from_config_rejects_missing_model_instructions_file() {
        let dir = make_temp_dir();
        let config_path = dir.join("config.toml");
        let roles_dir = dir.join("roles");
        fs::create_dir_all(&roles_dir).expect("create roles dir");
        fs::write(
            &config_path,
            "\
[agents.reviewer]
config_file = \"roles/reviewer.toml\"
",
        )
        .expect("write config");
        fs::write(
            roles_dir.join("reviewer.toml"),
            "model_instructions_file = \"missing.md\"\n",
        )
        .expect("write role config");

        let error = load_agent_profile_from_config("reviewer", &config_path)
            .expect_err("missing model instructions file");
        assert!(matches!(error, LunaError::Config(_)));
        let message = format!("{error}");
        assert!(message.contains("failed to read model_instructions_file"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    fn make_temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let seq = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("luna-tests-{}-{stamp}-{seq}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
