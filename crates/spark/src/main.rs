use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

use codex_app_server_sdk::api::{
    ApprovalMode, Codex, DynamicToolSpec, ModelReasoningEffort, ModelReasoningSummary, Personality,
    SandboxMode, StreamedTurn, ThreadEvent, ThreadItem, ThreadOptions, ThreadRunError, TurnOptions,
    WebSearchMode,
};
use codex_app_server_sdk::{ClientError, StdioConfig, requests, responses};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};
use serde::Deserialize;
use serde_json::{Map, Value};
use thiserror::Error;

const APP_NAME: &str = "spark";
const MODEL: &str = "gpt-5.3-codex-spark";
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:4222";
const THREAD_LIST_PAGE_LIMIT: u32 = 100;
const MAX_THREAD_LIST_PAGES: usize = 100;
const SESSION_PREVIEW_CHAR_LIMIT: usize = 96;

const USAGE: &str = "\
Usage:
  spark [OPTIONS] [PROMPT...]
  spark sessions [--all] [--ws-url URL | --stdio]

Runs one turn with:
  model default: gpt-5.3-codex-spark
  reasoning effort default: xhigh
  transport default: websocket (ws://127.0.0.1:4222)

Commands:
  sessions                         List recorded sessions ordered by last activity
                                   (default: sessions from current directory; use --all for all)

Options:
  --agent NAME                     Load ~/.codex/config.toml [agents.NAME]
  --cwd PATH                       Set Codex working directory (default: current shell directory)
  --ws-url URL                     Set websocket URL (default: ws://127.0.0.1:4222)
  --stdio                          Use app-server stdio transport instead of websocket
  --model MODEL                    Override model (default: gpt-5.3-codex-spark)
  --model-provider PROVIDER        Override model provider
  --reasoning-effort LEVEL         Override reasoning effort: none|minimal|low|medium|high|xhigh
  --reasoning-summary MODE         Override reasoning summary: none|auto|concise|detailed
  --model-verbosity LEVEL          Set model verbosity via config: low|medium|high
  --config-profile NAME            Set config profile override
  --approval-policy MODE           Set approval policy: never|on-request|on-failure|untrusted
  --sandbox MODE                   Set sandbox mode: read-only|workspace-write|danger-full-access
  --sandbox-policy-json JSON       Set sandbox policy JSON payload
  --sandbox-network-access-enabled Enable workspace-write network access in config
  --sandbox-network-access-disabled Disable workspace-write network access in config
  --sandbox-writable-root PATH     Add workspace-write writable root (repeatable)
  --web-search-mode MODE           Set web search mode: disabled|cached|live
  --dynamic-tools-json JSON        Set thread/start dynamicTools JSON array
  --personality MODE               Set personality: none|friendly|pragmatic
  --base-instructions TEXT         Set base instructions
  --developer-instructions TEXT    Set developer instructions (overrides --agent instructions)
  --ephemeral                      Run without persisting session files
  --experimental-raw-events        Enable raw response item events
  --persist-extended-history       Persist extended history for resume/fork/read
  --config KEY=VALUE               Set config override (VALUE parsed as JSON when valid)
  --config-json JSON               Merge config object JSON into thread config
  --output-schema FILE             Load turn output schema JSON from file (codex exec compat)
  --output-schema-json JSON        Set turn output schema JSON
  --output-schema-file PATH        Load turn output schema JSON from file
  --turn-extra-json JSON           Merge object JSON into turn/start raw extras
  -c, --continue
                                  Resume the most recent recorded session
  -r, --resume ID
                                  Resume the specified session id
  --final-response
                                  Output only the final message text (no streamed deltas)
  --json                           Print events to stdout as JSONL (codex exec compat)
  --all                            Show all sessions (used with sessions command)
  -h, --help                       Show this help

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
    websocket_url: Option<String>,
    model: Option<String>,
    model_provider: Option<String>,
    reasoning_effort: Option<ModelReasoningEffort>,
    reasoning_summary: Option<ModelReasoningSummary>,
    model_verbosity: Option<ModelVerbosity>,
    config_profile: Option<String>,
    approval_policy: Option<ApprovalMode>,
    sandbox_mode: Option<SandboxMode>,
    sandbox_policy_json: Option<String>,
    sandbox_network_access_enabled: Option<bool>,
    sandbox_writable_roots: Vec<String>,
    web_search_mode: Option<WebSearchMode>,
    dynamic_tools_json: Option<String>,
    personality: Option<Personality>,
    base_instructions: Option<String>,
    developer_instructions: Option<String>,
    ephemeral: Option<bool>,
    experimental_raw_events: Option<bool>,
    persist_extended_history: Option<bool>,
    config_entries: Vec<String>,
    config_json: Option<String>,
    output_schema_json: Option<String>,
    output_schema_file: Option<String>,
    turn_extra_json: Option<String>,
    resume_target: Option<ResumeTarget>,
    sessions: bool,
    sessions_all: bool,
    final_response_only: bool,
    json_output: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelVerbosity {
    Low,
    Medium,
    High,
}

impl ModelVerbosity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CliDynamicToolSpec {
    name: String,
    description: String,
    input_schema: Value,
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
        websocket_url,
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
        sessions: show_sessions,
        sessions_all,
        final_response_only,
        json_output,
        transport_mode,
        prompt_parts,
    } = cli;

    let websocket_url = websocket_url.unwrap_or_else(|| DEFAULT_WS_URL.to_string());
    if show_sessions {
        let cwd_filter = if sessions_all {
            None
        } else {
            let cwd = working_directory
                .clone()
                .map(Ok)
                .unwrap_or_else(resolve_current_working_directory)?;
            Some(cwd)
        };
        let codex = match transport_mode {
            TransportMode::WebSocket => connect_ws_codex(&websocket_url).await?,
            TransportMode::Stdio => spawn_stdio_codex().await?,
        };
        list_sessions(&codex, cwd_filter.as_deref()).await?;
        return Ok(());
    }

    let websocket_url_for_connect = websocket_url.clone();
    let connect_task = tokio::spawn(async move {
        match transport_mode {
            TransportMode::WebSocket => connect_ws_codex(&websocket_url_for_connect).await,
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
    turn_options = turn_options
        .model_reasoning_effort(reasoning_effort.unwrap_or(ModelReasoningEffort::XHigh));
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

    let codex = connect_task
        .await
        .map_err(|error| SparkError::Config(format!("transport connect task failed: {error}")))??;

    ensure_authenticated(&codex).await?;

    let options = thread_options.build();
    let turn_options = turn_options.build();
    let mut thread = match resume_target {
        Some(ResumeTarget::Last) => {
            let thread_id = resolve_last_session_id(&codex).await?;
            codex.resume_thread(thread_id, options)
        }
        Some(ResumeTarget::SessionId(session_id)) => codex.resume_thread(session_id, options),
        None => codex.start_thread(options),
    };

    if final_response_only {
        let final_response = thread.ask(prompt, turn_options).await?;
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

    let mut streamed = thread.run_streamed(prompt, turn_options).await?;

    if json_output {
        stream_json_events(&mut streamed).await?;
    } else {
        stream_text_events(&mut streamed).await?;
    }

    Ok(())
}

async fn stream_text_events(streamed: &mut StreamedTurn) -> Result<(), SparkError> {
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

async fn stream_json_events(streamed: &mut StreamedTurn) -> Result<(), SparkError> {
    let mut stdout = io::stdout();

    while let Some(next) = streamed.next_event().await {
        let event = next?;
        let json_event = match &event {
            ThreadEvent::ThreadStarted { thread_id } => {
                serde_json::json!({ "type": "thread.started", "threadId": thread_id })
            }
            ThreadEvent::TurnStarted => {
                serde_json::json!({ "type": "turn.started" })
            }
            ThreadEvent::TurnCompleted { usage } => {
                let mut obj = serde_json::json!({ "type": "turn.completed" });
                if let Some(usage) = usage {
                    obj["usage"] = serde_json::json!({
                        "inputTokens": usage.input_tokens,
                        "cachedInputTokens": usage.cached_input_tokens,
                        "outputTokens": usage.output_tokens,
                    });
                }
                obj
            }
            ThreadEvent::TurnFailed { error } => {
                serde_json::json!({ "type": "turn.failed", "error": { "message": error.message } })
            }
            ThreadEvent::ItemStarted { item } => {
                serde_json::json!({ "type": "item.started", "item": thread_item_to_json(item) })
            }
            ThreadEvent::ItemUpdated { item } => {
                serde_json::json!({ "type": "item.updated", "item": thread_item_to_json(item) })
            }
            ThreadEvent::ItemCompleted { item } => {
                serde_json::json!({ "type": "item.completed", "item": thread_item_to_json(item) })
            }
            ThreadEvent::Error { message } => {
                serde_json::json!({ "type": "error", "message": message })
            }
        };

        let line = serde_json::to_string(&json_event)
            .map_err(|err| SparkError::Config(format!("failed to serialize event: {err}")))?;
        writeln!(stdout, "{line}")?;

        match event {
            ThreadEvent::TurnCompleted { .. } => break,
            ThreadEvent::TurnFailed { error } => {
                return Err(SparkError::Config(format!(
                    "turn failed: {}",
                    error.message
                )));
            }
            ThreadEvent::Error { message } => {
                return Err(SparkError::Config(format!("stream error: {message}")));
            }
            _ => {}
        }
    }

    Ok(())
}

fn thread_item_to_json(item: &ThreadItem) -> Value {
    match item {
        ThreadItem::AgentMessage(msg) => serde_json::json!({
            "type": "agentMessage",
            "id": msg.id,
            "text": msg.text,
        }),
        ThreadItem::Reasoning(r) => serde_json::json!({
            "type": "reasoning",
            "id": r.id,
            "text": r.text,
        }),
        ThreadItem::CommandExecution(cmd) => serde_json::json!({
            "type": "commandExecution",
            "id": cmd.id,
            "command": cmd.command,
            "aggregatedOutput": cmd.aggregated_output,
            "exitCode": cmd.exit_code,
            "status": format!("{:?}", cmd.status),
        }),
        ThreadItem::FileChange(fc) => {
            let changes: Vec<Value> = fc
                .changes
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "path": c.path,
                        "kind": format!("{:?}", c.kind),
                    })
                })
                .collect();
            serde_json::json!({
                "type": "fileChange",
                "id": fc.id,
                "changes": changes,
                "status": format!("{:?}", fc.status),
            })
        }
        ThreadItem::McpToolCall(mcp) => serde_json::json!({
            "type": "mcpToolCall",
            "id": mcp.id,
            "server": mcp.server,
            "tool": mcp.tool,
            "arguments": mcp.arguments,
            "result": mcp.result,
            "error": mcp.error.as_ref().map(|e| &e.message),
            "status": format!("{:?}", mcp.status),
        }),
        ThreadItem::WebSearch(ws) => serde_json::json!({
            "type": "webSearch",
            "id": ws.id,
            "query": ws.query,
        }),
        ThreadItem::TodoList(todo) => {
            let items: Vec<Value> = todo
                .items
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "text": t.text,
                        "completed": t.completed,
                    })
                })
                .collect();
            serde_json::json!({
                "type": "todoList",
                "id": todo.id,
                "items": items,
            })
        }
        ThreadItem::Error(err) => serde_json::json!({
            "type": "error",
            "id": err.id,
            "message": err.message,
        }),
        ThreadItem::Unknown(u) => serde_json::json!({
            "type": "unknown",
            "id": u.id,
            "itemType": u.item_type,
            "raw": u.raw,
        }),
    }
}

