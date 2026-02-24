use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

use codex_app_server_sdk::api::{
    Codex, ModelReasoningEffort, ThreadEvent, ThreadItem, ThreadOptions, ThreadRunError,
    TurnOptions,
};
use codex_app_server_sdk::{ClientError, StdioConfig, requests, responses};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const APP_NAME: &str = "spark";
const MODEL: &str = "gpt-5.3-codex-spark";
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:4222";
const THREAD_LIST_PAGE_LIMIT: u32 = 100;
const MAX_THREAD_LIST_PAGES: usize = 100;

const USAGE: &str = "\
Usage: spark [--agent NAME] [--cwd PATH] [--final-response] [--stdio] [-c | -r SESSION_ID] [PROMPT...]

Runs one turn with:
  model: gpt-5.3-codex-spark
  reasoning effort: xhigh
  transport: websocket (default, ws://127.0.0.1:4222)

Options:
  --agent NAME    Load ~/.codex/config.toml [agents.NAME]
  --cwd PATH      Set Codex working directory (default: current shell directory)
  --stdio        Use app-server stdio transport instead of websocket default
  -c, --continue
                 Resume the most recent recorded session (codex resume --last equivalent)
  -r, --resume ID
                 Resume the specified session id
  --final-response
                 Output only the final message text (no streamed deltas)
  -h, --help      Show this help

If PROMPT is omitted, spark reads the prompt from stdin.
";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResumeTarget {
    Last,
    SessionId(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportMode {
    WebSocket,
    Stdio,
}

#[derive(Debug)]
struct CliArgs {
    agent: Option<String>,
    working_directory: Option<String>,
    resume_target: Option<ResumeTarget>,
    final_response_only: bool,
    transport_mode: TransportMode,
    prompt_parts: Vec<String>,
}

#[derive(Debug)]
struct LoadedAgent {
    instructions: String,
}

#[derive(Debug, Deserialize)]
struct CodexConfigFile {
    #[serde(default)]
    agents: toml::Table,
}

#[derive(Debug, Deserialize)]
struct AgentRoleConfig {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    config_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgentConfigLayer {
    #[serde(default)]
    developer_instructions: Option<String>,
    #[serde(default)]
    model_instructions_file: Option<String>,
}

#[derive(Debug)]
enum ParsedCommand {
    Help,
    Run(CliArgs),
}

#[derive(Debug, Error)]
enum SparkError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    ThreadRun(#[from] ThreadRunError),
    #[error("failed to parse TOML in {}: {source}", .path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(SparkError::Usage(message)) => {
            eprintln!("{message}\n");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("{APP_NAME}: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), SparkError> {
    let command = parse_cli_args(env::args().skip(1))?;
    let cli = match command {
        ParsedCommand::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        ParsedCommand::Run(cli) => cli,
    };

    let CliArgs {
        agent,
        working_directory,
        resume_target,
        final_response_only,
        transport_mode,
        prompt_parts,
    } = cli;

    let prompt = resolve_prompt(prompt_parts)?;
    let working_directory = match working_directory {
        Some(path) => path,
        None => resolve_current_working_directory()?,
    };

    let mut thread_options = ThreadOptions::builder()
        .model(MODEL)
        .model_reasoning_effort(ModelReasoningEffort::XHigh)
        .working_directory(working_directory);
    if let Some(agent_name) = agent {
        let agent = load_agent_profile(&agent_name)?;
        thread_options = thread_options.developer_instructions(agent.instructions);
    }

    let codex = match transport_mode {
        TransportMode::WebSocket => connect_default_ws_codex().await?,
        TransportMode::Stdio => spawn_stdio_codex().await?,
    };
    let options = thread_options.build();
    let mut thread = match resume_target {
        Some(ResumeTarget::Last) => {
            let thread_id = resolve_last_session_id(&codex).await?;
            codex.resume_thread(thread_id, options)
        }
        Some(ResumeTarget::SessionId(session_id)) => codex.resume_thread(session_id, options),
        None => codex.start_thread(options),
    };

    if final_response_only {
        let final_response = thread.ask(prompt, TurnOptions::default()).await?;
        let mut stdout = io::stdout();
        let mut ended_with_newline = false;

        if !final_response.is_empty() {
            print_chunk(&mut stdout, &final_response)?;
            ended_with_newline = final_response.ends_with('\n');
        }
        ensure_message_separator(
            &mut stdout,
            !final_response.is_empty(),
            &mut ended_with_newline,
        )?;
        return Ok(());
    }

    let mut streamed = thread.run_streamed(prompt, TurnOptions::default()).await?;

    let mut stdout = io::stdout();
    let mut saw_terminal = false;
    let mut saw_delta_for_message = false;
    let mut printed_any = false;
    let mut ended_with_newline = false;

    while let Some(next) = streamed.next_event().await {
        let event = next?;
        match event {
            ThreadEvent::ItemUpdated { item } => {
                if let ThreadItem::AgentMessage(agent_message) = item {
                    if !agent_message.text.is_empty() {
                        print_chunk(&mut stdout, &agent_message.text)?;
                        saw_delta_for_message = true;
                        printed_any = true;
                        ended_with_newline = agent_message.text.ends_with('\n');
                    }
                }
            }
            ThreadEvent::ItemCompleted { item } => {
                if let ThreadItem::AgentMessage(agent_message) = item {
                    let mut message_had_output = saw_delta_for_message;
                    if !saw_delta_for_message && !agent_message.text.is_empty() {
                        print_chunk(&mut stdout, &agent_message.text)?;
                        printed_any = true;
                        ended_with_newline = agent_message.text.ends_with('\n');
                        message_had_output = true;
                    }
                    if message_had_output {
                        ensure_message_separator(
                            &mut stdout,
                            printed_any,
                            &mut ended_with_newline,
                        )?;
                    }
                    saw_delta_for_message = false;
                }
            }
            ThreadEvent::TurnCompleted { .. } => {
                saw_terminal = true;
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                return Err(SparkError::Config(format!(
                    "turn failed: {}",
                    error.message
                )));
            }
            ThreadEvent::Error { message } => {
                return Err(SparkError::Config(format!("stream error: {message}")));
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. } => {}
        }
    }

    if !saw_terminal {
        return Err(SparkError::Config(
            "stream closed before receiving turn completion".to_string(),
        ));
    }

    if printed_any {
        ensure_message_separator(&mut stdout, printed_any, &mut ended_with_newline)?;
    }

    Ok(())
}

async fn connect_default_ws_codex() -> Result<Codex, SparkError> {
    let client = CodexClient::connect_ws(WsConfig {
        url: DEFAULT_WS_URL.to_string(),
        env: Default::default(),
        options: ClientOptions::default(),
    })
    .await?;
    Ok(client.as_api())
}

async fn spawn_stdio_codex() -> Result<Codex, SparkError> {
    let codex_binary = resolve_codex_binary()?;
    let mut stdio_config = StdioConfig::default();
    stdio_config.codex_binary = codex_binary;
    Ok(Codex::spawn_stdio(stdio_config).await?)
}

async fn resolve_last_session_id(codex: &Codex) -> Result<String, SparkError> {
    let mut cursor: Option<String> = None;
    let mut pages_scanned = 0usize;
    let mut newest: Option<(i64, String)> = None;
    let mut fallback_id: Option<String> = None;

    loop {
        pages_scanned += 1;
        if pages_scanned > MAX_THREAD_LIST_PAGES {
            return Err(SparkError::Config(format!(
                "could not resolve latest session for --continue after scanning {MAX_THREAD_LIST_PAGES} pages"
            )));
        }

        let params = requests::ThreadListParams {
            limit: Some(THREAD_LIST_PAGE_LIMIT),
            cursor: cursor.clone(),
            ..Default::default()
        };
        let result = codex.thread_list(params).await?;

        for thread in result.data {
            if fallback_id.is_none() {
                fallback_id = Some(thread.id.clone());
            }
            if let Some(score) = thread_recency_score(&thread) {
                match &newest {
                    Some((best_score, _)) if score <= *best_score => {}
                    _ => newest = Some((score, thread.id)),
                }
            }
        }

        match result.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    if let Some((_, thread_id)) = newest {
        return Ok(thread_id);
    }
    if let Some(thread_id) = fallback_id {
        return Ok(thread_id);
    }

    Err(SparkError::Config(
        "no recorded sessions found for --continue; start a spark session first or use --resume <session_id>".to_string(),
    ))
}

fn thread_recency_score(thread: &responses::ThreadSummary) -> Option<i64> {
    parse_timestamp(thread.extra.get("updatedAt"))
        .or_else(|| parse_timestamp(thread.extra.get("createdAt")))
}

fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|raw| i64::try_from(raw).ok())),
        Value::String(raw) => raw.parse::<i64>().ok(),
        _ => None,
    }
}

fn resolve_codex_binary() -> Result<String, SparkError> {
    resolve_codex_binary_with(|program, args| {
        Command::new(program)
            .args(args)
            .output()
            .map(|output| CommandResult {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            })
    })
}

#[derive(Debug)]
struct CommandResult {
    success: bool,
    stdout: String,
}

fn resolve_codex_binary_with<F>(mut run_command: F) -> Result<String, SparkError>
where
    F: FnMut(&str, &[&str]) -> io::Result<CommandResult>,
{
    let result = run_command("which", &["codex"]).map_err(|error| {
        SparkError::Config(format!(
            "failed to resolve codex binary via `which codex`: {error}"
        ))
    })?;

    if !result.success {
        return Err(SparkError::Config(
            "could not locate `codex` on PATH (which codex returned non-zero status)".to_string(),
        ));
    }

    let resolved = result
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            SparkError::Config(
                "could not locate `codex` on PATH (which codex produced empty output)".to_string(),
            )
        })?;

    Ok(resolved.to_string())
}

