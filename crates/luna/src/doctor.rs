use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use codex_app_server_sdk::requests;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::environment::{display_path_redacted, resolve_codex_binary};
use crate::error::LunaError;
use crate::{
    TransportMode, account_is_authenticated, connect_ws_codex, resolve_codex_home_dir,
    spawn_stdio_codex,
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_COMMAND_OUTPUT_BYTES: usize = 1024 * 1024;
const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub(crate) struct DoctorOptions {
    pub(crate) json: bool,
    pub(crate) live: bool,
    pub(crate) transport: TransportMode,
    pub(crate) websocket_url: String,
    pub(crate) manage_daemon: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct DoctorReportV1 {
    schema_version: u32,
    generated_at_unix_seconds: u64,
    overall_status: CheckStatus,
    build: BuildInfo,
    resolution: ResolutionInfo,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug, Serialize)]
struct BuildInfo {
    name: &'static str,
    version: &'static str,
    target: String,
    commit: &'static str,
    executable: String,
}

#[derive(Debug, Serialize)]
struct ResolutionInfo {
    transport: &'static str,
    websocket_source: &'static str,
    websocket_endpoint: String,
    daemon_policy: &'static str,
    codex_home: &'static str,
    daemon_log_directory: &'static str,
    model_default: &'static str,
    reasoning_effort_default: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Ok,
    Warning,
    Fail,
    Skipped,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    category: &'static str,
    code: &'static str,
    status: CheckStatus,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_action: Option<String>,
    redacted: bool,
}

struct CommandOutput {
    success: bool,
    stdout: Vec<u8>,
    truncated: bool,
}

pub(crate) async fn run_doctor(options: DoctorOptions) -> Result<bool, LunaError> {
    let report = build_report(&options).await;
    if options.json {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        serde_json::to_writer_pretty(&mut lock, &report).map_err(io::Error::other)?;
        writeln!(lock)?;
    } else {
        print_summary(&report)?;
    }
    Ok(report.overall_status != CheckStatus::Fail)
}

async fn build_report(options: &DoctorOptions) -> DoctorReportV1 {
    let mut checks = Vec::new();
    checks.push(check_luna_binary());
    checks.push(check_temp_directory());

    let codex_binary = match resolve_codex_binary() {
        Ok(path) => {
            checks.push(DoctorCheck {
                category: "codex",
                code: "codex.binary.found",
                status: CheckStatus::Ok,
                summary: "Codex CLI is available".to_string(),
                evidence: Some(display_path_redacted(&path)),
                next_action: None,
                redacted: true,
            });
            Some(path)
        }
        Err(error) => {
            checks.push(DoctorCheck {
                category: "codex",
                code: "codex.binary.missing",
                status: CheckStatus::Fail,
                summary: "Codex CLI is not available".to_string(),
                evidence: None,
                next_action: Some(format!("{error}")),
                redacted: true,
            });
            None
        }
    };

    if let Some(path) = codex_binary.as_deref() {
        checks.push(check_codex_version(path).await);
        checks.push(check_auth_availability());
        if options.live {
            checks.push(check_upstream_doctor(path).await);
            checks.extend(check_live_readiness(options).await);
        } else {
            checks.push(DoctorCheck {
                category: "codex",
                code: "codex.doctor.skipped",
                status: CheckStatus::Skipped,
                summary: "Upstream Codex network diagnostics were not run".to_string(),
                evidence: None,
                next_action: Some(
                    "Use `luna doctor --live` to run upstream and app-server checks".to_string(),
                ),
                redacted: true,
            });
        }
    }

    checks.push(DoctorCheck {
        category: "transport",
        code: "luna.transport.resolved",
        status: CheckStatus::Ok,
        summary: match options.transport {
            TransportMode::WebSocket if options.manage_daemon => {
                "WebSocket transport will reuse or start Luna's default loopback app-server"
                    .to_string()
            }
            TransportMode::WebSocket => {
                "WebSocket transport will connect without managing the supplied endpoint"
                    .to_string()
            }
            TransportMode::Stdio => "stdio transport will own one app-server process".to_string(),
        },
        evidence: Some(match options.transport {
            TransportMode::WebSocket => redact_websocket_url(&options.websocket_url),
            TransportMode::Stdio => "stdio".to_string(),
        }),
        next_action: None,
        redacted: true,
    });

    let overall_status = if checks.iter().any(|check| check.status == CheckStatus::Fail) {
        CheckStatus::Fail
    } else if checks
        .iter()
        .any(|check| check.status == CheckStatus::Warning)
    {
        CheckStatus::Warning
    } else {
        CheckStatus::Ok
    };

    DoctorReportV1 {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        overall_status,
        build: BuildInfo {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            target: option_env!("LUNA_BUILD_TARGET")
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}-{}", env::consts::ARCH, env::consts::OS)),
            commit: option_env!("LUNA_BUILD_COMMIT").unwrap_or("unknown"),
            executable: env::current_exe()
                .ok()
                .as_deref()
                .map(display_path_redacted)
                .unwrap_or_else(|| "luna".to_string()),
        },
        resolution: ResolutionInfo {
            transport: match options.transport {
                TransportMode::WebSocket => "websocket",
                TransportMode::Stdio => "stdio",
            },
            websocket_source: if options.manage_daemon {
                "implicit_default"
            } else if options.transport == TransportMode::WebSocket {
                "user_supplied_or_connect_only"
            } else {
                "not_applicable"
            },
            websocket_endpoint: if options.transport == TransportMode::WebSocket {
                redact_websocket_url(&options.websocket_url)
            } else {
                "not_applicable".to_string()
            },
            daemon_policy: if options.manage_daemon {
                "reuse_or_start"
            } else {
                "connect_only"
            },
            codex_home: if resolve_codex_home_dir().is_some() {
                "resolved (path redacted)"
            } else {
                "unresolved"
            },
            daemon_log_directory: "system temporary directory/codex-app-server-sdk",
            model_default: crate::MODEL,
            reasoning_effort_default: "max",
        },
        checks,
    }
}

