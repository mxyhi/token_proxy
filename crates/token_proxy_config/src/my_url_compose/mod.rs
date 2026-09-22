//! MY-URL-COMPOSE —— 渠道级出站 URL 显式组合（本仓库 fork 的本地增强模块）。
//!
//! 与官方代码完全隔离：全部逻辑在本目录，官方代码只保留少量横幅标记注入点
//! （PATCH C1/C2/C3，见 `MY-URL-COMPOSE-PATCHES.md`）。上游 pull 后如需恢复
//! 本模块，按该指南重新注入即可。
//!
//! 语义（与 GUI「接口地址组合」面板一一对应）：
//! - 出站 URL = 基础地址 + prefix（特殊拼接）+ 版本段替换后的客户端路径。
//! - suffix 第一段为版本段（`/v1`、`/v3`、`/abc` 皆可）；客户端 `/v1` 开头
//!   路径的第一段替换为它，子路径（models、count_tokens 等）自动跟随。
//! - 未配置家族恒等回退：prefix 空、版本段 `/v1`。
//! - 不做任何段的删除/去重/补全；重复段由 GUI 预览提示，后端不修正。

pub mod config;

mod compose;

pub use config::{EndpointCompose, UrlComposeConfig};
pub(crate) use compose::compose_outbound_url;

/// 旧 base_url 迁移提示：base 末段形如 `/v<数字>` 且未配置 `url_compose` 时，
/// 纯拼接可能产生 `/v1/v1/...` 双前缀。仅告警，不自动改写（见维护手册迁移表）。
pub fn warn_legacy_versioned_base_url(
    upstream_id: &str,
    base_url: &str,
    compose: Option<&UrlComposeConfig>,
) {
    if needs_legacy_version_warn(base_url, compose) {
        tracing::warn!(
            upstream_id = %upstream_id,
            "base_url ends with a version segment but url_compose is not configured; \
             outbound URLs are joined verbatim and may duplicate the segment \
             (e.g. /v1/v1/...). Configure url_compose.<family>.suffix explicitly."
        );
    }
}

/// 纯判定：是否需要输出旧式版本段 base_url 告警。
fn needs_legacy_version_warn(base_url: &str, compose: Option<&UrlComposeConfig>) -> bool {
    let fully_unconfigured = match compose {
        Some(value) => value.is_empty(),
        None => true,
    };
    if !fully_unconfigured {
        return false;
    }
    let base = base_url.trim().trim_end_matches('/');
    let last_segment = base.rsplit('/').next().unwrap_or("");
    last_segment.len() > 1
        && last_segment.starts_with('v')
        && last_segment[1..].bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