fn print_chunk<W: Write>(writer: &mut W, chunk: &str) -> Result<(), SparkError> {
    write!(writer, "{chunk}")?;
    writer.flush()?;
    Ok(())
}

fn ensure_message_separator<W: Write>(
    writer: &mut W,
    printed_any: bool,
    ended_with_newline: &mut bool,
) -> Result<(), SparkError> {
    if printed_any && !*ended_with_newline {
        writeln!(writer)?;
        writer.flush()?;
        *ended_with_newline = true;
    }
    Ok(())
}

fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<ParsedCommand, SparkError> {
    let mut agent: Option<String> = None;
    let mut working_directory: Option<String> = None;
    let mut resume_target: Option<ResumeTarget> = None;
    let mut final_response_only = false;
    let mut transport_mode = TransportMode::WebSocket;
    let mut prompt_parts = Vec::new();
    let mut parse_options = true;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        if parse_options {
            if arg == "--" {
                parse_options = false;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Ok(ParsedCommand::Help);
            }
            if arg == "-c" || arg == "--continue" {
                set_resume_target(&mut resume_target, ResumeTarget::Last)?;
                continue;
            }
            if arg == "-r" || arg == "--resume" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --resume".to_string()))?;
                set_resume_target(
                    &mut resume_target,
                    ResumeTarget::SessionId(normalize_session_id(&raw)?),
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--resume=") {
                set_resume_target(
                    &mut resume_target,
                    ResumeTarget::SessionId(normalize_session_id(raw)?),
                )?;
                continue;
            }
            if arg == "--agent" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --agent".to_string()))?;
                if agent.is_some() {
                    return Err(SparkError::Usage(
                        "--agent may only be provided once".to_string(),
                    ));
                }
                agent = Some(normalize_agent_name(&raw)?);
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--agent=") {
                if agent.is_some() {
                    return Err(SparkError::Usage(
                        "--agent may only be provided once".to_string(),
                    ));
                }
                agent = Some(normalize_agent_name(raw)?);
                continue;
            }
            if arg == "--cwd" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --cwd".to_string()))?;
                set_working_directory(&mut working_directory, &raw)?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--cwd=") {
                set_working_directory(&mut working_directory, raw)?;
                continue;
            }
            if arg == "--final-response" {
                if final_response_only {
                    return Err(SparkError::Usage(
                        "--final-response may only be provided once".to_string(),
                    ));
                }
                final_response_only = true;
                continue;
            }
            if arg == "--stdio" {
                if transport_mode == TransportMode::Stdio {
                    return Err(SparkError::Usage(
                        "--stdio may only be provided once".to_string(),
                    ));
                }
                transport_mode = TransportMode::Stdio;
                continue;
            }
            if arg.starts_with('-') {
                return Err(SparkError::Usage(format!("unknown option: {arg}")));
            }
        }

        prompt_parts.push(arg);
    }

    Ok(ParsedCommand::Run(CliArgs {
        agent,
        working_directory,
        resume_target,
        final_response_only,
        transport_mode,
        prompt_parts,
    }))
}

