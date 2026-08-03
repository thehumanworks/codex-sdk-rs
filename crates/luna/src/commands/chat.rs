use std::collections::BTreeSet;
use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Duration;

use codex_app_server_sdk::{
    ModelReasoningEffort, RenderedItem, RenderedItemKind, Thread, ThreadEvent, ThreadEventRenderer,
    ThreadItem, TurnOptions, requests, responses,
};
use crossterm::cursor::MoveTo;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::turn;
use crate::cli::CliArgs;
use crate::error::LunaError;
use crate::output;

const DEFAULT_MODEL: &str = "gpt-5.6-luna";
const EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

const COMMANDS: [(&str, &str); 7] = [
    ("/help", "show chat commands and keys"),
    ("/compact", "compact this conversation's context"),
    ("/effort", "show or set reasoning effort"),
    ("/skills", "list available Codex skills"),
    ("/skill", "insert a skill mention"),
    ("/clear", "clear the visible transcript"),
    ("/quit", "leave Luna chat"),
];

pub(super) async fn run(mut cli: CliArgs) -> Result<ExitCode, LunaError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(LunaError::Usage(
            "`luna chat` requires an interactive terminal; use `luna exec` for pipes and scripts"
                .to_string(),
        ));
    }

    let initial_prompt =
        (!cli.prompt_parts.is_empty()).then(|| std::mem::take(&mut cli.prompt_parts).join(" "));
    let final_response_only = cli.final_response_only;
    let json_output = cli.json_output;
    let initial_model = cli
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let initial_effort = cli.reasoning_effort.unwrap_or(ModelReasoningEffort::Max);

    execute!(
        io::stdout(),
        Clear(ClearType::All),
        Clear(ClearType::Purge),
        MoveTo(0, 0)
    )?;
    let mut terminal = ratatui::try_init()?;
    let result = run_in_terminal(
        &mut terminal,
        cli,
        initial_prompt,
        initial_model,
        initial_effort,
        final_response_only,
        json_output,
    )
    .await;
    let restore_result = ratatui::try_restore();

    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(exit_code), Ok(())) => Ok(exit_code),
        (Ok(_), Err(error)) => Err(LunaError::Io(error)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_in_terminal(
    terminal: &mut ratatui::DefaultTerminal,
    cli: CliArgs,
    initial_prompt: Option<String>,
    initial_model: String,
    initial_effort: ModelReasoningEffort,
    final_response_only: bool,
    json_output: bool,
) -> Result<ExitCode, LunaError> {
    let mut app = ChatApp::new(
        initial_model,
        initial_effort,
        final_response_only,
        json_output,
    );
    app.status = "connecting to Codex".to_string();
    app.busy = true;
    terminal.draw(|frame| app.render(frame))?;

    let prepared = turn::prepare(cli).await?;
    app.model = prepared.model.clone();
    app.effort = prepared.reasoning_effort;
    app.status = "loading skills".to_string();
    terminal.draw(|frame| app.render(frame))?;

    let (skills, warnings) = load_metadata(&prepared.codex, &prepared.working_directory).await;
    app.skills = skills;
    app.metadata_warning = warnings.join("; ");
    app.status = "ready".to_string();
    app.busy = false;

    let turn::PreparedTurn {
        mut thread,
        turn_options,
        ..
    } = prepared;
    run_chat_loop(terminal, &mut thread, turn_options, app, initial_prompt).await?;
    Ok(ExitCode::SUCCESS)
}

async fn run_chat_loop(
    terminal: &mut ratatui::DefaultTerminal,
    thread: &mut Thread,
    base_turn_options: TurnOptions,
    mut app: ChatApp,
    mut pending_prompt: Option<String>,
) -> Result<(), LunaError> {
    let mut events = EventStream::new();
    let mut ticks = tokio::time::interval(Duration::from_millis(120));

    loop {
        terminal.draw(|frame| app.render(frame))?;

        if let Some(prompt) = pending_prompt.take() {
            if !prompt.trim().is_empty() {
                run_prompt(
                    terminal,
                    &mut events,
                    &mut app,
                    thread,
                    &base_turn_options,
                    prompt,
                )
                .await?;
            }
            continue;
        }

        tokio::select! {
            _ = ticks.tick() => app.tick = app.tick.wrapping_add(1),
            event = events.next() => {
                let event = event.ok_or_else(|| {
                    LunaError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream closed"))
                })??;
                match event {
                    Event::Key(key) if accepts_key(key) => match app.handle_idle_key(key) {
                        IdleAction::None => {}
                        IdleAction::Quit => return Ok(()),
                        IdleAction::Submit(input) => match parse_input(&input) {
                            ParsedInput::Empty => {}
                            ParsedInput::Prompt(prompt) => pending_prompt = Some(prompt),
                            ParsedInput::Command(command) => {
                                if execute_command(&mut app, thread, command).await? {
                                    return Ok(());
                                }
                            }
                        },
                    },
                    Event::Paste(text) => app.input.insert_str(&single_line(&text)),
                    Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
                    _ => {}
                }
            }
        }
    }
}

async fn run_prompt(
    terminal: &mut ratatui::DefaultTerminal,
    events: &mut EventStream,
    app: &mut ChatApp,
    thread: &mut Thread,
    base_turn_options: &TurnOptions,
    prompt: String,
) -> Result<(), LunaError> {
    app.submit_user_message(prompt.clone());
    let mut turn_options = base_turn_options.clone();
    turn_options.model = Some(app.model.clone());
    turn_options.model_reasoning_effort = Some(app.effort);

    let mut streamed = match thread.run_streamed(prompt, turn_options).await {
        Ok(streamed) => streamed,
        Err(error) => {
            app.finish_turn_with_error(format!("could not start turn: {error}"));
            return Ok(());
        }
    };
    let turn_id = streamed.turn_id().to_string();
    let mut interrupt_requested = false;

    loop {
        terminal.draw(|frame| app.render(frame))?;
        tokio::select! {
            next = streamed.next_event() => {
                let Some(next) = next else {
                    app.finish_turn_with_error("stream closed before turn completion".to_string());
                    return Ok(());
                };
                let event = next?;
                let terminal_event = match &event {
                    ThreadEvent::TurnCompleted { .. } => Some(Ok(())),
                    ThreadEvent::TurnFailed { error } => Some(Err(format!("turn failed: {}", error.message))),
                    ThreadEvent::Error { message } => Some(Err(format!("stream error: {message}"))),
                    _ => None,
                };
                app.consume_thread_event(&event)?;
                if let Some(result) = terminal_event {
                    match result {
                        Ok(()) => app.finish_turn(),
                        Err(message) => app.finish_turn_with_error(message),
                    }
                    return Ok(());
                }
            }
            event = events.next() => {
                let event = event.ok_or_else(|| {
                    LunaError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream closed"))
                })??;
                match event {
                    Event::Key(key) if accepts_key(key) && is_ctrl(key, 'c') => {
                        if interrupt_requested {
                            app.status = "interrupt already requested".to_string();
                        } else {
                            match thread.interrupt(turn_id.clone()).await {
                                Ok(()) => {
                                    interrupt_requested = true;
                                    app.status = "interrupt requested".to_string();
                                }
                                Err(error) => app.append_error(format!("could not interrupt turn: {error}")),
                            }
                        }
                    }
                    Event::Key(key) if accepts_key(key) => app.handle_scrolling_key(key),
                    Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
                    Event::Paste(_) | Event::Key(_) => {}
                }
            }
        }
    }
}

async fn execute_command(
    app: &mut ChatApp,
    thread: &mut Thread,
    command: ChatCommand,
) -> Result<bool, LunaError> {
    match command {
        ChatCommand::Help => app.append_status(help_text()),
        ChatCommand::Compact => {
            app.status = "requesting compaction".to_string();
            match thread.compact_start().await {
                Ok(_) => app.append_status("Compaction requested."),
                Err(error) => app.append_error(format!("Compaction failed: {error}")),
            }
            app.status = "ready".to_string();
        }
        ChatCommand::ShowEffort => app.append_status(format!(
            "Current reasoning effort: {}\nAvailable: {}",
            app.effort.as_str(),
            EFFORTS.join(", ")
        )),
        ChatCommand::SetEffort(effort) => {
            app.effort = effort;
            app.append_status(format!(
                "Reasoning effort set to {} for subsequent turns.",
                effort.as_str()
            ));
        }
        ChatCommand::Skills => {
            let message = if app.skills.is_empty() {
                "No enabled skills were reported for this working directory.".to_string()
            } else {
                format!(
                    "Enabled skills (type `$` or `/skill ` to complete):\n{}",
                    app.skills.join("\n")
                )
            };
            app.append_status(message);
        }
        ChatCommand::InsertSkill(skill) => {
            if !app.skills.is_empty() && !app.skills.iter().any(|name| name == &skill) {
                app.append_error(format!("Unknown skill: {skill}"));
            } else {
                app.input.set(format!("${skill} "));
                app.append_status(format!("Inserted skill mention `${skill}`."));
            }
        }
        ChatCommand::Clear => app.clear_transcript(),
        ChatCommand::Quit => return Ok(true),
        ChatCommand::Unknown(command) => app.append_error(format!(
            "Unknown chat command `{command}`. Type `/help` for available commands; use `//` to send a prompt beginning with `/`."
        )),
    }
    Ok(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedInput {
    Empty,
    Prompt(String),
    Command(ChatCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChatCommand {
    Help,
    Compact,
    ShowEffort,
    SetEffort(ModelReasoningEffort),
    Skills,
    InsertSkill(String),
    Clear,
    Quit,
    Unknown(String),
}

fn parse_input(raw: &str) -> ParsedInput {
    let input = raw.trim();
    if input.is_empty() {
        return ParsedInput::Empty;
    }
    if let Some(literal) = input.strip_prefix("//") {
        return ParsedInput::Prompt(format!("/{literal}"));
    }
    if !input.starts_with('/') {
        return ParsedInput::Prompt(input.to_string());
    }

    let (command, argument) = input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(command, argument)| {
            (command, argument.trim())
        });
    let parsed = match command.to_ascii_lowercase().as_str() {
        "/help" => ChatCommand::Help,
        "/compact" => ChatCommand::Compact,
        "/effort" if argument.is_empty() => ChatCommand::ShowEffort,
        "/effort" => match ModelReasoningEffort::from_str(argument) {
            Ok(effort) => ChatCommand::SetEffort(effort),
            Err(_) => ChatCommand::Unknown(input.to_string()),
        },
        "/skills" => ChatCommand::Skills,
        "/skill" if !argument.is_empty() => ChatCommand::InsertSkill(argument.to_string()),
        "/clear" => ChatCommand::Clear,
        "/quit" | "/exit" => ChatCommand::Quit,
        _ => ChatCommand::Unknown(input.to_string()),
    };
    ParsedInput::Command(parsed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Suggestion {
    replacement: String,
    display: String,
    description: String,
}

async fn load_metadata(
    codex: &codex_app_server_sdk::Codex,
    working_directory: &str,
) -> (Vec<String>, Vec<String>) {
    let skills = codex
        .skills_list(requests::SkillsListParams {
            cwd: Some(vec![working_directory.to_string()]),
            force_reload: Some(false),
            ..Default::default()
        })
        .await;

    let mut warnings = Vec::new();
    let skills = match skills {
        Ok(result) => {
            let (skills, skill_warnings) = enabled_skill_names(&result);
            warnings.extend(skill_warnings);
            skills
        }
        Err(error) => {
            warnings.push(format!("skills unavailable: {error}"));
            Vec::new()
        }
    };
    (skills, warnings)
}

fn enabled_skill_names(result: &responses::SkillsListResult) -> (Vec<String>, Vec<String>) {
    let mut names = BTreeSet::new();
    let mut warnings = Vec::new();
    let Some(groups) = result
        .extra
        .get("data")
        .and_then(serde_json::Value::as_array)
    else {
        return (Vec::new(), Vec::new());
    };
    for group in groups {
        if let Some(errors) = group.get("errors").and_then(serde_json::Value::as_array) {
            warnings.extend(errors.iter().filter_map(skill_error_message));
        }
        let Some(skills) = group.get("skills").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for skill in skills {
            if skill.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(name) = skill
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                names.insert(name.to_string());
            }
        }
    }
    (names.into_iter().collect(), warnings)
}

fn skill_error_message(error: &serde_json::Value) -> Option<String> {
    error
        .as_str()
        .or_else(|| error.get("message").and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| format!("skills warning: {message}"))
}

struct ChatApp {
    transcript: Vec<TranscriptBlock>,
    input: InputBuffer,
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    suggestions_dismissed: bool,
    selected_suggestion: usize,
    skills: Vec<String>,
    model: String,
    effort: ModelReasoningEffort,
    status: String,
    metadata_warning: String,
    busy: bool,
    turn_running: bool,
    tick: usize,
    scroll: usize,
    max_scroll: usize,
    follow_tail: bool,
    transcript_page: usize,
    renderer: ThreadEventRenderer,
    final_response_only: bool,
    json_output: bool,
    fallback_final_response: Option<String>,
    emitted_final_response: bool,
}

impl ChatApp {
    fn new(
        model: String,
        effort: ModelReasoningEffort,
        final_response_only: bool,
        json_output: bool,
    ) -> Self {
        Self {
            transcript: Vec::new(),
            input: InputBuffer::default(),
            history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            suggestions_dismissed: false,
            selected_suggestion: 0,
            skills: Vec::new(),
            model,
            effort,
            status: "starting".to_string(),
            metadata_warning: String::new(),
            busy: false,
            turn_running: false,
            tick: 0,
            scroll: 0,
            max_scroll: 0,
            follow_tail: true,
            transcript_page: 1,
            renderer: ThreadEventRenderer::new(),
            final_response_only,
            json_output,
            fallback_final_response: None,
            emitted_final_response: false,
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let suggestions = self.suggestions();
        if self.selected_suggestion >= suggestions.len() {
            self.selected_suggestion = 0;
        }
        let suggestion_height = if suggestions.is_empty() {
            0
        } else {
            (suggestions.len().min(5) as u16).saturating_add(2)
        };
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(suggestion_height),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

        self.render_header(frame, areas[0]);
        self.render_transcript(frame, areas[1]);
        if suggestion_height > 0 {
            self.render_suggestions(frame, areas[2], &suggestions);
        }
        self.render_input(frame, areas[3], &suggestions);
        self.render_footer(frame, areas[4]);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let spinner = [".", "o", "O", "o"][self.tick % 4];
        let state = if self.busy { spinner } else { "o" };
        let line = Line::from(vec![
            Span::styled(" LUNA ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "{state}  {}  |  effort {}  |  {}",
                    self.model,
                    self.effort.as_str(),
                    self.status
                ),
                Style::default().fg(Color::Gray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line).block(moon_block()), area);
    }

    fn render_transcript(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = moon_block().title(" Conversation ");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.transcript.is_empty() {
            frame.render_widget(splash(inner.height), inner);
            self.max_scroll = 0;
            self.scroll = 0;
            return;
        }

        let text = self.transcript_text();
        let width = inner.width.max(1) as usize;
        let height = wrapped_height(&text, width);
        self.transcript_page = inner.height.max(1) as usize;
        self.max_scroll = height.saturating_sub(self.transcript_page);
        if self.follow_tail {
            self.scroll = self.max_scroll;
        } else {
            self.scroll = self.scroll.min(self.max_scroll);
        }
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll.min(u16::MAX as usize) as u16, 0)),
            inner,
        );
    }

    fn render_suggestions(&self, frame: &mut Frame<'_>, area: Rect, suggestions: &[Suggestion]) {
        let items = suggestions
            .iter()
            .map(|suggestion| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:<18}", suggestion.display),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        suggestion.description.clone(),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(moon_block().title(" Suggestions  [Tab] accept "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        let mut state = ListState::default().with_selected(Some(self.selected_suggestion));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_input(&self, frame: &mut Frame<'_>, area: Rect, suggestions: &[Suggestion]) {
        let block = moon_block().title(if self.turn_running {
            " Turn running  [Ctrl-C] interrupt "
        } else if self.busy {
            " Starting Luna "
        } else {
            " Message "
        });
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let prefix = &self.input.text[..self.input.cursor];
        let cursor_width = UnicodeWidthStr::width(prefix);
        let visible_width = inner.width.max(1) as usize;
        let horizontal_scroll = cursor_width.saturating_sub(visible_width.saturating_sub(1));
        let ghost = suggestions
            .get(self.selected_suggestion)
            .filter(|_| self.input.cursor == self.input.text.len())
            .and_then(|suggestion| suggestion.replacement.strip_prefix(&self.input.text))
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::raw(self.input.text.clone()),
            Span::styled(ghost.to_string(), Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(
            Paragraph::new(line).scroll((0, horizontal_scroll.min(u16::MAX as usize) as u16)),
            inner,
        );
        if !self.busy {
            frame.set_cursor_position((
                inner.x + cursor_width.saturating_sub(horizontal_scroll) as u16,
                inner.y,
            ));
        }
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let hint = if self.metadata_warning.is_empty() {
            "Enter send  Tab complete  Up/Down choose or history  PgUp/PgDn scroll  Ctrl-D quit"
                .to_string()
        } else {
            format!("{}  |  {}", self.status, self.metadata_warning)
        };
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::default().fg(Color::DarkGray))
                .alignment(Alignment::Center),
            area,
        );
    }

    fn transcript_text(&self) -> Text<'static> {
        let mut lines = Vec::new();
        for block in &self.transcript {
            let (label, style) = transcript_style(block.kind);
            lines.push(Line::from(Span::styled(label, style)));
            if block.text.is_empty() {
                lines.push(Line::default());
            } else {
                lines.extend(block.text.lines().map(|line| {
                    Line::from(Span::styled(
                        line.to_string(),
                        style.remove_modifier(Modifier::BOLD),
                    ))
                }));
            }
            lines.push(Line::default());
        }
        Text::from(lines)
    }

    fn suggestions(&self) -> Vec<Suggestion> {
        if self.suggestions_dismissed || self.busy {
            return Vec::new();
        }
        suggestions_for(&self.input.text, &self.skills)
    }

    fn handle_idle_key(&mut self, key: KeyEvent) -> IdleAction {
        if is_ctrl(key, 'd') && self.input.text.is_empty() {
            return IdleAction::Quit;
        }
        if is_ctrl(key, 'c') {
            if self.input.text.is_empty() {
                return IdleAction::Quit;
            }
            self.input.clear();
            self.after_input_edit();
            return IdleAction::None;
        }
        if is_ctrl(key, 'l') {
            self.clear_transcript();
            return IdleAction::None;
        }
        if is_ctrl(key, 'u') {
            self.input.clear();
            self.after_input_edit();
            return IdleAction::None;
        }

        match key.code {
            KeyCode::Enter => {
                let input = self.input.take();
                if !input.trim().is_empty() {
                    self.history.push(input.clone());
                }
                self.history_cursor = None;
                self.history_draft.clear();
                self.after_input_edit();
                IdleAction::Submit(input)
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert(character);
                self.after_input_edit();
                IdleAction::None
            }
            KeyCode::Backspace => {
                self.input.backspace();
                self.after_input_edit();
                IdleAction::None
            }
            KeyCode::Delete => {
                self.input.delete();
                self.after_input_edit();
                IdleAction::None
            }
            KeyCode::Left => {
                self.input.move_left();
                IdleAction::None
            }
            KeyCode::Right => {
                self.input.move_right();
                IdleAction::None
            }
            KeyCode::Home => {
                self.input.cursor = 0;
                IdleAction::None
            }
            KeyCode::End => {
                self.input.cursor = self.input.text.len();
                IdleAction::None
            }
            KeyCode::Tab => {
                self.accept_suggestion();
                IdleAction::None
            }
            KeyCode::BackTab => {
                self.select_previous_suggestion();
                IdleAction::None
            }
            KeyCode::Esc => {
                self.suggestions_dismissed = true;
                self.selected_suggestion = 0;
                IdleAction::None
            }
            KeyCode::Up if !self.suggestions().is_empty() => {
                self.select_previous_suggestion();
                IdleAction::None
            }
            KeyCode::Down if !self.suggestions().is_empty() => {
                self.select_next_suggestion();
                IdleAction::None
            }
            KeyCode::Up => {
                self.navigate_history_up();
                IdleAction::None
            }
            KeyCode::Down => {
                self.navigate_history_down();
                IdleAction::None
            }
            KeyCode::PageUp => {
                self.scroll_up(self.transcript_page.saturating_sub(1).max(1));
                IdleAction::None
            }
            KeyCode::PageDown => {
                self.scroll_down(self.transcript_page.saturating_sub(1).max(1));
                IdleAction::None
            }
            _ => IdleAction::None,
        }
    }

    fn handle_scrolling_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::PageUp | KeyCode::Up => {
                self.scroll_up(self.transcript_page.saturating_sub(1).max(1))
            }
            KeyCode::PageDown | KeyCode::Down => {
                self.scroll_down(self.transcript_page.saturating_sub(1).max(1))
            }
            KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.follow_tail = false;
                self.scroll = 0;
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.follow_tail = true;
            }
            _ => {}
        }
    }

    fn submit_user_message(&mut self, prompt: String) {
        self.transcript.push(TranscriptBlock {
            kind: TranscriptKind::User,
            item_id: None,
            text: prompt,
        });
        self.renderer = ThreadEventRenderer::new();
        self.fallback_final_response = None;
        self.emitted_final_response = false;
        self.busy = true;
        self.turn_running = true;
        self.status = "thinking".to_string();
        self.follow_tail = true;
    }

    fn consume_thread_event(&mut self, event: &ThreadEvent) -> Result<(), LunaError> {
        if self.json_output {
            let json =
                serde_json::to_string(&output::thread_event_to_json(event)).map_err(|error| {
                    LunaError::Protocol(format!("failed to serialize chat event: {error}"))
                })?;
            self.transcript.push(TranscriptBlock {
                kind: TranscriptKind::Raw,
                item_id: None,
                text: json,
            });
            return Ok(());
        }

        if self.final_response_only {
            if let ThreadEvent::ItemCompleted {
                item: ThreadItem::AgentMessage(message),
            } = event
            {
                if message.is_final_answer() {
                    self.push_final_response(message.text.clone());
                } else {
                    self.fallback_final_response = Some(message.text.clone());
                }
            }
            if matches!(event, ThreadEvent::TurnCompleted { .. })
                && !self.emitted_final_response
                && let Some(response) = self.fallback_final_response.take()
            {
                self.push_final_response(response);
            }
            return Ok(());
        }

        if let Some(fragment) = self.renderer.render(event)
            && fragment.kind != RenderedItemKind::UserMessage
        {
            self.push_fragment(fragment);
        }
        Ok(())
    }

    fn push_fragment(&mut self, fragment: RenderedItem) {
        let kind = TranscriptKind::from(fragment.kind);
        if fragment.continuation
            && let Some(index) = self.transcript.iter().rposition(|block| {
                block.kind == kind && block.item_id.is_some() && block.item_id == fragment.item_id
            })
        {
            self.transcript[index].text.push_str(&fragment.markdown);
        } else {
            self.transcript.push(TranscriptBlock {
                kind,
                item_id: fragment.item_id,
                text: fragment.markdown,
            });
        }
        self.follow_tail = true;
    }

    fn push_final_response(&mut self, response: String) {
        self.emitted_final_response = true;
        self.transcript.push(TranscriptBlock {
            kind: TranscriptKind::Agent,
            item_id: None,
            text: response,
        });
    }

    fn finish_turn(&mut self) {
        self.busy = false;
        self.turn_running = false;
        self.status = "ready".to_string();
    }

    fn finish_turn_with_error(&mut self, message: String) {
        self.append_error(message);
        self.busy = false;
        self.turn_running = false;
        self.status = "ready".to_string();
    }

    fn append_status(&mut self, message: impl Into<String>) {
        self.transcript.push(TranscriptBlock {
            kind: TranscriptKind::Status,
            item_id: None,
            text: message.into(),
        });
        self.follow_tail = true;
    }

    fn append_error(&mut self, message: impl Into<String>) {
        self.transcript.push(TranscriptBlock {
            kind: TranscriptKind::Error,
            item_id: None,
            text: message.into(),
        });
        self.follow_tail = true;
    }

    fn clear_transcript(&mut self) {
        self.transcript.clear();
        self.scroll = 0;
        self.max_scroll = 0;
        self.follow_tail = true;
    }

    fn after_input_edit(&mut self) {
        self.suggestions_dismissed = false;
        self.selected_suggestion = 0;
        self.history_cursor = None;
    }

    fn accept_suggestion(&mut self) {
        if let Some(suggestion) = self.suggestions().get(self.selected_suggestion) {
            self.input.set(suggestion.replacement.clone());
            self.suggestions_dismissed = false;
            self.selected_suggestion = 0;
        }
    }

    fn select_previous_suggestion(&mut self) {
        let count = self.suggestions().len();
        if count > 0 {
            self.selected_suggestion = self.selected_suggestion.checked_sub(1).unwrap_or(count - 1);
        }
    }

    fn select_next_suggestion(&mut self) {
        let count = self.suggestions().len();
        if count > 0 {
            self.selected_suggestion = (self.selected_suggestion + 1) % count;
        }
    }

    fn navigate_history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = match self.history_cursor {
            Some(index) => index.saturating_sub(1),
            None => {
                self.history_draft = self.input.text.clone();
                self.history.len() - 1
            }
        };
        self.history_cursor = Some(index);
        self.input.set(self.history[index].clone());
        self.suggestions_dismissed = true;
    }

    fn navigate_history_down(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 < self.history.len() {
            let next = index + 1;
            self.history_cursor = Some(next);
            self.input.set(self.history[next].clone());
        } else {
            self.history_cursor = None;
            self.input.set(std::mem::take(&mut self.history_draft));
        }
        self.suggestions_dismissed = true;
    }

    fn scroll_up(&mut self, amount: usize) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll = (self.scroll + amount).min(self.max_scroll);
        if self.scroll == self.max_scroll {
            self.follow_tail = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IdleAction {
    None,
    Quit,
    Submit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptKind {
    User,
    Agent,
    Reasoning,
    Tool,
    Status,
    Error,
    Raw,
}

impl From<RenderedItemKind> for TranscriptKind {
    fn from(kind: RenderedItemKind) -> Self {
        match kind {
            RenderedItemKind::UserMessage => Self::User,
            RenderedItemKind::AgentMessage => Self::Agent,
            RenderedItemKind::Reasoning | RenderedItemKind::Plan => Self::Reasoning,
            RenderedItemKind::ToolCall
            | RenderedItemKind::CommandExecution
            | RenderedItemKind::FileChange => Self::Tool,
            RenderedItemKind::Error => Self::Error,
            RenderedItemKind::Status | RenderedItemKind::Unknown => Self::Status,
        }
    }
}

struct TranscriptBlock {
    kind: TranscriptKind,
    item_id: Option<String>,
    text: String,
}

#[derive(Default)]
struct InputBuffer {
    text: String,
    cursor: usize,
}

impl InputBuffer {
    fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    fn insert(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
    }

    fn delete(&mut self) {
        if self.cursor == self.text.len() {
            return;
        }
        let next = self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |character| self.cursor + character.len_utf8());
        self.text.drain(self.cursor..next);
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(0, |(index, _)| index);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor += self.text[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        }
    }
}

fn suggestions_for(input: &str, skills: &[String]) -> Vec<Suggestion> {
    if let Some(prefix) = input.strip_prefix('$')
        && !prefix.contains(char::is_whitespace)
    {
        return skills
            .iter()
            .filter(|skill| {
                skill
                    .to_ascii_lowercase()
                    .starts_with(&prefix.to_ascii_lowercase())
            })
            .map(|skill| Suggestion {
                replacement: format!("${skill} "),
                display: format!("${skill}"),
                description: "mention skill".to_string(),
            })
            .collect();
    }
    if !input.starts_with('/') {
        return Vec::new();
    }

    let (command, argument) = input
        .split_once(char::is_whitespace)
        .map_or((input, None), |(command, argument)| {
            (command, Some(argument.trim_start()))
        });
    let command_lower = command.to_ascii_lowercase();
    let Some(argument) = argument else {
        return COMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(&command_lower))
            .map(|(name, description)| Suggestion {
                replacement: if matches!(*name, "/effort" | "/skill") {
                    format!("{name} ")
                } else {
                    (*name).to_string()
                },
                display: (*name).to_string(),
                description: (*description).to_string(),
            })
            .collect();
    };

    match command_lower.as_str() {
        "/effort" => EFFORTS
            .into_iter()
            .filter(|effort| effort.starts_with(&argument.to_ascii_lowercase()))
            .map(|effort| Suggestion {
                replacement: format!("/effort {effort}"),
                display: effort.to_string(),
                description: "reasoning effort".to_string(),
            })
            .collect(),
        "/skill" => skills
            .iter()
            .filter(|skill| {
                skill
                    .to_ascii_lowercase()
                    .starts_with(&argument.to_ascii_lowercase())
            })
            .map(|skill| Suggestion {
                replacement: format!("/skill {skill}"),
                display: skill.clone(),
                description: "insert skill mention".to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn transcript_style(kind: TranscriptKind) -> (&'static str, Style) {
    match kind {
        TranscriptKind::User => (
            "YOU",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        TranscriptKind::Agent => (
            "LUNA",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        TranscriptKind::Reasoning => (
            "THOUGHT",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        TranscriptKind::Tool => (
            "ACTIVITY",
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ),
        TranscriptKind::Status => ("LUNAR LOG", Style::default().fg(Color::DarkGray)),
        TranscriptKind::Error => (
            "ERROR",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
        TranscriptKind::Raw => ("JSON", Style::default().fg(Color::Gray)),
    }
}

fn moon_block<'a>() -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn splash(height: u16) -> Paragraph<'static> {
    let top_padding = (height as usize).saturating_sub(1) / 2;
    let mut lines = vec![Line::default(); top_padding];
    lines.push(Line::from(Span::styled(
        "Luna",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    Paragraph::new(lines).alignment(Alignment::Center)
}

fn wrapped_height(text: &Text<'_>, width: usize) -> usize {
    let wrap_width = width.max(1);
    text.lines
        .iter()
        .map(|line| {
            let line_width = UnicodeWidthStr::width(line.to_string().as_str());
            line_width.max(1).div_ceil(wrap_width)
        })
        .sum::<usize>()
        .max(1)
}

fn help_text() -> &'static str {
    "/compact          compact conversation context\n/effort [level]    show or set effort\n/skills            list enabled skills\n/skill <name>      insert $name into the composer\n/clear             clear the visible transcript\n/quit              leave chat\n\nTab accepts the ghost suggestion. Up/Down selects suggestions or history. PgUp/PgDn scrolls. Ctrl-C clears input or interrupts a running turn. Ctrl-D exits from an empty composer. Prefix a prompt with // to send a literal leading /."
}

fn accepts_key(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn is_ctrl(key: KeyEvent, character: char) -> bool {
    key.code == KeyCode::Char(character) && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn single_line(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_sdk::{AgentMessageItem, AgentMessagePhase};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn slash_commands_parse_into_observable_chat_actions() {
        assert_eq!(
            parse_input("/compact"),
            ParsedInput::Command(ChatCommand::Compact)
        );
        assert_eq!(
            parse_input("/effort high"),
            ParsedInput::Command(ChatCommand::SetEffort(ModelReasoningEffort::High))
        );
        assert_eq!(
            parse_input("//compact"),
            ParsedInput::Prompt("/compact".to_string())
        );
        assert!(matches!(
            parse_input("/something-new"),
            ParsedInput::Command(ChatCommand::Unknown(_))
        ));
    }

    #[test]
    fn completion_registry_suggests_commands_effort_and_skills() {
        let skills = vec!["reviewer".to_string()];

        assert_eq!(suggestions_for("/co", &skills)[0].display, "/compact");
        assert_eq!(
            suggestions_for("/effort h", &skills)[0].replacement,
            "/effort high"
        );
        assert!(matches!(
            parse_input("/model"),
            ParsedInput::Command(ChatCommand::Unknown(_))
        ));
        assert_eq!(
            suggestions_for("$rev", &skills)[0].replacement,
            "$reviewer "
        );
    }

    #[test]
    fn input_buffer_edits_unicode_at_character_boundaries() {
        let mut input = InputBuffer::default();
        input.insert('a');
        input.insert('🌙');
        input.insert('b');
        input.move_left();
        input.backspace();
        assert_eq!(input.text, "ab");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn skill_catalog_deduplicates_enabled_names_and_surfaces_group_errors() {
        let result = responses::SkillsListResult {
            extra: serde_json::Map::from_iter([(
                "data".to_string(),
                serde_json::json!([{
                    "errors": [{"message": "one skill could not be loaded"}],
                    "skills": [
                        {"name": "reviewer", "enabled": true},
                        {"name": "reviewer"},
                        {"name": "disabled", "enabled": false}
                    ]
                }]),
            )]),
        };
        let (skills, warnings) = enabled_skill_names(&result);
        assert_eq!(skills, vec!["reviewer"]);
        assert_eq!(
            warnings,
            vec!["skills warning: one skill could not be loaded"]
        );
    }

    #[test]
    fn splash_and_completion_popup_render_in_monochrome() {
        let backend = TestBackend::new(90, 32);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = ChatApp::new(
            DEFAULT_MODEL.to_string(),
            ModelReasoningEffort::Max,
            false,
            false,
        );
        app.status = "ready".to_string();
        terminal.draw(|frame| app.render(frame)).expect("splash");
        let splash = terminal.backend().buffer().clone();
        let text = buffer_text(&splash);
        assert!(text.contains("Luna"));
        assert!(!text.contains("_..._"));

        app.input.set("/co".to_string());
        terminal.draw(|frame| app.render(frame)).expect("popup");
        let popup = buffer_text(terminal.backend().buffer());
        assert!(popup.contains("/compact"));
        assert!(popup.contains("[Tab] accept"));
        assert!(terminal.get_cursor_position().is_ok());
    }

    #[test]
    fn narrow_terminal_keeps_composer_cursor_in_bounds() {
        let backend = TestBackend::new(40, 16);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut app = ChatApp::new(
            DEFAULT_MODEL.to_string(),
            ModelReasoningEffort::Max,
            false,
            false,
        );
        app.input.set("/".to_string());
        terminal.draw(|frame| app.render(frame)).expect("render");
        let cursor = terminal.get_cursor_position().expect("cursor");
        assert!(cursor.x < 40);
        assert!(cursor.y < 16);
        assert!(buffer_text(terminal.backend().buffer()).contains("/help"));
    }

    #[test]
    fn streamed_agent_deltas_merge_and_final_only_waits_for_final_message() {
        let mut streamed = ChatApp::new(
            DEFAULT_MODEL.to_string(),
            ModelReasoningEffort::Max,
            false,
            false,
        );
        streamed.submit_user_message("hello".to_string());
        for text in ["moon", "light"] {
            streamed
                .consume_thread_event(&ThreadEvent::ItemUpdated {
                    item: ThreadItem::AgentMessage(AgentMessageItem {
                        id: "agent".to_string(),
                        text: text.to_string(),
                        phase: None,
                    }),
                })
                .expect("delta");
        }
        let agent = streamed
            .transcript
            .iter()
            .find(|block| block.kind == TranscriptKind::Agent)
            .expect("agent block");
        assert_eq!(agent.text, "moonlight");

        let mut final_only = ChatApp::new(
            DEFAULT_MODEL.to_string(),
            ModelReasoningEffort::Max,
            true,
            false,
        );
        final_only.submit_user_message("hello".to_string());
        final_only
            .consume_thread_event(&ThreadEvent::ItemCompleted {
                item: ThreadItem::AgentMessage(AgentMessageItem {
                    id: "commentary".to_string(),
                    text: "working".to_string(),
                    phase: Some(AgentMessagePhase::Commentary),
                }),
            })
            .expect("commentary");
        assert_eq!(final_only.transcript.len(), 1);
        final_only
            .consume_thread_event(&ThreadEvent::ItemCompleted {
                item: ThreadItem::AgentMessage(AgentMessageItem {
                    id: "final".to_string(),
                    text: "done".to_string(),
                    phase: Some(AgentMessagePhase::FinalAnswer),
                }),
            })
            .expect("final");
        assert_eq!(final_only.transcript.len(), 2);
        assert_eq!(final_only.transcript[1].text, "done");
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
