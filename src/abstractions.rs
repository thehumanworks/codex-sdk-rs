use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value};
use thiserror::Error;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;

use crate::CodexClient;
use crate::client::StdioConfig;
#[cfg(feature = "ws")]
use crate::client::WsConfig;
use crate::error::ClientError;
use crate::events::{ServerEvent, ServerNotification};
use crate::protocol::requests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    Never,
    OnRequest,
    OnFailure,
    Untrusted,
}

impl ApprovalMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on-request",
            Self::OnFailure => "on-failure",
            Self::Untrusted => "untrusted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl SandboxMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ModelReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchMode {
    Disabled,
    Cached,
    Live,
}

impl WebSearchMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Cached => "cached",
            Self::Live => "live",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadOptions {
    pub model: Option<String>,
    pub sandbox_mode: Option<SandboxMode>,
    pub working_directory: Option<String>,
    pub skip_git_repo_check: Option<bool>,
    pub model_reasoning_effort: Option<ModelReasoningEffort>,
    pub network_access_enabled: Option<bool>,
    pub web_search_mode: Option<WebSearchMode>,
    pub web_search_enabled: Option<bool>,
    pub approval_policy: Option<ApprovalMode>,
    pub additional_directories: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct TurnOptions {
    pub output_schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadError {
    pub message: String,
}

#[derive(Debug, Error)]
pub enum ThreadRunError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}")]
    TurnFailed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInput {
    Text { text: String },
    LocalImage { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Text(String),
    Items(Vec<UserInput>),
}

impl Input {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub fn items(items: Vec<UserInput>) -> Self {
        Self::Items(items)
    }
}

impl From<String> for Input {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for Input {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<Vec<UserInput>> for Input {
    fn from(value: Vec<UserInput>) -> Self {
        Self::Items(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub items: Vec<ThreadItem>,
    pub final_response: String,
    pub usage: Option<Usage>,
}

pub type RunResult = Turn;

pub struct StreamedTurn {
    receiver: mpsc::Receiver<Result<ThreadEvent, ClientError>>,
    task: JoinHandle<()>,
}

impl StreamedTurn {
    pub async fn next_event(&mut self) -> Option<Result<ThreadEvent, ClientError>> {
        self.receiver.recv().await
    }
}

impl Drop for StreamedTurn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadEvent {
    ThreadStarted { thread_id: String },
    TurnStarted,
    TurnCompleted { usage: Option<Usage> },
    TurnFailed { error: ThreadError },
    ItemStarted { item: ThreadItem },
    ItemUpdated { item: ThreadItem },
    ItemCompleted { item: ThreadItem },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessageItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningItem {
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandExecutionStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandExecutionItem {
    pub id: String,
    pub command: String,
    pub aggregated_output: String,
    pub exit_code: Option<i32>,
    pub status: CommandExecutionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchChangeKind {
    Add,
    Delete,
    Update,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUpdateChange {
    pub path: String,
    pub kind: PatchChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchApplyStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeItem {
    pub id: String,
    pub changes: Vec<FileUpdateChange>,
    pub status: PatchApplyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpToolCallStatus {
    InProgress,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpToolCallItem {
    pub id: String,
    pub server: String,
    pub tool: String,
    pub arguments: Value,
    pub result: Option<Value>,
    pub error: Option<ThreadError>,
    pub status: McpToolCallStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchItem {
    pub id: String,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub text: String,
    pub completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoListItem {
    pub id: String,
    pub items: Vec<TodoItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorItem {
    pub id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnknownItem {
    pub id: Option<String>,
    pub item_type: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadItem {
    AgentMessage(AgentMessageItem),
    Reasoning(ReasoningItem),
    CommandExecution(CommandExecutionItem),
    FileChange(FileChangeItem),
    McpToolCall(McpToolCallItem),
    WebSearch(WebSearchItem),
    TodoList(TodoListItem),
    Error(ErrorItem),
    Unknown(UnknownItem),
}

#[derive(Clone)]
pub struct Codex {
    inner: Arc<CodexInner>,
}

struct CodexInner {
    client: CodexClient,
    initialize_params: requests::InitializeParams,
    initialized: AtomicBool,
    initialize_lock: Mutex<()>,
}

impl Codex {
    pub fn with_initialize_params(
        client: CodexClient,
        initialize_params: requests::InitializeParams,
    ) -> Self {
        Self {
            inner: Arc::new(CodexInner {
                client,
                initialize_params,
                initialized: AtomicBool::new(false),
                initialize_lock: Mutex::new(()),
            }),
        }
    }

    pub fn from_client(client: CodexClient) -> Self {
        let initialize_params = requests::InitializeParams::new(requests::ClientInfo::new(
            "codex_sdk_rs",
            "Codex Rust SDK",
            env!("CARGO_PKG_VERSION"),
        ));
        Self::with_initialize_params(client, initialize_params)
    }

    pub async fn spawn_stdio(config: StdioConfig) -> Result<Self, ClientError> {
        let client = CodexClient::spawn_stdio(config).await?;
        Ok(Self::from_client(client))
    }

    #[cfg(feature = "ws")]
    pub async fn connect_ws(config: WsConfig) -> Result<Self, ClientError> {
        let client = CodexClient::connect_ws(config).await?;
        Ok(Self::from_client(client))
    }

    pub fn start_thread(&self, options: ThreadOptions) -> Thread {
        Thread {
            codex: self.clone(),
            id: None,
            needs_resume: false,
            options,
        }
    }

    pub fn resume_thread(&self, id: impl Into<String>, options: ThreadOptions) -> Thread {
        Thread {
            codex: self.clone(),
            id: Some(id.into()),
            needs_resume: true,
            options,
        }
    }

    pub fn client(&self) -> CodexClient {
        self.inner.client.clone()
    }

    async fn ensure_initialized(&self) -> Result<(), ClientError> {
        if self.inner.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }

        let _guard = self.inner.initialize_lock.lock().await;
        if self.inner.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }

        match self
            .inner
            .client
            .initialize(self.inner.initialize_params.clone())
            .await
        {
            Ok(_) | Err(ClientError::AlreadyInitialized) => {}
            Err(err) => return Err(err),
        }

        self.inner.client.initialized().await?;
        self.inner.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }
}

pub struct Thread {
    codex: Codex,
    id: Option<String>,
    needs_resume: bool,
    options: ThreadOptions,
}

impl Thread {
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub async fn run_streamed(
        &mut self,
        input: impl Into<Input>,
        turn_options: TurnOptions,
    ) -> Result<StreamedTurn, ClientError> {
        self.codex.ensure_initialized().await?;

        let mut emit_thread_started = None;
        if self.id.is_none() {
            let thread = self
                .codex
                .inner
                .client
                .thread_start(build_thread_start_params(&self.options))
                .await?;
            self.id = Some(thread.thread.id.clone());
            emit_thread_started = Some(thread.thread.id);
            self.needs_resume = false;
        } else if self.needs_resume {
            let resume_params = requests::ThreadResumeParams {
                thread_id: self.id.clone().unwrap_or_default(),
                extra: Map::new(),
            };
            let resumed = self.codex.inner.client.thread_resume(resume_params).await?;
            self.id = Some(resumed.thread.id);
            self.needs_resume = false;
        }

        let thread_id = self.id.clone().ok_or_else(|| {
            ClientError::TransportSend("thread id unavailable after start/resume".to_string())
        })?;

        let server_events = self.codex.inner.client.subscribe();

        let turn_response = self
            .codex
            .inner
            .client
            .turn_start(build_turn_start_params(
                &thread_id,
                input.into(),
                &self.options,
                &turn_options,
            ))
            .await?;
        let turn_id = turn_response.turn.id;

        let (tx, rx) = mpsc::channel(256);

        if let Some(started_thread_id) = emit_thread_started {
            if tx
                .send(Ok(ThreadEvent::ThreadStarted {
                    thread_id: started_thread_id,
                }))
                .await
                .is_err()
            {
                return Err(ClientError::TransportClosed);
            }
        }

        if tx.send(Ok(ThreadEvent::TurnStarted)).await.is_err() {
            return Err(ClientError::TransportClosed);
        }

        let task = tokio::spawn(async move {
            pump_turn_events(server_events, tx, thread_id, turn_id).await;
        });

        Ok(StreamedTurn { receiver: rx, task })
    }

    pub async fn run(
        &mut self,
        input: impl Into<Input>,
        turn_options: TurnOptions,
    ) -> Result<Turn, ThreadRunError> {
        let mut streamed = self.run_streamed(input, turn_options).await?;
        let mut items = Vec::new();
        let mut final_response = String::new();
        let mut usage = None;
        let mut saw_terminal = false;

        while let Some(next) = streamed.next_event().await {
            let event = next.map_err(ThreadRunError::Client)?;
            match event {
                ThreadEvent::ItemCompleted { item } => {
                    if let ThreadItem::AgentMessage(agent) = &item {
                        final_response = agent.text.clone();
                    }
                    items.push(item);
                }
                ThreadEvent::TurnCompleted { usage: completed } => {
                    usage = completed;
                    saw_terminal = true;
                    break;
                }
                ThreadEvent::TurnFailed { error } => {
                    return Err(ThreadRunError::TurnFailed {
                        message: error.message,
                    });
                }
                ThreadEvent::Error { message } => {
                    return Err(ThreadRunError::TurnFailed { message });
                }
                ThreadEvent::ThreadStarted { .. }
                | ThreadEvent::TurnStarted
                | ThreadEvent::ItemStarted { .. }
                | ThreadEvent::ItemUpdated { .. } => {}
            }
        }

        if !saw_terminal {
            return Err(ThreadRunError::Client(ClientError::TransportClosed));
        }

        Ok(Turn {
            items,
            final_response,
            usage,
        })
    }
}

async fn pump_turn_events(
    mut server_events: tokio::sync::broadcast::Receiver<ServerEvent>,
    tx: mpsc::Sender<Result<ThreadEvent, ClientError>>,
    thread_id: String,
    turn_id: String,
) {
    let mut latest_usage: Option<Usage> = None;

    loop {
        let next = server_events.recv().await;
        let server_event = match next {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                let _ = tx.send(Err(ClientError::TransportClosed)).await;
                break;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                continue;
            }
        };

        match server_event {
            ServerEvent::Notification(notification) => match notification {
                ServerNotification::ItemStarted(payload)
                    if matches_target_from_extra(&payload.extra, &thread_id, Some(&turn_id)) =>
                {
                    if tx
                        .send(Ok(ThreadEvent::ItemStarted {
                            item: parse_thread_item(payload.item),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                ServerNotification::ItemCompleted(payload)
                    if matches_target_from_extra(&payload.extra, &thread_id, Some(&turn_id)) =>
                {
                    if tx
                        .send(Ok(ThreadEvent::ItemCompleted {
                            item: parse_thread_item(payload.item),
                        }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                ServerNotification::ItemAgentMessageDelta(delta)
                    if matches_target_from_extra(&delta.extra, &thread_id, Some(&turn_id)) =>
                {
                    let text = delta.delta.or(delta.text).unwrap_or_default();
                    if text.is_empty() {
                        continue;
                    }
                    let item = ThreadItem::AgentMessage(AgentMessageItem {
                        id: delta.item_id.unwrap_or_default(),
                        text,
                    });
                    if tx
                        .send(Ok(ThreadEvent::ItemUpdated { item }))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                ServerNotification::TurnCompleted(payload)
                    if payload.turn.id == turn_id
                        && matches_target_from_extra(
                            &payload.turn.extra,
                            &thread_id,
                            Some(&turn_id),
                        ) =>
                {
                    let status = payload.turn.status.unwrap_or_default().to_ascii_lowercase();
                    if status == "failed" {
                        let message = payload
                            .turn
                            .error
                            .map(|error| error.message)
                            .unwrap_or_else(|| "turn failed".to_string());
                        let _ = tx
                            .send(Ok(ThreadEvent::TurnFailed {
                                error: ThreadError { message },
                            }))
                            .await;
                        break;
                    }

                    let usage = parse_usage_from_turn_extra(&payload.turn.extra)
                        .or_else(|| latest_usage.clone());
                    let _ = tx.send(Ok(ThreadEvent::TurnCompleted { usage })).await;
                    break;
                }
                ServerNotification::ThreadTokenUsageUpdated(payload)
                    if payload
                        .thread_id
                        .as_deref()
                        .is_none_or(|incoming| incoming == thread_id) =>
                {
                    if !matches_target_from_extra(&payload.extra, &thread_id, Some(&turn_id)) {
                        continue;
                    }
                    latest_usage = payload
                        .usage
                        .as_ref()
                        .and_then(parse_usage_from_value)
                        .or_else(|| {
                            payload
                                .extra
                                .get("tokenUsage")
                                .and_then(parse_usage_from_value)
                        });
                }
                ServerNotification::Error(payload)
                    if matches_target_from_extra(&payload.extra, &thread_id, Some(&turn_id)) =>
                {
                    let _ = tx
                        .send(Ok(ThreadEvent::Error {
                            message: payload.error.message,
                        }))
                        .await;
                }
                _ => {}
            },
            ServerEvent::TransportClosed => {
                let _ = tx.send(Err(ClientError::TransportClosed)).await;
                break;
            }
            ServerEvent::CompatibilityWarning(_) | ServerEvent::ServerRequest(_) => {}
        }
    }
}

fn build_thread_start_params(options: &ThreadOptions) -> requests::ThreadStartParams {
    let mut extra = Map::new();
    if let Some(skip) = options.skip_git_repo_check {
        extra.insert("skipGitRepoCheck".to_string(), Value::Bool(skip));
    }
    if let Some(mode) = options.web_search_mode {
        extra.insert(
            "webSearchMode".to_string(),
            Value::String(mode.as_str().to_string()),
        );
    }
    if let Some(enabled) = options.web_search_enabled {
        extra.insert("webSearchEnabled".to_string(), Value::Bool(enabled));
    }
    if let Some(network) = options.network_access_enabled {
        extra.insert("networkAccessEnabled".to_string(), Value::Bool(network));
    }
    if let Some(additional) = &options.additional_directories {
        extra.insert(
            "additionalDirectories".to_string(),
            Value::Array(
                additional
                    .iter()
                    .map(|entry| Value::String(entry.clone()))
                    .collect(),
            ),
        );
    }

    requests::ThreadStartParams {
        model: options.model.clone(),
        model_provider: None,
        cwd: options.working_directory.clone(),
        approval_policy: options
            .approval_policy
            .map(|mode| mode.as_str().to_string()),
        sandbox: options.sandbox_mode.map(|mode| mode.as_str().to_string()),
        sandbox_policy: None,
        effort: options
            .model_reasoning_effort
            .map(|effort| effort.as_str().to_string()),
        summary: None,
        personality: None,
        ephemeral: None,
        base_instructions: None,
        developer_instructions: None,
        extra,
    }
}

fn build_turn_start_params(
    thread_id: &str,
    input: Input,
    options: &ThreadOptions,
    turn_options: &TurnOptions,
) -> requests::TurnStartParams {
    let mut extra = Map::new();
    if let Some(skip) = options.skip_git_repo_check {
        extra.insert("skipGitRepoCheck".to_string(), Value::Bool(skip));
    }
    if let Some(mode) = options.web_search_mode {
        extra.insert(
            "webSearchMode".to_string(),
            Value::String(mode.as_str().to_string()),
        );
    }
    if let Some(enabled) = options.web_search_enabled {
        extra.insert("webSearchEnabled".to_string(), Value::Bool(enabled));
    }
    if let Some(network) = options.network_access_enabled {
        extra.insert("networkAccessEnabled".to_string(), Value::Bool(network));
    }
    if let Some(additional) = &options.additional_directories {
        extra.insert(
            "additionalDirectories".to_string(),
            Value::Array(
                additional
                    .iter()
                    .map(|entry| Value::String(entry.clone()))
                    .collect(),
            ),
        );
    }

    requests::TurnStartParams {
        thread_id: thread_id.to_string(),
        input: normalize_input(input),
        cwd: options.working_directory.clone(),
        model: options.model.clone(),
        model_provider: None,
        effort: options
            .model_reasoning_effort
            .map(|effort| effort.as_str().to_string()),
        summary: None,
        personality: None,
        output_schema: turn_options.output_schema.clone(),
        approval_policy: options
            .approval_policy
            .map(|mode| mode.as_str().to_string()),
        sandbox_policy: None,
        collaboration_mode: None,
        extra,
    }
}

fn normalize_input(input: Input) -> Vec<requests::TurnInputItem> {
    match input {
        Input::Text(text) => vec![requests::TurnInputItem::Text { text }],
        Input::Items(items) => {
            let mut text_parts = Vec::new();
            let mut normalized = Vec::new();

            for item in items {
                match item {
                    UserInput::Text { text } => text_parts.push(text),
                    UserInput::LocalImage { path } => {
                        normalized.push(requests::TurnInputItem::LocalImage { path });
                    }
                }
            }

            if !text_parts.is_empty() {
                normalized.insert(
                    0,
                    requests::TurnInputItem::Text {
                        text: text_parts.join("\n\n"),
                    },
                );
            }

            normalized
        }
    }
}

fn matches_target_from_extra(
    extra: &Map<String, Value>,
    thread_id: &str,
    turn_id: Option<&str>,
) -> bool {
    let thread_matches = extra
        .get("threadId")
        .and_then(Value::as_str)
        .map(|incoming| incoming == thread_id)
        .unwrap_or(true);

    let turn_matches = match turn_id {
        Some(target_turn_id) => extra
            .get("turnId")
            .and_then(Value::as_str)
            .map(|incoming| incoming == target_turn_id)
            .unwrap_or(true),
        None => true,
    };

    thread_matches && turn_matches
}

fn parse_usage_from_turn_extra(extra: &Map<String, Value>) -> Option<Usage> {
    extra
        .get("usage")
        .and_then(parse_usage_from_value)
        .or_else(|| extra.get("tokenUsage").and_then(parse_usage_from_value))
}

fn parse_usage_from_value(value: &Value) -> Option<Usage> {
    let object = value.as_object()?;
    if let Some(last) = object.get("last") {
        return parse_usage_from_value(last);
    }

    let input_tokens = get_i64(object, "inputTokens", "input_tokens")?;
    let cached_input_tokens =
        get_i64(object, "cachedInputTokens", "cached_input_tokens").unwrap_or(0);
    let output_tokens = get_i64(object, "outputTokens", "output_tokens")?;

    Some(Usage {
        input_tokens,
        cached_input_tokens,
        output_tokens,
    })
}

fn get_i64(object: &Map<String, Value>, camel: &str, snake: &str) -> Option<i64> {
    object
        .get(camel)
        .or_else(|| object.get(snake))
        .and_then(Value::as_i64)
}

fn parse_thread_item(item: Value) -> ThreadItem {
    let object = match item.as_object() {
        Some(object) => object,
        None => {
            return ThreadItem::Unknown(UnknownItem {
                id: None,
                item_type: None,
                raw: item,
            });
        }
    };

    let item_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    match item_type.as_deref() {
        Some("agentMessage") => ThreadItem::AgentMessage(AgentMessageItem {
            id: id.unwrap_or_default(),
            text: object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        Some("reasoning") => ThreadItem::Reasoning(ReasoningItem {
            id: id.unwrap_or_default(),
            text: parse_reasoning_text(object),
        }),
        Some("commandExecution") => ThreadItem::CommandExecution(CommandExecutionItem {
            id: id.unwrap_or_default(),
            command: object
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            aggregated_output: object
                .get("aggregatedOutput")
                .or_else(|| object.get("aggregated_output"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            exit_code: object
                .get("exitCode")
                .or_else(|| object.get("exit_code"))
                .and_then(Value::as_i64)
                .map(|value| value as i32),
            status: parse_command_execution_status(
                object
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        }),
        Some("fileChange") => ThreadItem::FileChange(FileChangeItem {
            id: id.unwrap_or_default(),
            changes: object
                .get("changes")
                .and_then(Value::as_array)
                .map(|changes| {
                    changes
                        .iter()
                        .filter_map(parse_file_update_change)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            status: parse_patch_apply_status(
                object
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        }),
        Some("mcpToolCall") => {
            let error = object
                .get("error")
                .and_then(Value::as_object)
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(|message| ThreadError {
                    message: message.to_string(),
                });

            ThreadItem::McpToolCall(McpToolCallItem {
                id: id.unwrap_or_default(),
                server: object
                    .get("server")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                tool: object
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: object.get("arguments").cloned().unwrap_or(Value::Null),
                result: object.get("result").cloned(),
                error,
                status: parse_mcp_tool_call_status(
                    object
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
            })
        }
        Some("webSearch") => ThreadItem::WebSearch(WebSearchItem {
            id: id.unwrap_or_default(),
            query: object
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        Some("todoList") => ThreadItem::TodoList(TodoListItem {
            id: id.unwrap_or_default(),
            items: object
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(parse_todo_item).collect())
                .unwrap_or_default(),
        }),
        Some("error") => ThreadItem::Error(ErrorItem {
            id: id.unwrap_or_default(),
            message: object
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        _ => ThreadItem::Unknown(UnknownItem {
            id,
            item_type,
            raw: item,
        }),
    }
}

fn parse_reasoning_text(object: &Map<String, Value>) -> String {
    if let Some(text) = object.get("text").and_then(Value::as_str) {
        return text.to_string();
    }

    let summary = object
        .get("summary")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    let content = object
        .get("content")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();

    if summary.is_empty() {
        content
    } else if content.is_empty() {
        summary
    } else {
        format!("{summary}\n{content}")
    }
}

fn parse_file_update_change(change: &Value) -> Option<FileUpdateChange> {
    let object = change.as_object()?;
    let kind = match object.get("kind") {
        Some(Value::String(kind)) => parse_patch_change_kind(kind),
        Some(Value::Object(kind_object)) => kind_object
            .get("type")
            .and_then(Value::as_str)
            .map(parse_patch_change_kind)
            .unwrap_or(PatchChangeKind::Unknown),
        _ => PatchChangeKind::Unknown,
    };

    Some(FileUpdateChange {
        path: object
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind,
    })
}

fn parse_todo_item(value: &Value) -> Option<TodoItem> {
    let object = value.as_object()?;
    Some(TodoItem {
        text: object
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        completed: object
            .get("completed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_command_execution_status(status: &str) -> CommandExecutionStatus {
    match status {
        "inProgress" | "in_progress" => CommandExecutionStatus::InProgress,
        "completed" => CommandExecutionStatus::Completed,
        "failed" => CommandExecutionStatus::Failed,
        "declined" => CommandExecutionStatus::Declined,
        _ => CommandExecutionStatus::Unknown,
    }
}

fn parse_patch_change_kind(kind: &str) -> PatchChangeKind {
    match kind {
        "add" => PatchChangeKind::Add,
        "delete" => PatchChangeKind::Delete,
        "update" => PatchChangeKind::Update,
        _ => PatchChangeKind::Unknown,
    }
}

fn parse_patch_apply_status(status: &str) -> PatchApplyStatus {
    match status {
        "inProgress" | "in_progress" => PatchApplyStatus::InProgress,
        "completed" => PatchApplyStatus::Completed,
        "failed" => PatchApplyStatus::Failed,
        "declined" => PatchApplyStatus::Declined,
        _ => PatchApplyStatus::Unknown,
    }
}

fn parse_mcp_tool_call_status(status: &str) -> McpToolCallStatus {
    match status {
        "inProgress" | "in_progress" => McpToolCallStatus::InProgress,
        "completed" => McpToolCallStatus::Completed,
        "failed" => McpToolCallStatus::Failed,
        _ => McpToolCallStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_input_combines_text_and_images() {
        let normalized = normalize_input(Input::Items(vec![
            UserInput::Text {
                text: "first".to_string(),
            },
            UserInput::Text {
                text: "second".to_string(),
            },
            UserInput::LocalImage {
                path: "/tmp/one.png".to_string(),
            },
            UserInput::LocalImage {
                path: "/tmp/two.png".to_string(),
            },
        ]));

        assert_eq!(normalized.len(), 3);
        match &normalized[0] {
            requests::TurnInputItem::Text { text } => assert_eq!(text, "first\n\nsecond"),
            other => panic!("expected text input item, got {other:?}"),
        }
        match &normalized[1] {
            requests::TurnInputItem::LocalImage { path } => assert_eq!(path, "/tmp/one.png"),
            other => panic!("expected local image input item, got {other:?}"),
        }
        match &normalized[2] {
            requests::TurnInputItem::LocalImage { path } => assert_eq!(path, "/tmp/two.png"),
            other => panic!("expected local image input item, got {other:?}"),
        }
    }

    #[test]
    fn parse_agent_message_item() {
        let item = parse_thread_item(json!({
            "id": "item_1",
            "type": "agentMessage",
            "text": "hello"
        }));

        assert_eq!(
            item,
            ThreadItem::AgentMessage(AgentMessageItem {
                id: "item_1".to_string(),
                text: "hello".to_string(),
            })
        );
    }

    #[test]
    fn parse_usage_from_token_usage_payload() {
        let usage = parse_usage_from_value(&json!({
            "last": {
                "inputTokens": 10,
                "cachedInputTokens": 2,
                "outputTokens": 7
            }
        }));

        assert_eq!(
            usage,
            Some(Usage {
                input_tokens: 10,
                cached_input_tokens: 2,
                output_tokens: 7,
            })
        );
    }

    #[test]
    fn parse_unknown_item_preserves_payload() {
        let raw = json!({
            "id": "x",
            "type": "newItemType",
            "payload": {"a": 1}
        });
        let parsed = parse_thread_item(raw.clone());

        match parsed {
            ThreadItem::Unknown(unknown) => {
                assert_eq!(unknown.id.as_deref(), Some("x"));
                assert_eq!(unknown.item_type.as_deref(), Some("newItemType"));
                assert_eq!(unknown.raw, raw);
            }
            other => panic!("expected unknown item, got {other:?}"),
        }
    }
}
