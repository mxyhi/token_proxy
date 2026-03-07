use serde_json::{Map, Value};
use std::collections::HashSet;

use super::types::KiroToolUse;
use super::utils::random_uuid;

pub(crate) struct ToolUseState {
    id: String,
    name: String,
    input_buffer: String,
}

pub(crate) fn parse_embedded_tool_calls(
    text: &str,
    processed: &mut HashSet<String>,
) -> (String, Vec<KiroToolUse>) {
    if !text.contains("[Called") {
        return (text.to_string(), Vec::new());
    }

    let mut matches = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find("[Called") {
        let start = cursor + offset;
        let mut idx = start + "[Called".len();
        idx = skip_whitespace(text, idx);

        let name_start = idx;
        while idx < text.len() && is_tool_name_char(text.as_bytes()[idx]) {
            idx += 1;
        }
        if name_start == idx {
            cursor = start + 1;
            continue;
        }
        let tool_name = &text[name_start..idx];

        idx = skip_whitespace(text, idx);
        if !text[idx..].starts_with("with") {
            cursor = start + 1;
            continue;
        }
        idx += "with".len();

        idx = skip_whitespace(text, idx);
        if !text[idx..].starts_with("args:") {
            cursor = start + 1;
            continue;
        }
        idx += "args:".len();
        idx = skip_whitespace(text, idx);

        if idx >= text.len() || text.as_bytes()[idx] != b'{' {
            cursor = start + 1;
            continue;
        }
        let json_start = idx;
        let json_end = match find_matching_bracket(text, json_start) {
            Some(end) => end,
            None => {
                cursor = start + 1;
                continue;
            }
        };

        let mut closing = json_end + 1;
        while closing < text.len() && text.as_bytes()[closing] != b']' {
            closing += 1;
        }
        if closing >= text.len() {
            cursor = start + 1;
            continue;
        }
        let match_end = closing + 1;
        matches.push((
            start,
            match_end,
            tool_name.to_string(),
            text[json_start..=json_end].to_string(),
        ));

        cursor = match_end;
    }

    if matches.is_empty() {
        return (text.to_string(), Vec::new());
    }

    let mut clean_text = text.to_string();
    let mut tool_uses = Vec::new();
    for (start, end, name, json_str) in matches.into_iter().rev() {
        let repaired = repair_json(&json_str);
        let input = match serde_json::from_str::<Map<String, Value>>(&repaired) {
            Ok(map) => map,
            Err(_) => continue,
        };

        let dedupe_key = format!("content:{name}:{repaired}");
        if processed.contains(&dedupe_key) {
            if clean_text.is_char_boundary(start) && clean_text.is_char_boundary(end) {
                clean_text.replace_range(start..end, "");
            }
            continue;
        }
        processed.insert(dedupe_key);

        let tool_use_id = generate_tool_use_id();
        tool_uses.push(KiroToolUse {
            tool_use_id,
            name,
            input,
        });

        if clean_text.is_char_boundary(start) && clean_text.is_char_boundary(end) {
            clean_text.replace_range(start..end, "");
        }
    }

    (clean_text, tool_uses)
}

pub(crate) fn process_tool_use_event(
    event: &Map<String, Value>,
    current: Option<ToolUseState>,
    processed: &mut HashSet<String>,
) -> (Vec<KiroToolUse>, Option<ToolUseState>) {
    let mut tool_uses = Vec::new();
    let mut state = current;

    let source = event
        .get("toolUseEvent")
        .and_then(Value::as_object)
        .unwrap_or(event);

    let tool_use_id = tool_use_id(source);
    let name = source.get("name").and_then(Value::as_str).unwrap_or("");
    let stop = source.get("stop").and_then(Value::as_bool).unwrap_or(false);

    if let (Some(tool_use_id), true) = (tool_use_id, !name.is_empty()) {
        let dedupe_key = format!("id:{tool_use_id}");
        if let Some(current_state) = &state {
            if current_state.id != tool_use_id {
                if !processed.contains(&format!("id:{}", current_state.id)) {
                    let input = parse_tool_input(&current_state.input_buffer);
                    tool_uses.push(KiroToolUse {
                        tool_use_id: current_state.id.clone(),
                        name: current_state.name.clone(),
                        input,
                    });
                    processed.insert(format!("id:{}", current_state.id));
                }
                state = None;
            }
        }

        if state.is_none() && !processed.contains(&dedupe_key) {
            state = Some(ToolUseState {
                id: tool_use_id.to_string(),
                name: name.to_string(),
                input_buffer: String::new(),
            });
        }
    }

    if let Some(current_state) = &mut state {
        if let Some(Value::String(fragment)) = source.get("input") {
            current_state.input_buffer.push_str(fragment);
        } else if let Some(Value::Object(input)) = source.get("input") {
            let serialized = serde_json::to_string(input).unwrap_or_default();
            current_state.input_buffer = serialized;
        }
    }

    if stop {
        if let Some(current_state) = state.take() {
            let input = parse_tool_input(&current_state.input_buffer);
            let dedupe_key = format!("id:{}", current_state.id);
            if !processed.contains(&dedupe_key) {
                processed.insert(dedupe_key);
                tool_uses.push(KiroToolUse {
                    tool_use_id: current_state.id,
                    name: current_state.name,
                    input,
                });
            }
        }
    }

    (tool_uses, state)
}

