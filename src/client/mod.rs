use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};

use crate::compat::{CompatibilityPolicy, check_cli_version, parse_cli_version};
use crate::error::{ClientError, IncomingClassified, RpcError, classify_incoming};
use crate::events::{
    ServerEvent, ServerNotification, ServerRequestEvent, parse_notification, parse_server_request,
};
use crate::protocol::requests;
use crate::protocol::responses;
use crate::protocol::server_requests;
use crate::protocol::shared::{EmptyObject, RequestId};
use crate::transport::TransportHandle;
#[cfg(feature = "stdio")]
use crate::transport::stdio::spawn_stdio_transport;
#[cfg(feature = "ws")]
use crate::transport::ws::connect_ws_transport;
#[cfg(feature = "ws")]
use crate::transport::ws_daemon::ensure_local_ws_app_server;

type PendingMap = HashMap<RequestId, oneshot::Sender<Result<Value, RpcError>>>;
type RefreshFuture = Pin<
    Box<
        dyn Future<Output = Result<server_requests::ChatgptAuthTokensRefreshResponse, ClientError>>
            + Send,
    >,
>;
type RefreshHandler =
    Arc<dyn Fn(server_requests::ChatgptAuthTokensRefreshParams) -> RefreshFuture + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub default_timeout: Duration,
    pub compatibility_policy: CompatibilityPolicy,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            compatibility_policy: CompatibilityPolicy::Warn,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub codex_binary: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub options: ClientOptions,
}

