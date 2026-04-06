use clap::Parser;
use codex_app_server_sdk::{
    CodexClient, CommandExecutionStatus, PatchApplyStatus, PatchChangeKind, ThreadEvent,
    ThreadItem, ThreadOptions, TurnOptions, WsConfig,
};
use owo_colors::{OwoColorize, Stream::Stdout};

#[derive(Parser)]
#[command(name = "agent", about = "Agent CLI using codex-app-server-sdk")]
struct Cli {
    /// The prompt to send to the model
    prompt: String,

    /// Show token usage and thread ID in output
    #[arg(long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = CodexClient::connect_ws(WsConfig::default()).await?;
    let mut thread = client.start_thread(
        ThreadOptions::builder()
            .model("gpt-5.3-codex-spark")
            .model_reasoning_effort(codex_app_server_sdk::ModelReasoningEffort::XHigh)
            .ephemeral(true)
            .skip_git_repo_check(true)
            .build(),
    );
    let mut streamed = thread
        .run_streamed(
            cli.prompt.as_str(),
            TurnOptions::default(),
        )
        .await?;
    while let Some(next) = streamed.next_event().await {
        match next? {
            ThreadEvent::TurnCompleted { usage } => {
                if cli.verbose {
                    if let Some(usage) = usage {
                        let input_tokens = usage.input_tokens;
                        let cached_input_tokens = usage.cached_input_tokens;
                        let output_tokens = usage.output_tokens;

                        println!(
                            "\n\n{}",
                            format!("---\nUsage:\n- Input tokens: {input_tokens}\n- Cached input tokens: {cached_input_tokens}\nOutput tokens: {output_tokens}")
                                .if_supports_color(Stdout, |t| t.dimmed().to_string())
                        );
                    }
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
            ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. }
            | ThreadEvent::ItemUpdated { .. } => {}
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
                    if reasoning.text.is_empty() {
                        println!(
                            "{}\n",
                            "Thinking..."
                                .if_supports_color(Stdout, |t| t.dimmed().italic().to_string())
                        );
                    } else {
                        println!(
                            "{}\n\t{}\n",
                            "Thinking..."
                                .if_supports_color(Stdout, |t| t.dimmed().italic().to_string()),
                            reasoning
                                .text
                                .if_supports_color(Stdout, |t| t.dimmed().to_string())
                        );
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
