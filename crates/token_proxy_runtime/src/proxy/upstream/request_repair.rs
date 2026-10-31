//! 根据上游 400 的精确拒绝信息做白名单字段删除；修复次数和请求体哈希共同阻止循环。

use axum::http::StatusCode;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use crate::proxy::request_body::ReplayableBody;

#[path = "codex_encrypted.rs"]
mod codex_encrypted;
pub(super) use codex_encrypted::repair_codex_encrypted_body;

const MAX_REJECTED_FIELD_REPAIRS: usize = 6;
// Each removal must be named by the upstream error. Cap the total loss while
// allowing long histories to migrate when several old items are rejected.
// Long-running Codex sessions can legitimately contain more than 64 historical
// reasoning items. Keep a hard upper bound to avoid an unbounded replay loop,
// while allowing normal context migration to finish.
const MAX_CODEX_ENCRYPTED_REPAIRS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RequestRepairKind {
    RejectedField,
    InvalidEncryptedContent,
    CodexEncryptedContent,
}

pub(super) struct RequestRepair {
    pub(super) body: ReplayableBody,
    pub(super) reason: &'static str,
    pub(super) kind: RequestRepairKind,
}

pub(super) struct RequestRepairState {
    rejected_field_repairs: usize,
    codex_encrypted_repairs: usize,
    invalid_encrypted_content_repaired: bool,
    seen_body_hashes: HashSet<[u8; 32]>,
}

impl RequestRepairState {
    pub(super) fn new(initial_body: &ReplayableBody) -> Self {
        let mut seen_body_hashes =
            HashSet::with_capacity(MAX_REJECTED_FIELD_REPAIRS + MAX_CODEX_ENCRYPTED_REPAIRS + 2);
        seen_body_hashes.insert(body_hash(initial_body.as_bytes()));
        Self {
            rejected_field_repairs: 0,
            codex_encrypted_repairs: 0,
            invalid_encrypted_content_repaired: false,
            seen_body_hashes,
        }
    }

    fn allow(&mut self, kind: RequestRepairKind, body: &[u8]) -> bool {
        match kind {
            RequestRepairKind::RejectedField
                if self.rejected_field_repairs >= MAX_REJECTED_FIELD_REPAIRS =>
            {
                return false;
            }
            RequestRepairKind::InvalidEncryptedContent
                if self.invalid_encrypted_content_repaired =>
            {
                return false;
            }
            RequestRepairKind::CodexEncryptedContent
                if self.codex_encrypted_repairs >= MAX_CODEX_ENCRYPTED_REPAIRS =>
            {
                return false;
            }
            _ => {}
        }
        if !self.seen_body_hashes.insert(body_hash(body)) {
            return false;
        }
        match kind {
            RequestRepairKind::RejectedField => self.rejected_field_repairs += 1,
            RequestRepairKind::InvalidEncryptedContent => {
                self.invalid_encrypted_content_repaired = true;
            }
            RequestRepairKind::CodexEncryptedContent => self.codex_encrypted_repairs += 1,
        }
        true
    }
}

pub(super) fn repair_request_body(
    status: StatusCode,
    request_body: &ReplayableBody,
    response_body: &[u8],
    state: &mut RequestRepairState,
) -> Result<Option<RequestRepair>, String> {
    if status != StatusCode::BAD_REQUEST || response_body.is_empty() {
        return Ok(None);
    }
    let response: Value = match serde_json::from_slice(response_body) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let request: Value = match serde_json::from_slice(request_body.as_bytes()) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    if let Some((body, reason)) = repair_rejected_field(request.clone(), &response)? {
        return allowed_repair(state, RequestRepairKind::RejectedField, body, reason);
    }
    if is_invalid_encrypted_content_error(&response) {
        if let Some(body) = remove_reasoning_encrypted_content(request)? {
            return allowed_repair(
                state,
                RequestRepairKind::InvalidEncryptedContent,
                body,
                "invalid encrypted_content",
            );
        }
    }
    Ok(None)
}

fn allowed_repair(
    state: &mut RequestRepairState,
    kind: RequestRepairKind,
    body: Vec<u8>,
    reason: &'static str,
) -> Result<Option<RequestRepair>, String> {
    if !state.allow(kind, &body) {
        return Ok(None);
    }
    Ok(Some(RequestRepair {
        body: ReplayableBody::from_bytes(body.into()),
        reason,
        kind,
    }))
}

