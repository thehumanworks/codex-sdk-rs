//! Terminal-agnostic rendering of thread items and streamed thread events.

use std::collections::HashSet;

use serde_json::Value;

use crate::api::{
    CommandExecutionStatus, McpToolCallStatus, PatchApplyStatus, PatchChangeKind, ThreadEvent,
    ThreadItem, UserMessageContentItem,
};

/// Semantic category for a rendered fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderedItemKind {
    AgentMessage,
    UserMessage,
    Reasoning,
    Plan,
    ToolCall,
    CommandExecution,
    FileChange,
    Status,
    Error,
    Unknown,
}

/// A markdown fragment produced from an SDK thread item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedItem {
    pub kind: RenderedItemKind,
    pub item_id: Option<String>,
    pub markdown: String,
    /// True when this fragment continues a previously streamed item.
    pub continuation: bool,
}

impl RenderedItem {
    fn new(kind: RenderedItemKind, item_id: Option<&str>, markdown: String) -> Self {
        Self {
            kind,
            item_id: item_id.map(str::to_string),
            markdown,
            continuation: false,
        }
    }
}

/// Renders one complete thread item as concise markdown.
///
/// Every thread item variant produces a fragment, including unknown items.
/// Items with no displayable content can produce an empty markdown fragment.
pub fn render_thread_item(item: &ThreadItem) -> RenderedItem {
    match item {
        ThreadItem::AgentMessage(item) => RenderedItem::new(
            RenderedItemKind::AgentMessage,
            Some(&item.id),
            item.text.clone(),
        ),
        ThreadItem::UserMessage(item) => {
            let content = item
                .content
                .iter()
                .map(|content| match content {
                    UserMessageContentItem::Text { text } => text.clone(),
                    UserMessageContentItem::Image { url } => format!("[image]({url})"),
                    UserMessageContentItem::LocalImage { path } => format!("local image: {path}"),
                    UserMessageContentItem::Unknown(value) => {
                        format!("unknown content: {}", compact_json(value))
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            RenderedItem::new(
                RenderedItemKind::UserMessage,
                Some(&item.id),
                format!("**User**\n\n{content}"),
            )
        }
        ThreadItem::Plan(item) => RenderedItem::new(
            RenderedItemKind::Plan,
            Some(&item.id),
            format!("**Plan**\n\n{}", item.text),
        ),
        ThreadItem::Reasoning(item) => RenderedItem::new(
            RenderedItemKind::Reasoning,
            Some(&item.id),
            quote_block("Reasoning", &item.text),
        ),
        ThreadItem::CommandExecution(item) => {
            let mut markdown = format!(
                "**Command** {} — {}",
                one_line(&item.command),
                command_status(item.status)
            );
            if let Some(exit_code) = item.exit_code {
                markdown.push_str(&format!(" (exit {exit_code})"));
            }
            if !item.aggregated_output.is_empty() {
                markdown.push_str(&format!(
                    "\n\n    {}",
                    item.aggregated_output.replace('\n', "\n    ")
                ));
            }
            RenderedItem::new(RenderedItemKind::CommandExecution, Some(&item.id), markdown)
        }
        ThreadItem::FileChange(item) => {
            let changes = if item.changes.is_empty() {
                "no paths reported".to_string()
            } else {
                item.changes
                    .iter()
                    .map(|change| {
                        format!(
                            "- {} {}",
                            patch_change_kind(change.kind),
                            one_line(&change.path)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            RenderedItem::new(
                RenderedItemKind::FileChange,
                Some(&item.id),
                format!(
                    "**File changes** — {}\n\n{changes}",
                    patch_status(item.status)
                ),
            )
        }
        ThreadItem::McpToolCall(item) => {
            let mut markdown = format!(
                "**Tool** {}.{} — {}\n\nArguments: {}",
                one_line(&item.server),
                one_line(&item.tool),
                mcp_status(item.status),
                compact_json(&item.arguments)
            );
            if let Some(result) = &item.result {
                markdown.push_str(&format!("\n\nResult: {}", compact_json(result)));
            }
            if let Some(error) = &item.error {
                markdown.push_str(&format!("\n\nError: {}", error.message));
            }
            RenderedItem::new(RenderedItemKind::ToolCall, Some(&item.id), markdown)
        }
        ThreadItem::DynamicToolCall(item) => {
            let mut details = vec![format!("status: {}", display_or_unknown(&item.status))];
            if let Some(success) = item.success {
                details.push(format!("success: {success}"));
            }
            if let Some(duration_ms) = item.duration_ms {
                details.push(format!("{duration_ms} ms"));
            }
            let mut markdown = format!(
                "**Tool** {} — {}\n\nArguments: {}",
                one_line(&item.tool),
                details.join(", "),
                compact_json(&item.arguments)
            );
            if !item.content_items.is_empty() {
                markdown.push_str(&format!(
                    "\n\nResult: {}",
                    compact_json(&Value::Array(item.content_items.clone()))
                ));
            }
            RenderedItem::new(RenderedItemKind::ToolCall, Some(&item.id), markdown)
        }
        ThreadItem::CollabToolCall(item) => {
            let mut details = vec![format!("status: {}", display_or_unknown(&item.status))];
            if let Some(agent_status) = &item.agent_status {
                details.push(format!("agent: {agent_status}"));
            }
            let mut markdown = format!(
                "**Collaboration** {} — {}",
                one_line(&item.tool),
                details.join(", ")
            );
            if let Some(prompt) = &item.prompt {
                markdown.push_str(&format!("\n\nPrompt: {prompt}"));
            }
            if let Some(thread_id) = item
                .new_thread_id
                .as_ref()
                .or(item.receiver_thread_id.as_ref())
            {
                markdown.push_str(&format!("\n\nThread: {}", one_line(thread_id)));
            }
            RenderedItem::new(RenderedItemKind::ToolCall, Some(&item.id), markdown)
        }
        ThreadItem::WebSearch(item) => RenderedItem::new(
            RenderedItemKind::ToolCall,
            Some(&item.id),
            format!("**Web search** — {}", one_line(&item.query)),
        ),
        ThreadItem::ImageView(item) => RenderedItem::new(
            RenderedItemKind::ToolCall,
            Some(&item.id),
            format!("**Viewed image** — {}", one_line(&item.path)),
        ),
        ThreadItem::EnteredReviewMode(item) => RenderedItem::new(
            RenderedItemKind::Status,
            Some(&item.id),
            format!(
                "**Entered review mode** — {}",
                display_or_unknown(&item.review)
            ),
        ),
        ThreadItem::ExitedReviewMode(item) => RenderedItem::new(
            RenderedItemKind::Status,
            Some(&item.id),
            format!(
                "**Exited review mode** — {}",
                display_or_unknown(&item.review)
            ),
        ),
        ThreadItem::ContextCompaction(item) => RenderedItem::new(
            RenderedItemKind::Status,
            Some(&item.id),
            "**Context compacted**".to_string(),
        ),
        ThreadItem::TodoList(item) => {
            let items = if item.items.is_empty() {
                "- (empty)".to_string()
            } else {
                item.items
                    .iter()
                    .map(|todo| {
                        format!(
                            "- [{}] {}",
                            if todo.completed { "x" } else { " " },
                            todo.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            RenderedItem::new(
                RenderedItemKind::Plan,
                Some(&item.id),
                format!("**Todo list**\n\n{items}"),
            )
        }
        ThreadItem::Error(item) => RenderedItem::new(
            RenderedItemKind::Error,
            Some(&item.id),
            format!("**Error:** {}", display_or_unknown(&item.message)),
        ),
        ThreadItem::Unknown(item) => {
            let label = item.item_type.as_deref().unwrap_or("unknown");
            RenderedItem::new(
                RenderedItemKind::Unknown,
                item.id.as_deref(),
                format!(
                    "**Unknown item** {} — {}",
                    one_line(label),
                    compact_json(&item.raw)
                ),
            )
        }
    }
}

/// Stateful renderer for streamed events.
///
/// Text deltas are emitted immediately. Their matching completed snapshots are
/// suppressed, preventing duplicated agent, reasoning, and plan text.
#[derive(Debug, Default)]
pub struct ThreadEventRenderer {
    streamed_items: HashSet<(RenderedItemKind, String)>,
}

impl ThreadEventRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn render(&mut self, event: &ThreadEvent) -> Option<RenderedItem> {
        match event {
            ThreadEvent::ItemUpdated { item } => {
                let mut rendered = match item {
                    ThreadItem::AgentMessage(item) => RenderedItem::new(
                        RenderedItemKind::AgentMessage,
                        Some(&item.id),
                        item.text.clone(),
                    ),
                    ThreadItem::Reasoning(item) => RenderedItem::new(
                        RenderedItemKind::Reasoning,
                        Some(&item.id),
                        item.text.clone(),
                    ),
                    ThreadItem::Plan(item) => {
                        RenderedItem::new(RenderedItemKind::Plan, Some(&item.id), item.text.clone())
                    }
                    _ => render_thread_item(item),
                };
                if rendered.markdown.is_empty() {
                    return None;
                }
                rendered.continuation = true;
                if let Some(item_id) = &rendered.item_id {
                    self.streamed_items.insert((rendered.kind, item_id.clone()));
                }
                Some(rendered)
            }
            ThreadEvent::ItemCompleted { item } => {
                let rendered = render_thread_item(item);
                let was_streamed = rendered.item_id.as_ref().is_some_and(|item_id| {
                    self.streamed_items
                        .remove(&(rendered.kind, item_id.clone()))
                });
                (!was_streamed && !rendered.markdown.is_empty()).then_some(rendered)
            }
            ThreadEvent::Error { message } => Some(RenderedItem::new(
                RenderedItemKind::Error,
                None,
                format!("**Error:** {}", display_or_unknown(message)),
            )),
            ThreadEvent::TurnFailed { error } => Some(RenderedItem::new(
                RenderedItemKind::Error,
                None,
                format!("**Turn failed:** {}", display_or_unknown(&error.message)),
            )),
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::TurnCompleted { .. }
            | ThreadEvent::ItemStarted { .. } => None,
        }
    }
}

fn quote_block(label: &str, text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    format!(
        "> **{label}**\n> {}",
        text.lines().collect::<Vec<_>>().join("\n> ")
    )
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn one_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn display_or_unknown(value: &str) -> &str {
    if value.is_empty() { "unknown" } else { value }
}

fn command_status(status: CommandExecutionStatus) -> &'static str {
    match status {
        CommandExecutionStatus::InProgress => "running",
        CommandExecutionStatus::Completed => "completed",
        CommandExecutionStatus::Failed => "failed",
        CommandExecutionStatus::Declined => "declined",
        CommandExecutionStatus::Unknown => "unknown status",
    }
}

fn patch_status(status: PatchApplyStatus) -> &'static str {
    match status {
        PatchApplyStatus::InProgress => "applying",
        PatchApplyStatus::Completed => "applied",
        PatchApplyStatus::Failed => "failed",
        PatchApplyStatus::Declined => "declined",
        PatchApplyStatus::Unknown => "unknown status",
    }
}

fn patch_change_kind(kind: PatchChangeKind) -> &'static str {
    match kind {
        PatchChangeKind::Add => "added",
        PatchChangeKind::Delete => "deleted",
        PatchChangeKind::Update => "updated",
        PatchChangeKind::Unknown => "changed",
    }
}

fn mcp_status(status: McpToolCallStatus) -> &'static str {
    match status {
        McpToolCallStatus::InProgress => "running",
        McpToolCallStatus::Completed => "completed",
        McpToolCallStatus::Failed => "failed",
        McpToolCallStatus::Unknown => "unknown status",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::api::*;

    fn assert_rendered(item: ThreadItem, kind: RenderedItemKind, needles: &[&str]) {
        let rendered = render_thread_item(&item);
        assert_eq!(rendered.kind, kind);
        assert!(!rendered.markdown.is_empty());
        for needle in needles {
            assert!(
                rendered.markdown.contains(needle),
                "missing {needle:?} in {:?}",
                rendered.markdown
            );
        }
    }

    #[test]
    fn renders_every_thread_item_variant() {
        let cases = vec![
            (
                ThreadItem::AgentMessage(AgentMessageItem {
                    id: "a".into(),
                    text: "hello".into(),
                    phase: None,
                }),
                RenderedItemKind::AgentMessage,
                vec!["hello"],
            ),
            (
                ThreadItem::UserMessage(UserMessageItem {
                    id: "u".into(),
                    content: vec![
                        UserMessageContentItem::Text {
                            text: "prompt".into(),
                        },
                        UserMessageContentItem::Image {
                            url: "https://example/image".into(),
                        },
                        UserMessageContentItem::LocalImage {
                            path: "/tmp/a.png".into(),
                        },
                        UserMessageContentItem::Unknown(json!({"x": 1})),
                    ],
                }),
                RenderedItemKind::UserMessage,
                vec!["prompt", "image", "local image", "unknown content"],
            ),
            (
                ThreadItem::Plan(PlanItem {
                    id: "p".into(),
                    text: "step".into(),
                }),
                RenderedItemKind::Plan,
                vec!["Plan", "step"],
            ),
            (
                ThreadItem::Reasoning(ReasoningItem {
                    id: "r".into(),
                    text: "because".into(),
                }),
                RenderedItemKind::Reasoning,
                vec!["Reasoning", "> because"],
            ),
            (
                ThreadItem::CommandExecution(CommandExecutionItem {
                    id: "c".into(),
                    command: "ls".into(),
                    aggregated_output: "file".into(),
                    exit_code: Some(0),
                    status: CommandExecutionStatus::Completed,
                }),
                RenderedItemKind::CommandExecution,
                vec!["Command", "ls", "exit 0", "file"],
            ),
            (
                ThreadItem::FileChange(FileChangeItem {
                    id: "f".into(),
                    changes: vec![FileUpdateChange {
                        path: "src/lib.rs".into(),
                        kind: PatchChangeKind::Update,
                    }],
                    status: PatchApplyStatus::Completed,
                }),
                RenderedItemKind::FileChange,
                vec!["File changes", "updated", "src/lib.rs"],
            ),
            (
                ThreadItem::McpToolCall(McpToolCallItem {
                    id: "m".into(),
                    server: "docs".into(),
                    tool: "search".into(),
                    arguments: json!({"q":"rust"}),
                    result: Some(json!({"hits":1})),
                    error: None,
                    status: McpToolCallStatus::Completed,
                }),
                RenderedItemKind::ToolCall,
                vec!["docs.search", "Arguments", "Result"],
            ),
            (
                ThreadItem::DynamicToolCall(DynamicToolCallItem {
                    id: "d".into(),
                    tool: "lookup".into(),
                    arguments: json!({}),
                    status: "completed".into(),
                    content_items: vec![json!("ok")],
                    success: Some(true),
                    duration_ms: Some(12),
                }),
                RenderedItemKind::ToolCall,
                vec!["lookup", "success: true", "12 ms", "Result"],
            ),
            (
                ThreadItem::CollabToolCall(CollabToolCallItem {
                    id: "co".into(),
                    tool: "spawn_agent".into(),
                    status: "completed".into(),
                    sender_thread_id: "one".into(),
                    receiver_thread_id: None,
                    new_thread_id: Some("two".into()),
                    prompt: Some("inspect".into()),
                    agent_status: Some("done".into()),
                }),
                RenderedItemKind::ToolCall,
                vec!["Collaboration", "inspect", "two"],
            ),
            (
                ThreadItem::WebSearch(WebSearchItem {
                    id: "w".into(),
                    query: "Rust docs".into(),
                }),
                RenderedItemKind::ToolCall,
                vec!["Web search", "Rust docs"],
            ),
            (
                ThreadItem::ImageView(ImageViewItem {
                    id: "i".into(),
                    path: "/tmp/i.png".into(),
                }),
                RenderedItemKind::ToolCall,
                vec!["Viewed image", "/tmp/i.png"],
            ),
            (
                ThreadItem::EnteredReviewMode(ReviewModeItem {
                    id: "er".into(),
                    review: "code".into(),
                }),
                RenderedItemKind::Status,
                vec!["Entered review mode", "code"],
            ),
            (
                ThreadItem::ExitedReviewMode(ReviewModeItem {
                    id: "xr".into(),
                    review: "code".into(),
                }),
                RenderedItemKind::Status,
                vec!["Exited review mode", "code"],
            ),
            (
                ThreadItem::ContextCompaction(ContextCompactionItem { id: "cc".into() }),
                RenderedItemKind::Status,
                vec!["Context compacted"],
            ),
            (
                ThreadItem::TodoList(TodoListItem {
                    id: "t".into(),
                    items: vec![
                        TodoItem {
                            text: "done".into(),
                            completed: true,
                        },
                        TodoItem {
                            text: "next".into(),
                            completed: false,
                        },
                    ],
                }),
                RenderedItemKind::Plan,
                vec!["[x] done", "[ ] next"],
            ),
            (
                ThreadItem::Error(ErrorItem {
                    id: "e".into(),
                    message: "bad".into(),
                }),
                RenderedItemKind::Error,
                vec!["Error", "bad"],
            ),
            (
                ThreadItem::Unknown(UnknownItem {
                    id: None,
                    item_type: None,
                    raw: json!({"future": true}),
                }),
                RenderedItemKind::Unknown,
                vec!["Unknown item", "future"],
            ),
        ];
        for (item, kind, needles) in cases {
            assert_rendered(item, kind, &needles);
        }
    }

    #[test]
    fn renders_empty_and_missing_fields_with_visible_fallbacks() {
        assert_rendered(
            ThreadItem::FileChange(FileChangeItem {
                id: String::new(),
                changes: vec![],
                status: PatchApplyStatus::Unknown,
            }),
            RenderedItemKind::FileChange,
            &["no paths reported", "unknown status"],
        );
        assert_rendered(
            ThreadItem::Error(ErrorItem {
                id: String::new(),
                message: String::new(),
            }),
            RenderedItemKind::Error,
            &["unknown"],
        );
        assert_rendered(
            ThreadItem::Unknown(UnknownItem {
                id: None,
                item_type: None,
                raw: Value::Null,
            }),
            RenderedItemKind::Unknown,
            &["unknown", "null"],
        );
    }

    #[test]
    fn streamed_text_is_emitted_once_across_multiple_chunks_and_completion() {
        let mut renderer = ThreadEventRenderer::new();
        for text in ["hel", "lo"] {
            let fragment = renderer
                .render(&ThreadEvent::ItemUpdated {
                    item: ThreadItem::AgentMessage(AgentMessageItem {
                        id: "a".into(),
                        text: text.into(),
                        phase: None,
                    }),
                })
                .expect("delta");
            assert_eq!(fragment.markdown, text);
            assert!(fragment.continuation);
        }
        let completed = renderer.render(&ThreadEvent::ItemCompleted {
            item: ThreadItem::AgentMessage(AgentMessageItem {
                id: "a".into(),
                text: "hello".into(),
                phase: Some(AgentMessagePhase::FinalAnswer),
            }),
        });
        assert!(completed.is_none());
    }

    #[test]
    fn completed_items_render_when_no_delta_was_seen() {
        let mut renderer = ThreadEventRenderer::new();
        let fragment = renderer
            .render(&ThreadEvent::ItemCompleted {
                item: ThreadItem::Reasoning(ReasoningItem {
                    id: "r".into(),
                    text: "summary".into(),
                }),
            })
            .expect("completed item");
        assert_eq!(fragment.kind, RenderedItemKind::Reasoning);
        assert!(fragment.markdown.contains("summary"));
        assert!(!fragment.continuation);
    }

    #[test]
    fn empty_reasoning_items_are_not_rendered() {
        let mut renderer = ThreadEventRenderer::new();

        for (id, text) in [("empty", ""), ("whitespace", " \n\t")] {
            let item = ThreadItem::Reasoning(ReasoningItem {
                id: id.into(),
                text: text.into(),
            });

            assert!(render_thread_item(&item).markdown.is_empty());
            assert!(
                renderer
                    .render(&ThreadEvent::ItemCompleted { item })
                    .is_none()
            );
        }
    }

    #[test]
    fn stream_dedup_is_scoped_by_kind_and_item_id() {
        let mut renderer = ThreadEventRenderer::new();
        let _ = renderer.render(&ThreadEvent::ItemUpdated {
            item: ThreadItem::Reasoning(ReasoningItem {
                id: "same".into(),
                text: "why".into(),
            }),
        });
        let agent = renderer
            .render(&ThreadEvent::ItemCompleted {
                item: ThreadItem::AgentMessage(AgentMessageItem {
                    id: "same".into(),
                    text: "answer".into(),
                    phase: None,
                }),
            })
            .expect("different kind");
        assert_eq!(agent.markdown, "answer");
    }
}
