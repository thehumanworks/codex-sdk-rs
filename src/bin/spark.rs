use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::ExitCode;

use codex_app_server_sdk::api::{
    Codex, ModelReasoningEffort, ThreadEvent, ThreadItem, ThreadOptions, TurnOptions,
};
use codex_app_server_sdk::{ClientError, StdioConfig};
use serde::Deserialize;
use thiserror::Error;

const APP_NAME: &str = "spark";
const MODEL: &str = "gpt-5.3-codex-spark";

const USAGE: &str = "\
Usage: spark [--agent NAME] [PROMPT...]

Runs one streamed turn with:
  model: gpt-5.3-codex-spark
  reasoning effort: xhigh

Options:
  --agent NAME    Load ~/.codex/agents/NAME.md
  -h, --help      Show this help

If PROMPT is omitted, spark reads the prompt from stdin.
";

#[derive(Debug)]
struct CliArgs {
    agent: Option<String>,
    prompt_parts: Vec<String>,
}

#[derive(Debug)]
struct LoadedAgent {
    instructions: String,
}

#[derive(Debug, Deserialize)]
struct AgentFrontmatter {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Option<SkillsField>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SkillsField {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug)]
enum ParsedCommand {
    Help,
    Run(CliArgs),
}

#[derive(Debug, Error)]
enum SparkError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("failed to parse YAML frontmatter in {}: {source}", .path.display())]
    Frontmatter {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(SparkError::Usage(message)) => {
            eprintln!("{message}\n");
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("{APP_NAME}: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), SparkError> {
    let command = parse_cli_args(env::args().skip(1))?;
    let cli = match command {
        ParsedCommand::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        ParsedCommand::Run(cli) => cli,
    };

    let prompt = resolve_prompt(cli.prompt_parts)?;

    let mut thread_options = ThreadOptions::builder()
        .model(MODEL)
        .model_reasoning_effort(ModelReasoningEffort::XHigh);
    if let Some(agent_name) = cli.agent {
        let agent = load_agent_profile(&agent_name)?;
        thread_options = thread_options.developer_instructions(agent.instructions);
    }

    let codex_binary = resolve_codex_binary()?;
    let mut stdio_config = StdioConfig::default();
    stdio_config.codex_binary = codex_binary;

    let codex = Codex::spawn_stdio(stdio_config).await?;
    let mut thread = codex.start_thread(thread_options.build());
    let mut streamed = thread.run_streamed(prompt, TurnOptions::default()).await?;

    let mut stdout = io::stdout();
    let mut saw_terminal = false;
    let mut saw_delta = false;
    let mut printed_any = false;
    let mut ended_with_newline = false;

    while let Some(next) = streamed.next_event().await {
        let event = next?;
        match event {
            ThreadEvent::ItemUpdated { item } => {
                if let ThreadItem::AgentMessage(agent_message) = item {
                    if !agent_message.text.is_empty() {
                        print_chunk(&mut stdout, &agent_message.text)?;
                        saw_delta = true;
                        printed_any = true;
                        ended_with_newline = agent_message.text.ends_with('\n');
                    }
                }
            }
            ThreadEvent::ItemCompleted { item } => {
                if let ThreadItem::AgentMessage(agent_message) = item {
                    if !saw_delta && !agent_message.text.is_empty() {
                        print_chunk(&mut stdout, &agent_message.text)?;
                        printed_any = true;
                        ended_with_newline = agent_message.text.ends_with('\n');
                    }
                }
            }
            ThreadEvent::TurnCompleted { .. } => {
                saw_terminal = true;
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                return Err(SparkError::Config(format!(
                    "turn failed: {}",
                    error.message
                )));
            }
            ThreadEvent::Error { message } => {
                return Err(SparkError::Config(format!("stream error: {message}")));
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. } => {}
        }
    }

    if !saw_terminal {
        return Err(SparkError::Config(
            "stream closed before receiving turn completion".to_string(),
        ));
    }

    if printed_any && !ended_with_newline {
        writeln!(stdout)?;
        stdout.flush()?;
    }

    Ok(())
}

fn resolve_codex_binary() -> Result<String, SparkError> {
    resolve_codex_binary_with(|program, args| {
        Command::new(program)
            .args(args)
            .output()
            .map(|output| CommandResult {
                success: output.status.success(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            })
    })
}

#[derive(Debug)]
struct CommandResult {
    success: bool,
    stdout: String,
}

fn resolve_codex_binary_with<F>(mut run_command: F) -> Result<String, SparkError>
where
    F: FnMut(&str, &[&str]) -> io::Result<CommandResult>,
{
    let result = run_command("which", &["codex"]).map_err(|error| {
        SparkError::Config(format!(
            "failed to resolve codex binary via `which codex`: {error}"
        ))
    })?;

    if !result.success {
        return Err(SparkError::Config(
            "could not locate `codex` on PATH (which codex returned non-zero status)".to_string(),
        ));
    }

    let resolved = result
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| {
            SparkError::Config(
                "could not locate `codex` on PATH (which codex produced empty output)".to_string(),
            )
        })?;

    Ok(resolved.to_string())
}

fn print_chunk(stdout: &mut io::Stdout, chunk: &str) -> Result<(), SparkError> {
    write!(stdout, "{chunk}")?;
    stdout.flush()?;
    Ok(())
}

fn parse_cli_args(args: impl IntoIterator<Item = String>) -> Result<ParsedCommand, SparkError> {
    let mut agent: Option<String> = None;
    let mut prompt_parts = Vec::new();
    let mut parse_options = true;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        if parse_options {
            if arg == "--" {
                parse_options = false;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Ok(ParsedCommand::Help);
            }
            if arg == "--agent" {
                let raw = iter
                    .next()
                    .ok_or_else(|| SparkError::Usage("missing value for --agent".to_string()))?;
                if agent.is_some() {
                    return Err(SparkError::Usage(
                        "--agent may only be provided once".to_string(),
                    ));
                }
                agent = Some(normalize_agent_name(&raw)?);
                continue;
            }
            if let Some(raw) = arg.strip_prefix("--agent=") {
                if agent.is_some() {
                    return Err(SparkError::Usage(
                        "--agent may only be provided once".to_string(),
                    ));
                }
                agent = Some(normalize_agent_name(raw)?);
                continue;
            }
            if arg.starts_with('-') {
                return Err(SparkError::Usage(format!("unknown option: {arg}")));
            }
        }

        prompt_parts.push(arg);
    }

    Ok(ParsedCommand::Run(CliArgs {
        agent,
        prompt_parts,
    }))
}

fn normalize_agent_name(raw: &str) -> Result<String, SparkError> {
    let candidate = raw.trim().trim_end_matches(".md");
    if candidate.is_empty() {
        return Err(SparkError::Usage("agent name cannot be empty".to_string()));
    }
    if candidate == "." || candidate == ".." {
        return Err(SparkError::Usage(
            "agent name cannot be '.' or '..'".to_string(),
        ));
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(SparkError::Usage(
            "agent name cannot include path separators".to_string(),
        ));
    }
    if !candidate
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    {
        return Err(SparkError::Usage(
            "agent name may only contain [A-Za-z0-9._-]".to_string(),
        ));
    }
    Ok(candidate.to_string())
}

fn resolve_prompt(prompt_parts: Vec<String>) -> Result<String, SparkError> {
    if !prompt_parts.is_empty() {
        return Ok(prompt_parts.join(" "));
    }

    if io::stdin().is_terminal() {
        return Err(SparkError::Usage(
            "missing prompt; pass text args or pipe stdin".to_string(),
        ));
    }

    let mut piped = String::new();
    io::stdin().read_to_string(&mut piped)?;
    let prompt = piped.trim();
    if prompt.is_empty() {
        return Err(SparkError::Usage("stdin prompt is empty".to_string()));
    }
    Ok(prompt.to_string())
}

fn load_agent_profile(agent_name: &str) -> Result<LoadedAgent, SparkError> {
    let home = resolve_home_dir().ok_or_else(|| {
        SparkError::Config(
            "unable to resolve home directory (expected HOME, USERPROFILE, or HOMEDRIVE+HOMEPATH)"
                .to_string(),
        )
    })?;
    let agents_dir = home.join(".codex").join("agents");
    load_agent_profile_from_dir(agent_name, &agents_dir)
}

fn resolve_home_dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    if let Some(userprofile) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(userprofile));
    }
    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) if !drive.is_empty() && !path.is_empty() => {
            let mut resolved = PathBuf::from(drive);
            resolved.push(path);
            Some(resolved)
        }
        _ => None,
    }
}

