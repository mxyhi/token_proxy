use serde_json::{json, Map, Value};
use std::collections::VecDeque;

struct CliToolResponseFixer {
    out: Vec<Value>,
    pending_groups: Vec<usize>,
    collected_responses: VecDeque<Value>,
}

impl CliToolResponseFixer {
    fn new(capacity: usize) -> Self {
        Self {
            out: Vec::with_capacity(capacity),
            pending_groups: Vec::new(),
            collected_responses: VecDeque::new(),
        }
    }

    fn push_content(&mut self, content: Value) {
        let Some(obj) = content.as_object() else {
            return;
        };

        let parts = parts_slice(obj);
        if let Some(response_parts) = response_only_parts(parts) {
            self.push_responses_and_maybe_merge(response_parts);
            return;
        }

        if is_model_content(obj) {
            let function_call_count = count_function_calls(parts);
            if function_call_count > 0 {
                self.out.push(content);
                self.pending_groups.push(function_call_count);
                return;
            }
        }

        self.out.push(content);
    }

    fn push_responses_and_maybe_merge(&mut self, response_parts: Vec<Value>) {
        for part in response_parts {
            self.collected_responses.push_back(part);
        }
        self.try_satisfy_latest_group();
    }

    fn try_satisfy_latest_group(&mut self) {
        for idx in (0..self.pending_groups.len()).rev() {
            let needed = self.pending_groups[idx];
            if self.collected_responses.len() < needed {
                continue;
            }
            let merged_parts = self.take_merged_parts(needed);
            if !merged_parts.is_empty() {
                self.out.push(json!({ "role": "function", "parts": merged_parts }));
            }
            self.pending_groups.remove(idx);
            break;
        }
    }

    fn take_merged_parts(&mut self, needed: usize) -> Vec<Value> {
        let mut merged_parts = Vec::with_capacity(needed);
        for _ in 0..needed {
            let Some(next) = self.collected_responses.pop_front() else {
                break;
            };
            if next.is_object() {
                merged_parts.push(next);
            } else {
                merged_parts.push(fallback_function_response_part(&next));
            }
        }
        merged_parts
    }

    fn flush_remaining(mut self) -> Vec<Value> {
        for needed in std::mem::take(&mut self.pending_groups) {
            if self.collected_responses.len() < needed {
                break;
            }
            let merged_parts = self.take_merged_parts(needed);
            if !merged_parts.is_empty() {
                self.out.push(json!({ "role": "function", "parts": merged_parts }));
            }
        }
        self.out
    }
}

/// Align with CLIProxyAPIPlus `fixCLIToolResponse()`:
/// - Collect standalone `functionResponse` contents.
/// - For each preceding `model` content that contains N `functionCall` parts,
///   merge the next N `functionResponse` parts into a single content entry.
///
/// NOTE: This intentionally assumes that a "tool response content" contains ONLY
/// `functionResponse` parts; if mixed parts exist, we keep the content unchanged.
pub(super) fn fix_cli_tool_response(request: &mut Map<String, Value>) {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };

    let original = std::mem::take(contents);
    let mut fixer = CliToolResponseFixer::new(original.len());
    for content in original {
        fixer.push_content(content);
    }
    *contents = fixer.flush_remaining();
}

pub(super) fn normalize_contents_roles(request: &mut Map<String, Value>) {
    let Some(contents) = request.get_mut("contents").and_then(Value::as_array_mut) else {
        return;
    };
    let mut prev_role = String::new();
    for content in contents {
        let Some(obj) = content.as_object_mut() else {
            continue;
        };
        let role = obj
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let valid = role == "user" || role == "model";
        let role = if valid {
            role
        } else {
            let next = if prev_role.is_empty() {
                "user"
            } else if prev_role == "user" {
                "model"
            } else {
                "user"
            };
            obj.insert("role".to_string(), Value::String(next.to_string()));
            next.to_string()
        };
        prev_role = role;
    }
}

fn fallback_function_response_part(value: &Value) -> Value {
    // Best-effort fallback; should be rare in practice.
    json!({
        "functionResponse": {
            "name": "unknown",
            "response": { "result": value_to_string(value) }
        }
    })
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn parts_slice(content: &Map<String, Value>) -> &[Value] {
    content
        .get("parts")
        .and_then(Value::as_array)
        .map(|value| value.as_slice())
        .unwrap_or(&[])
}

fn response_only_parts(parts: &[Value]) -> Option<Vec<Value>> {
    let mut responses = Vec::new();
    for part in parts {
        if part.get("functionResponse").is_some() {
            responses.push(part.clone());
        } else {
            return None;
        }
    }
    if responses.is_empty() {
        None
    } else {
        Some(responses)
    }
}

fn count_function_calls(parts: &[Value]) -> usize {
    parts
        .iter()
        .filter(|part| part.get("functionCall").is_some())
        .count()
}

fn is_model_content(content: &Map<String, Value>) -> bool {
    content.get("role").and_then(Value::as_str) == Some("model")
}
