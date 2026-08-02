use std::collections::HashMap;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codex_app_server_sdk::api::{Codex, ModelReasoningEffort, ThreadOptions, TurnOptions};
use codex_app_server_sdk::requests::{ClientInfo, InitializeParams};
use codex_app_server_sdk::{ClientOptions, CodexClient, StdioConfig, WsConfig};

const TEST_TIMEOUT: Duration = Duration::from_secs(120);
static INTEGRATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn shared_env() -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        env.insert("OPENAI_API_KEY".to_string(), api_key);
    }
    env
}

fn stdio_config_with_env(env: &HashMap<String, String>) -> StdioConfig {
    StdioConfig {
        env: env.clone(),
        ..Default::default()
    }
}

fn luna_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_luna")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let workspace_root = manifest_dir
                .parent()
                .and_then(|dir| dir.parent())
                .unwrap_or(manifest_dir.as_path());
            workspace_root
                .join("target")
                .join("debug")
                .join(format!("luna{}", std::env::consts::EXE_SUFFIX))
        })
}

fn reserve_local_ws_url() -> Result<String, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(format!("ws://127.0.0.1:{port}"))
}

async fn seed_session(
    config: StdioConfig,
    token: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let working_directory = std::env::current_dir()?.to_string_lossy().to_string();
    let codex = Codex::spawn_stdio(config).await?;
    let mut thread = codex.start_thread(
        ThreadOptions::builder()
            .model("gpt-5.6-luna")
            .working_directory(working_directory)
            .model_reasoning_effort(ModelReasoningEffort::Max)
            .build(),
    );
    let prompt = format!(
        "Remember this exact sentinel token for future turns in this session: {token}. Reply with exactly STORED."
    );
    let response = thread.ask(prompt, TurnOptions::default()).await?;
    assert!(
        !response.trim().is_empty(),
        "seed response should not be empty"
    );
    Ok(thread
        .id()
        .ok_or("thread id should be populated after seeding")?
        .to_string())
}

async fn run_luna_with_env(
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new(luna_binary());
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = tokio::time::timeout(TEST_TIMEOUT, cmd.output()).await??;
    Ok(output)
}

fn unique_token(label: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    format!("{label}-{}-{stamp}", std::process::id())
}

fn assert_luna_resume_output(
    output: std::process::Output,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "luna command should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(token),
        "luna output should contain sentinel token.\nexpected token: {token}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    Ok(())
}

#[tokio::test]
async fn luna_doctor_json_reports_missing_codex_without_leaking_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new(luna_binary());
    cmd.args(["doctor", "--json"])
        .env("CODEX_BINARY", "/definitely/missing/codex");
    let output = tokio::time::timeout(TEST_TIMEOUT, cmd.output()).await??;
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout)?;
    let report: serde_json::Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["overall_status"], "fail");
    assert!(report["checks"].as_array().is_some_and(|checks| {
        checks
            .iter()
            .any(|check| check["code"] == "codex.binary.missing")
    }));
    assert!(!stdout.contains("/Users/"));
    assert!(!stdout.contains("/definitely/missing/codex"));
    Ok(())
}

#[tokio::test]
async fn luna_doctor_redacts_env_url_and_marks_it_connect_only()
-> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new(luna_binary());
    cmd.args(["doctor", "--json"]).env(
        "CODEX_APP_SERVER_WS_URL",
        "wss://user:secret@example.com/app?token=secret",
    );
    let output = tokio::time::timeout(TEST_TIMEOUT, cmd.output()).await??;
    let stdout = String::from_utf8(output.stdout)?;
    let report: serde_json::Value = serde_json::from_str(&stdout)?;
    assert_eq!(report["resolution"]["daemon_policy"], "connect_only");
    assert_eq!(
        report["resolution"]["websocket_endpoint"],
        "wss://example.com/app"
    );
    assert!(!stdout.contains("user:secret"));
    assert!(!stdout.contains("token=secret"));
    Ok(())
}

#[tokio::test]
async fn luna_dependency_failure_has_stable_code_and_exit_status()
-> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new(luna_binary());
    cmd.args(["exec", "--stdio", "Reply with ok"])
        .env("CODEX_BINARY", "/definitely/missing/codex");
    let output = tokio::time::timeout(TEST_TIMEOUT, cmd.output()).await??;
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("[luna.dependency]"));
    assert!(stderr.contains("luna doctor --summary"));
    assert!(!stderr.contains("/definitely/missing/codex"));
    Ok(())
}

#[tokio::test]
async fn luna_resume_flag_accepts_session_id() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = INTEGRATION_LOCK.lock().await;
    let env = shared_env();
    let token = unique_token("luna-resume");
    let thread_id = seed_session(stdio_config_with_env(&env), &token).await?;

    let args = vec![
        "exec".to_string(),
        "--final-response".to_string(),
        "--stdio".to_string(),
        "--resume".to_string(),
        thread_id,
        "Return only the sentinel token from earlier in this same session.".to_string(),
    ];
    let output = run_luna_with_env(&args, &env).await?;
    assert_luna_resume_output(output, &token)
}

#[tokio::test]
async fn luna_continue_flag_resumes_most_recent_session() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = INTEGRATION_LOCK.lock().await;
    let env = shared_env();
    let token = unique_token("luna-continue");
    let _thread_id = seed_session(stdio_config_with_env(&env), &token).await?;

    let args = vec![
        "exec".to_string(),
        "--final-response".to_string(),
        "--stdio".to_string(),
        "--continue".to_string(),
        "Return only the sentinel token from earlier in this same session.".to_string(),
    ];
    let output = run_luna_with_env(&args, &env).await?;
    assert_luna_resume_output(output, &token)
}

#[tokio::test]
async fn luna_start_command_readies_websocket_daemon() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = INTEGRATION_LOCK.lock().await;
    let env = shared_env();
    let url = reserve_local_ws_url()?;

    let args = vec!["start".to_string(), "--ws-url".to_string(), url.clone()];
    let output = run_luna_with_env(&args, &env).await?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "luna start should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&url),
        "luna start should report the websocket URL.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let client = CodexClient::connect_ws(WsConfig {
        url: url.clone(),
        env: shared_env(),
        options: ClientOptions::default(),
    })
    .await?;
    client
        .initialize(InitializeParams::new(ClientInfo::new(
            "luna_start_test",
            "Luna Start Test",
            env!("CARGO_PKG_VERSION"),
        )))
        .await?;
    client.initialized().await?;

    Ok(())
}