fn load_agent_profile_from_dir(
    agent_name: &str,
    agents_dir: &Path,
) -> Result<LoadedAgent, SparkError> {
    let path = agents_dir.join(format!("{agent_name}.md"));
    let raw = fs::read_to_string(&path).map_err(|error| {
        SparkError::Config(format!(
            "failed to read agent file {}: {error}",
            path.display()
        ))
    })?;
    let (frontmatter_raw, body_raw) = split_frontmatter(&raw).map_err(|message| {
        SparkError::Config(format!("invalid agent file {}: {message}", path.display()))
    })?;

    let frontmatter: AgentFrontmatter =
        serde_yaml::from_str(&frontmatter_raw).map_err(|source| SparkError::Frontmatter {
            path: path.clone(),
            source,
        })?;

    let file_stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if frontmatter.name != file_stem {
        return Err(SparkError::Config(format!(
            "agent file {} must declare name '{}' in frontmatter, found '{}'",
            path.display(),
            file_stem,
            frontmatter.name
        )));
    }
    if frontmatter.name != agent_name {
        return Err(SparkError::Config(format!(
            "agent file {} declares '{}', but --agent requested '{}'",
            path.display(),
            frontmatter.name,
            agent_name
        )));
    }

    let skills = normalize_skills(frontmatter.skills);
    let description = frontmatter
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&frontmatter.name);
    let body = body_raw.trim().to_string();
    let instructions = compose_instructions(description, &skills, &body);

    Ok(LoadedAgent { instructions })
}

