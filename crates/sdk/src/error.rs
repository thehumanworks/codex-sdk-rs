use crate::protocol::shared::RequestId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

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
    #[error("compatibility check failed: {0}")]
    Compatibility(String),
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
