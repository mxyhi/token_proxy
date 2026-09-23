//! my_proxy_endpoint —— 仪表盘「代理接入地址」的命令层（fork 本地增强）。
//!
//! 与 config 层 `token_proxy_config::my_proxy_endpoint` 的分工：
//! - 本文件：网卡扫描（sysinfo）、读配置、解析、按需落盘、对前端暴露两个命令；
//! - config 层：段结构、容错反序列化、扫描门控、回落算法（纯逻辑，已单测）。
//!
//! 依赖 `sysinfo` 是 src-tauri 既有声明（`Cargo.toml`）且此前无调用点，本功能首次使用，
//! 因此不新增依赖、不改 `Cargo.lock`。
//!
//! 恢复指南见 `crates/token_proxy_config/src/my_proxy_endpoint/MY-PROXY-ENDPOINT-PATCHES.md`。

use std::net::IpAddr;
use std::sync::Arc;

use tauri::Manager;

use token_proxy_config::my_proxy_endpoint as core;

type PathsState = Arc<token_proxy_account_store::paths::TokenProxyPaths>;

/// 返回给前端的快照（自有 DTO，不依赖上游 `ConfigResponse` 形状）。
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyProxyEndpointSnapshot {
    listen_host: String,
    port: u16,
    scan_enabled: bool,
    effective_ip: String,
    effective_format: String,
    available_ips: Vec<String>,
    /// 本地 IPC 专用；前端只渲染掩码，明文仅用于复制。
    api_key: Option<String>,
    api_key_configured: bool,
}

/// 纯函数：只保留 IPv4 并交给 config 层的归一化（与回落规则共用同一套排序）。
fn filter_and_sort(raw: impl IntoIterator<Item = IpAddr>) -> Vec<String> {
    core::normalize_available(raw.into_iter().filter_map(|addr| match addr {
        IpAddr::V4(v4) => Some(v4.to_string()),
        IpAddr::V6(_) => None,
    }))
}

/// 扫描本机网卡 IPv4。任何异常都退化为仅回环地址，绝不 panic。
fn scan_local_ipv4() -> Vec<String> {
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let raw: Vec<IpAddr> = networks
        .iter()
        .flat_map(|(_name, data)| data.ip_networks().iter().map(|net| net.addr))
        .collect();
    filter_and_sort(raw)
}

fn into_snapshot(
    resolved: core::Resolved,
    port: u16,
    api_key: Option<String>,
) -> MyProxyEndpointSnapshot {
    let api_key_configured = api_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());
    MyProxyEndpointSnapshot {
        listen_host: resolved.listen_host,
        port,
        scan_enabled: resolved.scan_enabled,
        effective_ip: resolved.effective_ip,
        effective_format: resolved.effective_format,
        available_ips: resolved.available_ips,
        api_key,
        api_key_configured,
    }
}

/// 读配置 → 门控判定 →（必要时）扫描一次并落盘 → 解析回落 → 返回快照。
pub(crate) async fn load_snapshot(
    paths: &token_proxy_account_store::paths::TokenProxyPaths,
) -> Result<MyProxyEndpointSnapshot, String> {
    let mut config = token_proxy_config::read_config(paths).await?.config;
    let host = config.host.clone();
    let section = config.my_proxy_endpoint.clone();
    let scanned = if core::needs_scan(&host, section.as_ref()) {
        Some(scan_local_ipv4())
    } else {
        None
    };
    let resolved = core::resolve(&host, section.as_ref(), scanned);
    if resolved.should_persist {
        config.my_proxy_endpoint = Some(resolved.section.clone());
        token_proxy_config::write_config(paths, config.clone()).await?;
    }
    Ok(into_snapshot(
        resolved,
        config.port,
        config.local_api_key.clone(),
    ))
}

#[tauri::command]
pub(crate) async fn my_proxy_endpoint_snapshot(
    app: tauri::AppHandle,
) -> Result<MyProxyEndpointSnapshot, String> {
    let paths = app.state::<PathsState>().inner().clone();
    load_snapshot(paths.as_ref()).await
}

/// 落盘一处选择：只替换 `my_proxy_endpoint` 段，不触发代理 reload / 既有保存编排。
#[tauri::command]
pub(crate) async fn my_proxy_endpoint_select(
    app: tauri::AppHandle,
    current_ip: String,
    current_format: String,
) -> Result<MyProxyEndpointSnapshot, String> {
    let paths = app.state::<PathsState>().inner().clone();
    let mut config = token_proxy_config::read_config(paths.as_ref()).await?.config;
    let host = config.host.clone();
    let available = config
        .my_proxy_endpoint
        .as_ref()
        .map(|section| core::normalize_available(section.available_ips.iter()))
        .unwrap_or_default();
    let (ip, format) =
        core::validate_selection(&host, &available, &current_ip, &current_format)?;
    config.my_proxy_endpoint = Some(core::MyProxyEndpointSection {
        current_ip: Some(ip),
        current_format: Some(format),
        available_ips: available,
    });
    token_proxy_config::write_config(paths.as_ref(), config.clone()).await?;
    let resolved = core::resolve(&host, config.my_proxy_endpoint.as_ref(), None);
    Ok(into_snapshot(
        resolved,
        config.port,
        config.local_api_key.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{filter_and_sort, scan_local_ipv4};
    use std::net::IpAddr;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("valid ip")
    }

    #[test]
    fn filter_and_sort_keeps_ipv4_only_and_sorted() {
        let out = filter_and_sort([
            ip("10.0.0.5"),
            ip("192.168.1.23"),
            ip("::1"),
            ip("169.254.10.10"),
            ip("172.20.0.3"),
        ]);
        assert_eq!(
            out,
            vec!["192.168.1.23", "172.20.0.3", "10.0.0.5", "127.0.0.1"]
        );
    }

    #[test]
    fn filter_and_sort_empty_input_yields_loopback() {
        assert_eq!(filter_and_sort(Vec::new()), vec!["127.0.0.1"]);
    }

    #[test]
    fn scan_never_panics_and_always_includes_loopback() {
        let out = scan_local_ipv4();
        assert!(out.contains(&"127.0.0.1".to_string()));
        assert!(out.iter().all(|ip| ip.parse::<std::net::Ipv4Addr>().is_ok()));
    }
}
