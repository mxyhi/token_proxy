//! my_proxy_endpoint —— 仪表盘「代理接入地址」栏的配置段与解析（fork 本地增强）。
//!
//! 与 `my_stats_api` / `my_url_compose` 同一 fork 补丁惯例：所有新增代码都在本目录，
//! 官方代码仅保留带横幅标记的注入点；恢复指南见本目录 `MY-PROXY-ENDPOINT-PATCHES.md`。
//!
//! 本模块只做**纯逻辑**：配置段结构、容错反序列化、扫描门控、地址快照归一化、
//! 默认选中与回落、选择校验。真正的网卡扫描在 `src-tauri::my_proxy_endpoint`，
//! 文件读写复用 `crate::read_config` / `crate::write_config`。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const FORMAT_OPENAI: &str = "openai";
pub const FORMAT_ANTHROPIC: &str = "anthropic";
pub const FORMAT_GEMINI: &str = "gemini";

/// 支持的接口格式（校验与归一化共用）。
pub const SUPPORTED_FORMATS: [&str; 3] = [FORMAT_OPENAI, FORMAT_ANTHROPIC, FORMAT_GEMINI];

/// 非扫描态固定回环主机。
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// `config.jsonc` 顶层 `my_proxy_endpoint` 段（三个字段均可缺省）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MyProxyEndpointSection {
    /// 上次选中的 IP。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_ip: Option<String>,
    /// 上次选中的接口格式（openai / anthropic / gemini）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_format: Option<String>,
    /// 启动扫描得到并可用的本机 IPv4 快照（非扫描态为空）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub available_ips: Vec<String>,
}

/// 容错反序列化：形态非法（字符串 / 数组 / 字段类型错误）时按「段不存在」处理，
/// 并记一条 warn。这样用户手改配置写坏该段时，GUI 与 CLI 都不会加载失败。
pub fn de_lenient<'de, D>(deserializer: D) -> Result<Option<MyProxyEndpointSection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    // 只接受对象：serde 的 struct 也能从序列反序列化，必须显式排除数组形态。
    if !value.is_object() {
        if !value.is_null() {
            tracing::warn!("my_proxy_endpoint section ignored: expected an object");
        }
        return Ok(None);
    }
    match serde_json::from_value::<MyProxyEndpointSection>(value) {
        Ok(section) => Ok(Some(section)),
        Err(err) => {
            tracing::warn!(error = %err, "my_proxy_endpoint section ignored: invalid shape");
            Ok(None)
        }
    }
}

/// 解析结果（纯逻辑，不含扫描与 IO）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 规范化后的监听主机（非扫描态界面固定显示它）。
    pub listen_host: String,
    /// 是否处于扫描态（host 为 `0.0.0.0` / `::`）。
    pub scan_enabled: bool,
    /// 生效 IP。
    pub effective_ip: String,
    /// 生效接口格式。
    pub effective_format: String,
    /// 生效地址快照（非扫描态为空）。
    pub available_ips: Vec<String>,
    /// 规范化后应当落盘的段。
    pub section: MyProxyEndpointSection,
    /// 是否需要把 `section` 写回配置。
    pub should_persist: bool,
}

/// 扫描门控：仅 `0.0.0.0` / `::` 需要扫描局域网地址。
pub fn scan_enabled(host: &str) -> bool {
    matches!(host.trim(), "0.0.0.0" | "::")
}

/// 非扫描态界面固定显示的监听主机；空串按 `127.0.0.1` 处理。
pub fn normalize_display_host(host: &str) -> String {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        LOOPBACK_HOST.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 格式归一化：非法 / 缺失一律回落 `openai`。
pub fn normalize_format(value: Option<&str>) -> &'static str {
    match value.map(str::trim) {
        Some(FORMAT_OPENAI) => FORMAT_OPENAI,
        Some(FORMAT_ANTHROPIC) => FORMAT_ANTHROPIC,
        Some(FORMAT_GEMINI) => FORMAT_GEMINI,
        _ => FORMAT_OPENAI,
    }
}

/// `value` 是否为受支持的接口格式。
pub fn format_is_valid(value: &str) -> bool {
    SUPPORTED_FORMATS.contains(&value.trim())
}

fn is_ipv4(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 4 && parts.iter().all(|part| !part.is_empty() && part.parse::<u8>().is_ok())
}

/// 排序优先级：192 → 172 → 10 → 其它 → 127.0.0.1。
fn ip_rank(ip: &str) -> u8 {
    if ip == LOOPBACK_HOST {
        4
    } else if ip.starts_with("192.") {
        0
    } else if ip.starts_with("172.") {
        1
    } else if ip.starts_with("10.") {
        2
    } else {
        3
    }
}