fn split_frontmatter(content: &str) -> Result<(String, String), String> {
    let normalized = content.replace("\r\n", "\n");
    let without_bom = normalized.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = without_bom.split('\n').collect();
    if lines.first().is_none_or(|line| line.trim() != "---") {
        return Err("missing leading YAML frontmatter delimiter '---'".to_string());
    }

    let closing_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, line)| (line.trim() == "---").then_some(index))
        .ok_or_else(|| "missing closing YAML frontmatter delimiter '---'".to_string())?;

    let frontmatter = lines[1..closing_index].join("\n");
    let body = lines[(closing_index + 1)..].join("\n");
    Ok((frontmatter, body))
}

fn normalize_skills(skills: Option<SkillsField>) -> Vec<String> {
    let values = match skills {
        Some(SkillsField::Single(skill)) => vec![skill],
        Some(SkillsField::Multiple(items)) => items,
        None => Vec::new(),
    };
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn compose_instructions(description: &str, skills: &[String], body: &str) -> String {
    let role = format!("<ROLE>{description}</ROLE>");
    let skill_lines = if skills.is_empty() {
        String::new()
    } else {
        skills
            .iter()
            .map(|skill| format!("\t\t<SKILL>{skill}</SKILL>"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let instructions = format!(
        "<INSTRUCTIONS>\n\t<SKILLS>\n{skill_lines}\n\t</SKILLS>\n\t<CONTENT>\n\t\t{body}\n\t</CONTENT>\n</INSTRUCTIONS>"
    );

    format!("{role}\n{instructions}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_cli_args_supports_agent_flag_and_prompt() {
        let parsed = parse_cli_args(
            vec![
                "--agent".to_string(),
                "writer".to_string(),
                "hello".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.agent.as_deref(), Some("writer"));
        assert_eq!(cli.prompt_parts, vec!["hello"]);
    }

    #[test]
    fn parse_cli_args_supports_equals_form() {
        let parsed = parse_cli_args(
            vec![
                "--agent=writer".to_string(),
                "hi".to_string(),
                "there".to_string(),
            ]
            .into_iter(),
        )
        .expect("parse args");

        let ParsedCommand::Run(cli) = parsed else {
            panic!("expected run command");
        };

        assert_eq!(cli.agent.as_deref(), Some("writer"));
        assert_eq!(cli.prompt_parts, vec!["hi", "there"]);
    }

    #[test]
    fn parse_cli_args_rejects_unknown_option() {
        let error = parse_cli_args(vec!["--nope".to_string()].into_iter()).expect_err("invalid");
        assert!(matches!(error, SparkError::Usage(_)));
    }

    #[test]
    fn split_frontmatter_extracts_yaml_and_body() {
        let input = "\
---
name: spark
skills:
  - checks
---

body text
";
        let (frontmatter, body) = split_frontmatter(input).expect("frontmatter parsed");
        assert!(frontmatter.contains("name: spark"));
        assert!(frontmatter.contains("skills"));
        assert!(body.contains("body text"));
    }

    #[test]
    fn split_frontmatter_accepts_closing_delimiter_with_trailing_spaces() {
        let input = "\
---
name: spark
---   
body text
";
        let (frontmatter, body) = split_frontmatter(input).expect("frontmatter parsed");
        assert!(frontmatter.contains("name: spark"));
        assert_eq!(body, "body text\n");
    }

    #[test]
    fn load_agent_profile_rejects_name_mismatch() {
        let dir = make_temp_dir();
        let path = dir.join("spark.md");
        fs::write(
            &path,
            "\
---
name: different
skills: [a]
---
instructions
",
        )
        .expect("write");

        let error = load_agent_profile_from_dir("spark", &dir).expect_err("name mismatch");
        assert!(matches!(error, SparkError::Config(_)));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_uses_skills_and_body() {
        let dir = make_temp_dir();
        let path = dir.join("spark.md");
        fs::write(
            &path,
            "\
---
name: spark
description: Spark coding assistant
skills:
  - checks
  - lint
---
Do the task.
",
        )
        .expect("write");

        let profile = load_agent_profile_from_dir("spark", &dir).expect("load profile");
        assert!(
            profile
                .instructions
                .starts_with("<ROLE>Spark coding assistant</ROLE>\n<INSTRUCTIONS>")
        );
        assert!(profile.instructions.contains("<SKILLS>"));
        assert!(profile.instructions.contains("<SKILL>checks</SKILL>"));
        assert!(profile.instructions.contains("<SKILL>lint</SKILL>"));
        assert!(profile.instructions.contains("<CONTENT>"));
        assert!(profile.instructions.contains("Do the task."));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn load_agent_profile_falls_back_to_name_when_description_missing() {
        let dir = make_temp_dir();
        let path = dir.join("spark.md");
        fs::write(
            &path,
            "\
---
name: spark
skills: [checks]
---
Do the task.
",
        )
        .expect("write");

        let profile = load_agent_profile_from_dir("spark", &dir).expect("load profile");
        assert!(profile.instructions.starts_with("<ROLE>spark</ROLE>"));

        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn resolve_codex_binary_parses_trimmed_path() {
        let resolved = resolve_codex_binary_with(|program, args| {
            assert_eq!(program, "which");
            assert_eq!(args, ["codex"]);
            Ok(CommandResult {
                success: true,
                stdout: " /usr/local/bin/codex  \n".to_string(),
            })
        })
        .expect("resolve codex");

        assert_eq!(resolved, "/usr/local/bin/codex");
    }

    #[test]
    fn resolve_codex_binary_uses_first_non_empty_line() {
        let resolved = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: true,
                stdout: "\n/usr/bin/codex\n/opt/bin/codex\n".to_string(),
            })
        })
        .expect("resolve codex");

        assert_eq!(resolved, "/usr/bin/codex");
    }

    #[test]
    fn resolve_codex_binary_errors_when_which_fails() {
        let error = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: false,
                stdout: String::new(),
            })
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    #[test]
    fn resolve_codex_binary_errors_on_empty_output() {
        let error = resolve_codex_binary_with(|_, _| {
            Ok(CommandResult {
                success: true,
                stdout: "   \n".to_string(),
            })
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    #[test]
    fn resolve_codex_binary_errors_when_command_cannot_run() {
        let error = resolve_codex_binary_with(|_, _| {
            Err(io::Error::new(io::ErrorKind::NotFound, "which not found"))
        })
        .expect_err("expected failure");

        assert!(matches!(error, SparkError::Config(_)));
    }

    fn make_temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = env::temp_dir().join(format!("spark-tests-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }
}
