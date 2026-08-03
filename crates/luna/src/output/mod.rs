mod json;

use std::env;
use std::io::{self, IsTerminal, Write};

use codex_app_server_sdk::{
    RenderedItem, RenderedItemKind, StreamedTurn, ThreadEvent, ThreadEventRenderer,
};
use owo_colors::{OwoColorize, Style};

use crate::error::LunaError;

pub(crate) use json::stream_json_events;
pub(crate) use json::thread_event_to_json;
#[cfg(test)]
pub(crate) use json::thread_item_to_json;

pub(crate) async fn stream_human_events(streamed: &mut StreamedTurn) -> Result<(), LunaError> {
    let stdout = io::stdout();
    let styling = Styling::detect(stdout.is_terminal());
    let mut output = HumanOutput::new(stdout.lock(), styling);
    let mut renderer = ThreadEventRenderer::new();
    let mut saw_terminal = false;

    while let Some(next) = streamed.next_event().await {
        let event = next?;
        if let Some(fragment) = renderer.render(&event) {
            output.write_fragment(&fragment)?;
        }

        match event {
            ThreadEvent::TurnCompleted { .. } => {
                saw_terminal = true;
                break;
            }
            ThreadEvent::TurnFailed { error } => {
                output.finish()?;
                return Err(LunaError::Turn(format!("turn failed: {}", error.message)));
            }
            ThreadEvent::Error { message } => {
                output.finish()?;
                return Err(LunaError::Turn(format!("stream error: {message}")));
            }
            ThreadEvent::ThreadStarted { .. }
            | ThreadEvent::TurnStarted
            | ThreadEvent::ItemStarted { .. }
            | ThreadEvent::ItemUpdated { .. }
            | ThreadEvent::ItemCompleted { .. } => {}
        }
    }

    if !saw_terminal {
        return Err(LunaError::Turn(
            "stream closed before receiving turn completion".to_string(),
        ));
    }

    output.finish()
}

#[derive(Debug, Clone, Copy)]
struct Styling {
    enabled: bool,
}

impl Styling {
    fn detect(is_terminal: bool) -> Self {
        Self::detect_with_no_color(is_terminal, env::var_os("NO_COLOR").is_some())
    }

    fn detect_with_no_color(is_terminal: bool, no_color: bool) -> Self {
        Self {
            enabled: is_terminal && !no_color,
        }
    }

    #[cfg(test)]
    fn plain() -> Self {
        Self { enabled: false }
    }

    fn render(self, kind: RenderedItemKind, text: &str) -> String {
        if !self.enabled {
            return text.to_string();
        }

        let style = match kind {
            RenderedItemKind::AgentMessage => Style::new().green(),
            RenderedItemKind::Reasoning => Style::new().dimmed().italic(),
            RenderedItemKind::Plan => Style::new().magenta(),
            RenderedItemKind::ToolCall
            | RenderedItemKind::CommandExecution
            | RenderedItemKind::FileChange => Style::new().cyan().bold(),
            RenderedItemKind::Error => Style::new().red().bold(),
            RenderedItemKind::Status | RenderedItemKind::Unknown => Style::new().yellow(),
            RenderedItemKind::UserMessage => Style::new().blue(),
        };
        text.style(style).to_string()
    }
}

pub(crate) fn format_error(message: &str) -> String {
    let styling = Styling::detect(io::stderr().is_terminal());
    styling.render(RenderedItemKind::Error, message)
}

struct HumanOutput<W> {
    writer: W,
    styling: Styling,
    current_item: Option<(RenderedItemKind, Option<String>)>,
    printed_any: bool,
    ended_with_newline: bool,
}

impl<W: Write> HumanOutput<W> {
    fn new(writer: W, styling: Styling) -> Self {
        Self {
            writer,
            styling,
            current_item: None,
            printed_any: false,
            ended_with_newline: false,
        }
    }

    fn write_fragment(&mut self, fragment: &RenderedItem) -> Result<(), LunaError> {
        if fragment.kind == RenderedItemKind::UserMessage {
            return Ok(());
        }

        let identity = (fragment.kind, fragment.item_id.clone());
        let same_streamed_item = fragment.continuation
            && self
                .current_item
                .as_ref()
                .is_some_and(|current| current == &identity);

        if !same_streamed_item {
            self.separate_blocks()?;
        }

        let mut markdown = fragment.markdown.clone();
        if fragment.continuation && !same_streamed_item {
            let prefix = match fragment.kind {
                RenderedItemKind::Reasoning => Some("Reasoning: "),
                RenderedItemKind::Plan => Some("Plan: "),
                _ => None,
            };
            if let Some(prefix) = prefix {
                markdown.insert_str(0, prefix);
            }
        }

        let styled = self.styling.render(fragment.kind, &markdown);
        write!(self.writer, "{styled}")?;
        self.writer.flush()?;
        self.printed_any = true;
        self.ended_with_newline = markdown.ends_with('\n');
        self.current_item = Some(identity);
        Ok(())
    }

