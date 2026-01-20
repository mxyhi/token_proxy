use axum::http::header::{HeaderName, HeaderValue};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use super::model_mapping::ModelMappingRules;
use crate::logging::LogLevel;

fn default_enabled() -> bool {
    true
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UpstreamStrategy {
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
pub(crate) enum TrayTokenRateFormat {
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
pub(crate) enum KiroPreferredEndpoint {
    Ide,
    Cli,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TrayTokenRateConfig {
    #[serde(default = "default_tray_token_rate_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) format: TrayTokenRateFormat,
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
pub(crate) struct UpstreamConfig {
    pub(crate) id: String,
    pub(crate) provider: String,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kiro_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub(crate) proxy_url: Option<String>,
    pub(crate) priority: Option<i32>,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) model_mappings: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) overrides: Option<UpstreamOverrides>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub(crate) struct UpstreamOverrides {
    #[serde(default)]
    pub(crate) header: HashMap<String, Option<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProxyConfigFile {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) local_api_key: Option<String>,
    pub(crate) app_proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    #[serde(default = "default_log_level", deserialize_with = "deserialize_log_level")]
    pub(crate) log_level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_request_body_bytes: Option<u64>,
    #[serde(default)]
    pub(crate) tray_token_rate: TrayTokenRateConfig,
    /// 是否允许在不同 API 格式之间自动互转（例如 OpenAI Chat↔Responses、Claude Messages↔OpenAI Responses）。
    /// 默认为开启；关闭时将严格按 provider 路由，不做格式转换。
    #[serde(default)]
    pub(crate) enable_api_format_conversion: bool,
    #[serde(default)]
    pub(crate) upstream_strategy: UpstreamStrategy,
    #[serde(default)]
    pub(crate) upstreams: Vec<UpstreamConfig>,
}

impl Default for ProxyConfigFile {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: default_proxy_port(),
            local_api_key: None,
            app_proxy_url: None,
            kiro_preferred_endpoint: None,
            log_level: LogLevel::default(),
            max_request_body_bytes: None,
            tray_token_rate: TrayTokenRateConfig::default(),
            enable_api_format_conversion: true,
            upstream_strategy: UpstreamStrategy::PriorityFillFirst,
            upstreams: vec![
                UpstreamConfig {
                    id: "openai-default".to_string(),
                    provider: "openai".to_string(),
                    base_url: "https://api.openai.com".to_string(),
                    api_key: None,
                    kiro_account_id: None,
                    preferred_endpoint: None,
                    proxy_url: None,
                    priority: Some(0),
                    enabled: true,
                    model_mappings: HashMap::new(),
                    overrides: None,
                },
                UpstreamConfig {
                    id: "openai-responses".to_string(),
                    provider: "openai-response".to_string(),
                    base_url: "https://api.openai.com".to_string(),
                    api_key: None,
                    kiro_account_id: None,
                    preferred_endpoint: None,
                    proxy_url: None,
                    priority: Some(0),
                    enabled: true,
                    model_mappings: HashMap::new(),
                    overrides: None,
                },
                UpstreamConfig {
                    id: "anthropic-default".to_string(),
                    provider: "anthropic".to_string(),
                    base_url: "https://api.anthropic.com".to_string(),
                    api_key: None,
                    kiro_account_id: None,
                    preferred_endpoint: None,
                    proxy_url: None,
                    priority: Some(0),
                    enabled: true,
                    model_mappings: HashMap::new(),
                    overrides: None,
                },
                UpstreamConfig {
                    id: "gemini-default".to_string(),
                    provider: "gemini".to_string(),
                    base_url: "https://generativelanguage.googleapis.com".to_string(),
                    api_key: None,
                    kiro_account_id: None,
                    preferred_endpoint: None,
                    proxy_url: None,
                    priority: Some(0),
                    enabled: true,
                    model_mappings: HashMap::new(),
                    overrides: None,
                },
            ],
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProxyConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) local_api_key: Option<String>,
    pub(crate) log_level: LogLevel,
    pub(crate) max_request_body_bytes: usize,
    pub(crate) enable_api_format_conversion: bool,
    pub(crate) upstream_strategy: UpstreamStrategy,
    pub(crate) upstreams: HashMap<String, ProviderUpstreams>,
    pub(crate) kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
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
pub(crate) struct ProviderUpstreams {
    pub(crate) groups: Vec<UpstreamGroup>,
}

#[derive(Clone)]
pub(crate) struct UpstreamGroup {
    pub(crate) priority: i32,
    pub(crate) items: Vec<UpstreamRuntime>,
}

#[derive(Clone)]
pub(crate) struct UpstreamRuntime {
    pub(crate) id: String,
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) kiro_account_id: Option<String>,
    pub(crate) kiro_preferred_endpoint: Option<KiroPreferredEndpoint>,
    pub(crate) proxy_url: Option<String>,
    pub(crate) priority: i32,
    pub(crate) model_mappings: Option<ModelMappingRules>,
    pub(crate) header_overrides: Option<Vec<HeaderOverride>>,
}

#[derive(Clone)]
pub(crate) struct HeaderOverride {
    pub(crate) name: HeaderName,
    pub(crate) value: Option<HeaderValue>,
}

impl UpstreamRuntime {
    /// 构建上游请求 URL，智能处理 base_url 与 path 的路径重叠
    /// 例如：base_url = "https://example.com/openai/v1", path = "/v1/chat/completions"
    /// 结果：https://example.com/openai/v1/chat/completions（去掉重复的 /v1）
    pub(crate) fn upstream_url(&self, path: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let effective_path = strip_overlapping_prefix(base, path);
        format!("{base}{effective_path}")
    }

    pub(crate) fn map_model(&self, model: &str) -> Option<String> {
        self.model_mappings
            .as_ref()
            .and_then(|rules| rules.map_model(model))
            .map(|value| value.to_string())
    }
}

#[derive(Serialize)]
pub(crate) struct ConfigResponse {
    pub(crate) path: String,
    pub(crate) config: ProxyConfigFile,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_overlapping_prefix() {
        // 标准 OpenAI 兼容格式：base_url 包含 /v1
        assert_eq!(
            strip_overlapping_prefix("https://api.example.com/openai/v1", "/v1/chat/completions"),
            "/chat/completions"
        );
        assert_eq!(
            strip_overlapping_prefix("https://api.example.com/v1", "/v1/chat/completions"),
            "/chat/completions"
        );

        // 无重叠情况：base_url 不包含路径
        assert_eq!(
            strip_overlapping_prefix("https://api.openai.com", "/v1/chat/completions"),
            "/v1/chat/completions"
        );

        // 无重叠情况：base_url 路径与请求路径无公共后缀
        assert_eq!(
            strip_overlapping_prefix("https://api.example.com/openai/", "/v1/chat/completions"),
            "/v1/chat/completions"
        );
        assert_eq!(
            strip_overlapping_prefix("https://api.example.com/openai", "/v1/chat/completions"),
            "/v1/chat/completions"
        );

        // 多层路径重叠
        assert_eq!(
            strip_overlapping_prefix("https://example.com/api/openai/v1", "/v1/models"),
            "/models"
        );

        // 完整路径重叠
        assert_eq!(
            strip_overlapping_prefix("https://example.com/openai/v1", "/openai/v1/completions"),
            "/completions"
        );

        // 带尾斜杠的 base_url
        assert_eq!(
            strip_overlapping_prefix("https://example.com/v1/", "/v1/chat/completions"),
            "/chat/completions"
        );

        // 无效 URL 回退
        assert_eq!(
            strip_overlapping_prefix("not-a-valid-url", "/v1/chat/completions"),
            "/v1/chat/completions"
        );
    }

    #[test]
    fn test_upstream_url() {
        // openai provider: /v1/chat/completions
        let upstream = UpstreamRuntime {
            id: "test".to_string(),
            base_url: "https://api.example.com/openai/v1".to_string(),
            api_key: None,
            kiro_account_id: None,
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: 0,
            model_mappings: None,
            header_overrides: None,
        };
        assert_eq!(
            upstream.upstream_url("/v1/chat/completions"),
            "https://api.example.com/openai/v1/chat/completions"
        );

        // openai-response provider: /v1/responses
        let upstream_responses = UpstreamRuntime {
            id: "test".to_string(),
            base_url: "https://api.example.com/openai/v1".to_string(),
            api_key: None,
            kiro_account_id: None,
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: 0,
            model_mappings: None,
            header_overrides: None,
        };
        assert_eq!(
            upstream_responses.upstream_url("/v1/responses"),
            "https://api.example.com/openai/v1/responses"
        );

        // 无路径前缀的 base_url
        let upstream_no_path = UpstreamRuntime {
            id: "test".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: None,
            kiro_account_id: None,
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: 0,
            model_mappings: None,
            header_overrides: None,
        };
        assert_eq!(
            upstream_no_path.upstream_url("/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            upstream_no_path.upstream_url("/v1/responses"),
            "https://api.openai.com/v1/responses"
        );

        // 带尾斜杠的 base_url
        let upstream_trailing_slash = UpstreamRuntime {
            id: "test".to_string(),
            base_url: "https://api.example.com/openai/v1/".to_string(),
            api_key: None,
            kiro_account_id: None,
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: 0,
            model_mappings: None,
            header_overrides: None,
        };
        // openai: /v1/chat/completions
        assert_eq!(
            upstream_trailing_slash.upstream_url("/v1/chat/completions"),
            "https://api.example.com/openai/v1/chat/completions"
        );
        // openai-response: /v1/responses
        assert_eq!(
            upstream_trailing_slash.upstream_url("/v1/responses"),
            "https://api.example.com/openai/v1/responses"
        );
        // anthropic: /v1/messages
        assert_eq!(
            upstream_trailing_slash.upstream_url("/v1/messages"),
            "https://api.example.com/openai/v1/messages"
        );
    }
}
