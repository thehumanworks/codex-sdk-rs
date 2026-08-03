use std::fs;
use std::path::Path;

use codex_app_server_sdk::websocket_url_allows_auth_token;

use crate::error::LunaError;

pub(crate) const DEFAULT_WS_URL: &str = "ws://127.0.0.1:4222";
pub(crate) const CODEX_APP_SERVER_WS_URL_ENV: &str = "CODEX_APP_SERVER_WS_URL";
pub(crate) const CODEX_WEB_SERVER_URL_ENV: &str = "CODEX_WEB_SERVER_URL";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebsocketUrlSource {
    Flag,
    Environment,
    LegacyEnvironment,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWebsocketUrl {
    pub(crate) url: String,
    pub(crate) source: WebsocketUrlSource,
}

impl ResolvedWebsocketUrl {
    pub(crate) fn manage_daemon(&self) -> bool {
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

pub(crate) fn resolve_websocket_url(
    explicit: Option<&str>,
    env_websocket_url: Option<&str>,
    legacy_env_websocket_url: Option<&str>,
) -> Result<ResolvedWebsocketUrl, LunaError> {
    if let Some(url) = explicit {
        return resolved(url, "--ws-url", WebsocketUrlSource::Flag);
    }
    if let Some(url) = env_websocket_url {
        return resolved(
            url,
            CODEX_APP_SERVER_WS_URL_ENV,
            WebsocketUrlSource::Environment,
        );
    }
    if let Some(url) = legacy_env_websocket_url {
        return resolved(
            url,
            CODEX_WEB_SERVER_URL_ENV,
            WebsocketUrlSource::LegacyEnvironment,
        );
    }
    Ok(ResolvedWebsocketUrl::default())
}

fn resolved(
    raw: &str,
    source_name: &str,
    source: WebsocketUrlSource,
) -> Result<ResolvedWebsocketUrl, LunaError> {
    Ok(ResolvedWebsocketUrl {
        url: normalize_websocket_url(raw, source_name)?,
        source,
    })
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

/// Resolve a websocket bearer credential from `--ws-auth-token` or
/// `--ws-auth-token-file`. The flags are mutually exclusive at the Clap layer.
pub(crate) fn resolve_ws_auth_token(
    token: Option<&str>,
    token_file: Option<&str>,
) -> Result<Option<String>, LunaError> {
    match (token, token_file) {
        (Some(_), Some(_)) => Err(LunaError::Usage(
            "--ws-auth-token and --ws-auth-token-file cannot be combined".to_string(),
        )),
        (Some(token), None) => {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                return Err(LunaError::Usage(
                    "value for --ws-auth-token cannot be empty".to_string(),
                ));
            }
            Ok(Some(trimmed.to_string()))
        }
        (None, Some(path)) => {
            let contents = fs::read_to_string(Path::new(path)).map_err(|err| {
                LunaError::Usage(format!(
                    "failed to read --ws-auth-token-file `{path}`: {err}"
                ))
            })?;
            let trimmed = contents.trim();
            if trimmed.is_empty() {
                return Err(LunaError::Usage(
                    "value for --ws-auth-token-file cannot be empty".to_string(),
                ));
            }
            Ok(Some(trimmed.to_string()))
        }
        (None, None) => Ok(None),
    }
}

/// Bearer auth is only allowed for `wss://` or loopback `ws://` endpoints.
pub(crate) fn ensure_ws_auth_allowed(url: &str) -> Result<(), LunaError> {
    if websocket_url_allows_auth_token(url) {
        Ok(())
    } else {
        Err(LunaError::Usage(
            "websocket auth tokens require `wss://` or a loopback `ws://` URL".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_url_wins_over_both_environment_variables() {
        let resolved = resolve_websocket_url(
            Some("ws://127.0.0.1:5555"),
            Some("ws://127.0.0.1:4444"),
            Some("ws://127.0.0.1:3333"),
        )
        .expect("explicit URL");
        assert_eq!(resolved.url, "ws://127.0.0.1:5555");
        assert_eq!(resolved.source, WebsocketUrlSource::Flag);
        assert!(!resolved.manage_daemon());
    }

    #[test]
    fn primary_environment_wins_over_legacy_environment() {
        let resolved = resolve_websocket_url(
            None,
            Some("ws://127.0.0.1:4444"),
            Some("ws://127.0.0.1:3333"),
        )
        .expect("primary environment URL");
        assert_eq!(resolved.source, WebsocketUrlSource::Environment);
    }

    #[test]
    fn legacy_environment_remains_a_fallback() {
        let resolved =
            resolve_websocket_url(None, None, Some("ws://127.0.0.1:3333")).expect("legacy URL");
        assert_eq!(resolved.source, WebsocketUrlSource::LegacyEnvironment);
    }

    #[test]
    fn absent_sources_use_managed_default() {
        let resolved = resolve_websocket_url(None, None, None).expect("default URL");
        assert_eq!(resolved.url, DEFAULT_WS_URL);
        assert!(resolved.manage_daemon());
    }

    #[test]
    fn values_are_trimmed_and_selected_empty_source_is_an_error() {
        let resolved =
            resolve_websocket_url(Some("  ws://localhost:4222  "), None, None).expect("trimmed");
        assert_eq!(resolved.url, "ws://localhost:4222");

        let error = resolve_websocket_url(Some("  "), Some("ws://ignored"), None)
            .expect_err("empty explicit source must not fall through");
        assert!(matches!(error, LunaError::Usage(_)));
        assert!(error.to_string().contains("--ws-url"));
    }

    #[test]
    fn ws_auth_is_allowed_for_wss_and_loopback_ws_only() {
        assert!(websocket_url_allows_auth_token("wss://example.com/app"));
        assert!(websocket_url_allows_auth_token("ws://127.0.0.1:4222"));
        assert!(websocket_url_allows_auth_token("ws://localhost:4222"));
        assert!(websocket_url_allows_auth_token("ws://[::1]:4222"));
        assert!(!websocket_url_allows_auth_token("ws://192.168.1.10:4222"));
        assert!(
            ensure_ws_auth_allowed("ws://example.com:4222")
                .expect_err("remote ws auth")
                .to_string()
                .contains("loopback")
        );
    }

    #[test]
    fn resolve_ws_auth_token_reads_file_and_trims() {
        let path = std::env::temp_dir().join("luna-ws-auth-token-test");
        fs::write(&path, "  secret-token\n").expect("write token file");
        let token = resolve_ws_auth_token(None, Some(path.to_str().expect("utf8 path")))
            .expect("resolve file token")
            .expect("token present");
        assert_eq!(token, "secret-token");
        let _ = fs::remove_file(path);
    }
}