fn set_working_directory(slot: &mut Option<String>, raw: &str) -> Result<(), SparkError> {
    if slot.is_some() {
        return Err(SparkError::Usage(
            "--cwd may only be provided once".to_string(),
        ));
    }
    *slot = Some(normalize_working_directory(raw)?);
    Ok(())
}

fn set_resume_target(
    slot: &mut Option<ResumeTarget>,
    target: ResumeTarget,
) -> Result<(), SparkError> {
    if slot.is_some() {
        return Err(SparkError::Usage(
            "only one of --continue and --resume may be provided".to_string(),
        ));
    }
    *slot = Some(target);
    Ok(())
}

fn normalize_session_id(raw: &str) -> Result<String, SparkError> {
    let session_id = raw.trim();
    if session_id.is_empty() {
        return Err(SparkError::Usage(
            "session id for --resume cannot be empty".to_string(),
        ));
    }
    Ok(session_id.to_string())
}

fn normalize_agent_name(raw: &str) -> Result<String, SparkError> {
    let candidate = raw.trim();
    let candidate = candidate.strip_suffix(".toml").unwrap_or(candidate);
    let candidate = candidate.strip_suffix(".md").unwrap_or(candidate);
    if candidate.is_empty() {
        return Err(SparkError::Usage("agent name cannot be empty".to_string()));
    }
    if candidate == "." || candidate == ".." {
        return Err(SparkError::Usage(
            "agent name cannot be '.' or '..'".to_string(),
        ));
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(SparkError::Usage(
            "agent name cannot include path separators".to_string(),
        ));
    }
    if !candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(SparkError::Usage(
            "agent name may only contain [A-Za-z0-9._-]".to_string(),
        ));
    }
    Ok(candidate.to_string())
}