/// 快照归一化：只保留 IPv4、去重、排除链路本地 `169.254.*`、补齐 `127.0.0.1`，
/// 并按「192 → 172 → 10 → 其它 → 127.0.0.1」稳定排序（同段内按字典序）。
pub fn normalize_available<I, S>(ips: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for raw in ips {
        let ip = raw.as_ref().trim();
        if !is_ipv4(ip) || ip.starts_with("169.254.") {
            continue;
        }
        if seen.insert(ip.to_string()) {
            out.push(ip.to_string());
        }
    }
    if seen.insert(LOOPBACK_HOST.to_string()) {
        out.push(LOOPBACK_HOST.to_string());
    }
    out.sort_by(|a, b| ip_rank(a).cmp(&ip_rank(b)).then_with(|| a.cmp(b)));
    out
}

/// 是否需要扫描：扫描态且当前没有快照。
pub fn needs_scan(host: &str, section: Option<&MyProxyEndpointSection>) -> bool {
    scan_enabled(host) && section.map_or(true, |existing| existing.available_ips.is_empty())
}

/// 选择回落：`current_ip` 仍在快照中则沿用；否则首个 `192.`，其次首个 `172.`，
/// 都没有则回环 `127.0.0.1`。
fn pick_ip(available: &[String], current: Option<&str>) -> String {
    if let Some(ip) = current.map(str::trim).filter(|value| !value.is_empty()) {
        if available.iter().any(|candidate| candidate == ip) {
            return ip.to_string();
        }
    }
    available
        .iter()
        .find(|ip| ip.starts_with("192."))
        .or_else(|| available.iter().find(|ip| ip.starts_with("172.")))
        .cloned()
        .unwrap_or_else(|| LOOPBACK_HOST.to_string())
}

/// 解析生效选择并给出「是否需要落盘」。
///
/// `scanned` 为调用方在 `needs_scan` 为真时提供的扫描结果；为 `None` 时使用段内已存快照。
pub fn resolve(
    host: &str,
    section: Option<&MyProxyEndpointSection>,
    scanned: Option<Vec<String>>,
) -> Resolved {
    let scan = scan_enabled(host);
    let listen_host = normalize_display_host(host);
    let scanned_provided = scanned.is_some();
    let available_ips = if scan {
        let source = scanned
            .unwrap_or_else(|| section.map(|s| s.available_ips.clone()).unwrap_or_default());
        normalize_available(source)
    } else {
        // 非扫描态：清空快照，避免留下会误导的地址。
        Vec::new()
    };
    let effective_ip = if scan {
        pick_ip(&available_ips, section.and_then(|s| s.current_ip.as_deref()))
    } else {
        listen_host.clone()
    };
    let effective_format =
        normalize_format(section.and_then(|s| s.current_format.as_deref())).to_string();
    let next_section = MyProxyEndpointSection {
        current_ip: Some(effective_ip.clone()),
        current_format: Some(effective_format.clone()),
        available_ips: available_ips.clone(),
    };
    // 段不存在时只有「刚扫描过」才值得新建；否则保持配置文件零变化。
    let should_persist = match section {
        None => scan && scanned_provided,
        Some(existing) => existing != &next_section,
    };
    Resolved {
        listen_host,
        scan_enabled: scan,
        effective_ip,
        effective_format,
        available_ips,
        section: next_section,
        should_persist,
    }
}

/// 校验界面提交的选择；成功返回归一化后的 `(ip, format)`。
pub fn validate_selection(
    host: &str,
    available_ips: &[String],
    current_ip: &str,
    current_format: &str,
) -> Result<(String, String), String> {
    let format = current_format.trim();
    if !format_is_valid(format) {
        return Err(format!(
            "unsupported format: {current_format} (expected one of {})",
            SUPPORTED_FORMATS.join(", ")
        ));
    }
    let ip = current_ip.trim();
    if scan_enabled(host) {
        let normalized = normalize_available(available_ips.iter());
        if !normalized.iter().any(|candidate| candidate == ip) {
            return Err(format!("ip {current_ip} is not in the available list"));
        }
        Ok((ip.to_string(), normalize_format(Some(format)).to_string()))
    } else {
        let fixed = normalize_display_host(host);
        if ip != fixed {
            return Err(format!(
                "ip {current_ip} is not selectable while listening on {fixed}"
            ));
        }
        Ok((fixed, normalize_format(Some(format)).to_string()))
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