    fn separate_blocks(&mut self) -> Result<(), LunaError> {
        if !self.printed_any {
            return Ok(());
        }
        if !self.ended_with_newline {
            writeln!(self.writer)?;
        }
        writeln!(self.writer)?;
        self.ended_with_newline = true;
        Ok(())
    }

    fn finish(&mut self) -> Result<(), LunaError> {
        if self.printed_any && !self.ended_with_newline {
            writeln!(self.writer)?;
            self.ended_with_newline = true;
        }
        self.writer.flush()?;
        Ok(())
    }
}

pub(crate) fn print_final_response<W: Write>(
    writer: &mut W,
    response: &str,
) -> Result<(), LunaError> {
    if !response.is_empty() {
        write!(writer, "{response}")?;
        writer.flush()?;
        if !response.ends_with('\n') {
            writeln!(writer)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use codex_app_server_sdk::{AgentMessageItem, ReasoningItem, ThreadItem};

    use super::*;

    #[test]
    fn human_output_separates_kinds_and_joins_stream_chunks() {
        let mut bytes = Vec::new();
        let mut output = HumanOutput::new(&mut bytes, Styling::plain());
        output
            .write_fragment(&RenderedItem {
                kind: RenderedItemKind::Reasoning,
                item_id: Some("r".to_string()),
                markdown: "checking".to_string(),
                continuation: true,
            })
            .expect("first reasoning chunk");
        output
            .write_fragment(&RenderedItem {
                kind: RenderedItemKind::Reasoning,
                item_id: Some("r".to_string()),
                markdown: " files".to_string(),
                continuation: true,
            })
            .expect("second reasoning chunk");
        output
            .write_fragment(&RenderedItem {
                kind: RenderedItemKind::AgentMessage,
                item_id: Some("a".to_string()),
                markdown: "done".to_string(),
                continuation: true,
            })
            .expect("agent chunk");
        output.finish().expect("finish");

        assert_eq!(
            String::from_utf8(bytes).expect("utf8"),
            "Reasoning: checking files\n\ndone\n"
        );
    }

    #[test]
    fn human_output_omits_user_message_echoes() {
        let mut bytes = Vec::new();
        let mut output = HumanOutput::new(&mut bytes, Styling::plain());
        output
            .write_fragment(&RenderedItem {
                kind: RenderedItemKind::UserMessage,
                item_id: Some("u".to_string()),
                markdown: "**User**\n\ndo not echo this prompt".to_string(),
                continuation: false,
            })
            .expect("user message");
        output
            .write_fragment(&RenderedItem {
                kind: RenderedItemKind::AgentMessage,
                item_id: Some("a".to_string()),
                markdown: "answer".to_string(),
                continuation: true,
            })
            .expect("agent message");
        output.finish().expect("finish");

        assert_eq!(String::from_utf8(bytes).expect("utf8"), "answer\n");
    }

    #[test]
    fn plain_styling_never_emits_ansi() {
        let rendered = Styling::plain().render(RenderedItemKind::ToolCall, "**Tool** search");
        assert_eq!(rendered, "**Tool** search");
        assert!(!rendered.contains("\u{1b}["));
    }

    #[test]
    fn styling_is_disabled_for_no_color_or_non_tty_output() {
        assert!(!Styling::detect_with_no_color(false, false).enabled);
        assert!(!Styling::detect_with_no_color(true, true).enabled);
        assert!(Styling::detect_with_no_color(true, false).enabled);
    }

    #[test]
    fn sdk_renderer_integration_deduplicates_completed_text() {
        let mut renderer = ThreadEventRenderer::new();
        let updated = renderer
            .render(&ThreadEvent::ItemUpdated {
                item: ThreadItem::AgentMessage(AgentMessageItem {
                    id: "a".to_string(),
                    text: "done".to_string(),
                    phase: None,
                }),
            })
            .expect("updated");
        assert_eq!(updated.markdown, "done");
        assert!(
            renderer
                .render(&ThreadEvent::ItemCompleted {
                    item: ThreadItem::AgentMessage(AgentMessageItem {
                        id: "a".to_string(),
                        text: "done".to_string(),
                        phase: None,
                    }),
                })
                .is_none()
        );

        let reasoning = renderer
            .render(&ThreadEvent::ItemCompleted {
                item: ThreadItem::Reasoning(ReasoningItem {
                    id: "r".to_string(),
                    text: "summary".to_string(),
                }),
            })
            .expect("reasoning");
        assert_eq!(reasoning.kind, RenderedItemKind::Reasoning);
    }
}