fn check_luna_binary() -> DoctorCheck {
    DoctorCheck {
        category: "luna",
        code: "luna.binary.ready",
        status: CheckStatus::Ok,
        summary: format!("Luna {} is runnable", env!("CARGO_PKG_VERSION")),
        evidence: Some(format!("{}-{}", env::consts::ARCH, env::consts::OS)),
        next_action: None,
        redacted: true,
    }
}

fn check_temp_directory() -> DoctorCheck {
    let directory = env::temp_dir().join("codex-app-server-sdk");
    let result = (|| -> io::Result<()> {
        if let Ok(metadata) = fs::symlink_metadata(&directory)
            && metadata.file_type().is_symlink()
        {
            return Err(io::Error::other("app-server log directory is a symlink"));
        }
        fs::create_dir_all(&directory)?;
        let probe = directory.join(format!(".luna-doctor-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        let write_result = file.write_all(b"luna-doctor");
        drop(file);
        let cleanup_result = fs::remove_file(probe);
        write_result?;
        cleanup_result?;
        Ok(())
    })();

    match result {
        Ok(()) => DoctorCheck {
            category: "luna",
            code: "luna.temp.writable",
            status: CheckStatus::Ok,
            summary: "App-server log directory is writable".to_string(),
            evidence: Some("system temporary directory (path redacted)".to_string()),
            next_action: None,
            redacted: true,
        },
        Err(error) => DoctorCheck {
            category: "luna",
            code: "luna.temp.unwritable",
            status: CheckStatus::Fail,
            summary: "App-server log directory is not writable".to_string(),
            evidence: Some(error.kind().to_string()),
            next_action: Some(
                "Fix permissions for the system temporary directory or use `luna exec --stdio`"
                    .to_string(),
            ),
            redacted: true,
        },
    }
}

async fn check_codex_version(path: &Path) -> DoctorCheck {
    match run_bounded_command(path, &["--version"]).await {
        Ok(output) if output.success => {
            let version = first_line(&output.stdout).unwrap_or("version output unavailable");
            DoctorCheck {
                category: "codex",
                code: "codex.version.ready",
                status: CheckStatus::Ok,
                summary: "Codex CLI version command succeeded".to_string(),
                evidence: Some(sanitize_version(version)),
                next_action: None,
                redacted: true,
            }
        }
        Ok(_output) => DoctorCheck {
            category: "codex",
            code: "codex.version.failed",
            status: CheckStatus::Fail,
            summary: "Codex CLI version command failed".to_string(),
            evidence: Some("non-zero exit status (stderr redacted)".to_string()),
            next_action: Some("Reinstall or update the Codex CLI".to_string()),
            redacted: true,
        },
        Err(error) => DoctorCheck {
            category: "codex",
            code: "codex.version.failed",
            status: CheckStatus::Fail,
            summary: "Codex CLI version command could not run".to_string(),
            evidence: Some(error.to_string()),
            next_action: Some("Reinstall or update the Codex CLI".to_string()),
            redacted: true,
        },
    }
}

fn check_auth_availability() -> DoctorCheck {
    let env_auth = env::var_os("OPENAI_API_KEY").is_some()
        || (env::var_os("CODEX_ID_TOKEN").is_some() && env::var_os("CODEX_ACCESS_TOKEN").is_some());
    let cached_auth = resolve_codex_home_dir()
        .map(|home| home.join("auth.json").is_file())
        .unwrap_or(false);
    if env_auth || cached_auth {
        DoctorCheck {
            category: "authentication",
            code: "codex.auth.available",
            status: CheckStatus::Ok,
            summary: "Codex credentials are available (validity not tested offline)".to_string(),
            evidence: Some(if env_auth {
                "credential environment variable present (value redacted)".to_string()
            } else {
                "cached credential file present (path and contents redacted)".to_string()
            }),
            next_action: None,
            redacted: true,
        }
    } else {
        DoctorCheck {
            category: "authentication",
            code: "codex.auth.unconfirmed",
            status: CheckStatus::Warning,
            summary: "Codex credentials could not be confirmed offline".to_string(),
            evidence: None,
            next_action: Some(
                "Run `luna doctor --live`; if needed, authenticate with `codex login`".to_string(),
            ),
            redacted: true,
        }
    }
}

async fn check_upstream_doctor(path: &Path) -> DoctorCheck {
    match run_bounded_command(path, &["doctor", "--json"]).await {
        Ok(output) => match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(report) => {
                let status = report
                    .get("overallStatus")
                    .and_then(Value::as_str)
                    .filter(|status| matches!(*status, "ok" | "warning" | "warn" | "fail"))
                    .unwrap_or("unknown");
                DoctorCheck {
                    category: "codex",
                    code: if output.success {
                        "codex.doctor.ready"
                    } else {
                        "codex.doctor.reported_failure"
                    },
                    status: if output.success {
                        CheckStatus::Ok
                    } else {
                        CheckStatus::Fail
                    },
                    summary: format!("Upstream Codex doctor status: {status}"),
                    evidence: report
                        .get("codexVersion")
                        .and_then(Value::as_str)
                        .map(|version| format!("Codex {}", sanitize_version(version))),
                    next_action: (!output.success).then(|| {
                        "Run `codex doctor --summary` for upstream remediation".to_string()
                    }),
                    redacted: true,
                }
            }
            Err(_) => DoctorCheck {
                category: "codex",
                code: "codex.doctor.unavailable",
                status: CheckStatus::Warning,
                summary: "Installed Codex does not provide a readable JSON doctor report"
                    .to_string(),
                evidence: output
                    .truncated
                    .then(|| "output exceeded 1 MiB cap".to_string()),
                next_action: Some(
                    "Update Codex or run `codex doctor --summary` directly".to_string(),
                ),
                redacted: true,
            },
        },
        Err(error) => DoctorCheck {
            category: "codex",
            code: "codex.doctor.unavailable",
            status: CheckStatus::Warning,
            summary: "Upstream Codex doctor could not complete".to_string(),
            evidence: Some(error.to_string()),
            next_action: Some("Run `codex doctor --summary` directly".to_string()),
            redacted: true,
        },
    }
}

async fn check_live_readiness(options: &DoctorOptions) -> Vec<DoctorCheck> {
    let connection = match options.transport {
        TransportMode::WebSocket => {
            connect_ws_codex(&options.websocket_url, options.manage_daemon).await
        }
        TransportMode::Stdio => spawn_stdio_codex().await,
    };

    let codex = match connection {
        Ok(codex) => codex,
        Err(error) => {
            return vec![DoctorCheck {
                category: "app-server",
                code: "app_server.connection.failed",
                status: CheckStatus::Fail,
                summary: "Luna could not connect to the selected app-server transport".to_string(),
                evidence: Some(error.code().to_string()),
                next_action: Some(
                    "Check the selected endpoint, or retry with `luna doctor --live --stdio`"
                        .to_string(),
                ),
                redacted: true,
            }];
        }
    };

    match codex
        .account_read(requests::GetAccountParams::default())
        .await
    {
        Ok(account) => {
            let authenticated = account_is_authenticated(&account.extra);
            vec![
                DoctorCheck {
                    category: "app-server",
                    code: "app_server.connection.ready",
                    status: CheckStatus::Ok,
                    summary: "Luna completed the app-server handshake".to_string(),
                    evidence: None,
                    next_action: None,
                    redacted: true,
                },
                DoctorCheck {
                    category: "authentication",
                    code: if authenticated {
                        "codex.auth.valid"
                    } else {
                        "codex.auth.invalid"
                    },
                    status: if authenticated {
                        CheckStatus::Ok
                    } else {
                        CheckStatus::Fail
                    },
                    summary: if authenticated {
                        "App-server reports an authenticated account".to_string()
                    } else {
                        "App-server reports that the account is not authenticated".to_string()
                    },
                    evidence: None,
                    next_action: (!authenticated)
                        .then(|| "Run `codex login`, then rerun `luna doctor --live`".to_string()),
                    redacted: true,
                },
            ]
        }
        Err(error) => vec![DoctorCheck {
            category: "app-server",
            code: "app_server.handshake.failed",
            status: CheckStatus::Fail,
            summary: "App-server connected but its account-readiness check failed".to_string(),
            evidence: Some(LunaError::from(error).code().to_string()),
            next_action: Some(
                "Run `codex doctor --summary` and check app-server compatibility".to_string(),
            ),
            redacted: true,
        }],
    }
}

async fn run_bounded_command(path: &Path, args: &[&str]) -> io::Result<CommandOutput> {
    let mut child = Command::new(path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr"))?;
    let stdout_task = tokio::spawn(read_capped(stdout));
    let stderr_task = tokio::spawn(read_capped(stderr));

    let status = match tokio::time::timeout(COMMAND_TIMEOUT, child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
        }
    };
    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(|error| io::Error::other(format!("stdout reader failed: {error}")))??;
    let (_stderr, stderr_truncated) = stderr_task
        .await
        .map_err(|error| io::Error::other(format!("stderr reader failed: {error}")))??;

    Ok(CommandOutput {
        success: status.success(),
        stdout,
        truncated: stdout_truncated || stderr_truncated,
    })
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(output.len());
        let retained = remaining.min(read);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((output, truncated))
}

fn first_line(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

fn sanitize_version(version: &str) -> String {
    let trimmed = version.trim();
    if trimmed.len() > 120
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '.' | '-' | '_' | '+' | '(' | ')')
        })
    {
        "version text redacted".to_string()
    } else {
        trimmed.to_string()
    }
}