async fn connect_ws_codex(url: &str) -> Result<Codex, SparkError> {
    let client = CodexClient::connect_ws(WsConfig {
        url: url.to_string(),
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

/// Ensure the codex session is authenticated using a cascading strategy:
/// 1. Rely on existing cached auth (auth.json) — check via `account_read`.
/// 2. If not authenticated, try `CODEX_ID_TOKEN` + `CODEX_ACCESS_TOKEN` env vars (ChatGPT login).
/// 3. If those aren't available, try `OPENAI_API_KEY` env var (API key login).
async fn ensure_authenticated(codex: &Codex) -> Result<(), SparkError> {
    // Check if the app-server already has valid cached auth.
    let account = codex
        .account_read(requests::GetAccountParams::default())
        .await?;
    if account.extra.get("isLoggedIn") == Some(&Value::Bool(true)) {
        return Ok(());
    }

    // Fallback 1: ChatGPT Pro bearer tokens from env.
    if let (Ok(id_token), Ok(access_token)) =
        (env::var("CODEX_ID_TOKEN"), env::var("CODEX_ACCESS_TOKEN"))
    {
        codex
            .account_login_start(requests::LoginAccountParams::chatgpt(
                id_token,
                access_token,
            ))
            .await?;
        return Ok(());
    }

    // Fallback 2: OpenAI API key from env.
    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        codex
            .account_login_start(requests::LoginAccountParams::api_key(api_key))
            .await?;
        return Ok(());
    }

    Ok(())
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

#[derive(Debug)]
struct SessionListEntry {
    id: String,
    recency_score: i64,
    last_message: String,
}

async fn list_sessions(codex: &Codex, cwd_filter: Option<&str>) -> Result<(), SparkError> {
    let mut cursor: Option<String> = None;
    let mut pages_scanned = 0usize;
    let mut sessions = Vec::new();

    loop {
        pages_scanned += 1;
        if pages_scanned > MAX_THREAD_LIST_PAGES {
            return Err(SparkError::Config(format!(
                "could not list sessions after scanning {MAX_THREAD_LIST_PAGES} pages"
            )));
        }

        let params = requests::ThreadListParams {
            limit: Some(THREAD_LIST_PAGE_LIMIT),
            cursor: cursor.clone(),
            ..Default::default()
        };
        let result = codex.thread_list(params).await?;

        for thread in result.data {
            if let Some(filter_cwd) = cwd_filter {
                let thread_cwd = thread.extra.get("cwd").and_then(|v| v.as_str());
                match thread_cwd {
                    Some(cwd) if cwd == filter_cwd => {}
                    _ => continue,
                }
            }

            let recency_score = thread_recency_score(&thread).unwrap_or(i64::MIN);
            let last_message = resolve_last_message_preview(codex, &thread).await?;
            sessions.push(SessionListEntry {
                id: thread.id,
                recency_score,
                last_message,
            });
        }

        match result.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    sessions.sort_by(|left, right| {
        right
            .recency_score
            .cmp(&left.recency_score)
            .then_with(|| left.id.cmp(&right.id))
    });

    if sessions.is_empty() {
        match cwd_filter {
            Some(cwd) => {
                println!("No recorded sessions found for {cwd}. Use --all to show all sessions.")
            }
            None => println!("No recorded sessions found."),
        }
        return Ok(());
    }

    println!("SESSION_ID\tLAST_MESSAGE");
    for session in sessions {
        let preview = crop_preview_text(&session.last_message, SESSION_PREVIEW_CHAR_LIMIT);
        println!("{}\t{}", session.id, preview);
    }

    Ok(())
}

async fn resolve_last_message_preview(
    codex: &Codex,
    thread: &responses::ThreadSummary,
) -> Result<String, SparkError> {
    if let Some(preview) = extract_summary_preview(&thread.extra) {
        return Ok(preview);
    }

    let read_result = codex
        .thread_read(requests::ThreadReadParams {
            thread_id: thread.id.clone(),
            include_turns: Some(true),
            extra: Map::new(),
        })
        .await?;

    Ok(extract_last_message_from_thread_read(&read_result.extra).unwrap_or_default())
}

fn extract_summary_preview(extra: &Map<String, Value>) -> Option<String> {
    for key in [
        "lastMessage",
        "lastAgentMessage",
        "lastAssistantMessage",
        "snippet",
        "preview",
    ] {
        if let Some(text) = extra.get(key).and_then(value_to_text) {
            return Some(text);
        }
    }
    None
}

fn extract_last_message_from_thread_read(extra: &Map<String, Value>) -> Option<String> {
    let mut last_assistant: Option<String> = None;
    let mut last_any: Option<String> = None;

    if let Some(items) = extra.get("items").and_then(Value::as_array) {
        let (assistant, any) = extract_last_message_from_items(items);
        if assistant.is_some() {
            last_assistant = assistant;
        }
        if any.is_some() {
            last_any = any;
        }
    }

    if let Some(turns) = extra.get("turns").and_then(Value::as_array) {
        for turn in turns {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            let (assistant, any) = extract_last_message_from_items(items);
            if assistant.is_some() {
                last_assistant = assistant;
            }
            if any.is_some() {
                last_any = any;
            }
        }
    }

    last_assistant.or(last_any)
}

fn extract_last_message_from_items(items: &[Value]) -> (Option<String>, Option<String>) {
    let mut last_assistant: Option<String> = None;
    let mut last_any: Option<String> = None;

    for item in items {
        let Some(text) = extract_item_text(item) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if item_is_assistant_message(item) {
            last_assistant = Some(text.clone());
        }
        last_any = Some(text);
    }

    (last_assistant, last_any)
}

fn extract_item_text(item: &Value) -> Option<String> {
    match item {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Array(parts) => {
            let texts: Vec<String> = parts.iter().filter_map(extract_item_text).collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" "))
            }
        }
        Value::Object(object) => {
            for key in ["text", "content", "message", "output_text", "outputText"] {
                if let Some(text) = object.get(key).and_then(extract_item_text) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn item_is_assistant_message(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };

    if let Some(role) = object.get("role").and_then(Value::as_str) {
        let role = role.to_ascii_lowercase();
        if role.contains("assistant") || role.contains("agent") {
            return true;
        }
    }

    if let Some(author) = object.get("author").and_then(Value::as_object) {
        if let Some(role) = author.get("role").and_then(Value::as_str) {
            let role = role.to_ascii_lowercase();
            if role.contains("assistant") || role.contains("agent") {
                return true;
            }
        }
    }

    if let Some(item_type) = object.get("type").and_then(Value::as_str) {
        let item_type = item_type.to_ascii_lowercase();
        if item_type.contains("assistant") || item_type.contains("agent_message") {
            return true;
        }
    }

    false
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => extract_item_text(value),
    }
}

fn crop_preview_text(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "(no message)".to_string();
    }
    if max_chars == 0 {
        return "...".to_string();
    }

    let mut preview = String::new();
    let mut chars = normalized.chars();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return normalized;
        };
        preview.push(ch);
    }

    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
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
    let mut websocket_url: Option<String> = None;
    let mut model: Option<String> = None;
    let mut model_provider: Option<String> = None;
    let mut reasoning_effort: Option<ModelReasoningEffort> = None;
    let mut reasoning_summary: Option<ModelReasoningSummary> = None;
    let mut model_verbosity: Option<ModelVerbosity> = None;
    let mut config_profile: Option<String> = None;
    let mut approval_policy: Option<ApprovalMode> = None;
    let mut sandbox_mode: Option<SandboxMode> = None;
    let mut sandbox_policy_json: Option<String> = None;
    let mut sandbox_network_access_enabled: Option<bool> = None;
    let mut sandbox_writable_roots = Vec::new();
    let mut web_search_mode: Option<WebSearchMode> = None;
    let mut dynamic_tools_json: Option<String> = None;
    let mut personality: Option<Personality> = None;
    let mut base_instructions: Option<String> = None;
    let mut developer_instructions: Option<String> = None;
    let mut ephemeral: Option<bool> = None;
    let mut experimental_raw_events: Option<bool> = None;
    let mut persist_extended_history: Option<bool> = None;
    let mut config_entries = Vec::new();
    let mut config_json: Option<String> = None;
    let mut output_schema_json: Option<String> = None;
    let mut output_schema_file: Option<String> = None;
    let mut turn_extra_json: Option<String> = None;
    let mut resume_target: Option<ResumeTarget> = None;
    let mut sessions = false;
    let mut sessions_all = false;
    let mut final_response_only = false;
    let mut json_output = false;
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
            if arg == "sessions" || arg == "--sessions" {
                if sessions {
                    return Err(SparkError::Usage(
                        "sessions may only be provided once".to_string(),
                    ));
                }
                sessions = true;
                continue;
            }
            if arg == "--all" {
                if sessions_all {
                    return Err(SparkError::Usage(
                        "--all may only be provided once".to_string(),
                    ));
                }
                sessions_all = true;
                continue;
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
            if arg == "--ws-url" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --ws-url".to_string()))?;
                set_string_option_once(&mut websocket_url, &raw, "--ws-url")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--ws-url=") {
                set_string_option_once(&mut websocket_url, raw, "--ws-url")?;
                continue;
            }
            if arg == "--model" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --model".to_string()))?;
                set_string_option_once(&mut model, &raw, "--model")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--model=") {
                set_string_option_once(&mut model, raw, "--model")?;
                continue;
            }
            if arg == "--model-provider" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --model-provider".to_string())
                })?;
                set_string_option_once(&mut model_provider, &raw, "--model-provider")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--model-provider=") {
                set_string_option_once(&mut model_provider, raw, "--model-provider")?;
                continue;
            }
            if arg == "--reasoning-effort" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --reasoning-effort".to_string())
                })?;
                set_option_once(
                    &mut reasoning_effort,
                    parse_reasoning_effort(&raw)?,
                    "--reasoning-effort",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--reasoning-effort=") {
                set_option_once(
                    &mut reasoning_effort,
                    parse_reasoning_effort(raw)?,
                    "--reasoning-effort",
                )?;
                continue;
            }
            if arg == "--reasoning-summary" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --reasoning-summary".to_string())
                })?;
                set_option_once(
                    &mut reasoning_summary,
                    parse_reasoning_summary(&raw)?,
                    "--reasoning-summary",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--reasoning-summary=") {
                set_option_once(
                    &mut reasoning_summary,
                    parse_reasoning_summary(raw)?,
                    "--reasoning-summary",
                )?;
                continue;
            }
            if arg == "--model-verbosity" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --model-verbosity".to_string())
                })?;
                set_option_once(
                    &mut model_verbosity,
                    parse_model_verbosity(&raw)?,
                    "--model-verbosity",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--model-verbosity=") {
                set_option_once(
                    &mut model_verbosity,
                    parse_model_verbosity(raw)?,
                    "--model-verbosity",
                )?;
                continue;
            }
            if arg == "--config-profile" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --config-profile".to_string())
                })?;
                set_string_option_once(&mut config_profile, &raw, "--config-profile")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--config-profile=") {
                set_string_option_once(&mut config_profile, raw, "--config-profile")?;
                continue;
            }
            if arg == "--approval-policy" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --approval-policy".to_string())
                })?;
                set_option_once(
                    &mut approval_policy,
                    parse_approval_mode(&raw)?,
                    "--approval-policy",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--approval-policy=") {
                set_option_once(
                    &mut approval_policy,
                    parse_approval_mode(raw)?,
                    "--approval-policy",
                )?;
                continue;
            }
            if arg == "--sandbox" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --sandbox".to_string()))?;
                set_option_once(&mut sandbox_mode, parse_sandbox_mode(&raw)?, "--sandbox")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--sandbox=") {
                set_option_once(&mut sandbox_mode, parse_sandbox_mode(raw)?, "--sandbox")?;
                continue;
            }
            if arg == "--sandbox-policy-json" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --sandbox-policy-json".to_string())
                })?;
                set_string_option_once(&mut sandbox_policy_json, &raw, "--sandbox-policy-json")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--sandbox-policy-json=") {
                set_string_option_once(&mut sandbox_policy_json, raw, "--sandbox-policy-json")?;
                continue;
            }
            if arg == "--sandbox-network-access-enabled" {
                set_option_once(
                    &mut sandbox_network_access_enabled,
                    true,
                    "--sandbox-network-access-enabled/--sandbox-network-access-disabled",
                )?;
                continue;
            }
            if arg == "--sandbox-network-access-disabled" {
                set_option_once(
                    &mut sandbox_network_access_enabled,
                    false,
                    "--sandbox-network-access-enabled/--sandbox-network-access-disabled",
                )?;
                continue;
            }
            if arg == "--sandbox-writable-root" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --sandbox-writable-root".to_string())
                })?;
                sandbox_writable_roots.push(normalize_working_directory(&raw)?);
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--sandbox-writable-root=") {
                sandbox_writable_roots.push(normalize_working_directory(raw)?);
                continue;
            }
            if arg == "--web-search-mode" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --web-search-mode".to_string())
                })?;
                set_option_once(
                    &mut web_search_mode,
                    parse_web_search_mode(&raw)?,
                    "--web-search-mode",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--web-search-mode=") {
                set_option_once(
                    &mut web_search_mode,
                    parse_web_search_mode(raw)?,
                    "--web-search-mode",
                )?;
                continue;
            }
            if arg == "--dynamic-tools-json" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --dynamic-tools-json".to_string())
                })?;
                set_string_option_once(&mut dynamic_tools_json, &raw, "--dynamic-tools-json")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--dynamic-tools-json=") {
                set_string_option_once(&mut dynamic_tools_json, raw, "--dynamic-tools-json")?;
                continue;
            }
            if arg == "--personality" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --personality".to_string())
                })?;
                set_option_once(&mut personality, parse_personality(&raw)?, "--personality")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--personality=") {
                set_option_once(&mut personality, parse_personality(raw)?, "--personality")?;
                continue;
            }
            if arg == "--base-instructions" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --base-instructions".to_string())
                })?;
                set_string_option_once(&mut base_instructions, &raw, "--base-instructions")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--base-instructions=") {
                set_string_option_once(&mut base_instructions, raw, "--base-instructions")?;
                continue;
            }
            if arg == "--developer-instructions" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --developer-instructions".to_string())
                })?;
                set_string_option_once(
                    &mut developer_instructions,
                    &raw,
                    "--developer-instructions",
                )?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--developer-instructions=") {
                set_string_option_once(
                    &mut developer_instructions,
                    raw,
                    "--developer-instructions",
                )?;
                continue;
            }
            if arg == "--ephemeral" {
                set_option_once(&mut ephemeral, true, "--ephemeral")?;
                continue;
            }
            if arg == "--experimental-raw-events" {
                set_option_once(
                    &mut experimental_raw_events,
                    true,
                    "--experimental-raw-events",
                )?;
                continue;
            }
            if arg == "--persist-extended-history" {
                set_option_once(
                    &mut persist_extended_history,
                    true,
                    "--persist-extended-history",
                )?;
                continue;
            }
            if arg == "--config" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --config".to_string()))?;
                config_entries.push(raw);
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--config=") {
                config_entries.push(raw.to_string());
                continue;
            }
            if arg == "--config-json" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --config-json".to_string())
                })?;
                set_string_option_once(&mut config_json, &raw, "--config-json")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--config-json=") {
                set_string_option_once(&mut config_json, raw, "--config-json")?;
                continue;
            }
            if arg == "--output-schema-json" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --output-schema-json".to_string())
                })?;
                set_string_option_once(&mut output_schema_json, &raw, "--output-schema-json")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--output-schema-json=") {
                set_string_option_once(&mut output_schema_json, raw, "--output-schema-json")?;
                continue;
            }
            if arg == "--output-schema-file" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --output-schema-file".to_string())
                })?;
                set_string_option_once(&mut output_schema_file, &raw, "--output-schema-file")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--output-schema-file=") {
                set_string_option_once(&mut output_schema_file, raw, "--output-schema-file")?;
                continue;
            }
            if arg == "--output-schema" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --output-schema".to_string())
                })?;
                set_string_option_once(&mut output_schema_file, &raw, "--output-schema")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--output-schema=") {
                set_string_option_once(&mut output_schema_file, raw, "--output-schema")?;
                continue;
            }
            if arg == "--turn-extra-json" {
                let raw = iter.next().ok_or_else(|| {
                    SparkError::Usage("missing value for --turn-extra-json".to_string())
                })?;
                set_string_option_once(&mut turn_extra_json, &raw, "--turn-extra-json")?;
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--turn-extra-json=") {
                set_string_option_once(&mut turn_extra_json, raw, "--turn-extra-json")?;
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
            if arg == "--json" {
                if json_output {
                    return Err(SparkError::Usage(
                        "--json may only be provided once".to_string(),
                    ));
                }
                json_output = true;
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

    if transport_mode == TransportMode::Stdio && websocket_url.is_some() {
        return Err(SparkError::Usage(
            "--ws-url cannot be used with --stdio".to_string(),
        ));
    }

    if sessions_all && !sessions {
        return Err(SparkError::Usage(
            "--all can only be used with the sessions command".to_string(),
        ));
    }

    if sessions {
        if !prompt_parts.is_empty() {
            return Err(SparkError::Usage(
                "sessions does not accept prompt arguments".to_string(),
            ));
        }
        if resume_target.is_some() {
            return Err(SparkError::Usage(
                "sessions cannot be used with --continue or --resume".to_string(),
            ));
        }
        if final_response_only {
            return Err(SparkError::Usage(
                "sessions cannot be used with --final-response".to_string(),
            ));
        }
    }

    if output_schema_json.is_some() && output_schema_file.is_some() {
        return Err(SparkError::Usage(
            "only one of --output-schema-json, --output-schema-file, and --output-schema may be provided".to_string(),
        ));
    }

    if json_output && final_response_only {
        return Err(SparkError::Usage(
            "--json cannot be used with --final-response".to_string(),
        ));
    }

    if sessions && json_output {
        return Err(SparkError::Usage(
            "sessions cannot be used with --json".to_string(),
        ));
    }

    Ok(ParsedCommand::Run(CliArgs {
        agent,
        working_directory,
        websocket_url,
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
        sessions,
        sessions_all,
        final_response_only,
        json_output,
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

fn set_option_once<T>(slot: &mut Option<T>, value: T, flag: &str) -> Result<(), SparkError> {
    if slot.is_some() {
        return Err(SparkError::Usage(format!(
            "{flag} may only be provided once"
        )));
    }
    *slot = Some(value);
    Ok(())
}

fn set_string_option_once(
    slot: &mut Option<String>,
    raw: &str,
    flag: &str,
) -> Result<(), SparkError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(SparkError::Usage(format!(
            "value for {flag} cannot be empty"
        )));
    }
    set_option_once(slot, value.to_string(), flag)
}

fn parse_reasoning_effort(raw: &str) -> Result<ModelReasoningEffort, SparkError> {
    match raw.trim() {
        "none" => Ok(ModelReasoningEffort::None),
        "minimal" => Ok(ModelReasoningEffort::Minimal),
        "low" => Ok(ModelReasoningEffort::Low),
        "medium" => Ok(ModelReasoningEffort::Medium),
        "high" => Ok(ModelReasoningEffort::High),
        "xhigh" => Ok(ModelReasoningEffort::XHigh),
        _ => Err(SparkError::Usage(format!(
            "invalid --reasoning-effort '{raw}'; expected one of: none, minimal, low, medium, high, xhigh"
        ))),
    }
}

fn parse_reasoning_summary(raw: &str) -> Result<ModelReasoningSummary, SparkError> {
    match raw.trim() {
        "none" => Ok(ModelReasoningSummary::None),
        "auto" => Ok(ModelReasoningSummary::Auto),
        "concise" => Ok(ModelReasoningSummary::Concise),
        "detailed" => Ok(ModelReasoningSummary::Detailed),
        _ => Err(SparkError::Usage(format!(
            "invalid --reasoning-summary '{raw}'; expected one of: none, auto, concise, detailed"
        ))),
    }
}

fn parse_model_verbosity(raw: &str) -> Result<ModelVerbosity, SparkError> {
    match raw.trim() {
        "low" => Ok(ModelVerbosity::Low),
        "medium" => Ok(ModelVerbosity::Medium),
        "high" => Ok(ModelVerbosity::High),
        _ => Err(SparkError::Usage(format!(
            "invalid --model-verbosity '{raw}'; expected one of: low, medium, high"
        ))),
    }
}

fn parse_approval_mode(raw: &str) -> Result<ApprovalMode, SparkError> {
    match raw.trim() {
        "never" => Ok(ApprovalMode::Never),
        "on-request" => Ok(ApprovalMode::OnRequest),
        "on-failure" => Ok(ApprovalMode::OnFailure),
        "untrusted" => Ok(ApprovalMode::Untrusted),
        _ => Err(SparkError::Usage(format!(
            "invalid --approval-policy '{raw}'; expected one of: never, on-request, on-failure, untrusted"
        ))),
    }
}

fn parse_sandbox_mode(raw: &str) -> Result<SandboxMode, SparkError> {
    match raw.trim() {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
        _ => Err(SparkError::Usage(format!(
            "invalid --sandbox '{raw}'; expected one of: read-only, workspace-write, danger-full-access"
        ))),
    }
}

fn parse_web_search_mode(raw: &str) -> Result<WebSearchMode, SparkError> {
    match raw.trim() {
        "disabled" => Ok(WebSearchMode::Disabled),
        "cached" => Ok(WebSearchMode::Cached),
        "live" => Ok(WebSearchMode::Live),
        _ => Err(SparkError::Usage(format!(
            "invalid --web-search-mode '{raw}'; expected one of: disabled, cached, live"
        ))),
    }
}

fn web_search_mode_as_str(mode: WebSearchMode) -> &'static str {
    match mode {
        WebSearchMode::Disabled => "disabled",
        WebSearchMode::Cached => "cached",
        WebSearchMode::Live => "live",
    }
}

fn parse_personality(raw: &str) -> Result<Personality, SparkError> {
    match raw.trim() {
        "none" => Ok(Personality::None),
        "friendly" => Ok(Personality::Friendly),
        "pragmatic" => Ok(Personality::Pragmatic),
        _ => Err(SparkError::Usage(format!(
            "invalid --personality '{raw}'; expected one of: none, friendly, pragmatic"
        ))),
    }
}

fn parse_json_value(raw: &str, flag: &str) -> Result<Value, SparkError> {
    serde_json::from_str(raw)
        .map_err(|error| SparkError::Usage(format!("failed to parse JSON for {flag}: {error}")))
}

fn parse_optional_json_value(raw: Option<String>, flag: &str) -> Result<Option<Value>, SparkError> {
    raw.map(|value| parse_json_value(&value, flag)).transpose()
}

fn parse_optional_json_object(
    raw: Option<String>,
    flag: &str,
) -> Result<Option<Map<String, Value>>, SparkError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_json_value(&raw, flag)?;
    match value {
        Value::Object(object) => Ok(Some(object)),
        _ => Err(SparkError::Usage(format!("{flag} must be a JSON object"))),
    }
}

fn parse_optional_dynamic_tools(
    raw: Option<String>,
) -> Result<Option<Vec<DynamicToolSpec>>, SparkError> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let parsed = parse_json_value(&raw, "--dynamic-tools-json")?;
    let array = match parsed {
        Value::Array(array) => array,
        _ => {
            return Err(SparkError::Usage(
                "--dynamic-tools-json must be a JSON array".to_string(),
            ));
        }
    };

    let mut tools = Vec::with_capacity(array.len());
    for (index, item) in array.into_iter().enumerate() {
        let spec: CliDynamicToolSpec = serde_json::from_value(item).map_err(|error| {
            SparkError::Usage(format!(
                "invalid --dynamic-tools-json entry at index {index}: {error}"
            ))
        })?;
        tools.push(DynamicToolSpec::new(
            spec.name,
            spec.description,
            spec.input_schema,
        ));
    }

    Ok(Some(tools))
}

