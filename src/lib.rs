pub mod client;
pub mod compat;
pub mod error;
pub mod events;
pub mod protocol;
pub mod transport;

#[cfg(feature = "ws")]
pub use client::WsConfig;
pub use client::{ClientOptions, CodexClient, StdioConfig};
pub use compat::{CompatibilityPolicy, TESTED_CLI_VERSION_REQ};
pub use error::{ClientError, RpcError};
pub use events::{ServerEvent, ServerNotification, ServerRequestEvent};
pub use protocol::{notifications, requests, responses, server_requests, shared};
