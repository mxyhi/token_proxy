use serde_json::{json, Map, Value};

pub(crate) const CLAUDE_WEB_SEARCH_NAME: &str = "web_search";

pub(crate) fn responses_search_id(claude_id: &str) -> String {
    format!("ws_{claude_id}")
}

pub(crate) fn claude_search_id(responses_id: &str) -> String {
    let body = responses_id
        .trim()
        .strip_prefix("ws_")
        .unwrap_or(responses_id)
        .strip_prefix("srvtoolu_")
        .unwrap_or_else(|| {
            responses_id
                .trim()
                .strip_prefix("ws_")
                .unwrap_or(responses_id)
        });
    let sanitized: String = body
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        String::new()
    } else {
        format!("srvtoolu_{sanitized}")
    }
}

pub(crate) fn responses_search_query(item: &Map<String, Value>) -> String {
    item.get("action")
        .and_then(Value::as_object)
        .and_then(|action| {
            action
                .get("query")
                .and_then(Value::as_str)
                .or_else(|| {
                    action
                        .get("queries")
                        .and_then(Value::as_array)
                        .and_then(|queries| queries.first())
                        .and_then(Value::as_str)
                })
                .or_else(|| action.get("url").and_then(Value::as_str))
        })
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(crate) fn claude_search_query(input: Option<&Value>) -> String {
    input
        .and_then(Value::as_object)
        .and_then(|input| input.get("query").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Reads a completed Claude `input_json_delta` buffer without treating an
/// incomplete fragment as a malformed function call.
pub(crate) fn claude_search_query_json(input_json: &str) -> Option<String> {
    let input = serde_json::from_str::<Value>(input_json).ok()?;
    Some(claude_search_query(Some(&input)))
}

pub(crate) fn responses_search_results_from_claude(content: Option<&Value>) -> Value {
    let Some(content) = content else {
        return Value::Array(Vec::new());
    };
    let entries = match content {
        Value::Array(items) => items
            .iter()
            .filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("web_search_tool_result_error")
                    || item
                        .get("url")
                        .and_then(Value::as_str)
                        .is_some_and(|url| !url.trim().is_empty())
            })
            .cloned()
            .collect(),
        Value::Object(_) => vec![content.clone()],
        _ => Vec::new(),
    };
    Value::Array(entries)
}

pub(crate) fn claude_search_results_from_responses(results: Option<&Value>) -> Value {
    let Some(Value::Array(items)) = results else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("web_search_tool_result_error")
                {
                    return Some(item.clone());
                }
                let Some(encrypted) = item.get("encrypted_content").and_then(Value::as_str) else {
                    tracing::debug!("丢弃缺少 encrypted_content 的 Responses 搜索结果");
                    return None;
                };
                if encrypted.trim().is_empty() {
                    tracing::debug!("丢弃 encrypted_content 为空的 Responses 搜索结果");
                    return None;
                }
                let mut block = item.clone();
                if let Some(object) = block.as_object_mut() {
                    object.insert(
                        "type".to_string(),
                        Value::String("web_search_result".to_string()),
                    );
                }
                Some(block)
            })
            .collect(),
    )
}

pub(super) fn responses_search_call_to_claude(item: &Map<String, Value>) -> Option<(Value, Value)> {
    let source_id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let tool_use_id = claude_search_id(source_id);
    if tool_use_id.is_empty() {
        return None;
    }
    let query = responses_search_query(item);
    let use_block = json!({
        "type": "server_tool_use",
        "id": tool_use_id,
        "name": CLAUDE_WEB_SEARCH_NAME,
        "input": { "query": query }
    });
    let result_block = json!({
        "type": "web_search_tool_result",
        "tool_use_id": tool_use_id,
        "content": claude_search_results_from_responses(item.get("results"))
    });
    Some((use_block, result_block))
}

pub(super) fn claude_search_blocks_to_responses(
    use_block: &Map<String, Value>,
    result_block: Option<&Map<String, Value>>,
) -> Value {
    let claude_id = use_block.get("id").and_then(Value::as_str).unwrap_or("");
    let query = claude_search_query(use_block.get("input"));
    let results =
        result_block.map(|result| responses_search_results_from_claude(result.get("content")));
    let mut item = json!({
        "id": responses_search_id(claude_id),
        "type": "web_search_call",
        "status": "completed",
        "action": { "type": "search", "query": query }
    });
    if let Some(results) = results {
        item.as_object_mut()
            .expect("search item object")
            .insert("results".to_string(), results);
    }
    item
}

pub(super) fn annotations_to_citations(annotations: Option<&Value>) -> Value {
    let Some(Value::Array(items)) = annotations else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .filter(|item| {
                item.get("encrypted_index")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
            })
            .cloned()
            .collect(),
    )
}

pub(super) fn citations_to_annotations(citations: Option<&Value>) -> Value {
    let Some(Value::Array(items)) = citations else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .filter(|item| {
                item.get("encrypted_index")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
            })
            .cloned()
            .collect(),
    )
}
