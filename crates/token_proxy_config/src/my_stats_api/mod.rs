//! my_stats_api —— 只读行级统计 HTTP API 的配置与鉴权（fork 本地增强）。
//!
//! 与 `my_conf_openai` 同一 fork 补丁惯例：所有新增代码都在本目录，官方代码
//! 仅保留带横幅标记的注入点；恢复指南见本目录 `MY-STATS-API-PATCHES.md`。
//!
//! 本文件职责：`config.jsonc` 顶层 `my_stats_api` 段的解析结构、
//! 「代理端口 + 100」端口推导、解析快照的全局同步（reload 即生效鉴权）、
//! token 校验助手。HTTP 服务本体在 `token_proxy_app::my_stats_api`。

use crate::ProxyConfigFile;
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

/// `config.jsonc` 顶层 `my_stats_api` 段的原始形态（全部字段可选，缺省见 `resolve_section`）。
///
/// `token` 为缺失 / `null` / 空串时视为**无鉴权**（可信内网场景）；
/// 为非空字符串时消费方必须携带 `Authorization: Bearer <token>` 或 `?token=<token>`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MyStatsApiSection {
    pub enabled: Option<bool>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub token: Option<String>,
}

/// 解析后的生效快照（`port` 已完成「代理端口 + 100」推导，token 已规范化）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyStatsApiResolved {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
}

/// 段 → 生效快照：`enabled` 缺省 false；`host` 缺省 127.0.0.1（允许 0.0.0.0）；
/// `port` 缺省 = `proxy_port + 100`（u16 溢出饱和封顶 65535）；token 规范化（trim、空串 → 无鉴权）。
pub fn resolve_section(section: &MyStatsApiSection, proxy_port: u16) -> MyStatsApiResolved {
    MyStatsApiResolved {
        enabled: section.enabled.unwrap_or(false),
        host: section
            .host
            .clone()
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        port: section.port.unwrap_or(proxy_port.saturating_add(100)),
        token: section
            .token
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
    }
}

/// 全局生效快照：`OnceLock` 承载 `RwLock` 外壳（初始化一次），内层 `RwLock`
/// 保证每次 `sync_settings`（配置加载 / reload）真正覆盖旧值——
/// 不用 my_conf_openai 的裸 `OnceLock.set`（第二次调用静默失败，reload 不生效）。
static RESOLVED: OnceLock<RwLock<Option<MyStatsApiResolved>>> = OnceLock::new();

fn settings_cell() -> &'static RwLock<Option<MyStatsApiResolved>> {
    RESOLVED.get_or_init(|| RwLock::new(None))
}

/// PATCH 1 注入点：`build_runtime_config` 中调用（每次配置加载 / reload 都会走到）。
///
/// 同步「代理端口 + 100」推导后的生效快照到全局；鉴权中间件每请求读取，
/// 因此 **token 变更 reload 即生效**。返回本次解析结果便于日志 / 测试。
pub fn sync_settings(config: &ProxyConfigFile) -> Option<MyStatsApiResolved> {
    let resolved = config
        .my_stats_api
        .as_ref()
        .map(|section| resolve_section(section, config.port));
    if let Ok(mut slot) = settings_cell().write() {
        *slot = resolved.clone();
    }
    resolved
}

/// 读取当前生效快照；从未同步过（无段 / 未加载）返回 `None`。
pub fn current_settings() -> Option<MyStatsApiResolved> {
    settings_cell().read().ok().and_then(|slot| slot.clone())
}

/// 鉴权判定：快照 `token` 为 `None`（缺省 / `null` / 空串）→ 无鉴权，恒通过；
/// 非空 → `provided` 必须与之相等（常量时间比较，防时序侧信道）。
pub fn auth_ok(resolved: &MyStatsApiResolved, provided: Option<&str>) -> bool {
    match &resolved.token {
        None => true,
        Some(expected) => match provided {
            Some(p) => constant_time_eq(p.trim(), expected),
            None => false,
        },
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
