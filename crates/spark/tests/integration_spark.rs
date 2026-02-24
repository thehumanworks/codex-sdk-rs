use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codex_app_server_sdk::StdioConfig;
use codex_app_server_sdk::api::{Codex, ModelReasoningEffort, ThreadOptions, TurnOptions};

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
    let mut config = StdioConfig::default();
    config.env = env.clone();
    config
}

fn spark_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_spark")
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
                .join(format!("spark{}", std::env::consts::EXE_SUFFIX))
        })
}

async fn seed_session(
    config: StdioConfig,
    token: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let codex = Codex::spawn_stdio(config).await?;
    let mut thread = codex.start_thread(
        ThreadOptions::builder()
            .model("gpt-5.3-codex-spark")
            .model_reasoning_effort(ModelReasoningEffort::XHigh)
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

async fn run_spark_with_env(
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let mut cmd = tokio::process::Command::new(spark_binary());
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

fn assert_spark_resume_output(
    output: std::process::Output,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        output.status.success(),
        "spark command should succeed.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(token),
        "spark output should contain sentinel token.\nexpected token: {token}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    Ok(())
}

#[tokio::test]
async fn spark_resume_flag_accepts_session_id() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = INTEGRATION_LOCK.lock().await;
    let env = shared_env();
    let token = unique_token("spark-resume");
    let thread_id = seed_session(stdio_config_with_env(&env), &token).await?;

    let args = vec![
        "--final-response".to_string(),
        "--stdio".to_string(),
        "--resume".to_string(),
        thread_id,
        "Return only the sentinel token from earlier in this same session.".to_string(),
    ];
    let output = run_spark_with_env(&args, &env).await?;
    assert_spark_resume_output(output, &token)
}

#[tokio::test]
async fn spark_continue_flag_resumes_most_recent_session() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = INTEGRATION_LOCK.lock().await;
    let env = shared_env();
    let token = unique_token("spark-continue");
    let _thread_id = seed_session(stdio_config_with_env(&env), &token).await?;

    let args = vec![
        "--final-response".to_string(),
        "--stdio".to_string(),
        "--continue".to_string(),
        "Return only the sentinel token from earlier in this same session.".to_string(),
    ];
    let output = run_spark_with_env(&args, &env).await?;
    assert_spark_resume_output(output, &token)
}
