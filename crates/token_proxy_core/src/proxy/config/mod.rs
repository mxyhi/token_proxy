mod io;
mod migrate;
mod model_mapping;
mod normalize;
mod types;

use crate::paths::TokenProxyPaths;
use std::time::{Duration, Instant};

const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 20 * 1024 * 1024;
const MIN_UPSTREAM_NO_DATA_TIMEOUT_SECS: u64 = 3;

pub use types::{
    ConfigResponse, HeaderOverride, InboundApiFormat, KiroPreferredEndpoint, ProviderUpstreams,
    ProxyConfig, ProxyConfigFile, TrayTokenRateConfig, TrayTokenRateFormat, UpstreamConfig,
    UpstreamDispatchRuntime, UpstreamDispatchStrategy, UpstreamGroup, UpstreamOrderStrategy,
    UpstreamOverrides, UpstreamRuntime, UpstreamStrategy, UpstreamStrategyRuntime,
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
        retryable_failure_cooldown: resolve_retryable_failure_cooldown(
            config.retryable_failure_cooldown_secs,
        )?,
        upstream_no_data_timeout: resolve_upstream_no_data_timeout(
            config.upstream_no_data_timeout_secs,
        )?,
        upstream_strategy: resolve_upstream_strategy(config.upstream_strategy)?,
        upstreams,
        kiro_preferred_endpoint: config.kiro_preferred_endpoint,
        antigravity_user_agent: config.antigravity_user_agent,
    })
}

fn resolve_retryable_failure_cooldown(value: u64) -> Result<Duration, String> {
    let duration = Duration::from_secs(value);
    if Instant::now().checked_add(duration).is_none() {
        return Err("retryable_failure_cooldown_secs is too large.".to_string());
    }
    Ok(duration)
}

fn resolve_upstream_no_data_timeout(value: u64) -> Result<Duration, String> {
    if value < MIN_UPSTREAM_NO_DATA_TIMEOUT_SECS {
        return Err(format!(
            "upstream_no_data_timeout_secs must be at least {MIN_UPSTREAM_NO_DATA_TIMEOUT_SECS}."
        ));
    }
    let duration = Duration::from_secs(value);
    if Instant::now().checked_add(duration).is_none() {
        return Err("upstream_no_data_timeout_secs is too large.".to_string());
    }
    Ok(duration)
}

fn resolve_upstream_strategy(value: UpstreamStrategy) -> Result<UpstreamStrategyRuntime, String> {
    let dispatch = match value.dispatch {
        UpstreamDispatchStrategy::Serial => UpstreamDispatchRuntime::Serial,
        UpstreamDispatchStrategy::Hedged {
            delay_ms,
            max_parallel,
        } => UpstreamDispatchRuntime::Hedged {
            delay: resolve_hedged_delay(delay_ms)?,
            max_parallel: resolve_parallel_attempts("hedged", max_parallel)?,
        },
        UpstreamDispatchStrategy::Race { max_parallel } => UpstreamDispatchRuntime::Race {
            max_parallel: resolve_parallel_attempts("race", max_parallel)?,
        },
    };
    Ok(UpstreamStrategyRuntime {
        order: value.order,
        dispatch,
    })
}

fn resolve_hedged_delay(value: u64) -> Result<Duration, String> {
    if value == 0 {
        return Err("upstream_strategy.dispatch.delay_ms must be at least 1.".to_string());
    }
    let duration = Duration::from_millis(value);
    if Instant::now().checked_add(duration).is_none() {
        return Err("upstream_strategy.dispatch.delay_ms is too large.".to_string());
    }
    Ok(duration)
}

fn resolve_parallel_attempts(dispatch: &str, value: u64) -> Result<usize, String> {
    if value < 2 {
        return Err(format!(
            "upstream_strategy.dispatch.max_parallel must be at least 2 for {dispatch}."
        ));
    }
    usize::try_from(value)
        .map_err(|_| "upstream_strategy.dispatch.max_parallel is too large.".to_string())
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
    let parsed =
        url::Url::parse(value).map_err(|_| "app_proxy_url is not a valid URL.".to_string())?;
    match parsed.scheme() {
        "http" | "https" | "socks5" | "socks5h" => Ok(Some(value.to_string())),
        scheme => Err(format!("app_proxy_url scheme is not supported: {scheme}.")),
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
