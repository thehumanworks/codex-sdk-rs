use std::env;

use codex_app_server_sdk::{ClientOptions, Codex, CodexClient, StdioConfig, WsConfig, requests};
use serde_json::{Map, Value};

use crate::environment::resolve_codex_binary;
use crate::error::LunaError;

/// Environment variables forwarded to a daemon the SDK may spawn, so a
/// freshly started `codex app-server` inherits the caller's credentials.
const FORWARDED_ENV_VARS: [&str; 4] = [
    "OPENAI_API_KEY",
    "CODEX_API_KEY",
    "CODEX_ID_TOKEN",
    "CODEX_ACCESS_TOKEN",
];

pub(crate) fn daemon_env() -> std::collections::HashMap<String, String> {
    FORWARDED_ENV_VARS
        .iter()
        .filter_map(|name| env::var(name).ok().map(|value| (name.to_string(), value)))
        .collect()
}

pub(crate) async fn connect_ws_codex(url: &str, manage_daemon: bool) -> Result<Codex, LunaError> {
    let config = WsConfig {
        url: url.to_string(),
        options: ClientOptions::default(),
    };
    let client = if manage_daemon {
        CodexClient::start_and_connect_ws(config, daemon_env()).await?
    } else {
        CodexClient::connect_ws(config).await?
    };
    Ok(client.as_api())
}

pub(crate) async fn start_ws_server(url: &str) -> Result<(), LunaError> {
    let config = WsConfig {
        url: url.to_string(),
        options: ClientOptions::default(),
    };
    let _client = CodexClient::start_and_connect_ws(config, daemon_env()).await?;
    Ok(())
}

pub(crate) async fn spawn_stdio_codex() -> Result<Codex, LunaError> {
    let codex_binary = resolve_codex_binary()?;
    let stdio_config = StdioConfig {
        codex_binary: codex_binary.to_string_lossy().into_owned(),
        ..Default::default()
    };
    Ok(Codex::spawn_stdio(stdio_config).await?)
}

/// Ensure the codex session is authenticated using a cascading strategy:
/// 1. Rely on existing cached auth (auth.json) — check via `account_read`.
/// 2. If not authenticated, try `CODEX_ID_TOKEN` + `CODEX_ACCESS_TOKEN` env vars (ChatGPT login).
/// 3. If those aren't available, try `OPENAI_API_KEY` env var (API key login).
pub(crate) async fn ensure_authenticated(codex: &Codex) -> Result<(), LunaError> {
    // Check if the app-server already has valid cached auth.
    let account = codex
        .account_read(requests::GetAccountParams::default())
        .await?;
    if account_is_authenticated(&account.extra) {
        return Ok(());
    }

    // Fallback 1: ChatGPT Pro bearer tokens from env.
    if let (Ok(id_token), Ok(access_token)) =
        (env::var("CODEX_ID_TOKEN"), env::var("CODEX_ACCESS_TOKEN"))
    {
        codex
            .account_login_start(requests::LoginAccountParams::chatgpt(
                id_token,
                access_token,
            ))
            .await?;
        return Ok(());
    }

    // Fallback 2: OpenAI API key from env.
    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        codex
            .account_login_start(requests::LoginAccountParams::api_key(api_key))
            .await?;
        return Ok(());
    }

    Err(LunaError::Authentication(
        "Codex is not authenticated; run `codex login`, then rerun `luna doctor --live`"
            .to_string(),
    ))
}

pub(crate) fn account_is_authenticated(account: &Map<String, Value>) -> bool {
    account.get("isLoggedIn") == Some(&Value::Bool(true))
        || account.get("account").is_some_and(|value| !value.is_null())
        || account.get("requiresOpenaiAuth") == Some(&Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_detection_accepts_current_and_legacy_shapes() {
        assert!(account_is_authenticated(&Map::from_iter([(
            "account".to_string(),
            serde_json::json!({"type": "chatgpt"}),
        )])));
        assert!(account_is_authenticated(&Map::from_iter([(
            "isLoggedIn".to_string(),
            Value::Bool(true),
        )])));
        assert!(account_is_authenticated(&Map::from_iter([(
            "requiresOpenaiAuth".to_string(),
            Value::Bool(false),
        )])));
        assert!(!account_is_authenticated(&Map::from_iter([
            ("account".to_string(), Value::Null),
            ("requiresOpenaiAuth".to_string(), Value::Bool(true)),
        ])));
    }
}
