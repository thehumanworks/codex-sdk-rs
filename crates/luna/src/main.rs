mod doctor;
mod environment;
mod error;

use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use codex_app_server_sdk::api::{
    ApprovalMode, Codex, DynamicToolSpec, ModelReasoningEffort, ModelReasoningSummary, Personality,
    SandboxMode, StreamedTurn, ThreadEvent, ThreadItem, ThreadOptions, TurnOptions,
    UserMessageContentItem, WebSearchMode,
};
use codex_app_server_sdk::{ClientOptions, CodexClient, WsConfig};
use codex_app_server_sdk::{StdioConfig, requests, responses};
use doctor::{DoctorOptions, run_doctor};
use environment::resolve_codex_binary;
use error::LunaError;
use serde::Deserialize;
use serde_json::{Map, Value};

const APP_NAME: &str = "luna";
const MODEL: &str = "gpt-5.6-luna";
const DEFAULT_WS_URL: &str = "ws://127.0.0.1:4222";
const CODEX_APP_SERVER_WS_URL_ENV: &str = "CODEX_APP_SERVER_WS_URL";
const CODEX_WEB_SERVER_URL_ENV: &str = "CODEX_WEB_SERVER_URL";
const THREAD_LIST_PAGE_LIMIT: u32 = 100;
const MAX_THREAD_LIST_PAGES: usize = 100;
const SESSION_PREVIEW_CHAR_LIMIT: usize = 96;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Exec,
    Start,
    Sessions,
    Doctor,
}