fn build_thread_config(
    config_json: Option<String>,
    config_entries: Vec<String>,
    web_search_mode: Option<WebSearchMode>,
    config_profile: Option<String>,
    model_verbosity: Option<ModelVerbosity>,
    sandbox_network_access_enabled: Option<bool>,
    sandbox_writable_roots: Vec<String>,
) -> Result<Option<Map<String, Value>>, SparkError> {
    let mut config = parse_optional_json_object(config_json, "--config-json")?.unwrap_or_default();

    for entry in config_entries {
        let Some((raw_key, raw_value)) = entry.split_once('=') else {
            return Err(SparkError::Usage(
                "invalid --config entry; expected KEY=VALUE".to_string(),
            ));
        };

        let key = raw_key.trim();
        if key.is_empty() {
            return Err(SparkError::Usage(
                "invalid --config entry; key cannot be empty".to_string(),
            ));
        }

        let value = raw_value.trim();
        if value.is_empty() {
            return Err(SparkError::Usage(format!(
                "invalid --config entry for key '{key}'; value cannot be empty"
            )));
        }

        let parsed = serde_json::from_str::<Value>(value)
            .unwrap_or_else(|_| Value::String(value.to_string()));
        config.insert(key.to_string(), parsed);
    }

    if let Some(mode) = web_search_mode {
        config.insert(
            "web_search".to_string(),
            Value::String(web_search_mode_as_str(mode).to_string()),
        );
    }
    if let Some(profile) = config_profile {
        config.insert("profile".to_string(), Value::String(profile));
    }
    if let Some(verbosity) = model_verbosity {
        config.insert(
            "model_verbosity".to_string(),
            Value::String(verbosity.as_str().to_string()),
        );
    }
    if let Some(enabled) = sandbox_network_access_enabled {
        config.insert(
            "sandbox_workspace_write.network_access".to_string(),
            Value::Bool(enabled),
        );
    }
    if !sandbox_writable_roots.is_empty() {
        config.insert(
            "sandbox_workspace_write.writable_roots".to_string(),
            Value::Array(
                sandbox_writable_roots
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }

    if config.is_empty() {
        Ok(None)
    } else {
        Ok(Some(config))
    }
}

fn resolve_output_schema(
    output_schema_json: Option<String>,
    output_schema_file: Option<String>,
) -> Result<Option<Value>, SparkError> {
    if let Some(raw) = output_schema_json {
        return Ok(Some(parse_json_value(&raw, "--output-schema-json")?));
    }

    let Some(path) = output_schema_file else {
        return Ok(None);
    };
    let path = path.trim();
    if path.is_empty() {
        return Err(SparkError::Usage(
            "value for --output-schema-file cannot be empty".to_string(),
        ));
    }
    let schema_path = PathBuf::from(path);
    let raw = fs::read_to_string(&schema_path).map_err(|error| {
        SparkError::Config(format!(
            "failed to read output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    let schema = serde_json::from_str::<Value>(&raw).map_err(|error| {
        SparkError::Usage(format!(
            "failed to parse JSON in output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    Ok(Some(schema))
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
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    fn parse_cli_args_supports_sessions_command() {
        let parsed =
            parse_cli_args(vec!["sessions".to_string()].into_iter()).expect("parse sessions");
        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert!(cli.sessions);
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

        assert!(cli.sessions);
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

        assert!(cli.sessions);
        assert!(cli.sessions_all);
        assert!(cli.prompt_parts.is_empty());
    }

    #[test]
    fn parse_cli_args_rejects_all_without_sessions() {
        let error = parse_cli_args(vec!["--all".to_string(), "hello".to_string()].into_iter())
            .expect_err("--all without sessions");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_sessions_with_prompt() {
        let error = parse_cli_args(vec!["sessions".to_string(), "hello".to_string()].into_iter())
            .expect_err("sessions prompt conflict");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn crop_preview_text_truncates_and_normalizes_whitespace() {
        let preview = crop_preview_text("hello   world from   spark", 12);
        assert_eq!(preview, "hello world ...");
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
    fn parse_cli_args_supports_extended_configuration_flags() {
        let parsed = parse_cli_args(
            vec![
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
        assert!(matches!(error, SparkError::Usage(_)));
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
    fn parse_cli_args_rejects_ws_url_with_stdio() {
        let error = parse_cli_args(
            vec![
                "--stdio".to_string(),
                "--ws-url".to_string(),
                "ws://127.0.0.1:9000".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("ws-url should conflict with stdio");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn parse_cli_args_rejects_multiple_output_schema_sources() {
        let error = parse_cli_args(
            vec![
                "--output-schema-json={\"type\":\"object\"}".to_string(),
                "--output-schema-file".to_string(),
                "/tmp/schema.json".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect_err("output schema source conflict");
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
        let seq = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            env::temp_dir().join(format!("spark-tests-{}-{stamp}-{seq}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
