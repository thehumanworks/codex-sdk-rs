use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
}

impl From<i64> for RequestId {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest<P> {
    pub method: String,
    pub id: RequestId,
    pub params: P,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification<P> {
    pub method: String,
    pub params: P,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse<R> {
    pub id: RequestId,
    pub result: R,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmptyObject {
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}