#[derive(Debug)]
struct CliArgs {
    command_kind: CommandKind,
    no_daemon: bool,
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
    sessions_all: bool,
    final_response_only: bool,
    json_output: bool,
    doctor_live: bool,
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum ParsedCommand {
    Help(String),
    Version(String),
    Completions(Shell),
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

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(exit_code) => exit_code,
        Err(LunaError::Usage(message)) => {
            let error = LunaError::Usage(message);
            if error.to_string().starts_with("error:") {
                eprint!("{error}");
            } else {
                eprintln!("{} [{}]\n", error, error.code());
                eprintln!("{}", CliParser::command().render_help());
            }
            ExitCode::from(error.exit_code())
        }
        Err(error) => {
            eprintln!("{APP_NAME} [{}]: {error}", error.code());
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run() -> Result<ExitCode, LunaError> {
    let command = parse_cli_args(env::args().skip(1))?;
    let cli = match command {
        ParsedCommand::Help(help) => {
            print!("{help}");
            return Ok(ExitCode::SUCCESS);
        }
        ParsedCommand::Version(version) => {
            print!("{version}");
            return Ok(ExitCode::SUCCESS);
        }
        ParsedCommand::Completions(shell) => {
            generate(
                shell,
                &mut CliParser::command(),
                APP_NAME,
                &mut io::stdout(),
            );
            return Ok(ExitCode::SUCCESS);
        }
        ParsedCommand::Run(cli) => cli,
    };

    let CliArgs {
        command_kind,
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
        sessions_all,
        final_response_only,
        json_output,
        doctor_live,
        transport_mode,
        prompt_parts,
        no_daemon,
    } = cli;

    let resolved_websocket_url = if transport_mode == TransportMode::WebSocket {
        let primary_env_url = env::var(CODEX_APP_SERVER_WS_URL_ENV).ok();
        let legacy_env_url = env::var(CODEX_WEB_SERVER_URL_ENV).ok();
        resolve_websocket_url(
            websocket_url.as_deref(),
            primary_env_url.as_deref(),
            legacy_env_url.as_deref(),
        )?
    } else {
        ResolvedWebsocketUrl::default()
    };

    if command_kind == CommandKind::Sessions {
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
            TransportMode::WebSocket => {
                connect_ws_codex(
                    &resolved_websocket_url.url,
                    resolved_websocket_url.manage_daemon() && !no_daemon,
                )
                .await?
            }
            TransportMode::Stdio => spawn_stdio_codex().await?,
        };
        list_sessions(&codex, cwd_filter.as_deref()).await?;
        return Ok(ExitCode::SUCCESS);
    }

    if command_kind == CommandKind::Start {
        start_ws_server(&resolved_websocket_url.url).await?;
        println!("WebSocket server ready at {}", resolved_websocket_url.url);
        return Ok(ExitCode::SUCCESS);
    }

    if command_kind == CommandKind::Doctor {
        let manage_daemon = resolved_websocket_url.manage_daemon() && !no_daemon;
        let passed = run_doctor(DoctorOptions {
            json: json_output,
            live: doctor_live,
            transport: transport_mode,
            websocket_url: resolved_websocket_url.url,
            manage_daemon,
        })
        .await?;
        return Ok(if passed {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }

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
        return Ok(ExitCode::SUCCESS);
    }

    let mut streamed = thread.run_streamed(prompt, turn_options).await?;

    if json_output {
        stream_json_events(&mut streamed).await?;
    } else {
        stream_text_events(&mut streamed).await?;
    }

    Ok(ExitCode::SUCCESS)
}

async fn stream_text_events(streamed: &mut StreamedTurn) -> Result<(), LunaError> {
    let mut stdout = io::stdout();
    let mut saw_terminal = false;
    let mut saw_delta_for_message = false;
    let mut printed_any = false;
    let mut ended_with_newline = false;

    while let Some(next) = streamed.next_event().await {
        let event = next?;
        match event {
            ThreadEvent::ItemUpdated { item } => {
                if let ThreadItem::AgentMessage(agent_message) = item
                    && !agent_message.text.is_empty()
                {
                    print_chunk(&mut stdout, &agent_message.text)?;
                    saw_delta_for_message = true;
                    printed_any = true;
                    ended_with_newline = agent_message.text.ends_with('\n');
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
                return Err(LunaError::Turn(format!("turn failed: {}", error.message)));
            }
            ThreadEvent::Error { message } => {
                return Err(LunaError::Turn(format!("stream error: {message}")));
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. } => {}
        }
    }

    if !saw_terminal {
        return Err(LunaError::Turn(
            "stream closed before receiving turn completion".to_string(),
        ));
    }

    if printed_any {
        ensure_message_separator(&mut stdout, printed_any, &mut ended_with_newline)?;
    }

    Ok(())
}

async fn stream_json_events(streamed: &mut StreamedTurn) -> Result<(), LunaError> {
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
            .map_err(|err| LunaError::Protocol(format!("failed to serialize event: {err}")))?;
        writeln!(stdout, "{line}")?;

        match event {
            ThreadEvent::TurnCompleted { .. } => break,
            ThreadEvent::TurnFailed { error } => {
                return Err(LunaError::Turn(format!("turn failed: {}", error.message)));
            }
            ThreadEvent::Error { message } => {
                return Err(LunaError::Turn(format!("stream error: {message}")));
            }
            _ => {}
        }
    }

    Ok(())
}

fn thread_item_to_json(item: &ThreadItem) -> Value {
    match item {
        ThreadItem::AgentMessage(msg) => {
            let mut value = serde_json::json!({
                "type": "agentMessage",
                "id": msg.id,
                "text": msg.text,
            });
            if let Some(phase) = msg.phase {
                value["phase"] = Value::String(phase.as_str().to_string());
            }
            value
        }
        ThreadItem::UserMessage(msg) => {
            let content: Vec<Value> = msg
                .content
                .iter()
                .map(|entry| match entry {
                    UserMessageContentItem::Text { text } => {
                        serde_json::json!({ "type": "text", "text": text })
                    }
                    UserMessageContentItem::Image { url } => {
                        serde_json::json!({ "type": "image", "url": url })
                    }
                    UserMessageContentItem::LocalImage { path } => {
                        serde_json::json!({ "type": "localImage", "path": path })
                    }
                    UserMessageContentItem::Unknown(raw) => raw.clone(),
                })
                .collect();
            serde_json::json!({
                "type": "userMessage",
                "id": msg.id,
                "content": content,
            })
        }
        ThreadItem::Plan(plan) => serde_json::json!({
            "type": "plan",
            "id": plan.id,
            "text": plan.text,
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
        ThreadItem::DynamicToolCall(tool) => serde_json::json!({
            "type": "dynamicToolCall",
            "id": tool.id,
            "tool": tool.tool,
            "arguments": tool.arguments,
            "status": tool.status,
            "contentItems": tool.content_items,
            "success": tool.success,
            "durationMs": tool.duration_ms,
        }),
        ThreadItem::CollabToolCall(tool) => serde_json::json!({
            "type": "collabToolCall",
            "id": tool.id,
            "tool": tool.tool,
            "status": tool.status,
            "senderThreadId": tool.sender_thread_id,
            "receiverThreadId": tool.receiver_thread_id,
            "newThreadId": tool.new_thread_id,
            "prompt": tool.prompt,
            "agentStatus": tool.agent_status,
        }),
        ThreadItem::WebSearch(ws) => serde_json::json!({
            "type": "webSearch",
            "id": ws.id,
            "query": ws.query,
        }),
        ThreadItem::ImageView(image) => serde_json::json!({
            "type": "imageView",
            "id": image.id,
            "path": image.path,
        }),
        ThreadItem::EnteredReviewMode(review) => serde_json::json!({
            "type": "enteredReviewMode",
            "id": review.id,
            "review": review.review,
        }),
        ThreadItem::ExitedReviewMode(review) => serde_json::json!({
            "type": "exitedReviewMode",
            "id": review.id,
            "review": review.review,
        }),
        ThreadItem::ContextCompaction(item) => serde_json::json!({
            "type": "contextCompaction",
            "id": item.id,
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

async fn connect_ws_codex(url: &str, manage_daemon: bool) -> Result<Codex, LunaError> {
    let config = WsConfig {
        url: url.to_string(),
        env: Default::default(),
        options: ClientOptions::default(),
    };
    let client = if manage_daemon {
        CodexClient::start_and_connect_ws(config).await?
    } else {
        CodexClient::connect_ws(config).await?
    };
    Ok(client.as_api())
}

async fn start_ws_server(url: &str) -> Result<(), LunaError> {
    let config = WsConfig {
        url: url.to_string(),
        env: Default::default(),
        options: ClientOptions::default(),
    };
    let _client = CodexClient::start_and_connect_ws(config).await?;
    Ok(())
}

async fn spawn_stdio_codex() -> Result<Codex, LunaError> {
    let codex_binary = resolve_codex_binary()?;
    let stdio_config = StdioConfig {
        codex_binary: codex_binary.to_string_lossy().into_owned(),
        ..Default::default()
    };
    Ok(Codex::spawn_stdio(stdio_config).await?)
}

/// Ensure the codex session is authenticated using a cascading strategy:
/// 1. Rely on existing cached auth (auth.json) — check via `account_read`.
/// 2. If not authenticated, try `CODEX_ID_TOKEN` + `CODEX_ACCESS_TOKEN` env vars (ChatGPT login).
/// 3. If those aren't available, try `OPENAI_API_KEY` env var (API key login).
async fn ensure_authenticated(codex: &Codex) -> Result<(), LunaError> {
    // Check if the app-server already has valid cached auth.
    let account = codex
        .account_read(requests::GetAccountParams::default())
        .await?;
    if account_is_authenticated(&account.extra) {
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

    Err(LunaError::Authentication(
        "Codex is not authenticated; run `codex login`, then rerun `luna doctor --live`"
            .to_string(),
    ))
}

fn account_is_authenticated(account: &Map<String, Value>) -> bool {
    account.get("isLoggedIn") == Some(&Value::Bool(true))
        || account.get("account").is_some_and(|value| !value.is_null())
        || account.get("requiresOpenaiAuth") == Some(&Value::Bool(false))
}

#[derive(Debug)]
struct SessionListEntry {
    id: String,
    recency_score: i64,
    last_message: String,
}

async fn list_sessions(codex: &Codex, cwd_filter: Option<&str>) -> Result<(), LunaError> {
    let mut cursor: Option<String> = None;
    let mut pages_scanned = 0usize;
    let mut sessions = Vec::new();

    loop {
        pages_scanned += 1;
        if pages_scanned > MAX_THREAD_LIST_PAGES {
            return Err(LunaError::Protocol(format!(
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
) -> Result<String, LunaError> {
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
                if let Some(text) = object.get(key).and_then(extract_item_text)
                    && !text.is_empty()
                {
                    return Some(text);
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

    if let Some(author) = object.get("author").and_then(Value::as_object)
        && let Some(role) = author.get("role").and_then(Value::as_str)
    {
        let role = role.to_ascii_lowercase();
        if role.contains("assistant") || role.contains("agent") {
            return true;
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

fn print_chunk<W: Write>(writer: &mut W, chunk: &str) -> Result<(), LunaError> {
    write!(writer, "{chunk}")?;
    writer.flush()?;
    Ok(())
}

fn ensure_message_separator<W: Write>(
    writer: &mut W,
    printed_any: bool,
    ended_with_newline: &mut bool,
) -> Result<(), LunaError> {
    if printed_any && !*ended_with_newline {
        writeln!(writer)?;
        writer.flush()?;
        *ended_with_newline = true;
    }
    Ok(())
}

#[derive(Debug, Parser)]
#[command(
    name = "luna",
    version,
    about = "Opinionated one-shot Codex app-server CLI",
    disable_help_subcommand = true
)]
struct CliParser {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Run one Codex turn from an argument or stdin
    #[command(alias = "x")]
    Exec(Box<ExecCliArgs>),
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
    /// Connect without managing a local WebSocket daemon
    #[arg(long)]
    no_daemon: bool,
    /// Spawn an app-server over stdio instead of WebSocket
    #[arg(long, conflicts_with = "ws_url")]
    stdio: bool,
}

#[derive(Debug, Args)]
struct ExecCliArgs {
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
    /// Print only the final agent message
    #[arg(long, conflicts_with = "json")]
    final_response: bool,
    /// Print turn events as JSONL
    #[arg(long, conflicts_with = "final_response")]
    json: bool,
    /// Prompt text; read from stdin when omitted
    #[arg(value_name = "PROMPT")]
    prompt: Vec<String>,
}

#[derive(Debug, Args)]
struct StartCliArgs {
    /// Loopback WebSocket URL to reuse or start
    #[arg(long, value_name = "URL")]
    ws_url: Option<String>,
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

fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<ParsedCommand, LunaError> {
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
        CliCommand::Exec(args) => Ok(ParsedCommand::Run(exec_cli_args(*args)?)),
        CliCommand::Start(args) => Ok(ParsedCommand::Run(start_cli_args(args)?)),
        CliCommand::Sessions(args) => Ok(ParsedCommand::Run(sessions_cli_args(args)?)),
        CliCommand::Doctor(args) => Ok(ParsedCommand::Run(doctor_cli_args(args)?)),
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
                    "exec" | "x" | "start" | "sessions" | "doctor" | "completions"
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

fn exec_cli_args(args: ExecCliArgs) -> Result<CliArgs, LunaError> {
    let transport_mode = transport_mode(args.transport.stdio);
    let mut cli = empty_cli_args(CommandKind::Exec, transport_mode);
    cli.no_daemon = args.transport.no_daemon;
    cli.websocket_url = normalize_optional_string(args.transport.ws_url, "--ws-url")?;
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
        .map(parse_reasoning_effort)
        .transpose()?;
    cli.reasoning_summary = args
        .reasoning_summary
        .as_deref()
        .map(parse_reasoning_summary)
        .transpose()?;
    cli.model_verbosity = args
        .model_verbosity
        .as_deref()
        .map(parse_model_verbosity)
        .transpose()?;
    cli.config_profile = normalize_optional_string(args.config_profile, "--config-profile")?;
    cli.approval_policy = args
        .approval_policy
        .as_deref()
        .map(parse_approval_mode)
        .transpose()?;
    cli.sandbox_mode = args
        .sandbox
        .as_deref()
        .map(parse_sandbox_mode)
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
        .map(parse_web_search_mode)
        .transpose()?;
    cli.dynamic_tools_json =
        normalize_optional_string(args.dynamic_tools_json, "--dynamic-tools-json")?;
    cli.personality = args
        .personality
        .as_deref()
        .map(parse_personality)
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
    Ok(cli)
}

fn sessions_cli_args(args: SessionsCliArgs) -> Result<CliArgs, LunaError> {
    let mut cli = empty_cli_args(CommandKind::Sessions, transport_mode(args.transport.stdio));
    cli.no_daemon = args.transport.no_daemon;
    cli.websocket_url = normalize_optional_string(args.transport.ws_url, "--ws-url")?;
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
    cli.no_daemon = args.transport.no_daemon;
    cli.websocket_url = normalize_optional_string(args.transport.ws_url, "--ws-url")?;
    cli.json_output = args.json;
    cli.doctor_live = args.live;
    let _ = (args.summary, args.no_color, args.ascii);
    Ok(cli)
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
        model: None,
        model_provider: None,
        reasoning_effort: None,
        reasoning_summary: None,
        model_verbosity: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebsocketUrlSource {
    Flag,
    Environment,
    LegacyEnvironment,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedWebsocketUrl {
    url: String,
    source: WebsocketUrlSource,
}

impl ResolvedWebsocketUrl {
    fn manage_daemon(&self) -> bool {
        self.source == WebsocketUrlSource::Default
    }
}

impl Default for ResolvedWebsocketUrl {
    fn default() -> Self {
        Self {
            url: DEFAULT_WS_URL.to_string(),
            source: WebsocketUrlSource::Default,
        }
    }
}

fn resolve_websocket_url(
    explicit: Option<&str>,
    env_websocket_url: Option<&str>,
    legacy_env_websocket_url: Option<&str>,
) -> Result<ResolvedWebsocketUrl, LunaError> {
    if let Some(url) = explicit {
        return Ok(ResolvedWebsocketUrl {
            url: normalize_websocket_url(url, "--ws-url")?,
            source: WebsocketUrlSource::Flag,
        });
    }
    if let Some(url) = env_websocket_url {
        return Ok(ResolvedWebsocketUrl {
            url: normalize_websocket_url(url, CODEX_APP_SERVER_WS_URL_ENV)?,
            source: WebsocketUrlSource::Environment,
        });
    }
    if let Some(url) = legacy_env_websocket_url {
        return Ok(ResolvedWebsocketUrl {
            url: normalize_websocket_url(url, CODEX_WEB_SERVER_URL_ENV)?,
            source: WebsocketUrlSource::LegacyEnvironment,
        });
    }
    Ok(ResolvedWebsocketUrl::default())
}

fn normalize_websocket_url(raw: &str, source: &str) -> Result<String, LunaError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(LunaError::Usage(format!(
            "value for {source} cannot be empty"
        )));
    }
    Ok(value.to_string())
}

fn parse_reasoning_effort(raw: &str) -> Result<ModelReasoningEffort, LunaError> {
    match raw.trim() {
        "none" => Ok(ModelReasoningEffort::None),
        "minimal" => Ok(ModelReasoningEffort::Minimal),
        "low" => Ok(ModelReasoningEffort::Low),
        "medium" => Ok(ModelReasoningEffort::Medium),
        "high" => Ok(ModelReasoningEffort::High),
        "xhigh" => Ok(ModelReasoningEffort::XHigh),
        "max" => Ok(ModelReasoningEffort::Max),
        "ultra" => Ok(ModelReasoningEffort::Ultra),
        _ => Err(LunaError::Usage(format!(
            "invalid --reasoning-effort '{raw}'; expected one of: none, minimal, low, medium, high, xhigh, max, ultra"
        ))),
    }
}

fn parse_reasoning_summary(raw: &str) -> Result<ModelReasoningSummary, LunaError> {
    match raw.trim() {
        "none" => Ok(ModelReasoningSummary::None),
        "auto" => Ok(ModelReasoningSummary::Auto),
        "concise" => Ok(ModelReasoningSummary::Concise),
        "detailed" => Ok(ModelReasoningSummary::Detailed),
        _ => Err(LunaError::Usage(format!(
            "invalid --reasoning-summary '{raw}'; expected one of: none, auto, concise, detailed"
        ))),
    }
}

fn parse_model_verbosity(raw: &str) -> Result<ModelVerbosity, LunaError> {
    match raw.trim() {
        "low" => Ok(ModelVerbosity::Low),
        "medium" => Ok(ModelVerbosity::Medium),
        "high" => Ok(ModelVerbosity::High),
        _ => Err(LunaError::Usage(format!(
            "invalid --model-verbosity '{raw}'; expected one of: low, medium, high"
        ))),
    }
}

fn parse_approval_mode(raw: &str) -> Result<ApprovalMode, LunaError> {
    match raw.trim() {
        "never" => Ok(ApprovalMode::Never),
        "on-request" => Ok(ApprovalMode::OnRequest),
        "on-failure" => Ok(ApprovalMode::OnFailure),
        "untrusted" => Ok(ApprovalMode::Untrusted),
        _ => Err(LunaError::Usage(format!(
            "invalid --approval-policy '{raw}'; expected one of: never, on-request, on-failure, untrusted"
        ))),
    }
}

fn parse_sandbox_mode(raw: &str) -> Result<SandboxMode, LunaError> {
    match raw.trim() {
        "read-only" => Ok(SandboxMode::ReadOnly),
        "workspace-write" => Ok(SandboxMode::WorkspaceWrite),
        "danger-full-access" => Ok(SandboxMode::DangerFullAccess),
        _ => Err(LunaError::Usage(format!(
            "invalid --sandbox '{raw}'; expected one of: read-only, workspace-write, danger-full-access"
        ))),
    }
}

fn parse_web_search_mode(raw: &str) -> Result<WebSearchMode, LunaError> {
    match raw.trim() {
        "disabled" => Ok(WebSearchMode::Disabled),
        "cached" => Ok(WebSearchMode::Cached),
        "live" => Ok(WebSearchMode::Live),
        _ => Err(LunaError::Usage(format!(
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

fn parse_personality(raw: &str) -> Result<Personality, LunaError> {
    match raw.trim() {
        "none" => Ok(Personality::None),
        "friendly" => Ok(Personality::Friendly),
        "pragmatic" => Ok(Personality::Pragmatic),
        _ => Err(LunaError::Usage(format!(
            "invalid --personality '{raw}'; expected one of: none, friendly, pragmatic"
        ))),
    }
}

fn parse_json_value(raw: &str, flag: &str) -> Result<Value, LunaError> {
    serde_json::from_str(raw)
        .map_err(|error| LunaError::Usage(format!("failed to parse JSON for {flag}: {error}")))
}

fn parse_optional_json_value(raw: Option<String>, flag: &str) -> Result<Option<Value>, LunaError> {
    raw.map(|value| parse_json_value(&value, flag)).transpose()
}

fn parse_optional_json_object(
    raw: Option<String>,
    flag: &str,
) -> Result<Option<Map<String, Value>>, LunaError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = parse_json_value(&raw, flag)?;
    match value {
        Value::Object(object) => Ok(Some(object)),
        _ => Err(LunaError::Usage(format!("{flag} must be a JSON object"))),
    }
}

fn parse_optional_dynamic_tools(
    raw: Option<String>,
) -> Result<Option<Vec<DynamicToolSpec>>, LunaError> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    let parsed = parse_json_value(&raw, "--dynamic-tools-json")?;
    let array = match parsed {
        Value::Array(array) => array,
        _ => {
            return Err(LunaError::Usage(
                "--dynamic-tools-json must be a JSON array".to_string(),
            ));
        }
    };

    let mut tools = Vec::with_capacity(array.len());
    for (index, item) in array.into_iter().enumerate() {
        let spec: CliDynamicToolSpec = serde_json::from_value(item).map_err(|error| {
            LunaError::Usage(format!(
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
) -> Result<Option<Map<String, Value>>, LunaError> {
    let mut config = parse_optional_json_object(config_json, "--config-json")?.unwrap_or_default();

    for entry in config_entries {
        let Some((raw_key, raw_value)) = entry.split_once('=') else {
            return Err(LunaError::Usage(
                "invalid --config entry; expected KEY=VALUE".to_string(),
            ));
        };

        let key = raw_key.trim();
        if key.is_empty() {
            return Err(LunaError::Usage(
                "invalid --config entry; key cannot be empty".to_string(),
            ));
        }

        let value = raw_value.trim();
        if value.is_empty() {
            return Err(LunaError::Usage(format!(
                "invalid --config entry for key '{key}'; value cannot be empty"
            )));
        }

        let parsed = serde_json::from_str::<Value>(value)
            .unwrap_or_else(|_| Value::String(value.to_string()));
        insert_thread_config(&mut config, key, parsed, "--config")?;
    }

    if let Some(mode) = web_search_mode {
        insert_thread_config(
            &mut config,
            "web_search",
            Value::String(web_search_mode_as_str(mode).to_string()),
            "--web-search-mode",
        )?;
    }
    if let Some(profile) = config_profile {
        insert_thread_config(
            &mut config,
            "profile",
            Value::String(profile),
            "--config-profile",
        )?;
    }
    if let Some(verbosity) = model_verbosity {
        insert_thread_config(
            &mut config,
            "model_verbosity",
            Value::String(verbosity.as_str().to_string()),
            "--model-verbosity",
        )?;
    }
    if let Some(enabled) = sandbox_network_access_enabled {
        insert_thread_config(
            &mut config,
            "sandbox_workspace_write.network_access",
            Value::Bool(enabled),
            "--sandbox-network-access-enabled/--sandbox-network-access-disabled",
        )?;
    }
    if !sandbox_writable_roots.is_empty() {
        insert_thread_config(
            &mut config,
            "sandbox_workspace_write.writable_roots",
            Value::Array(
                sandbox_writable_roots
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
            "--sandbox-writable-root",
        )?;
    }

    if config.is_empty() {
        Ok(None)
    } else {
        Ok(Some(config))
    }
}

fn insert_thread_config(
    config: &mut Map<String, Value>,
    key: &str,
    value: Value,
    source: &str,
) -> Result<(), LunaError> {
    if let Some(existing) = config.get(key) {
        if existing == &value {
            return Ok(());
        }
        return Err(LunaError::Usage(format!(
            "conflicting configuration sources for '{key}'; {source} would replace an existing value"
        )));
    }
    config.insert(key.to_string(), value);
    Ok(())
}

fn resolve_output_schema(
    output_schema_json: Option<String>,
    output_schema_file: Option<String>,
) -> Result<Option<Value>, LunaError> {
    if let Some(raw) = output_schema_json {
        return Ok(Some(parse_json_value(&raw, "--output-schema-json")?));
    }

    let Some(path) = output_schema_file else {
        return Ok(None);
    };
    let path = path.trim();
    if path.is_empty() {
        return Err(LunaError::Usage(
            "value for --output-schema-file cannot be empty".to_string(),
        ));
    }
    let schema_path = PathBuf::from(path);
    let raw = fs::read_to_string(&schema_path).map_err(|error| {
        LunaError::Config(format!(
            "failed to read output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    let schema = serde_json::from_str::<Value>(&raw).map_err(|error| {
        LunaError::Usage(format!(
            "failed to parse JSON in output schema file {}: {error}",
            schema_path.display()
        ))
    })?;
    Ok(Some(schema))
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

fn normalize_agent_name(raw: &str) -> Result<String, LunaError> {
    let candidate = raw.trim();
    let candidate = candidate.strip_suffix(".toml").unwrap_or(candidate);
    let candidate = candidate.strip_suffix(".md").unwrap_or(candidate);
    if candidate.is_empty() {
        return Err(LunaError::Usage("agent name cannot be empty".to_string()));
    }
    if candidate == "." || candidate == ".." {
        return Err(LunaError::Usage(
            "agent name cannot be '.' or '..'".to_string(),
        ));
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(LunaError::Usage(
            "agent name cannot include path separators".to_string(),
        ));
    }
    if !candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(LunaError::Usage(
            "agent name may only contain [A-Za-z0-9._-]".to_string(),
        ));
    }
    Ok(candidate.to_string())
}

fn normalize_working_directory(raw: &str) -> Result<String, LunaError> {
    if raw.trim().is_empty() {
        return Err(LunaError::Usage(
            "working directory for --cwd cannot be empty".to_string(),
        ));
    }
    Ok(raw.to_string())
}

fn resolve_current_working_directory() -> Result<String, LunaError> {
    let cwd = env::current_dir().map_err(|error| {
        LunaError::Config(format!("failed to resolve current directory: {error}"))
    })?;
    Ok(cwd.to_string_lossy().to_string())
}

fn resolve_prompt(prompt_parts: Vec<String>) -> Result<String, LunaError> {
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

fn load_agent_profile(agent_name: &str) -> Result<LoadedAgent, LunaError> {
    let codex_home = resolve_codex_home_dir().ok_or_else(|| {
        LunaError::Config(
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
) -> Result<LoadedAgent, LunaError> {
    let config_raw = fs::read_to_string(config_path).map_err(|error| {
        LunaError::Config(format!(
            "failed to read Codex config file {}: {error}",
            config_path.display()
        ))
    })?;
    let config: CodexConfigFile =
        toml::from_str(&config_raw).map_err(|source| LunaError::Toml {
            path: config_path.to_path_buf(),
            source,
        })?;
    let role_value = config.agents.get(agent_name).ok_or_else(|| {
        LunaError::Config(format!(
            "agent '{}' was not found in {} under [agents.{}]",
            agent_name,
            config_path.display(),
            agent_name
        ))
    })?;
    let role: AgentRoleConfig = role_value.clone().try_into().map_err(|source| {
        LunaError::Config(format!(
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
            LunaError::Config(format!(
                "failed to read agent config file for '{}' at {}: {error}",
                agent_name,
                role_config_path.display()
            ))
        })?;
        let role_config: AgentConfigLayer =
            toml::from_str(&role_config_raw).map_err(|source| LunaError::Toml {
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
        return Err(LunaError::Config(format!(
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
) -> Result<String, LunaError> {
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
            LunaError::Config(format!(
                "failed to read model_instructions_file for '{}' at {}: {error}",
                agent_name,
                instructions_path.display()
            ))
        })?;
        let trimmed = instructions.trim();
        if trimmed.is_empty() {
            return Err(LunaError::Config(format!(
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

    Err(LunaError::Config(format!(
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
#[allow(clippy::useless_conversion)]
mod tests {
    use super::*;
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
        let path = env::temp_dir().join(format!("luna-tests-{}-{stamp}-{seq}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
