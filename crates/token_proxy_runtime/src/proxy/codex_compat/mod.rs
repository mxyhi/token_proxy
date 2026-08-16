use axum::body::Bytes;
use axum::http::HeaderMap;
use std::collections::HashMap;

mod headers;
mod namespace;
mod request;
mod response;
mod stream;
mod tool_names;

#[derive(Clone)]
pub(super) struct RestoredToolName {
    pub(super) name: String,
    pub(super) namespace: Option<String>,
}

pub(crate) use headers::{
    apply_codex_headers, ensure_remote_compaction_feature, is_native_codex_request,
};
#[cfg(test)]
pub(crate) use request::{chat_request_to_codex, responses_request_to_codex};
pub(crate) use request::{
    chat_request_to_codex_with_prompt_cache_key, responses_request_to_codex_with_prompt_cache_key,
    supported_codex_model_ids,
};
pub(crate) use response::{codex_response_to_chat, codex_response_to_responses};
pub(crate) use stream::{
    stream_chat_error_sse, stream_codex_to_chat, stream_codex_to_responses,
    stream_codex_to_responses_with_semantic_timeout, stream_responses_error_sse,
};

pub(crate) fn extract_tool_name_map_from_request_body(
    body: Option<&str>,
) -> HashMap<String, RestoredToolName> {
    let Some(body) = body else {
        return HashMap::new();
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
mod tests;
