use codex_app_server_sdk::{Codex, requests, responses};
use serde_json::{Map, Value};

use crate::error::LunaError;

const THREAD_LIST_PAGE_LIMIT: u32 = 100;
const MAX_THREAD_LIST_PAGES: usize = 100;
const SESSION_PREVIEW_CHAR_LIMIT: usize = 96;

#[derive(Debug)]
struct SessionListEntry {
    id: String,
    recency_score: i64,
    last_message: String,
}

pub(crate) async fn list_sessions(
    codex: &Codex,
    cwd_filter: Option<&str>,
) -> Result<(), LunaError> {
    let mut cursor: Option<String> = None;
    let mut pages_scanned = 0usize;
    let mut sessions = Vec::new();

    loop {
        pages_scanned += 1;
        if pages_scanned > MAX_THREAD_LIST_PAGES {
            return Err(LunaError::Protocol(format!(
                "could not list sessions after scanning {MAX_THREAD_LIST_PAGES} pages"
            )));
        }

        let params = requests::ThreadListParams {
            limit: Some(THREAD_LIST_PAGE_LIMIT),
            cursor: cursor.clone(),
            ..Default::default()
        };
        let result = codex.thread_list(params).await?;

        for thread in result.data {
            if let Some(filter_cwd) = cwd_filter {
                let thread_cwd = thread.extra.get("cwd").and_then(|v| v.as_str());
                match thread_cwd {
                    Some(cwd) if cwd == filter_cwd => {}
                    _ => continue,
                }
            }

            let recency_score = thread_recency_score(&thread).unwrap_or(i64::MIN);
            let last_message = resolve_last_message_preview(codex, &thread).await?;
            sessions.push(SessionListEntry {
                id: thread.id,
                recency_score,
                last_message,
            });
        }

        match result.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    sessions.sort_by(|left, right| {
        right
            .recency_score
            .cmp(&left.recency_score)
            .then_with(|| left.id.cmp(&right.id))
    });

    if sessions.is_empty() {
        match cwd_filter {
            Some(cwd) => {
                println!("No recorded sessions found for {cwd}. Use --all to show all sessions.")
            }
            None => println!("No recorded sessions found."),
        }
        return Ok(());
    }

    println!("SESSION_ID\tLAST_MESSAGE");
    for session in sessions {
        let preview = crop_preview_text(&session.last_message, SESSION_PREVIEW_CHAR_LIMIT);
        println!("{}\t{}", session.id, preview);
    }

    Ok(())
}

async fn resolve_last_message_preview(
    codex: &Codex,
    thread: &responses::ThreadSummary,
) -> Result<String, LunaError> {
    if let Some(preview) = extract_summary_preview(&thread.extra) {
        return Ok(preview);
    }

    let read_result = codex
        .thread_read(requests::ThreadReadParams {
            thread_id: thread.id.clone(),
            include_turns: Some(true),
            extra: Map::new(),
        })
        .await?;

    Ok(extract_last_message_from_thread_read(&read_result.extra).unwrap_or_default())
}

fn extract_summary_preview(extra: &Map<String, Value>) -> Option<String> {
    for key in [
        "lastMessage",
        "lastAgentMessage",
        "lastAssistantMessage",
        "snippet",
        "preview",
    ] {
        if let Some(text) = extra.get(key).and_then(value_to_text) {
            return Some(text);
        }
    }
    None
}

fn extract_last_message_from_thread_read(extra: &Map<String, Value>) -> Option<String> {
    let mut last_assistant: Option<String> = None;
    let mut last_any: Option<String> = None;

    if let Some(items) = extra.get("items").and_then(Value::as_array) {
        let (assistant, any) = extract_last_message_from_items(items);
        if assistant.is_some() {
            last_assistant = assistant;
        }
        if any.is_some() {
            last_any = any;
        }
    }

    if let Some(turns) = extra.get("turns").and_then(Value::as_array) {
        for turn in turns {
            let Some(items) = turn.get("items").and_then(Value::as_array) else {
                continue;
            };
            let (assistant, any) = extract_last_message_from_items(items);
            if assistant.is_some() {
                last_assistant = assistant;
            }
            if any.is_some() {
                last_any = any;
            }
        }
    }

    last_assistant.or(last_any)
}

fn extract_last_message_from_items(items: &[Value]) -> (Option<String>, Option<String>) {
    let mut last_assistant: Option<String> = None;
    let mut last_any: Option<String> = None;

    for item in items {
        let Some(text) = extract_item_text(item) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        if item_is_assistant_message(item) {
            last_assistant = Some(text.clone());
        }
        last_any = Some(text);
    }

    (last_assistant, last_any)
}

fn extract_item_text(item: &Value) -> Option<String> {
    match item {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Array(parts) => {
            let texts: Vec<String> = parts.iter().filter_map(extract_item_text).collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join(" "))
            }
        }
        Value::Object(object) => {
            for key in ["text", "content", "message", "output_text", "outputText"] {
                if let Some(text) = object.get(key).and_then(extract_item_text)
                    && !text.is_empty()
                {
                    return Some(text);
                }
            }
            None
        }
        _ => None,
    }
}

