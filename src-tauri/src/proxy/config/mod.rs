mod io;
mod model_mapping;
mod normalize;
mod types;

use tauri::AppHandle;

const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 20 * 1024 * 1024;

pub(crate) use io::config_dir_path;
pub(crate) use types::{
    ConfigResponse,
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

pub(crate) async fn read_config(app: AppHandle) -> Result<ConfigResponse, String> {
    let config = io::load_config_file(&app).await?;
    let path = io::config_file_path(&app)?;
    Ok(ConfigResponse {
        path: path.to_string_lossy().to_string(),
        config,
    })
}

pub(crate) fn app_proxy_url_from_config(config: &ProxyConfigFile) -> Result<Option<String>, String> {
    normalize_app_proxy_url(config.app_proxy_url.as_deref())
}

pub(crate) async fn write_config(
    app: AppHandle,
    config: ProxyConfigFile,
) -> Result<(), String> {
    build_runtime_config(config.clone())?;
    io::save_config_file(&app, &config).await
}

impl ProxyConfig {
    pub(crate) fn addr(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub(crate) async fn load(app: &AppHandle) -> Result<Self, String> {
        let config = io::load_config_file(app).await?;
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
        enable_api_format_conversion: config.enable_api_format_conversion,
        upstream_strategy: config.upstream_strategy,
        upstreams,
        kiro_preferred_endpoint: config.kiro_preferred_endpoint,
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