fn repair_rejected_field(
    mut request: Value,
    response: &Value,
) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    // 结构化 code 或明确错误文案至少命中一项，避免根据模糊参数名改写请求。
    let code = error_text(response, &["/error/code", "/code"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = error_text(response, &["/error/message", "/message"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(code.trim(), "unknown_parameter" | "unsupported_parameter")
        && !message.contains("unknown parameter")
        && !message.contains("unsupported parameter")
    {
        return Ok(None);
    }

    let structured_param = error_text(response, &["/error/param", "/param"])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let param = structured_param
        .clone()
        .or_else(|| rejected_param_from_message(&message));
    let Some(param) = param else {
        return Ok(None);
    };
    if param == "max_output_tokens" {
        let Some(object) = request.as_object_mut() else {
            return Ok(None);
        };
        if object.remove("max_output_tokens").is_none() {
            return Ok(None);
        }
        return serialize_repair(request, "max_output_tokens parameter rejection").map(Some);
    }
    let Some(index) = namespace_input_index(&param) else {
        return Ok(None);
    };
    let Some(item) = request
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .and_then(|items| items.get_mut(index))
        .and_then(Value::as_object_mut)
    else {
        return Ok(None);
    };
    if !matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "tool_call" | "custom_tool_call" | "mcp_tool_call")
    ) || item.remove("namespace").is_none()
    {
        return Ok(None);
    }
    serialize_repair(request, "indexed namespace parameter rejection").map(Some)
}

fn rejected_param_from_message(message: &str) -> Option<String> {
    let marker_start = ["unknown parameter", "unsupported parameter"]
        .iter()
        .filter_map(|marker| message.find(marker).map(|index| (index, marker.len())))
        .min_by_key(|(index, _)| *index)?;
    let remainder =
        message[marker_start.0 + marker_start.1..].trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, ':' | '=' | '\'' | '"')
        });
    let remainder = remainder
        .strip_prefix("is ")
        .unwrap_or(remainder)
        .trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || matches!(character, ':' | '=' | '\'' | '"')
        });
    for candidate in ["max_output_tokens", "input["] {
        if !remainder.starts_with(candidate) {
            continue;
        }
        if candidate == "max_output_tokens" {
            let boundary = remainder.as_bytes().get(candidate.len()).copied();
            if boundary.is_none_or(|byte| !byte.is_ascii_alphanumeric() && byte != b'_') {
                return Some(candidate.to_string());
            }
            return None;
        }
        let end = remainder.find("].namespace")? + "].namespace".len();
        let value = &remainder[..end];
        if namespace_input_index(value).is_some() {
            return Some(value.to_string());
        }
    }
    None
}

fn namespace_input_index(param: &str) -> Option<usize> {
    let suffix = param.strip_prefix("input[")?;
    let (index, tail) = suffix.split_once(']')?;
    if tail != ".namespace" || index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    index.parse().ok()
}

