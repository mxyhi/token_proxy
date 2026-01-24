use axum::body::Bytes;
use axum::http::HeaderMap;

mod headers;
mod request;
mod response;
mod stream;
mod tool_names;

pub(crate) use headers::apply_codex_headers;
pub(crate) use request::{chat_request_to_codex, responses_request_to_codex};
pub(crate) use response::{codex_response_to_chat, codex_response_to_responses};
pub(crate) use stream::{stream_codex_to_chat, stream_codex_to_responses};

pub(crate) fn extract_tool_name_map_from_request_body(
    body: Option<&str>,
) -> std::collections::HashMap<String, String> {
    let Some(body) = body else {
        return std::collections::HashMap::new();
    };
    let bytes = Bytes::copy_from_slice(body.as_bytes());
    request::extract_tool_name_map(&bytes).unwrap_or_default()
}

pub(crate) fn apply_codex_headers_if_needed(
    provider: &str,
    headers: &mut HeaderMap,
    inbound: &HeaderMap,
) {
    if provider != "codex" {
        return;
    }
    apply_codex_headers(headers, inbound);
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "codex_compat.test.rs"]
mod tests;
