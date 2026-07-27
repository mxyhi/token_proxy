use axum::http::header::{HeaderName, HeaderValue};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use super::hot_model_mappings::default_hot_model_mappings;
use super::model_mapping::ModelMappingRules;
use crate::LogLevel;

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

fn default_same_upstream_retry_count() -> u64 {
    1
}

fn default_model_list_prefix() -> bool {
    false
}

fn is_default_retryable_failure_cooldown_secs(value: &u64) -> bool {
    *value == default_retryable_failure_cooldown_secs()
}

fn is_default_same_upstream_retry_count(value: &u64) -> bool {
    *value == default_same_upstream_retry_count()
}

fn default_stream_first_output_timeout_secs() -> u64 {
    60
}

fn is_default_stream_first_output_timeout_secs(value: &u64) -> bool {
    *value == default_stream_first_output_timeout_secs()
}

fn default_sync_response_timeout_secs() -> u64 {
    300
}

fn is_default_sync_response_timeout_secs(value: &u64) -> bool {
    *value == default_sync_response_timeout_secs()
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
pub struct InboundApiFormatMask(u8);

impl InboundApiFormatMask {
    pub fn contains(self, format: InboundApiFormat) -> bool {
        self.0 & format.bit() != 0
    }

    pub fn insert(&mut self, format: InboundApiFormat) {
        self.0 |= format.bit();
    }

    pub fn extend(&mut self, formats: impl IntoIterator<Item = InboundApiFormat>) {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamOrderStrategy {
    #[default]
    FillFirst,
    RoundRobin,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpstreamDispatchStrategy {
    #[default]
    Serial,
    Hedged {
        delay_ms: u64,
        max_parallel: u64,
    },
    Race {
        max_parallel: u64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UpstreamStrategy {
    #[serde(default)]
    pub order: UpstreamOrderStrategy,
    #[serde(default)]
    pub dispatch: UpstreamDispatchStrategy,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayTokenRateFormat {
    Combined,
    #[default]
    Split,
    Both,
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

/// 账户型上游 provider：仅 kiro / codex / xai 可绑定 account_id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountProvider {
    Kiro,
    Codex,
    Xai,
}

impl AccountProvider {
    /// 配置与 runtime 共用的 provider 字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kiro => "kiro",
            Self::Codex => "codex",
            Self::Xai => "xai",
        }
    }

    /// 从配置字符串解析；非账户型返回 None。
    pub fn from_provider_str(value: &str) -> Option<Self> {
        match value.trim() {
            "kiro" => Some(Self::Kiro),
            "codex" => Some(Self::Codex),
            "xai" => Some(Self::Xai),
            _ => None,
        }
    }
}

/// Upstream 唯一凭据形态：API Key 列表、固定账户引用、或透传/无密钥。
/// 禁止再使用平铺 `api_keys` + 多个可选 `*_account_id` 的无效组合。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UpstreamCredential {
    /// 一个或多个静态 API Key；normalize 时按 key 展开为多条 runtime。
    ApiKeys {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        api_keys: Vec<String>,
    },
    /// 账户型上游：provider 必须与上游 providers 中唯一账户型 provider 一致，且固定 account_id。
    Account {
        provider: AccountProvider,
        account_id: String,
    },
    /// 无上游密钥（例如依赖客户端透传或 provider 自身鉴权）。
    Passthrough,
}

impl Default for UpstreamCredential {
    fn default() -> Self {
        Self::Passthrough
    }
}

impl UpstreamCredential {
    pub fn api_keys(keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::ApiKeys {
            api_keys: keys.into_iter().map(Into::into).collect(),
        }
    }

    pub fn account(provider: AccountProvider, account_id: impl Into<String>) -> Self {
        Self::Account {
            provider,
            account_id: account_id.into(),
        }
    }

    pub fn is_account(&self) -> bool {
        matches!(self, Self::Account { .. })
    }

    pub fn account_binding(&self) -> Option<(AccountProvider, &str)> {
        match self {
            Self::Account {
                provider,
                account_id,
            } => Some((*provider, account_id.as_str())),
            _ => None,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    pub base_url: String,
    /// 判别联合凭据；旧平铺字段由 migrate 一次性升级。
    #[serde(default)]
    pub credential: UpstreamCredential,
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
    pub preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub proxy_url: Option<String>,
    pub priority: Option<i32>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Inbound model ids accepted by this upstream. An empty list allows every model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_models: Vec<String>,
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub cors_enabled: bool,
    #[serde(
        default = "default_model_list_prefix",
        skip_serializing_if = "is_false"
    )]
    pub model_list_prefix: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
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
    /// 可重试失败时，同一上游原地额外重试次数（不含首次发送）；0 关闭，默认 1。
    #[serde(
        default = "default_same_upstream_retry_count",
        skip_serializing_if = "is_default_same_upstream_retry_count"
    )]
    pub same_upstream_retry_count: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub codex_session_scoped_cooldown_enabled: bool,
    /// 是否为原生 xAI Responses 请求自动补充服务端 `x_search` 工具。
    #[serde(default, skip_serializing_if = "is_false")]
    pub xai_inject_x_search: bool,
    #[serde(
        default = "default_stream_first_output_timeout_secs",
        skip_serializing_if = "is_default_stream_first_output_timeout_secs"
    )]
    pub stream_first_output_timeout_secs: u64,
    #[serde(
        default = "default_sync_response_timeout_secs",
        skip_serializing_if = "is_default_sync_response_timeout_secs"
    )]
    pub sync_response_timeout_secs: u64,
    #[serde(default)]
    pub tray_token_rate: TrayTokenRateConfig,
    #[serde(default)]
    pub upstream_strategy: UpstreamStrategy,
    #[serde(default = "default_hot_model_mappings")]
    pub hot_model_mappings: HashMap<String, String>,
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
            cors_enabled: false,
            model_list_prefix: default_model_list_prefix(),
            kiro_preferred_endpoint: None,
            log_level: LogLevel::default(),
            max_request_body_bytes: None,
            retryable_failure_cooldown_secs: default_retryable_failure_cooldown_secs(),
            same_upstream_retry_count: default_same_upstream_retry_count(),
            codex_session_scoped_cooldown_enabled: false,
            xai_inject_x_search: false,
            stream_first_output_timeout_secs: default_stream_first_output_timeout_secs(),
            sync_response_timeout_secs: default_sync_response_timeout_secs(),
            tray_token_rate: TrayTokenRateConfig::default(),
            upstream_strategy: UpstreamStrategy::default(),
            hot_model_mappings: default_hot_model_mappings(),
            upstreams: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpstreamStrategyRuntime {
    pub order: UpstreamOrderStrategy,
    pub dispatch: UpstreamDispatchRuntime,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum UpstreamDispatchRuntime {
    #[default]
    Serial,
    Hedged {
        delay: std::time::Duration,
        max_parallel: usize,
    },
    Race {
        max_parallel: usize,
    },
}

#[derive(Clone)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
    pub local_api_key: Option<String>,
    pub cors_enabled: bool,
    pub model_list_prefix: bool,
    pub log_level: LogLevel,
    pub max_request_body_bytes: usize,
    pub retryable_failure_cooldown: std::time::Duration,
    /// 同一上游原地额外重试次数（不含首次）；运行时已校验上限。
    pub same_upstream_retry_count: u32,
    pub codex_session_scoped_cooldown_enabled: bool,
    /// 原生 xAI Responses 是否注入服务端 X Search 工具。
    pub xai_inject_x_search: bool,
    pub stream_first_output_timeout: std::time::Duration,
    pub sync_response_timeout: std::time::Duration,
    pub upstream_strategy: UpstreamStrategyRuntime,
    pub hot_model_mappings: HashMap<String, String>,
    pub upstreams: HashMap<String, ProviderUpstreams>,
    pub kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
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
pub struct StaticApiKeyHeaders {
    raw: HeaderValue,
    bearer: HeaderValue,
}

impl StaticApiKeyHeaders {
    pub fn new(upstream_id: &str, key: &str) -> Result<Self, String> {
        let raw = HeaderValue::from_str(key).map_err(|_| {
            format!("Upstream {upstream_id} API key contains invalid header characters.")
        })?;
        let bearer = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| {
            format!("Upstream {upstream_id} API key contains invalid header characters.")
        })?;
        Ok(Self { raw, bearer })
    }

    pub fn raw(&self) -> HeaderValue {
        self.raw.clone()
    }

    pub fn bearer(&self) -> HeaderValue {
        self.bearer.clone()
    }
}