impl Default for StdioConfig {
    fn default() -> Self {
        Self {
            codex_binary: "codex".to_string(),
            args: vec!["app-server".to_string()],
            env: HashMap::new(),
            options: ClientOptions::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsConfig {
    pub url: String,
    pub options: ClientOptions,
}

struct Inner {
    outbound: mpsc::Sender<Value>,
    pending: Mutex<PendingMap>,
    default_timeout: Duration,
    initialized: AtomicBool,
    ready: AtomicBool,
    next_id: AtomicI64,
    event_tx: broadcast::Sender<ServerEvent>,
    event_rx: Mutex<broadcast::Receiver<ServerEvent>>,
    refresh_handler: RwLock<Option<RefreshHandler>>,
}

#[derive(Clone)]
pub struct CodexClient {
    inner: Arc<Inner>,
}

macro_rules! typed_method {
    ($fn_name:ident, $method:literal, $params_ty:ty, $result_ty:ty) => {
        pub async fn $fn_name(&self, params: $params_ty) -> Result<$result_ty, ClientError> {
            self.request_typed_internal($method, params, None, true)
                .await
        }
    };
}

macro_rules! typed_null_method {
    ($fn_name:ident, $method:literal, $result_ty:ty) => {
        pub async fn $fn_name(&self) -> Result<$result_ty, ClientError> {
            self.request_typed_value_internal($method, Value::Null, None, true)
                .await
        }
    };
}

impl CodexClient {
    #[cfg(feature = "stdio")]
    pub async fn spawn_stdio(config: StdioConfig) -> Result<Self, ClientError> {
        let version_output = detect_cli_version(&config.codex_binary).await;
        let warning = check_cli_version(
            version_output.as_deref().and_then(parse_cli_version),
            config.options.compatibility_policy,
        )?;

        let handle = spawn_stdio_transport(&config.codex_binary, &config.args, &config.env).await?;
        let client = Self::from_transport(handle, config.options.default_timeout);

        if let Some(w) = warning {
            client.publish_event(ServerEvent::CompatibilityWarning(w));
        }

        Ok(client)
    }

    #[cfg(feature = "ws")]
    pub async fn connect_ws(config: WsConfig) -> Result<Self, ClientError> {
        ensure_local_ws_app_server(&config.url).await?;
        let handle = connect_ws_transport(&config.url).await?;
        Ok(Self::from_transport(handle, config.options.default_timeout))
    }

    fn from_transport(handle: TransportHandle, default_timeout: Duration) -> Self {
        let (event_tx, event_rx) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            outbound: handle.outbound,
            pending: Mutex::new(HashMap::new()),
            default_timeout,
            initialized: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            next_id: AtomicI64::new(1),
            event_tx,
            event_rx: Mutex::new(event_rx),
            refresh_handler: RwLock::new(None),
        });

        tokio::spawn(run_inbound_loop(handle.inbound, inner.clone()));
        Self { inner }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.inner.event_tx.subscribe()
    }

    pub async fn next_event(&self) -> Result<ServerEvent, ClientError> {
        let mut rx = self.inner.event_rx.lock().await;
        rx.recv().await.map_err(|err| {
            ClientError::TransportSend(format!("event channel receive failed: {err}"))
        })
    }

    pub async fn set_chatgpt_auth_tokens_refresh_handler<F, Fut>(&self, handler: F)
    where
        F: Fn(server_requests::ChatgptAuthTokensRefreshParams) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<server_requests::ChatgptAuthTokensRefreshResponse, ClientError>>
            + Send
            + 'static,
    {
        let wrapped: RefreshHandler = Arc::new(move |params| Box::pin(handler(params)));
        *self.inner.refresh_handler.write().await = Some(wrapped);
    }

    pub async fn clear_chatgpt_auth_tokens_refresh_handler(&self) {
        *self.inner.refresh_handler.write().await = None;
    }

    pub async fn initialize(
        &self,
        params: requests::InitializeParams,
    ) -> Result<responses::InitializeResult, ClientError> {
        if self.inner.initialized.load(Ordering::SeqCst) {
            return Err(ClientError::AlreadyInitialized);
        }

        let result: responses::InitializeResult = self
            .request_typed_internal("initialize", params, None, false)
            .await?;

        self.inner.initialized.store(true, Ordering::SeqCst);
        Ok(result)
    }

    pub async fn initialized(&self) -> Result<(), ClientError> {
        if !self.inner.initialized.load(Ordering::SeqCst) {
            return Err(ClientError::NotInitialized {
                method: "initialized".to_string(),
            });
        }
        self.send_notification("initialized", EmptyObject::default(), false)
            .await?;
        self.inner.ready.store(true, Ordering::SeqCst);
        Ok(())
    }

    pub async fn send_raw_request(
        &self,
        method: impl Into<String>,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, ClientError> {
        let method = method.into();
        let requires_ready = method != "initialize";
        self.request_value_internal(&method, params, timeout, requires_ready)
            .await
    }

    pub async fn send_raw_notification(
        &self,
        method: impl Into<String>,
        params: Value,
    ) -> Result<(), ClientError> {
        let method = method.into();
        let requires_ready = method != "initialized";
        self.send_notification(&method, params, requires_ready)
            .await
    }

    pub async fn respond_server_request<R: Serialize>(
        &self,
        id: RequestId,
        result: R,
    ) -> Result<(), ClientError> {
        let result = serde_json::to_value(result)?;
        self.send_message(json!({ "id": id, "result": result }))
            .await
    }

    pub async fn respond_server_request_error(
        &self,
        id: RequestId,
        error: RpcError,
    ) -> Result<(), ClientError> {
        self.send_message(json!({ "id": id, "error": error })).await
    }

    pub async fn respond_chatgpt_auth_tokens_refresh(
        &self,
        id: RequestId,
        response: server_requests::ChatgptAuthTokensRefreshResponse,
    ) -> Result<(), ClientError> {
        self.respond_server_request(id, response).await
    }

    typed_method!(
        thread_start,
        "thread/start",
        requests::ThreadStartParams,
        responses::ThreadResult
    );
    typed_method!(
        thread_resume,
        "thread/resume",
        requests::ThreadResumeParams,
        responses::ThreadResult
    );
    typed_method!(
        thread_fork,
        "thread/fork",
        requests::ThreadForkParams,
        responses::ThreadResult
    );
    typed_method!(
        thread_archive,
        "thread/archive",
        requests::ThreadArchiveParams,
        responses::ThreadArchiveResult
    );
    typed_method!(
        thread_name_set,
        "thread/name/set",
        requests::ThreadSetNameParams,
        responses::ThreadSetNameResult
    );
    typed_method!(
        thread_unarchive,
        "thread/unarchive",
        requests::ThreadUnarchiveParams,
        responses::ThreadUnarchiveResult
    );
    typed_method!(
        thread_compact_start,
        "thread/compact/start",
        requests::ThreadCompactStartParams,
        responses::ThreadCompactStartResult
    );
    typed_method!(
        thread_rollback,
        "thread/rollback",
        requests::ThreadRollbackParams,
        responses::ThreadRollbackResult
    );
    typed_method!(
        thread_list,
        "thread/list",
        requests::ThreadListParams,
        responses::ThreadListResult
    );
    typed_method!(
        thread_loaded_list,
        "thread/loaded/list",
        requests::ThreadLoadedListParams,
        responses::ThreadLoadedListResult
    );
    typed_method!(
        thread_read,
        "thread/read",
        requests::ThreadReadParams,
        responses::ThreadReadResult
    );
    typed_method!(
        skills_list,
        "skills/list",
        requests::SkillsListParams,
        responses::SkillsListResult
    );
    typed_method!(
        skills_remote_read,
        "skills/remote/read",
        requests::SkillsRemoteReadParams,
        responses::SkillsRemoteReadResult
    );
    typed_method!(
        skills_remote_write,
        "skills/remote/write",
        requests::SkillsRemoteWriteParams,
        responses::SkillsRemoteWriteResult
    );
    typed_method!(
        app_list,
        "app/list",
        requests::AppsListParams,
        responses::AppsListResult
    );
    typed_method!(
        skills_config_write,
        "skills/config/write",
        requests::SkillsConfigWriteParams,
        responses::SkillsConfigWriteResult
    );
    typed_method!(
        turn_start,
        "turn/start",
        requests::TurnStartParams,
        responses::TurnResult
    );
    typed_method!(
        turn_steer,
        "turn/steer",
        requests::TurnSteerParams,
        responses::TurnSteerResult
    );
    typed_method!(
        turn_interrupt,
        "turn/interrupt",
        requests::TurnInterruptParams,
        EmptyObject
    );
    typed_method!(
        review_start,
        "review/start",
        requests::ReviewStartParams,
        responses::ReviewStartResult
    );
    typed_method!(
        model_list,
        "model/list",
        requests::ModelListParams,
        responses::ModelListResult
    );
    typed_method!(
        experimental_feature_list,
        "experimentalFeature/list",
        requests::ExperimentalFeatureListParams,
        responses::ExperimentalFeatureListResult
    );
    typed_method!(
        mcp_server_oauth_login,
        "mcpServer/oauth/login",
        requests::McpServerOauthLoginParams,
        responses::McpServerOauthLoginResult
    );
    typed_method!(
        mcp_server_status_list,
        "mcpServerStatus/list",
        requests::ListMcpServerStatusParams,
        responses::McpServerStatusListResult
    );
    typed_method!(
        account_login_start,
        "account/login/start",
        requests::LoginAccountParams,
        responses::LoginAccountResult
    );
    typed_method!(
        account_login_cancel,
        "account/login/cancel",
        requests::CancelLoginAccountParams,
        EmptyObject
    );
    typed_method!(
        feedback_upload,
        "feedback/upload",
        requests::FeedbackUploadParams,
        responses::FeedbackUploadResult
    );
    typed_method!(
        command_exec,
        "command/exec",
        requests::CommandExecParams,
        responses::CommandExecResult
    );
    typed_method!(
        config_read,
        "config/read",
        requests::ConfigReadParams,
        responses::ConfigReadResult
    );
    typed_method!(
        config_value_write,
        "config/value/write",
        requests::ConfigValueWriteParams,
        responses::ConfigValueWriteResult
    );
    typed_method!(
        config_batch_write,
        "config/batchWrite",
        requests::ConfigBatchWriteParams,
        responses::ConfigBatchWriteResult
    );
    typed_method!(
        account_read,
        "account/read",
        requests::GetAccountParams,
        responses::GetAccountResult
    );

    typed_null_method!(
        config_mcp_server_reload,
        "config/mcpServer/reload",
        EmptyObject
    );
    typed_null_method!(account_logout, "account/logout", EmptyObject);
    typed_null_method!(
        account_rate_limits_read,
        "account/rateLimits/read",
        responses::AccountRateLimitsReadResult
    );
    typed_null_method!(
        config_requirements_read,
        "configRequirements/read",
        responses::ConfigRequirementsReadResult
    );

    async fn send_notification<P: Serialize>(
        &self,
        method: &str,
        params: P,
        requires_ready: bool,
    ) -> Result<(), ClientError> {
        if requires_ready && !self.inner.ready.load(Ordering::SeqCst) {
            return Err(ClientError::NotReady {
                method: method.to_string(),
            });
        }

        let value = serde_json::to_value(params)?;
        self.send_message(json!({ "method": method, "params": value }))
            .await
    }

    async fn request_typed_internal<P, R>(
        &self,
        method: &str,
        params: P,
        timeout: Option<Duration>,
        requires_ready: bool,
    ) -> Result<R, ClientError>
    where
        P: Serialize,
        R: serde::de::DeserializeOwned,
    {
        let value = serde_json::to_value(params)?;
        self.request_typed_value_internal(method, value, timeout, requires_ready)
            .await
    }

    async fn request_typed_value_internal<R>(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        requires_ready: bool,
    ) -> Result<R, ClientError>
    where
        R: serde::de::DeserializeOwned,
    {
        let raw = self
            .request_value_internal(method, params, timeout, requires_ready)
            .await?;

        serde_json::from_value(raw).map_err(|source| ClientError::UnexpectedResult {
            method: method.to_string(),
            source,
        })
    }

    async fn request_value_internal(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
        requires_ready: bool,
    ) -> Result<Value, ClientError> {
        if requires_ready && !self.inner.ready.load(Ordering::SeqCst) {
            return Err(ClientError::NotReady {
                method: method.to_string(),
            });
        }

        if method == "initialize" && self.inner.initialized.load(Ordering::SeqCst) {
            return Err(ClientError::AlreadyInitialized);
        }

        let id_num = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let id = RequestId::Integer(id_num);

        let request = json!({
            "method": method,
            "id": id,
            "params": params,
        });

        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().await.insert(id.clone(), tx);

        if let Err(err) = self.send_message(request).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(err);
        }

        let timeout = timeout.unwrap_or(self.inner.default_timeout);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(ClientError::Rpc { error }),
            Ok(Err(_)) => Err(ClientError::TransportClosed),
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                Err(ClientError::Timeout {
                    method: method.to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                })
            }
        }
    }

    async fn send_message(&self, value: Value) -> Result<(), ClientError> {
        self.inner.outbound.send(value).await.map_err(|err| {
            ClientError::TransportSend(format!("failed to send outbound frame: {err}"))
        })
    }

    fn publish_event(&self, event: ServerEvent) {
        let _ = self.inner.event_tx.send(event);
    }
}

