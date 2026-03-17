use axum::http::header::{HeaderName, HeaderValue};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use super::model_mapping::ModelMappingRules;
use crate::logging::LogLevel;

fn default_enabled() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_proxy_port() -> u16 {
    // Dev 与安装包需要可并行运行；debug 默认换一个端口，避免与 release/安装包冲突。
    if cfg!(debug_assertions) {
        19208
    } else {
        9208
    }
}

fn default_tray_token_rate_enabled() -> bool {
    true
}

fn default_log_level() -> LogLevel {
    LogLevel::Silent
}

fn default_retryable_failure_cooldown_secs() -> u64 {
    15
}

fn is_default_retryable_failure_cooldown_secs(value: &u64) -> bool {
    *value == default_retryable_failure_cooldown_secs()
}

fn default_upstream_no_data_timeout_secs() -> u64 {
    120
}

fn is_default_upstream_no_data_timeout_secs(value: &u64) -> bool {
    *value == default_upstream_no_data_timeout_secs()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundApiFormat {
    OpenaiChat,
    OpenaiResponses,
    AnthropicMessages,
    Gemini,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InboundApiFormatMask(u8);

impl InboundApiFormatMask {
    pub(crate) fn contains(self, format: InboundApiFormat) -> bool {
        self.0 & format.bit() != 0
    }

    pub(crate) fn insert(&mut self, format: InboundApiFormat) {
        self.0 |= format.bit();
    }

    pub(crate) fn extend(&mut self, formats: impl IntoIterator<Item = InboundApiFormat>) {
        for format in formats {
            self.insert(format);
        }
    }
}

impl InboundApiFormat {
    const fn bit(self) -> u8 {
        match self {
            Self::OpenaiChat => 1 << 0,
            Self::OpenaiResponses => 1 << 1,
            Self::AnthropicMessages => 1 << 2,
            Self::Gemini => 1 << 3,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamStrategy {
    PriorityRoundRobin,
    PriorityFillFirst,
}

impl Default for UpstreamStrategy {
    fn default() -> Self {
        Self::PriorityFillFirst
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayTokenRateFormat {
    Combined,
    Split,
    Both,
}

impl Default for TrayTokenRateFormat {
    fn default() -> Self {
        Self::Split
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KiroPreferredEndpoint {
    Ide,
    Cli,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TrayTokenRateConfig {
    #[serde(default = "default_tray_token_rate_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub format: TrayTokenRateFormat,
}

impl Default for TrayTokenRateConfig {
    fn default() -> Self {
        Self {
            enabled: default_tray_token_rate_enabled(),
            format: TrayTokenRateFormat::default(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<String>,
    /// Only meaningful for provider "openai-response": strip `prompt_cache_retention` from /v1/responses requests.
    #[serde(default, skip_serializing_if = "is_false")]
    pub filter_prompt_cache_retention: bool,
    /// Only meaningful for provider "openai-response": strip `safety_identifier` from /v1/responses requests.
    #[serde(default, skip_serializing_if = "is_false")]
    pub filter_safety_identifier: bool,
    /// Only meaningful for provider "openai-response": send inbound `/v1/responses` traffic to `/v1/chat/completions`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_chat_completions_for_responses: bool,
    /// Rewrite OpenAI-compatible message role `developer` to `system` before forwarding upstream.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rewrite_developer_role_to_system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub proxy_url: Option<String>,
    pub priority: Option<i32>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub model_mappings: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub convert_from_map: HashMap<String, Vec<InboundApiFormat>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<UpstreamOverrides>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct UpstreamOverrides {
    #[serde(default)]
    pub header: HashMap<String, Option<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProxyConfigFile {
    pub host: String,
    pub port: u16,
    pub local_api_key: Option<String>,
    pub app_proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_ide_db_path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antigravity_app_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub antigravity_process_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antigravity_user_agent: Option<String>,
    #[serde(
        default = "default_log_level",
        deserialize_with = "deserialize_log_level"
    )]
    pub log_level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_request_body_bytes: Option<u64>,
    #[serde(
        default = "default_retryable_failure_cooldown_secs",
        skip_serializing_if = "is_default_retryable_failure_cooldown_secs"
    )]
    pub retryable_failure_cooldown_secs: u64,
    #[serde(
        default = "default_upstream_no_data_timeout_secs",
        skip_serializing_if = "is_default_upstream_no_data_timeout_secs"
    )]
    pub upstream_no_data_timeout_secs: u64,
    #[serde(default)]
    pub tray_token_rate: TrayTokenRateConfig,
    #[serde(default)]
    pub upstream_strategy: UpstreamStrategy,
    #[serde(default)]
    pub upstreams: Vec<UpstreamConfig>,
}

impl Default for ProxyConfigFile {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: default_proxy_port(),
            local_api_key: None,
            app_proxy_url: None,
            kiro_preferred_endpoint: None,
            antigravity_ide_db_path: None,
            antigravity_app_paths: Vec::new(),
            antigravity_process_names: Vec::new(),
            antigravity_user_agent: None,
            log_level: LogLevel::default(),
            max_request_body_bytes: None,
            retryable_failure_cooldown_secs: default_retryable_failure_cooldown_secs(),
            upstream_no_data_timeout_secs: default_upstream_no_data_timeout_secs(),
            tray_token_rate: TrayTokenRateConfig::default(),
            upstream_strategy: UpstreamStrategy::PriorityFillFirst,
            upstreams: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
    pub local_api_key: Option<String>,
    pub log_level: LogLevel,
    pub max_request_body_bytes: usize,
    pub retryable_failure_cooldown: std::time::Duration,
    pub upstream_no_data_timeout: std::time::Duration,
    pub upstream_strategy: UpstreamStrategy,
    pub upstreams: HashMap<String, ProviderUpstreams>,
    pub kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub antigravity_user_agent: Option<String>,
}

fn deserialize_log_level<'de, D>(deserializer: D) -> Result<LogLevel, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    let value = raw.unwrap_or_default().trim().to_ascii_lowercase();
    if value.is_empty() {
        return Ok(LogLevel::Silent);
    }
    match value.as_str() {
        "silent" => Ok(LogLevel::Silent),
        "error" => Ok(LogLevel::Error),
        "warn" | "warning" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        other => Err(de::Error::custom(format!("invalid log_level: {other}"))),
    }
}

#[derive(Clone)]
pub struct ProviderUpstreams {
    pub groups: Vec<UpstreamGroup>,
}

#[derive(Clone)]
pub struct UpstreamGroup {
    pub priority: i32,
    pub items: Vec<UpstreamRuntime>,
}

#[derive(Clone)]
pub struct UpstreamRuntime {
    pub(crate) id: String,
    pub(crate) selector_key: String,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) filter_prompt_cache_retention: bool,
    pub(crate) filter_safety_identifier: bool,
    pub(crate) rewrite_developer_role_to_system: bool,
    pub(crate) kiro_account_id: Option<String>,
    pub(crate) codex_account_id: Option<String>,
    pub(crate) antigravity_account_id: Option<String>,
    pub(crate) kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub(crate) proxy_url: Option<String>,
    pub(crate) priority: i32,
    pub(crate) model_mappings: Option<ModelMappingRules>,
    pub(crate) header_overrides: Option<Vec<HeaderOverride>>,
    pub(crate) allowed_inbound_formats: InboundApiFormatMask,
}

#[derive(Clone)]
pub struct HeaderOverride {
    pub name: HeaderName,
    pub value: Option<HeaderValue>,
}

impl UpstreamRuntime {
    /// 构建上游请求 URL，智能处理 base_url 与 path 的路径重叠
    /// 例如：base_url = "https://example.com/openai/v1", path = "/v1/chat/completions"
    /// 结果：https://example.com/openai/v1/chat/completions（去掉重复的 /v1）
    pub(crate) fn upstream_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let normalized_path = normalize_openai_compatible_path_for_base_url(base, path);
        let effective_path = strip_overlapping_prefix(base, normalized_path);
        format!("{base}{effective_path}")
    }

    pub(crate) fn map_model(&self, model: &str) -> Option<String> {
        self.model_mappings
            .as_ref()
            .and_then(|rules| rules.map_model(model))
            .map(|value| value.to_string())
    }

    pub(crate) fn supports_inbound(&self, format: InboundApiFormat) -> bool {
        self.allowed_inbound_formats.contains(format)
    }
}

fn is_bigmodel_coding_plan_base_url(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };
    url.path()
        .trim_end_matches('/')
        .contains("/api/coding/paas/")
}

fn normalize_openai_compatible_path_for_base_url<'a>(base_url: &str, path: &'a str) -> &'a str {
    if is_bigmodel_coding_plan_base_url(base_url) && path == "/v1/chat/completions" {
        return "/chat/completions";
    }
    path
}

#[derive(Serialize)]
pub struct ConfigResponse {
    pub path: String,
    pub config: ProxyConfigFile,
}

/// 去掉 path 开头与 base_url 路径部分重叠的前缀
/// base_url: "https://example.com/openai/v1" -> base_path: "/openai/v1"
/// 如果 path 以 base_path 的某个后缀开头（如 "/v1"），则去掉该重叠部分
pub(crate) fn strip_overlapping_prefix<'a>(base_url: &str, path: &'a str) -> &'a str {
    let Some(base_path) = url::Url::parse(base_url)
        .ok()
        .map(|url| url.path().to_string())
    else {
        return path;
    };
    // 检查 base_path 的每个后缀是否与 path 的前缀重叠
    // 例如 base_path = "/openai/v1"，依次检查 "/openai/v1", "/v1"
    let base_path = base_path.trim_end_matches('/');
    for (idx, ch) in base_path.char_indices() {
        if ch == '/' {
            let suffix = &base_path[idx..];
            if path.starts_with(suffix) {
                return &path[suffix.len()..];
            }
        }
    }
    // 完整匹配检查（base_path 本身）
    if path.starts_with(base_path) {
        return &path[base_path.len()..];
    }
    path
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "types.test.rs"]
mod tests;