#[derive(Clone)]
pub struct UpstreamRuntime {
    pub id: String,
    pub selector_key: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub api_key_headers: Option<StaticApiKeyHeaders>,
    pub filter_prompt_cache_retention: bool,
    pub filter_safety_identifier: bool,
    pub rewrite_developer_role_to_system: bool,
    pub kiro_account_id: Option<String>,
    pub codex_account_id: Option<String>,
    pub xai_account_id: Option<String>,
    pub kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub proxy_url: Option<String>,
    pub priority: i32,
    pub available_models: Vec<String>,
    pub advertised_model_ids: Vec<String>,
    pub model_mappings: Option<ModelMappingRules>,
    pub header_overrides: Option<Vec<HeaderOverride>>,
    pub allowed_inbound_formats: InboundApiFormatMask,
}

#[derive(Clone)]
pub struct HeaderOverride {
    pub name: HeaderName,
    pub value: Option<HeaderValue>,
}

impl UpstreamRuntime {
    /// Extends accepted inbound formats for runtime and integration setup.
    pub fn allow_inbound_formats(&mut self, formats: impl IntoIterator<Item = InboundApiFormat>) {
        self.allowed_inbound_formats.extend(formats);
    }

    /// 构建上游请求 URL，智能处理 base_url 与 path 的路径重叠
    /// 例如：base_url = "https://example.com/openai/v1", path = "/v1/chat/completions"
    /// 结果：https://example.com/openai/v1/chat/completions（去掉重复的 /v1）
    pub fn upstream_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let normalized_path = normalize_openai_compatible_path_for_base_url(base, path);
        let effective_path = strip_overlapping_prefix(base, normalized_path);
        format!("{base}{effective_path}")
    }

    pub fn map_model(&self, model: &str) -> Option<String> {
        self.model_mappings
            .as_ref()
            .and_then(|rules| rules.map_model(model))
            .map(|value| value.to_string())
    }

    pub fn supports_inbound(&self, format: InboundApiFormat) -> bool {
        self.allowed_inbound_formats.contains(format)
    }

    pub fn supports_model(&self, original_model: Option<&str>) -> bool {
        if self.available_models.is_empty() {
            return true;
        }
        let Some(original_model) = original_model else {
            return true;
        };
        let model = original_model
            .split_once('/')
            .filter(|(prefix, rest)| *prefix == self.id && !rest.trim().is_empty())
            .map_or(original_model, |(_, rest)| rest);
        self.available_models
            .binary_search_by(|candidate| candidate.as_str().cmp(model))
            .is_ok()
    }

    pub fn restrict_model_catalog(&self, models: &mut Vec<String>) {
        if self.available_models.is_empty() {
            return;
        }
        models.retain(|model| {
            self.available_models
                .binary_search_by(|candidate| candidate.as_str().cmp(model.as_str()))
                .is_ok()
        });
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
            if let Some(stripped) = path.strip_prefix(suffix) {
                return stripped;
            }
        }
    }
    // 完整匹配检查（base_path 本身）
    if let Some(stripped) = path.strip_prefix(base_path) {
        return stripped;
    }
    path
}

// 单元测试拆到独立文件，使用 `#[path]` 以保持 `.test.rs` 命名约定。
#[cfg(test)]
#[path = "types.test.rs"]
mod tests;