async fn run_inbound_loop(
    mut inbound: mpsc::Receiver<Result<Value, ClientError>>,
    inner: Arc<Inner>,
) {
    while let Some(frame) = inbound.recv().await {
        match frame {
            Ok(value) => {
                if let Err(err) = process_incoming_value(value, &inner).await {
                    fail_all_pending(&inner, &format!("processing inbound frame failed: {err}"))
                        .await;
                    let _ = inner.event_tx.send(ServerEvent::TransportClosed);
                    break;
                }
            }
            Err(err) => {
                fail_all_pending(&inner, &format!("transport error: {err}")).await;
                let _ = inner.event_tx.send(ServerEvent::TransportClosed);
                break;
            }
        }
    }
}

async fn process_incoming_value(value: Value, inner: &Arc<Inner>) -> Result<(), ClientError> {
    match classify_incoming(value)? {
        IncomingClassified::Response { id, result } => {
            if let Some(sender) = inner.pending.lock().await.remove(&id) {
                let _ = sender.send(result);
            }
        }
        IncomingClassified::Notification {
            method,
            params,
            raw: _,
        } => {
            let parsed = parse_notification(method.clone(), params.clone())
                .unwrap_or(ServerNotification::Unknown { method, params });
            let _ = inner.event_tx.send(ServerEvent::Notification(parsed));
        }
        IncomingClassified::ServerRequest {
            id,
            method,
            params,
            raw: _,
        } => {
            let parsed = parse_server_request(id.clone(), method.clone(), params.clone())
                .unwrap_or(ServerRequestEvent::Unknown { id, method, params });
            if !try_auto_handle_server_request(inner, &parsed).await {
                let _ = inner.event_tx.send(ServerEvent::ServerRequest(parsed));
            }
        }
    }
    Ok(())
}