fn is_invalid_encrypted_content_error(response: &Value) -> bool {
    // xAI/Grok 的恢复合同同时要求 code、decrypt 和 encrypted_content，普通 400 不触发。
    let code = error_text(response, &["/code", "/error/code"])
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if code != "invalid-argument" {
        return false;
    }
    let message = error_text(response, &["/error", "/error/message", "/message"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    message.contains("decrypt") && message.contains("encrypted_content")
}

fn remove_reasoning_encrypted_content(mut request: Value) -> Result<Option<Vec<u8>>, String> {
    let Some(input) = request.get_mut("input") else {
        return Ok(None);
    };
    let mut changed = false;
    match input {
        Value::Array(items) => {
            for item in items {
                changed |= remove_encrypted_content_from_reasoning(item);
            }
        }
        Value::Object(_) => changed = remove_encrypted_content_from_reasoning(input),
        _ => {}
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_vec(&request)
        .map(Some)
        .map_err(|error| format!("Failed to serialize encrypted_content repair: {error}"))
}

fn remove_encrypted_content_from_reasoning(item: &mut Value) -> bool {
    let Some(object) = item.as_object_mut() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("reasoning") {
        return false;
    }
    object.remove("encrypted_content").is_some()
}

fn error_text<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
}

fn serialize_repair(
    request: Value,
    reason: &'static str,
) -> Result<(Vec<u8>, &'static str), String> {
    serde_json::to_vec(&request)
        .map(|body| (body, reason))
        .map_err(|error| format!("Failed to serialize rejected-field repair: {error}"))
}

fn body_hash(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use serde_json::json;

    fn body(value: Value) -> ReplayableBody {
        ReplayableBody::from_bytes(Bytes::from(value.to_string()))
    }

    #[test]
    fn repairs_explicit_rejected_fields_without_touching_nested_values() {
        let initial = body(json!({
            "max_output_tokens": 2048,
            "input": [
                { "type": "message", "content": { "max_output_tokens": "keep" } },
                { "type": "function_call", "namespace": "remove", "arguments": "{}" }
            ]
        }));
        let mut state = RequestRepairState::new(&initial);
        let first_error = json!({
            "error": {
                "code": "unknown_parameter",
                "message": "Unknown parameter: input[1].namespace",
                "param": "input[1].namespace"
            }
        });
        let first = repair_request_body(
            StatusCode::BAD_REQUEST,
            &initial,
            first_error.to_string().as_bytes(),
            &mut state,
        )
        .expect("repair")
        .expect("namespace repair");
        let second_error = json!({
            "error": {
                "code": "unsupported_parameter",
                "message": "Unsupported parameter: max_output_tokens",
                "param": "max_output_tokens"
            }
        });
        let second = repair_request_body(
            StatusCode::BAD_REQUEST,
            &first.body,
            second_error.to_string().as_bytes(),
            &mut state,
        )
        .expect("repair")
        .expect("token repair");
        let value: Value = serde_json::from_slice(second.body.as_bytes()).expect("json");

        assert!(value.get("max_output_tokens").is_none());
        assert_eq!(value["input"][0]["content"]["max_output_tokens"], "keep");
        assert!(value["input"][1].get("namespace").is_none());
    }

    #[test]
    fn finds_indexed_namespace_in_rejection_message_without_structured_param() {
        let initial = body(json!({
            "input": [
                { "type": "function_call", "namespace": "keep" },
                { "type": "function_call", "namespace": "remove" }
            ]
        }));
        let error = json!({
            "error": {
                "code": "unknown_parameter",
                "message": "input[0].namespace is supported; Unknown parameter: 'input[1].namespace'."
            }
        });
        let mut state = RequestRepairState::new(&initial);

        let repaired = repair_request_body(
            StatusCode::BAD_REQUEST,
            &initial,
            error.to_string().as_bytes(),
            &mut state,
        )
        .expect("repair")
        .expect("namespace repair");
        let value: Value = serde_json::from_slice(repaired.body.as_bytes()).expect("json");

        assert_eq!(value["input"][0]["namespace"], "keep");
        assert!(value["input"][1].get("namespace").is_none());
    }

    #[test]
    fn rejects_ambiguous_field_errors() {
        let cases = [
            json!({ "error": { "code": "invalid_request_error", "message": "max_output_tokens must be positive", "param": "max_output_tokens" } }),
            json!({ "error": { "code": "unknown_parameter", "message": "Unknown parameter: max_tokens. Use max_output_tokens instead." } }),
            json!({ "error": { "code": "unknown_parameter", "message": "Unknown parameter: input[0].namespace", "param": "tools" } }),
        ];
        for error in cases {
            let initial = body(json!({
                "max_output_tokens": 2048,
                "input": [{ "type": "message", "namespace": "keep" }]
            }));
            let mut state = RequestRepairState::new(&initial);
            assert!(repair_request_body(
                StatusCode::BAD_REQUEST,
                &initial,
                error.to_string().as_bytes(),
                &mut state,
            )
            .expect("repair result")
            .is_none());
        }
    }

    #[test]
    fn removes_invalid_xai_encrypted_content_once_and_preserves_summary() {
        let initial = body(json!({
            "input": [
                { "type": "reasoning", "encrypted_content": "secret", "summary": [{ "type": "summary_text", "text": "keep" }] },
                { "type": "message", "encrypted_content": "not-reasoning" }
            ]
        }));
        let error =
            json!({ "code": "invalid-argument", "error": "Unable to decrypt encrypted_content" });
        let mut state = RequestRepairState::new(&initial);
        let repaired = repair_request_body(
            StatusCode::BAD_REQUEST,
            &initial,
            error.to_string().as_bytes(),
            &mut state,
        )
        .expect("repair")
        .expect("xAI repair");
        let value: Value = serde_json::from_slice(repaired.body.as_bytes()).expect("json");

        assert!(value["input"][0].get("encrypted_content").is_none());
        assert_eq!(value["input"][0]["summary"][0]["text"], "keep");
        assert_eq!(value["input"][1]["encrypted_content"], "not-reasoning");
        assert!(repair_request_body(
            StatusCode::BAD_REQUEST,
            &initial,
            error.to_string().as_bytes(),
            &mut state,
        )
        .expect("repair")
        .is_none());
    }
}

#[cfg(test)]
#[path = "encrypted_repair.test.rs"]
mod encrypted_repair_tests;
