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
use crate::schema::OpenAiSerializable;

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
    None,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

impl ModelReasoningEffort {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelReasoningSummary {
    None,
    Auto,
    Concise,
    Detailed,
}

impl ModelReasoningSummary {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Concise => "concise",
            Self::Detailed => "detailed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Personality {
    None,
    Friendly,
    Pragmatic,
}

impl Personality {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Friendly => "friendly",
            Self::Pragmatic => "pragmatic",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollaborationModeKind {
    Plan,
    Default,
}

impl CollaborationModeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationModeSettings {
    pub model: String,
    pub reasoning_effort: Option<ModelReasoningEffort>,
    pub developer_instructions: Option<String>,
}

impl CollaborationModeSettings {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            reasoning_effort: None,
            developer_instructions: None,
        }
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: ModelReasoningEffort) -> Self {
        self.reasoning_effort = Some(reasoning_effort);
        self
    }

    pub fn with_developer_instructions(
        mut self,
        developer_instructions: impl Into<String>,
    ) -> Self {
        self.developer_instructions = Some(developer_instructions.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollaborationMode {
    pub mode: CollaborationModeKind,
    pub settings: CollaborationModeSettings,
}

impl CollaborationMode {
    pub fn new(mode: CollaborationModeKind, settings: CollaborationModeSettings) -> Self {
        Self { mode, settings }
    }

    fn as_value(&self) -> Value {
        let mut settings = Map::new();
        settings.insert(
            "model".to_string(),
            Value::String(self.settings.model.clone()),
        );
        if let Some(reasoning_effort) = self.settings.reasoning_effort {
            settings.insert(
                "reasoning_effort".to_string(),
                Value::String(reasoning_effort.as_str().to_string()),
            );
        }
        if let Some(instructions) = &self.settings.developer_instructions {
            settings.insert(
                "developer_instructions".to_string(),
                Value::String(instructions.clone()),
            );
        }

        let mut value = Map::new();
        value.insert(
            "mode".to_string(),
            Value::String(self.mode.as_str().to_string()),
        );
        value.insert("settings".to_string(), Value::Object(settings));
        Value::Object(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl DynamicToolSpec {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }

    fn as_value(&self) -> Value {
        let mut value = Map::new();
        value.insert("name".to_string(), Value::String(self.name.clone()));
        value.insert(
            "description".to_string(),
            Value::String(self.description.clone()),
        );
        value.insert("inputSchema".to_string(), self.input_schema.clone());
        Value::Object(value)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadOptions {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub sandbox_mode: Option<SandboxMode>,
    pub sandbox_policy: Option<Value>,
    pub working_directory: Option<String>,
    pub skip_git_repo_check: Option<bool>,
    pub model_reasoning_effort: Option<ModelReasoningEffort>,
    pub model_reasoning_summary: Option<ModelReasoningSummary>,
    pub network_access_enabled: Option<bool>,
    pub web_search_mode: Option<WebSearchMode>,
    pub web_search_enabled: Option<bool>,
    pub approval_policy: Option<ApprovalMode>,
    pub additional_directories: Option<Vec<String>>,
    pub personality: Option<Personality>,
    pub base_instructions: Option<String>,
    pub developer_instructions: Option<String>,
    pub ephemeral: Option<bool>,
    pub collaboration_mode: Option<CollaborationMode>,
    pub config: Option<Map<String, Value>>,
    pub dynamic_tools: Option<Vec<DynamicToolSpec>>,
    pub experimental_raw_events: Option<bool>,
    pub persist_extended_history: Option<bool>,
}

impl ThreadOptions {
    pub fn builder() -> ThreadOptionsBuilder {
        ThreadOptionsBuilder::new()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThreadOptionsBuilder {
    options: ThreadOptions,
}

impl ThreadOptionsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(self) -> ThreadOptions {
        self.options
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.options.model = Some(model.into());
        self
    }

    pub fn model_provider(mut self, model_provider: impl Into<String>) -> Self {
        self.options.model_provider = Some(model_provider.into());
        self
    }

    pub fn sandbox_mode(mut self, sandbox_mode: SandboxMode) -> Self {
        self.options.sandbox_mode = Some(sandbox_mode);
        self
    }

    pub fn sandbox_policy(mut self, sandbox_policy: Value) -> Self {
        self.options.sandbox_policy = Some(sandbox_policy);
        self
    }

    pub fn working_directory(mut self, working_directory: impl Into<String>) -> Self {
        self.options.working_directory = Some(working_directory.into());
        self
    }

    pub fn skip_git_repo_check(mut self, enabled: bool) -> Self {
        self.options.skip_git_repo_check = Some(enabled);
        self
    }

    pub fn model_reasoning_effort(mut self, model_reasoning_effort: ModelReasoningEffort) -> Self {
        self.options.model_reasoning_effort = Some(model_reasoning_effort);
        self
    }

    pub fn model_reasoning_summary(
        mut self,
        model_reasoning_summary: ModelReasoningSummary,
    ) -> Self {
        self.options.model_reasoning_summary = Some(model_reasoning_summary);
        self
    }

    pub fn network_access_enabled(mut self, enabled: bool) -> Self {
        self.options.network_access_enabled = Some(enabled);
        self
    }

    pub fn web_search_mode(mut self, web_search_mode: WebSearchMode) -> Self {
        self.options.web_search_mode = Some(web_search_mode);
        self
    }

    pub fn web_search_enabled(mut self, enabled: bool) -> Self {
        self.options.web_search_enabled = Some(enabled);
        self
    }

    pub fn approval_policy(mut self, approval_policy: ApprovalMode) -> Self {
        self.options.approval_policy = Some(approval_policy);
        self
    }

    pub fn additional_directories(mut self, additional_directories: Vec<String>) -> Self {
        self.options.additional_directories = Some(additional_directories);
        self
    }

    pub fn add_directory(mut self, directory: impl Into<String>) -> Self {
        self.options
            .additional_directories
            .get_or_insert_with(Vec::new)
            .push(directory.into());
        self
    }

    pub fn personality(mut self, personality: Personality) -> Self {
        self.options.personality = Some(personality);
        self
    }

    pub fn base_instructions(mut self, base_instructions: impl Into<String>) -> Self {
        self.options.base_instructions = Some(base_instructions.into());
        self
    }

    pub fn developer_instructions(mut self, developer_instructions: impl Into<String>) -> Self {
        self.options.developer_instructions = Some(developer_instructions.into());
        self
    }

    pub fn ephemeral(mut self, ephemeral: bool) -> Self {
        self.options.ephemeral = Some(ephemeral);
        self
    }

    pub fn collaboration_mode(mut self, collaboration_mode: CollaborationMode) -> Self {
        self.options.collaboration_mode = Some(collaboration_mode);
        self
    }

    pub fn config(mut self, config: Map<String, Value>) -> Self {
        self.options.config = Some(config);
        self
    }

    pub fn insert_config(mut self, key: impl Into<String>, value: Value) -> Self {
        self.options
            .config
            .get_or_insert_with(Map::new)
            .insert(key.into(), value);
        self
    }

    pub fn dynamic_tools(mut self, dynamic_tools: Vec<DynamicToolSpec>) -> Self {
        self.options.dynamic_tools = Some(dynamic_tools);
        self
    }

    pub fn experimental_raw_events(mut self, enabled: bool) -> Self {
        self.options.experimental_raw_events = Some(enabled);
        self
    }

    pub fn persist_extended_history(mut self, enabled: bool) -> Self {
        self.options.persist_extended_history = Some(enabled);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct TurnOptions {
    pub output_schema: Option<Value>,
}

impl TurnOptions {
    pub fn builder() -> TurnOptionsBuilder {
        TurnOptionsBuilder::new()
    }

    pub fn with_output_schema(mut self, output_schema: Value) -> Self {
        self.output_schema = Some(output_schema);
        self
    }

    pub fn with_output_schema_for<T: OpenAiSerializable>(mut self) -> Self {
        self.output_schema = Some(T::openai_output_schema());
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct TurnOptionsBuilder {
    options: TurnOptions,
}

impl TurnOptionsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(self) -> TurnOptions {
        self.options
    }

    pub fn output_schema(mut self, output_schema: Value) -> Self {
        self.options.output_schema = Some(output_schema);
        self
    }

    pub fn output_schema_for<T: OpenAiSerializable>(mut self) -> Self {
        self.options.output_schema = Some(T::openai_output_schema());
        self
    }

    pub fn clear_output_schema(mut self) -> Self {
        self.options.output_schema = None;
        self
    }
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
    if let Some(config) = &options.config {
        extra.insert("config".to_string(), Value::Object(config.clone()));
    }
    if let Some(dynamic_tools) = &options.dynamic_tools {
        extra.insert(
            "dynamicTools".to_string(),
            Value::Array(
                dynamic_tools
                    .iter()
                    .map(DynamicToolSpec::as_value)
                    .collect(),
            ),
        );
    }
    if let Some(enabled) = options.experimental_raw_events {
        extra.insert("experimentalRawEvents".to_string(), Value::Bool(enabled));
    }
    if let Some(enabled) = options.persist_extended_history {
        extra.insert("persistExtendedHistory".to_string(), Value::Bool(enabled));
    }

    requests::ThreadStartParams {
        model: options.model.clone(),
        model_provider: options.model_provider.clone(),
        cwd: options.working_directory.clone(),
        approval_policy: options
            .approval_policy
            .map(|mode| mode.as_str().to_string()),
        sandbox: options.sandbox_mode.map(|mode| mode.as_str().to_string()),
        sandbox_policy: options.sandbox_policy.clone(),
        effort: options
            .model_reasoning_effort
            .map(|effort| effort.as_str().to_string()),
        summary: options
            .model_reasoning_summary
            .map(|summary| summary.as_str().to_string()),
        personality: options.personality.map(|value| value.as_str().to_string()),
        ephemeral: options.ephemeral,
        base_instructions: options.base_instructions.clone(),
        developer_instructions: options.developer_instructions.clone(),
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
    if let Some(collaboration_mode) = &options.collaboration_mode {
        extra.insert(
            "collaborationMode".to_string(),
            collaboration_mode.as_value(),
        );
    }

    requests::TurnStartParams {
        thread_id: thread_id.to_string(),
        input: normalize_input(input),
        cwd: options.working_directory.clone(),
        model: options.model.clone(),
        model_provider: options.model_provider.clone(),
        effort: options
            .model_reasoning_effort
            .map(|effort| effort.as_str().to_string()),
        summary: options
            .model_reasoning_summary
            .map(|summary| summary.as_str().to_string()),
        personality: options.personality.map(|value| value.as_str().to_string()),
        output_schema: turn_options.output_schema.clone(),
        approval_policy: options
            .approval_policy
            .map(|mode| mode.as_str().to_string()),
        sandbox_policy: options.sandbox_policy.clone(),
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
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        Serialize,
        Deserialize,
        JsonSchema,
        codex_app_server_sdk::OpenAiSerializable,
    )]
    struct StructuredReply {
        answer: String,
    }

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

    #[test]
    fn thread_options_builder_maps_extended_protocol_fields() {
        let collaboration_mode = CollaborationMode::new(
            CollaborationModeKind::Default,
            CollaborationModeSettings::new("gpt-5.2-codex")
                .with_reasoning_effort(ModelReasoningEffort::High),
        );

        let options = ThreadOptions::builder()
            .model("gpt-5.2-codex")
            .model_provider("mock_provider")
            .sandbox_mode(SandboxMode::WorkspaceWrite)
            .sandbox_policy(json!({"type": "dangerFullAccess"}))
            .working_directory("/tmp/workspace")
            .skip_git_repo_check(true)
            .model_reasoning_effort(ModelReasoningEffort::None)
            .model_reasoning_summary(ModelReasoningSummary::Auto)
            .network_access_enabled(true)
            .web_search_mode(WebSearchMode::Live)
            .web_search_enabled(false)
            .approval_policy(ApprovalMode::OnRequest)
            .add_directory("/tmp/one")
            .add_directory("/tmp/two")
            .personality(Personality::Pragmatic)
            .base_instructions("base instructions")
            .developer_instructions("developer instructions")
            .ephemeral(true)
            .insert_config("sandbox_workspace_write.network_access", Value::Bool(true))
            .dynamic_tools(vec![DynamicToolSpec::new(
                "demo_tool",
                "Demo dynamic tool",
                json!({"type": "object"}),
            )])
            .experimental_raw_events(true)
            .persist_extended_history(true)
            .collaboration_mode(collaboration_mode)
            .build();

        let thread_params = build_thread_start_params(&options);
        assert_eq!(thread_params.model.as_deref(), Some("gpt-5.2-codex"));
        assert_eq!(
            thread_params.model_provider.as_deref(),
            Some("mock_provider")
        );
        assert_eq!(thread_params.cwd.as_deref(), Some("/tmp/workspace"));
        assert_eq!(thread_params.approval_policy.as_deref(), Some("on-request"));
        assert_eq!(thread_params.sandbox.as_deref(), Some("workspace-write"));
        assert_eq!(
            thread_params.sandbox_policy,
            Some(json!({"type": "dangerFullAccess"}))
        );
        assert_eq!(thread_params.effort.as_deref(), Some("none"));
        assert_eq!(thread_params.summary.as_deref(), Some("auto"));
        assert_eq!(thread_params.personality.as_deref(), Some("pragmatic"));
        assert_eq!(thread_params.ephemeral, Some(true));
        assert_eq!(
            thread_params.base_instructions.as_deref(),
            Some("base instructions")
        );
        assert_eq!(
            thread_params.developer_instructions.as_deref(),
            Some("developer instructions")
        );
        assert_eq!(
            thread_params.extra.get("skipGitRepoCheck"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            thread_params.extra.get("webSearchMode"),
            Some(&Value::String("live".to_string()))
        );
        assert_eq!(
            thread_params.extra.get("webSearchEnabled"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            thread_params.extra.get("networkAccessEnabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            thread_params.extra.get("additionalDirectories"),
            Some(&json!(["/tmp/one", "/tmp/two"]))
        );
        assert_eq!(
            thread_params.extra.get("config"),
            Some(&json!({"sandbox_workspace_write.network_access": true}))
        );
        assert_eq!(
            thread_params.extra.get("dynamicTools"),
            Some(&json!([{
                "name": "demo_tool",
                "description": "Demo dynamic tool",
                "inputSchema": {"type": "object"}
            }]))
        );
        assert_eq!(
            thread_params.extra.get("experimentalRawEvents"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            thread_params.extra.get("persistExtendedHistory"),
            Some(&Value::Bool(true))
        );

        let turn_params = build_turn_start_params(
            "thread_123",
            Input::text("hello"),
            &options,
            &TurnOptions::default(),
        );
        assert_eq!(turn_params.model_provider.as_deref(), Some("mock_provider"));
        assert_eq!(turn_params.effort.as_deref(), Some("none"));
        assert_eq!(turn_params.summary.as_deref(), Some("auto"));
        assert_eq!(turn_params.personality.as_deref(), Some("pragmatic"));
        assert_eq!(
            turn_params.sandbox_policy,
            Some(json!({"type": "dangerFullAccess"}))
        );
        assert_eq!(
            turn_params.extra.get("collaborationMode"),
            Some(&json!({
                "mode": "default",
                "settings": {
                    "model": "gpt-5.2-codex",
                    "reasoning_effort": "high"
                }
            }))
        );
    }

    #[test]
    fn thread_options_builder_skip_git_repo_check_matches_cli_flag_semantics() {
        let enabled = ThreadOptions::builder().skip_git_repo_check(true).build();
        let enabled_params = build_thread_start_params(&enabled);
        assert_eq!(
            enabled_params.extra.get("skipGitRepoCheck"),
            Some(&Value::Bool(true))
        );

        let disabled = ThreadOptions::builder().skip_git_repo_check(false).build();
        let disabled_params = build_thread_start_params(&disabled);
        assert_eq!(
            disabled_params.extra.get("skipGitRepoCheck"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn turn_options_builder_sets_typed_output_schema() {
        let turn_options = TurnOptions::builder()
            .output_schema_for::<StructuredReply>()
            .build();
        let turn_params = build_turn_start_params(
            "thread_123",
            Input::text("hello"),
            &ThreadOptions::default(),
            &turn_options,
        );

        assert_eq!(
            turn_params.output_schema,
            Some(StructuredReply::openai_output_schema())
        );
    }

    #[test]
    fn turn_options_builder_clear_output_schema_overrides_previous_value() {
        let turn_options = TurnOptions::builder()
            .output_schema(json!({"type": "object"}))
            .clear_output_schema()
            .build();
        let turn_params = build_turn_start_params(
            "thread_123",
            Input::text("hello"),
            &ThreadOptions::default(),
            &turn_options,
        );

        assert_eq!(turn_params.output_schema, None);
    }

    #[test]
    fn turn_options_value_helpers_set_raw_and_typed_schemas() {
        let raw = TurnOptions::default().with_output_schema(json!({"type": "object"}));
        assert_eq!(raw.output_schema, Some(json!({"type": "object"})));

        let typed = TurnOptions::default().with_output_schema_for::<StructuredReply>();
        assert_eq!(
            typed.output_schema,
            Some(StructuredReply::openai_output_schema())
        );
    }
}
