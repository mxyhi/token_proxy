mod io;
mod migrate;
mod model_mapping;
mod normalize;
mod types;

use crate::paths::TokenProxyPaths;

const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 20 * 1024 * 1024;

pub use types::{
    ConfigResponse,
    InboundApiFormat,
    KiroPreferredEndpoint,
    ProxyConfig,
    ProxyConfigFile,
    ProviderUpstreams,
    TrayTokenRateConfig,
    TrayTokenRateFormat,
    UpstreamConfig,
    UpstreamOverrides,
    UpstreamGroup,
    HeaderOverride,
    UpstreamRuntime,
    UpstreamStrategy,
};

pub async fn read_config(paths: &TokenProxyPaths) -> Result<ConfigResponse, String> {
    let config = io::load_config_file(paths).await?;
    let path = paths.config_file();
    Ok(ConfigResponse {
        path: path.to_string_lossy().to_string(),
        config,
    })
}

pub fn app_proxy_url_from_config(config: &ProxyConfigFile) -> Result<Option<String>, String> {
    normalize_app_proxy_url(config.app_proxy_url.as_deref())
}

pub async fn write_config(paths: &TokenProxyPaths, config: ProxyConfigFile) -> Result<(), String> {
    build_runtime_config(config.clone())?;
    io::save_config_file(paths, &config).await
}

/// 初始化默认配置文件：
/// - 若文件不存在：创建并写入默认内容
/// - 若文件已存在：返回错误（避免误覆盖）
pub async fn init_default_config(paths: &TokenProxyPaths) -> Result<(), String> {
    io::init_default_config_file(paths).await
}

impl ProxyConfig {
    pub(crate) fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub async fn load(paths: &TokenProxyPaths) -> Result<Self, String> {
        let config = io::load_config_file(paths).await?;
        build_runtime_config(config)
    }

    pub(crate) fn provider_upstreams(&self, provider: &str) -> Option<&ProviderUpstreams> {
        self.upstreams.get(provider)
    }
}

fn build_runtime_config(config: ProxyConfigFile) -> Result<ProxyConfig, String> {
    let log_level = config.log_level;
    let max_request_body_bytes = resolve_max_request_body_bytes(config.max_request_body_bytes);
    let app_proxy_url = normalize_app_proxy_url(config.app_proxy_url.as_deref())?;
    let normalized_upstreams =
        normalize::normalize_upstreams(&config.upstreams, app_proxy_url.as_deref())?;
    let upstreams = normalize::build_provider_upstreams(normalized_upstreams)?;
    Ok(ProxyConfig {
        host: config.host,
        port: config.port,
        local_api_key: config.local_api_key,
        log_level,
        max_request_body_bytes,
        upstream_strategy: config.upstream_strategy,
        upstreams,
        kiro_preferred_endpoint: config.kiro_preferred_endpoint,
        antigravity_user_agent: config.antigravity_user_agent,
    })
}

fn resolve_max_request_body_bytes(value: Option<u64>) -> usize {
    let value = value.unwrap_or(DEFAULT_MAX_REQUEST_BODY_BYTES);
    let value = if value == 0 {
        DEFAULT_MAX_REQUEST_BODY_BYTES
    } else {
        value
    };
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn normalize_app_proxy_url(value: Option<&str>) -> Result<Option<String>, String> {
    let value = value.unwrap_or_default().trim();
    if value.is_empty() {
        return Ok(None);
    }
    let parsed = url::Url::parse(value).map_err(|_| "app_proxy_url is not a valid URL.".to_string())?;
    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => Ok(Some(value.to_string())),
        scheme => Err(format!("app_proxy_url scheme is not supported: {scheme}.")),
    }
}
