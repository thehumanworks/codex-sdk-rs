use std::{collections::HashSet, env, fs, path::PathBuf};

use anyhow::Context;
use clap::Parser;
use codex_app_server_sdk::{
    CodexClient, CommandExecutionStatus, ModelReasoningEffort, PatchApplyStatus, PatchChangeKind,
    ThreadEvent, ThreadItem, ThreadOptions, TurnOptions, Usage, WsConfig,
};
use owo_colors::{OwoColorize, Stream::Stdout};
use serde::Deserialize;

const DEFAULT_MODEL: &str = "gpt-5.3-codex-spark";
const DEV_INSTRUCTIONS_PREVIEW_CHARS: usize = 220;

#[derive(Parser)]
#[command(name = "agent", about = "Agent CLI using codex-app-server-sdk")]
struct Cli {
    /// The prompt to send to the model
    prompt: String,

    #[arg(long)]
    agent: Option<String>,

    /// Show token usage and thread ID in output
    #[arg(long)]
    verbose: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentConfig {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    developer_instructions: String,
}

#[derive(Debug, Clone)]
struct LoadedAgent {
    name: String,
    model: Option<String>,
    developer_instructions: String,
}

#[derive(Debug, Clone)]
struct TurnStartInfo {
    agent_name: String,
    model: Option<String>,
    developer_instructions_preview: String,
}

impl From<&LoadedAgent> for TurnStartInfo {
    fn from(agent: &LoadedAgent) -> Self {
        Self {
            agent_name: agent.name.clone(),
            model: agent.model.clone(),
            developer_instructions_preview: clipped_text(
                &agent.developer_instructions,
                DEV_INSTRUCTIONS_PREVIEW_CHARS,
            ),
        }
    }
}

fn load_agent(agent_name: &str) -> anyhow::Result<LoadedAgent> {
    let path = agent_config_path(agent_name)?;
    let file = fs::read_to_string(&path)
        .with_context(|| format!("failed to read agent config {}", path.display()))?;
    let config: AgentConfig = toml::from_str(&file)
        .with_context(|| format!("failed to parse agent config {}", path.display()))?;

    Ok(LoadedAgent {
        name: non_empty(config.name).unwrap_or_else(|| agent_name.to_string()),
        model: non_empty(config.model),
        developer_instructions: config.developer_instructions,
    })
}

fn agent_config_path(agent_name: &str) -> anyhow::Result<PathBuf> {
    let home = env::var_os("HOME").context("HOME is not set; cannot locate agent config")?;
    Ok(PathBuf::from(home)
        .join(".codex")
        .join("agents")
        .join(format!("{agent_name}.toml")))
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn clipped_text(text: &str, max_chars: usize) -> String {
    if text.trim().is_empty() {
        return "(empty)".to_string();
    }
    if max_chars == 0 {
        return "...".to_string();
    }

    let mut preview = String::with_capacity(max_chars.saturating_add(3).min(text.len()));
    let mut chars = 0usize;

    for word in text.split_whitespace() {
        let separator = usize::from(!preview.is_empty());
        let remaining = max_chars.saturating_sub(chars);

        if remaining <= separator {
            preview.push_str("...");
            return preview;
        }

        if separator == 1 {
            preview.push(' ');
            chars += 1;
        }

        for ch in word.chars() {
            if chars == max_chars {
                preview.push_str("...");
                return preview;
            }
            preview.push(ch);
            chars += 1;
        }
    }

    preview
}

fn print_turn_start_info(info: &TurnStartInfo) {
    let message = match &info.model {
        Some(model) => format!(
            "active agent: {} | model: {} | developer instructions: {}",
            info.agent_name, model, info.developer_instructions_preview
        ),
        None => format!(
            "active agent: {} | developer instructions: {}",
            info.agent_name, info.developer_instructions_preview
        ),
    };

    println!(
        "{} {}",
        "info:".if_supports_color(Stdout, |t| t.dimmed().to_string()),
        message.if_supports_color(Stdout, |t| t.dimmed().to_string())
    );
}

fn print_usage(usage: &Usage) {
    let input_tokens = usage.input_tokens;
    let cached_input_tokens = usage.cached_input_tokens;
    let output_tokens = usage.output_tokens;

    println!(
        "\n\n{}",
        format!("---\nUsage:\n- Input tokens: {input_tokens}\n- Cached input tokens: {cached_input_tokens}\nOutput tokens: {output_tokens}")
            .if_supports_color(Stdout, |t| t.dimmed().to_string())
    );
}

fn print_reasoning_text(text: &str) {
    if text.is_empty() {
        println!(
            "{}\n",
            "Thinking...".if_supports_color(Stdout, |t| t.dimmed().italic().to_string())
        );
    } else {
        println!(
            "{}\n\t{}\n",
            "Thinking...".if_supports_color(Stdout, |t| t.dimmed().italic().to_string()),
            text.if_supports_color(Stdout, |t| t.dimmed().to_string())
        );
    }
}

fn build_thread_config(active_agent: Option<LoadedAgent>) -> ThreadOptions {
    let mut builder = ThreadOptions::builder()
        .model(DEFAULT_MODEL)
        .model_reasoning_effort(ModelReasoningEffort::XHigh)
        .ephemeral(true)
        .skip_git_repo_check(true);

    if let Some(agent) = active_agent {
        if let Some(model) = agent.model {
            builder = builder.model(model);
        }
        builder = builder.developer_instructions(agent.developer_instructions);
    }

    builder.build()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let active_agent = cli.agent.as_deref().map(load_agent).transpose()?;
    let turn_start_info = active_agent.as_ref().map(TurnStartInfo::from);

    let client = CodexClient::connect_ws(WsConfig::default()).await?;
    let mut thread = client.start_thread(build_thread_config(active_agent));
    let mut streamed = thread
        .run_streamed(cli.prompt.as_str(), TurnOptions::default())
        .await?;
    let mut printed_reasoning_items = HashSet::new();

    while let Some(next) = streamed.next_event().await {
        match next? {
            ThreadEvent::TurnCompleted { usage } => {
                if cli.verbose
                    && let Some(usage) = usage
                {
                    print_usage(&usage);
                }
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                eprintln!(
                    "{}",
                    format!("streamed turn failed: {}", error.message)
                        .if_supports_color(Stdout, |t| t.bold().red().to_string())
                );
                break;
            }
            ThreadEvent::ThreadStarted { thread_id } => {
                if cli.verbose {
                    println!(
                        "{}",
                        format!("Thread ID: {thread_id}\n---")
                            .if_supports_color(Stdout, |t| t.dimmed().to_string())
                    );
                }
            }
            ThreadEvent::Error { message } => {
                eprintln!(
                    "{}",
                    format!("error: {message}")
                        .if_supports_color(Stdout, |t| t.bold().red().to_string())
                );
            }
            ThreadEvent::TurnStarted => {
                if let Some(ref info) = turn_start_info {
                    print_turn_start_info(info);
                }
            }
            ThreadEvent::ItemStarted { .. } => {}
            ThreadEvent::ItemUpdated {
                item: ThreadItem::Reasoning(reasoning),
            } => {
                if !reasoning.text.is_empty() {
                    print_reasoning_text(&reasoning.text);
                    printed_reasoning_items.insert(reasoning.id);
                }
            }
            ThreadEvent::ItemUpdated { .. } => {}
            ThreadEvent::ItemCompleted { item } => match item {
                ThreadItem::AgentMessage(message) => {
                    println!(
                        "{}",
                        message
                            .text
                            .if_supports_color(Stdout, |t| t.bold().bright_white().to_string())
                    );
                }
                ThreadItem::Reasoning(reasoning) => {
                    if !printed_reasoning_items.contains(&reasoning.id) {
                        print_reasoning_text(&reasoning.text);
                    }
                }
                ThreadItem::WebSearch(search) => {
                    println!(
                        "{} {}\n",
                        "⊙ Searching:".if_supports_color(Stdout, |t| t.cyan().to_string()),
                        search
                            .query
                            .if_supports_color(Stdout, |t| t.cyan().to_string())
                    );
                }
                ThreadItem::Plan(plan) => {
                    println!(
                        "{} {}\n",
                        "Plan:".if_supports_color(Stdout, |t| t.yellow().bold().to_string()),
                        plan.text
                            .if_supports_color(Stdout, |t| t.yellow().to_string())
                    );
                }
                ThreadItem::CommandExecution(cmd) => {
                    let failed = cmd.status == CommandExecutionStatus::Failed
                        || cmd.exit_code.is_some_and(|c| c != 0);

                    println!(
                        "{} {}",
                        ">".if_supports_color(Stdout, |t| if failed {
                            t.red().to_string()
                        } else {
                            t.green().to_string()
                        }),
                        cmd.command.if_supports_color(Stdout, |t| if failed {
                            t.red().to_string()
                        } else {
                            t.green().to_string()
                        }),
                    );
                    if !cmd.aggregated_output.is_empty() {
                        println!(
                            "{}",
                            cmd.aggregated_output
                                .if_supports_color(Stdout, |t| t.dimmed().to_string())
                        );
                    }
                    if let Some(code) = cmd.exit_code {
                        if code != 0 {
                            println!(
                                "{}",
                                format!("exit code: {code}")
                                    .if_supports_color(Stdout, |t| t.red().to_string())
                            );
                        }
                    }
                }
                ThreadItem::FileChange(fc) => {
                    for change in &fc.changes {
                        match change.kind {
                            PatchChangeKind::Add => println!(
                                "\t{} {}",
                                "+".if_supports_color(Stdout, |t| t.green().to_string()),
                                change
                                    .path
                                    .if_supports_color(Stdout, |t| t.green().to_string()),
                            ),
                            PatchChangeKind::Delete => println!(
                                "\t{} {}",
                                "-".if_supports_color(Stdout, |t| t.red().to_string()),
                                change
                                    .path
                                    .if_supports_color(Stdout, |t| t.red().to_string()),
                            ),
                            PatchChangeKind::Update | PatchChangeKind::Unknown => println!(
                                "\t{} {}",
                                "~".if_supports_color(Stdout, |t| t.magenta().to_string()),
                                change
                                    .path
                                    .if_supports_color(Stdout, |t| t.magenta().to_string()),
                            ),
                        }
                    }
                    if fc.status == PatchApplyStatus::Failed {
                        println!(
                            "\t{}",
                            "patch failed"
                                .if_supports_color(Stdout, |t| t.bold().red().to_string())
                        );
                    }
                }
                ThreadItem::McpToolCall(mcp) => {
                    print!(
                        "{} {}",
                        "MCP:".if_supports_color(Stdout, |t| t.blue().bold().to_string()),
                        format!("{}::{}", mcp.server, mcp.tool)
                            .if_supports_color(Stdout, |t| t.blue().to_string()),
                    );
                    if let Some(ref err) = mcp.error {
                        println!(
                            " {}",
                            format!("error: {}", err.message)
                                .if_supports_color(Stdout, |t| t.red().to_string())
                        );
                    } else {
                        println!();
                    }
                }
                ThreadItem::DynamicToolCall(dtc) => {
                    println!(
                        "{} {} ({})",
                        "Tool:".if_supports_color(Stdout, |t| t.blue().bold().to_string()),
                        dtc.tool.if_supports_color(Stdout, |t| t.blue().to_string()),
                        dtc.status
                            .if_supports_color(Stdout, |t| t.dimmed().to_string()),
                    );
                }
                ThreadItem::CollabToolCall(collab) => {
                    println!(
                        "{} {} ({})",
                        "Collab:".if_supports_color(Stdout, |t| t.blue().bold().to_string()),
                        collab
                            .tool
                            .if_supports_color(Stdout, |t| t.blue().to_string()),
                        collab
                            .status
                            .if_supports_color(Stdout, |t| t.dimmed().to_string()),
                    );
                }
                ThreadItem::TodoList(todo) => {
                    for item in &todo.items {
                        if item.completed {
                            println!(
                                "\t{} {}",
                                "✓".if_supports_color(Stdout, |t| t.green().to_string()),
                                item.text
                                    .if_supports_color(Stdout, |t| t.dimmed().to_string()),
                            );
                        } else {
                            println!(
                                "\t{} {}",
                                "○".if_supports_color(Stdout, |t| t.dimmed().to_string()),
                                item.text,
                            );
                        }
                    }
                }
                ThreadItem::Error(err) => {
                    eprintln!(
                        "{}",
                        format!("error: {}", err.message)
                            .if_supports_color(Stdout, |t| t.bold().red().to_string())
                    );
                }
                ThreadItem::ImageView(img) => {
                    println!(
                        "\t{}",
                        format!("Image: {}", img.path)
                            .if_supports_color(Stdout, |t| t.dimmed().to_string())
                    );
                }
                ThreadItem::EnteredReviewMode(review) => {
                    println!(
                        "{}",
                        format!("Entered review mode: {}", review.review)
                            .if_supports_color(Stdout, |t| t.yellow().italic().to_string())
                    );
                }
                ThreadItem::ExitedReviewMode(_) => {
                    println!(
                        "{}",
                        "Exited review mode"
                            .if_supports_color(Stdout, |t| t.yellow().italic().to_string())
                    );
                }
                ThreadItem::ContextCompaction(_) => {
                    println!(
                        "{}",
                        "Context compacted".if_supports_color(Stdout, |t| t.dimmed().to_string())
                    );
                }
                ThreadItem::UserMessage(_) => {}
                ThreadItem::Unknown(unk) => {
                    if let Some(ref ty) = unk.item_type {
                        println!(
                            "{}",
                            format!("Unknown item: {ty}")
                                .if_supports_color(Stdout, |t| t.dimmed().to_string())
                        );
                    }
                }
            },
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipped_text_normalizes_whitespace_without_clipping() {
        assert_eq!(
            clipped_text("  keep\nthese\tinstructions readable  ", 64),
            "keep these instructions readable"
        );
    }

    #[test]
    fn clipped_text_truncates_on_char_boundary() {
        assert_eq!(clipped_text("abcdef ghij", 8), "abcdef g...");
    }
}
