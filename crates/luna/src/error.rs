use std::io;
use std::path::PathBuf;

use codex_app_server_sdk::{ClientError, ThreadRunError};
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum LunaError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Dependency(String),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Authentication(String),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Turn(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Client(ClientError),
    #[error(transparent)]
    ThreadRun(ThreadRunError),
    #[error("failed to parse TOML in {}: {source}", .path.display())]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl LunaError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Usage(_) => "luna.usage",
            Self::Dependency(_) => "luna.dependency",
            Self::Config(_) | Self::Toml { .. } => "luna.configuration",
            Self::Authentication(_) => "luna.authentication",
            Self::Transport(_) => "luna.transport",
            Self::Protocol(_) => "luna.protocol",
            Self::Turn(_) => "luna.turn",
            Self::Io(_) => "luna.io",
            Self::Client(error) => match error {
                ClientError::Timeout { .. }
                | ClientError::TransportSend(_)
                | ClientError::TransportClosed
                | ClientError::Io(_) => "luna.transport",
                ClientError::InvalidMessage(_)
                | ClientError::Serialization(_)
                | ClientError::Rpc { .. }
                | ClientError::UnexpectedResult { .. }
                | ClientError::NotInitialized { .. }
                | ClientError::NotReady { .. }
                | ClientError::AlreadyInitialized => "luna.protocol",
            },
            Self::ThreadRun(_) => "luna.turn",
        }
    }

    pub(crate) fn exit_code(&self) -> u8 {
        match self.code() {
            "luna.usage" => 2,
            "luna.dependency" => 3,
            "luna.configuration" => 4,
            "luna.authentication" => 5,
            "luna.transport" => 6,
            "luna.protocol" => 7,
            "luna.turn" => 8,
            _ => 1,
        }
    }
}

impl From<ClientError> for LunaError {
    fn from(value: ClientError) -> Self {
        Self::Client(value)
    }
}

impl From<ThreadRunError> for LunaError {
    fn from(value: ThreadRunError) -> Self {
        Self::ThreadRun(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_error_categories_have_stable_exit_codes() {
        let cases = [
            (LunaError::Usage("usage".to_string()), "luna.usage", 2),
            (
                LunaError::Dependency("dependency".to_string()),
                "luna.dependency",
                3,
            ),
            (
                LunaError::Config("config".to_string()),
                "luna.configuration",
                4,
            ),
            (
                LunaError::Authentication("auth".to_string()),
                "luna.authentication",
                5,
            ),
            (
                LunaError::Transport("transport".to_string()),
                "luna.transport",
                6,
            ),
            (
                LunaError::Protocol("protocol".to_string()),
                "luna.protocol",
                7,
            ),
            (LunaError::Turn("turn".to_string()), "luna.turn", 8),
        ];
        for (error, code, exit) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.exit_code(), exit);
        }
    }

    #[test]
    fn client_errors_distinguish_transport_from_protocol() {
        let transport = LunaError::from(ClientError::TransportClosed);
        assert_eq!(transport.code(), "luna.transport");
        assert_eq!(transport.exit_code(), 6);

        let protocol = LunaError::from(ClientError::InvalidMessage("bad".to_string()));
        assert_eq!(protocol.code(), "luna.protocol");
        assert_eq!(protocol.exit_code(), 7);
    }
}