fn normalize_working_directory(raw: &str) -> Result<String, SparkError> {
    if raw.trim().is_empty() {
        return Err(SparkError::Usage(
            "working directory for --cwd cannot be empty".to_string(),
        ));
    }
    Ok(raw.to_string())
}

fn resolve_current_working_directory() -> Result<String, SparkError> {
    let cwd = env::current_dir().map_err(|error| {
        SparkError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    Ok(cwd.to_string_lossy().to_string())
}

fn resolve_prompt(prompt_parts: Vec<String>) -> Result<String, SparkError> {
    if !prompt_parts.is_empty() {
        return Ok(prompt_parts.join(" "));
    }

    if io::stdin().is_terminal() {
        return Err(SparkError::Usage(
            "missing prompt; pass text args or pipe stdin".to_string(),
        ));
    }

    let mut piped = String::new();
    io::stdin().read_to_string(&mut piped)?;
    let prompt = piped.trim();
    if prompt.is_empty() {
        return Err(SparkError::Usage("stdin prompt is empty".to_string()));
    }
    Ok(prompt.to_string())
}

fn load_agent_profile(agent_name: &str) -> Result<LoadedAgent, SparkError> {
    let codex_home = resolve_codex_home_dir().ok_or_else(|| {
        SparkError::Config(
            "unable to resolve Codex home directory (expected CODEX_HOME, HOME, USERPROFILE, or HOMEDRIVE+HOMEPATH)"
                .to_string(),
        )
    })?;
    let config_path = codex_home.join("config.toml");
    load_agent_profile_from_config(agent_name, &config_path)
}

fn resolve_codex_home_dir() -> Option<PathBuf> {
    if let Some(codex_home) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(codex_home));
    }
    resolve_home_dir().map(|home| home.join(".codex"))
}