pub(crate) fn deduplicate_tool_uses(tool_uses: Vec<KiroToolUse>) -> Vec<KiroToolUse> {
    let mut seen_ids = HashSet::new();
    let mut seen_content = HashSet::new();
    let mut output = Vec::new();

    for tool_use in tool_uses {
        if !seen_ids.insert(tool_use.tool_use_id.clone()) {
            continue;
        }
        let input_json = serde_json::to_string(&tool_use.input).unwrap_or_default();
        let content_key = format!("{}:{}", tool_use.name, input_json);
        if !seen_content.insert(content_key) {
            continue;
        }
        output.push(tool_use);
    }
    output
}

fn tool_use_id(source: &Map<String, Value>) -> Option<&str> {
    source
        .get("toolUseId")
        .or_else(|| source.get("tool_use_id"))
        .and_then(Value::as_str)
}

fn parse_tool_input(raw: &str) -> Map<String, Value> {
    if raw.trim().is_empty() {
        return Map::new();
    }
    let repaired = repair_json(raw);
    serde_json::from_str::<Map<String, Value>>(&repaired).unwrap_or_default()
}

fn generate_tool_use_id() -> String {
    let raw = random_uuid().replace('-', "");
    let suffix = if raw.len() >= 12 {
        &raw[..12]
    } else {
        raw.as_str()
    };
    format!("toolu_{suffix}")
}

fn skip_whitespace(text: &str, mut idx: usize) -> usize {
    while idx < text.len() && text.as_bytes()[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn is_tool_name_char(byte: u8) -> bool {
    matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-')
}

fn find_matching_bracket(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let open = *bytes.get(start)?;
    let close = match open {
        b'{' => b'}',
        b'[' => b']',
        _ => return None,
    };
    let mut depth = 1usize;
    let mut in_string = false;
    let mut escape_next = false;

    for idx in (start + 1)..bytes.len() {
        let ch = bytes[idx];
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == b'\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn repair_json(raw: &str) -> String {
    let mut value = raw.trim().to_string();
    if value.is_empty() {
        return "{}".to_string();
    }

    if serde_json::from_str::<Value>(&value).is_ok() {
        return value;
    }

    let original = value.clone();
    value = escape_newlines_in_strings(&value);
    value = remove_trailing_commas(&value);

    let mut brace_count = 0i32;
    let mut bracket_count = 0i32;
    let mut in_string = false;
    let mut escape_next = false;
    let mut last_valid_index: Option<usize> = None;

    for (idx, ch) in value.bytes().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == b'\\' {
            escape_next = true;
            continue;
        }
        if ch == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            b'{' => brace_count += 1,
            b'}' => brace_count -= 1,
            b'[' => bracket_count += 1,
            b']' => bracket_count -= 1,
            _ => {}
        }
        if brace_count >= 0 && bracket_count >= 0 {
            last_valid_index = Some(idx);
        }
    }

    if brace_count > 0 || bracket_count > 0 {
        if let Some(last) = last_valid_index {
            if last + 1 < value.len() {
                value.truncate(last + 1);
            }
        }
        while brace_count > 0 {
            value.push('}');
            brace_count -= 1;
        }
        while bracket_count > 0 {
            value.push(']');
            bracket_count -= 1;
        }
    }

    if serde_json::from_str::<Value>(&value).is_ok() {
        value
    } else {
        original
    }
}

fn escape_newlines_in_strings(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 64);
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && in_string {
            out.push(ch);
            escaped = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            continue;
        }
        if in_string {
            match ch {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push(ch),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn remove_trailing_commas(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut iter = raw.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch == ',' {
            if let Some(next) = iter.peek() {
                if *next == '}' || *next == ']' {
                    continue;
                }
            }
        }
        out.push(ch);
    }
    out
}
