pub mod stdio;
pub mod ws;
pub mod ws_daemon;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::error::ClientError;

pub struct TransportHandle {
    pub outbound: mpsc::Sender<Value>,
    pub inbound: mpsc::Receiver<Result<Value, ClientError>>,
}