fn resolve_home_dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(userprofile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(userprofile));
    }
    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) if !drive.is_empty() && !path.is_empty() => {
            let mut resolved = PathBuf::from(drive);
            resolved.push(path);
            Some(resolved)
        }
        _ => None,
    }
}

fn load_agent_profile_from_config(
    agent_name: &str,
    config_path: &Path,
) -> Result<LoadedAgent, SparkError> {
    let config_raw = fs::read_to_string(config_path).map_err(|error| {
        SparkError::Config(format!(
            "failed to read Codex config file {}: {error}",
            config_path.display()
        ))
    })?;
    let config: CodexConfigFile =
        toml::from_str(&config_raw).map_err(|source| SparkError::Toml {
            path: config_path.to_path_buf(),
            source,
        })?;
    let role_value = config.agents.get(agent_name).ok_or_else(|| {
        SparkError::Config(format!(
            "agent '{}' was not found in {} under [agents.{}]",
            agent_name,
            config_path.display(),
            agent_name
        ))
    })?;
    let role: AgentRoleConfig = role_value.clone().try_into().map_err(|source| {
        SparkError::Config(format!(
            "invalid [agents.{agent_name}] in {}: {source}",
            config_path.display()
        ))
    })?;
    let instructions = if let Some(config_file) = role
        .config_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let role_config_path = resolve_path_from_file(config_path, config_file);
        let role_config_raw = fs::read_to_string(&role_config_path).map_err(|error| {
            SparkError::Config(format!(
                "failed to read agent config file for '{}' at {}: {error}",
                agent_name,
                role_config_path.display()
            ))
        })?;
        let role_config: AgentConfigLayer =
            toml::from_str(&role_config_raw).map_err(|source| SparkError::Toml {
                path: role_config_path.clone(),
                source,
            })?;
        resolve_agent_instructions(agent_name, &role, &role_config, &role_config_path)?
    } else if let Some(description) = role
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        description.to_string()
    } else {
        return Err(SparkError::Config(format!(
            "[agents.{agent_name}] in {} must set config_file or description",
            config_path.display()
        )));
    };

    Ok(LoadedAgent { instructions })
}

fn resolve_agent_instructions(
    agent_name: &str,
    role: &AgentRoleConfig,
    role_config: &AgentConfigLayer,
    role_config_path: &Path,
) -> Result<String, SparkError> {
    if let Some(instructions) = role_config
        .developer_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(instructions.to_string());
    }

    if let Some(model_instructions_file) = role_config
        .model_instructions_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let instructions_path = resolve_path_from_file(role_config_path, model_instructions_file);
        let instructions = fs::read_to_string(&instructions_path).map_err(|error| {
            SparkError::Config(format!(
                "failed to read model_instructions_file for '{}' at {}: {error}",
                agent_name,
                instructions_path.display()
            ))
        })?;
        let trimmed = instructions.trim();
        if trimmed.is_empty() {
            return Err(SparkError::Config(format!(
                "model_instructions_file for '{}' at {} is empty",
                agent_name,
                instructions_path.display()
            )));
        }
        return Ok(trimmed.to_string());
    }

    if let Some(description) = role
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(description.to_string());
    }

    Err(SparkError::Config(format!(
        "agent '{}' config {} must set `developer_instructions` or `model_instructions_file`",
        agent_name,
        role_config_path.display()
    )))
}