async fn try_auto_handle_server_request(inner: &Arc<Inner>, request: &ServerRequestEvent) -> bool {
    match request {
        ServerRequestEvent::ChatgptAuthTokensRefresh { id, params } => {
            let handler = inner.refresh_handler.read().await.clone();
            let Some(handler) = handler else {
                return false;
            };

            let response = handler(params.clone()).await;
            let payload = match response {
                Ok(tokens) => json!({ "id": id, "result": tokens }),
                Err(err) => json!({
                    "id": id,
                    "error": {
                        "code": -32001,
                        "message": format!("chatgptAuthTokens refresh handler failed: {err}")
                    }
                }),
            };

            if inner.outbound.send(payload).await.is_err() {
                let _ = inner.event_tx.send(ServerEvent::TransportClosed);
            }

            true
        }
        _ => false,
    }
}

async fn fail_all_pending(inner: &Arc<Inner>, message: &str) {
    let mut pending = inner.pending.lock().await;
    let entries = std::mem::take(&mut *pending);
    drop(pending);

    for (_, sender) in entries {
        let _ = sender.send(Err(RpcError {
            code: -32098,
            message: message.to_string(),
            data: None,
        }));
    }
}

async fn detect_cli_version(binary: &str) -> Option<String> {
    let output = Command::new(binary).arg("--version").output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}
