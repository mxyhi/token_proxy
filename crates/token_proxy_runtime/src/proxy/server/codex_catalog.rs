use axum::{
    body::Body,
    http::{header, HeaderValue},
    response::Response,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use token_proxy_config::CodexModelCapabilities;

use crate::proxy::{
    http::RequestAuth, upstream::collect_model_catalog_entries_for_manifest, ProxyState,
};

pub(super) const PATH: &str = "/codex/models.json";
const EXTRA_MODEL_INSTRUCTIONS: &str = "You are a coding assistant. Help the user with software engineering tasks. Inspect relevant code before making changes, use the available tools when needed, and verify your work.";

/// 公共目录可远端刷新，本地额外模型在鉴权后按请求追加。
pub(super) async fn response(state: &ProxyState, auth: &RequestAuth) -> Response {
    let entries = collect_model_catalog_entries_for_manifest(state, auth).await;
    let client = state
        .http_clients
        .client_for_proxy_url(None)
        .expect("direct HTTP client initialized");
    let mut catalog = state.codex_catalog.get(&client).await;
    let models = catalog["models"]
        .as_array_mut()
        .expect("bundled Codex models array");
    let mut known: HashSet<String> = models
        .iter()
        .filter_map(|m| m["slug"].as_str().map(str::to_owned))
        .collect();
    let mut added = 0;
    for id in &state.config.codex_extra_models {
        // 仅广告显式选中且当前访问 Key 有权使用的模型，不泄露其它上游目录。
        let Some((_, display_name)) = entries.iter().find(|(entry, _)| entry == id) else {
            continue;
        };
        if known.insert(id.clone()) {
            let capabilities = state
                .config
                .codex_model_capabilities
                .get(id)
                .cloned()
                .unwrap_or_default();
            models.push(extra_model(
                id,
                display_name.as_deref().unwrap_or(id),
                &capabilities,
            ));
            added += 1;
        }
    }
    tracing::debug!(added, "serving Codex model catalog");
    let mut response = Response::new(Body::from(catalog.to_string()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    // 输出随本地配置和 Key 授权变化，不能复用上游 ETag 或共享缓存。
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn extra_model(id: &str, display_name: &str, capabilities: &CodexModelCapabilities) -> Value {
    // 只声明用户配置的能力；推理等级不隐含 summary、verbosity 或其它 GPT 专有能力。
    let input_modalities = if capabilities.supports_vision {
        vec!["text", "image"]
    } else {
        vec!["text"]
    };
    json!({
        "slug": id,
        "display_name": display_name,
        "description": "通过 Token Proxy 配置的额外模型",
        "default_reasoning_level": capabilities.reasoning_effort,
        "supported_reasoning_levels": [{"effort": capabilities.reasoning_effort, "description": "使用配置的推理等级"}],
        "shell_type": "unified_exec",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 100,
        "availability_nux": null,
        "upgrade": null,
        // Codex 的目录解析器要求每个模型有指令模板，否则会拒绝整个目录。
        "base_instructions": EXTRA_MODEL_INSTRUCTIONS,
        "model_messages": {"instructions_template": EXTRA_MODEL_INSTRUCTIONS},
        "supports_reasoning_summary_parameter": false,
        "default_reasoning_summary": "none",
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "tokens", "limit": 10000},
        "context_window": capabilities.context_window,
        "max_context_window": capabilities.context_window,
        "experimental_supported_tools": [],
        "input_modalities": input_modalities,
        "supports_image_detail_original": false,
        "supports_search_tool": false,
        "use_responses_lite": false
    })
}