fn resolve_path_from_file(file_path: &Path, raw_path: &str) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        file_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_cli_args_supports_agent_flag_and_prompt() {
        let parsed = parse_cli_args(
            vec![
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
    fn parse_cli_args_supports_equals_form() {
        let parsed = parse_cli_args(
            vec![
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
        let parsed =
            parse_cli_args(vec!["--cwd=/tmp/project".to_string(), "hello".to_string()].into_iter())
                .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.working_directory.as_deref(), Some("/tmp/project"));
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_continue_short_flag() {
        let parsed = parse_cli_args(vec!["-c".to_string(), "hello".to_string()].into_iter())
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
        let parsed =
            parse_cli_args(vec!["--continue".to_string(), "hello".to_string()].into_iter())
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
    fn parse_cli_args_supports_resume_equals_form() {
        let parsed = parse_cli_args(
            vec!["--resume=session_123".to_string(), "hello".to_string()].into_iter(),
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
    fn parse_cli_args_rejects_missing_resume_value() {
        let error = parse_cli_args(vec!["--resume".to_string()].into_iter())
            .expect_err("missing --resume value");
        assert!(matches!(error, SparkError::Usage(_)));
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
        assert!(matches!(error, SparkError::Usage(_)));
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
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_duplicate_stdio_flag() {
        let error = parse_cli_args(vec!["--stdio".to_string(), "--stdio".to_string()].into_iter())
            .expect_err("duplicate stdio");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_missing_cwd_value() {
        let error =
            parse_cli_args(vec!["--cwd".to_string()].into_iter()).expect_err("missing --cwd value");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_empty_cwd_value() {
        let error =
            parse_cli_args(vec!["--cwd=".to_string()].into_iter()).expect_err("empty --cwd value");
        assert!(matches!(error, SparkError::Usage(_)));
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
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_unknown_option() {
        let error = parse_cli_args(vec!["--nope".to_string()].into_iter()).expect_err("invalid");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn normalize_agent_name_trims_toml_extension() {
        let normalized = normalize_agent_name("reviewer.toml").expect("normalized");
        assert_eq!(normalized, "reviewer");
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
model = \"gpt-5.3-codex\"
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
            "model = \"gpt-5.3-codex\"\n",
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
        assert!(matches!(error, SparkError::Config(_)));
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
        assert!(matches!(error, SparkError::Config(_)));
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
        assert!(matches!(error, SparkError::Config(_)));
        let message = format!("{error}");
        assert!(message.contains("failed to read model_instructions_file"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn resolve_codex_binary_parses_trimmed_path() {
        let resolved = resolve_codex_binary_with(|program, args| {
            assert_eq!(program, "which");
            assert_eq!(args, ["codex"]);
            Ok(CommandResult {
                success: true,
                stdout: " /usr/local/bin/codex  \n".to_string(),
            })
        })
        .expect("resolve codex");

        assert_eq!(resolved, "/usr/local/bin/codex");
    }

    #[test]
    fn resolve_codex_binary_uses_first_non_empty_line() {
        let resolved = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: true,
                stdout: "\n/usr/bin/codex\n/opt/bin/codex\n".to_string(),
            })
        })
        .expect("resolve codex");

        assert_eq!(resolved, "/usr/bin/codex");
    }

    #[test]
    fn resolve_codex_binary_errors_when_which_fails() {
        let error = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: false,
                stdout: String::new(),
            })
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    #[test]
    fn resolve_codex_binary_errors_on_empty_output() {
        let error = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: true,
                stdout: "   \n".to_string(),
            })
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    #[test]
    fn resolve_codex_binary_errors_when_command_cannot_run() {
        let error = resolve_codex_binary_with(|_, _| {
            Err(io::Error::new(io::ErrorKind::NotFound, "which not found"))
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    #[test]
    fn ensure_message_separator_adds_newline_when_missing() {
        let mut output = b"hello".to_vec();
        let mut ended_with_newline = false;

        ensure_message_separator(&mut output, true, &mut ended_with_newline)
            .expect("separator write");

        assert_eq!(output, b"hello\n");
        assert!(ended_with_newline);
    }

    #[test]
    fn ensure_message_separator_skips_when_output_already_newline_terminated() {
        let mut output = b"hello\n".to_vec();
        let mut ended_with_newline = true;

        ensure_message_separator(&mut output, true, &mut ended_with_newline)
            .expect("separator write");

        assert_eq!(output, b"hello\n");
        assert!(ended_with_newline);
    }

    fn make_temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = env::temp_dir().join(format!("spark-tests-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