fn item_is_assistant_message(item: &Value) -> bool {
    let Some(object) = item.as_object() else {
        return false;
    };

    if let Some(role) = object.get("role").and_then(Value::as_str) {
        let role = role.to_ascii_lowercase();
        if role.contains("assistant") || role.contains("agent") {
            return true;
        }
    }

    if let Some(author) = object.get("author").and_then(Value::as_object)
        && let Some(role) = author.get("role").and_then(Value::as_str)
    {
        let role = role.to_ascii_lowercase();
        if role.contains("assistant") || role.contains("agent") {
            return true;
        }
    }

    if let Some(item_type) = object.get("type").and_then(Value::as_str) {
        let item_type = item_type.to_ascii_lowercase();
        if item_type.contains("assistant")
            || item_type.contains("agent_message")
            || item_type.contains("agentmessage")
        {
            return true;
        }
    }

    false
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => extract_item_text(value),
    }
}

pub(crate) fn crop_preview_text(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "(no message)".to_string();
    }
    if max_chars == 0 {
        return "...".to_string();
    }

    let mut preview = String::new();
    let mut chars = normalized.chars();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return normalized;
        };
        preview.push(ch);
    }

    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

fn thread_recency_score(thread: &responses::ThreadSummary) -> Option<i64> {
    parse_timestamp(thread.extra.get("updatedAt"))
        .or_else(|| parse_timestamp(thread.extra.get("createdAt")))
}

fn parse_timestamp(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|raw| i64::try_from(raw).ok())),
        Value::String(raw) => raw.parse::<i64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_corrupt_thread_items_have_no_preview() {
        assert_eq!(extract_last_message_from_items(&[]), (None, None));
        let corrupt = vec![
            Value::Null,
            serde_json::json!(42),
            serde_json::json!({"type": "agentMessage", "content": [null, 7]}),
        ];
        assert_eq!(extract_last_message_from_items(&corrupt), (None, None));
    }

    #[test]
    fn assistant_preview_wins_over_later_non_assistant_content() {
        let items = vec![
            serde_json::json!({"type": "agentMessage", "text": "answer"}),
            serde_json::json!({"type": "userMessage", "text": "later prompt"}),
        ];
        assert_eq!(
            extract_last_message_from_items(&items),
            (Some("answer".to_string()), Some("later prompt".to_string()))
        );
    }

    #[test]
    fn preview_normalizes_whitespace_and_crops_on_character_boundaries() {
        assert_eq!(
            crop_preview_text("hello   world from   luna", 12),
            "hello world ..."
        );
        assert_eq!(crop_preview_text("ééé", 2), "éé...");
        assert_eq!(crop_preview_text("anything", 0), "...");
    }

    #[test]
    fn malformed_timestamps_are_ignored_without_panicking() {
        assert_eq!(
            parse_timestamp(Some(&Value::String("bad".to_string()))),
            None
        );
        assert_eq!(
            parse_timestamp(Some(&serde_json::json!({"seconds": "bad"}))),
            None
        );
        assert_eq!(parse_timestamp(None), None);
    }
}
