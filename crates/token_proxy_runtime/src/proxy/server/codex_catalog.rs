use axum::{
    body::Body,
    http::{header, HeaderValue},
    response::Response,
};
use serde_json::{json, Value};
use std::{collections::HashSet, sync::OnceLock};

use crate::proxy::{
    http::RequestAuth, upstream::collect_model_catalog_entries_for_manifest, ProxyState,
};

pub(super) const PATH: &str = "/codex/models.json";
const OFFICIAL_CATALOG: &str = include_str!("../../../resources/codex-models-0.163.0.json");
const EXTRA_MODEL_INSTRUCTIONS: &str = "You are a coding assistant. Help the user with software engineering tasks. Inspect relevant code before making changes, use the available tools when needed, and verify your work.";

fn official_catalog() -> &'static Value {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(OFFICIAL_CATALOG).expect("bundled Codex catalog must be valid JSON")
    })
}

/// 官方完整元数据在构建时固定版本；读取目录不依赖 OAuth 账号或实时上游请求。
pub(super) async fn response(state: &ProxyState, auth: &RequestAuth) -> Response {
    let entries = collect_model_catalog_entries_for_manifest(state, auth).await;
    let mut catalog = official_catalog().clone();
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
            models.push(extra_model(id, display_name.as_deref().unwrap_or(id)));
            added += 1;
        }
    }
    tracing::debug!(
        official_version = "0.163.0",
        added,
        "serving Codex model catalog"
    );
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

fn extra_model(id: &str, display_name: &str) -> Value {
    // shortcut: 未知模型用保守的 32K 文本/函数工具配置；有经验证的能力元数据时再扩展。
    // 不继承 GPT 的 reasoning、verbosity、Responses lite、内置搜索或专有工具声明。
    json!({
        "slug": id,
        "display_name": display_name,
        "description": "通过 Token Proxy 配置的额外模型",
        "default_reasoning_level": "none",
        "supported_reasoning_levels": [{"effort": "none", "description": "使用模型默认设置"}],
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
        "context_window": 32768,
        "experimental_supported_tools": [],
        "input_modalities": ["text"],
        "supports_search_tool": false,
        "use_responses_lite": false
    })
}
