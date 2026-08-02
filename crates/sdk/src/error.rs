use std::path::PathBuf;

use crate::protocol::shared::RequestId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// JSON-RPC error code sent back to the server when a registered
/// server-request handler returns an error.
pub const RPC_ERROR_CODE_HANDLER_FAILED: i64 = -32001;

/// JSON-RPC error code injected into all pending requests when the transport
/// fails or an inbound frame cannot be processed.
pub const RPC_ERROR_CODE_TRANSPORT_FAILURE: i64 = -32098;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("not initialized: call initialize() before invoking {method}")]
    NotInitialized { method: String },
    #[error("connection not ready: call initialized() before invoking {method}")]
    NotReady { method: String },
    #[error("connection is already initialized")]
    AlreadyInitialized,
    #[error("request timed out after {timeout_ms}ms for method {method}")]
    Timeout { method: String, timeout_ms: u64 },
    /// Invalid URL or otherwise invalid client/transport configuration.
    #[error("invalid configuration: {0}")]
    Config(String),
    /// Failure while starting, reusing, or shutting down a managed
    /// `codex app-server` process (spawn failure, readiness timeout, reuse
    /// conflict, port not released). `log_path` points at the daemon log when
    /// one exists, so startup failures can be debugged.
    #[error("app-server startup failed: {message}{}", startup_log_suffix(.log_path))]
    Startup {
        message: String,
        log_path: Option<PathBuf>,
    },
    #[error("transport send failed: {0}")]
    TransportSend(String),
    #[error("transport closed")]
    TransportClosed,
    #[error("invalid message from server: {0}")]
    InvalidMessage(String),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("rpc error {error:?}")]
    Rpc { error: RpcError },
    #[error("unexpected result shape for method {method}: {source}")]
    UnexpectedResult {
        method: String,
        source: serde_json::Error,
    },
}

fn startup_log_suffix(log_path: &Option<PathBuf>) -> String {
    match log_path {
        Some(path) => format!("; logs: {}", path.display()),
        None => String::new(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug)]
pub enum IncomingClassified {
    Response {
        id: RequestId,
        result: Result<Value, RpcError>,
    },
    Notification {
        method: String,
        params: Value,
        raw: Value,
    },
    ServerRequest {
        id: RequestId,
        method: String,
        params: Value,
        raw: Value,
    },
}

pub fn classify_incoming(value: Value) -> Result<IncomingClassified, ClientError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ClientError::InvalidMessage("expected JSON object".to_string()))?;

    let id = obj
        .get("id")
        .map(|v| serde_json::from_value::<RequestId>(v.clone()))
        .transpose()
        .map_err(ClientError::Serialization)?;

    let method = obj
        .get("method")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    match (id, method) {
        (Some(id), Some(method)) => {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            Ok(IncomingClassified::ServerRequest {
                id,
                method,
                params,
                raw: value,
            })
        }
        (None, Some(method)) => {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            Ok(IncomingClassified::Notification {
                method,
                params,
                raw: value,
            })
        }
        (Some(id), None) => {
            if let Some(result) = obj.get("result") {
                return Ok(IncomingClassified::Response {
                    id,
                    result: Ok(result.clone()),
                });
            }
            if let Some(error) = obj.get("error") {
                let parsed = serde_json::from_value::<RpcError>(error.clone())?;
                return Ok(IncomingClassified::Response {
                    id,
                    result: Err(parsed),
                });
            }
            Err(ClientError::InvalidMessage(
                "response missing both result and error".to_string(),
            ))
        }
        (None, None) => Err(ClientError::InvalidMessage(
            "message missing method and id".to_string(),
        )),
    }
}