fn redact_websocket_url(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let scheme_end = without_query
        .find("://")
        .map(|index| index + 3)
        .unwrap_or(0);
    let authority_and_path = &without_query[scheme_end..];
    let (authority, path) = authority_and_path
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((authority_and_path, String::new()));
    let authority = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    format!("{}{}{}", &without_query[..scheme_end], authority, path)
}

fn print_summary(report: &DoctorReportV1) -> Result<(), LunaError> {
    println!(
        "Luna doctor {} ({})",
        env!("CARGO_PKG_VERSION"),
        status_label(report.overall_status)
    );
    for check in &report.checks {
        println!(
            "{:<7} {:<16} {}",
            status_label(check.status),
            check.category,
            check.summary
        );
        if let Some(action) = &check.next_action {
            println!("        next: {action}");
        }
    }
    let ok = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Ok)
        .count();
    let warning = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warning)
        .count();
    let failed = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    let skipped = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Skipped)
        .count();
    println!("{ok} ok, {warning} warning, {failed} failed, {skipped} skipped");
    Ok(())
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "OK",
        CheckStatus::Warning => "WARN",
        CheckStatus::Fail => "FAIL",
        CheckStatus::Skipped => "SKIP",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_redaction_removes_credentials_and_query_values() {
        assert_eq!(
            redact_websocket_url("wss://user:secret@example.com/app?token=secret#fragment"),
            "wss://example.com/app"
        );
    }

    #[test]
    fn doctor_report_uses_versioned_snake_case_contract() {
        let report = DoctorReportV1 {
            schema_version: 1,
            generated_at_unix_seconds: 1,
            overall_status: CheckStatus::Ok,
            build: BuildInfo {
                name: "luna",
                version: "0.1.0",
                target: "test-target".to_string(),
                commit: "unknown",
                executable: "luna".to_string(),
            },
            resolution: ResolutionInfo {
                transport: "stdio",
                websocket_source: "not_applicable",
                websocket_endpoint: "not_applicable".to_string(),
                daemon_policy: "connect_only",
                codex_home: "resolved (path redacted)",
                daemon_log_directory: "system temporary directory/codex-app-server-sdk",
                model_default: "gpt-test",
                reasoning_effort_default: "max",
            },
            checks: Vec::new(),
        };
        let value = serde_json::to_value(report).expect("serialize report");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["overall_status"], "ok");
        assert!(value.get("schemaVersion").is_none());
    }

    #[test]
    fn version_evidence_rejects_paths_and_long_untrusted_text() {
        assert_eq!(sanitize_version("codex-cli 1.2.3"), "codex-cli 1.2.3");
        assert_eq!(
            sanitize_version("/Users/name/bin/codex"),
            "version text redacted"
        );
        assert_eq!(sanitize_version(&"x".repeat(121)), "version text redacted");
    }
}
