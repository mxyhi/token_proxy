//! `url_compose` 配置域：渠道级出站地址组合声明。
//!
//! 一条渠道可同时声明 openai / openai-response / anthropic /
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) START ══════════
//! dashscope 四个接口家族的
// ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) END ══════════
//! `prefix`（特殊拼接）与 `suffix`（完整后缀，第一段为"版本段"），
//! 由 `compose.rs` 在出站时做纯拼接与版本段替换。

use serde::{Deserialize, Serialize};

/// 未配置家族（或 suffix 为空）时的回退版本段。
pub(crate) const DEFAULT_VERSION_SEGMENT: &str = "/v1";

/// 渠道级出站 URL 组合配置；键为接口家族（kebab-case：`openai-response`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UrlComposeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<EndpointCompose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai_response: Option<EndpointCompose>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<EndpointCompose>,
    // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) START ══════════
    /// DashScope 原生协议家族（`/v1/services` 前缀透传的出站组合）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashscope: Option<EndpointCompose>,
    // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) END ══════════
}

/// 单接口家族的出站地址组合：二者均可留空（等价不对该接口做额外拼接）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCompose {
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
}

impl EndpointCompose {
    /// 规范化副本：去首尾空白、去结尾 `/`、非空时补开头 `/`。
    pub fn normalized(&self) -> Self {
        Self {
            prefix: normalize_segment(&self.prefix),
            suffix: normalize_segment(&self.suffix),
        }
    }
}

impl UrlComposeConfig {
    /// 深拷贝的规范化副本（配置加载期调用一次）。
    pub fn normalized(&self) -> Self {
        Self {
            openai: self.openai.as_ref().map(|value| value.normalized()),
            openai_response: self.openai_response.as_ref().map(|value| value.normalized()),
            anthropic: self.anthropic.as_ref().map(|value| value.normalized()),
            // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) START ══════════
            dashscope: self.dashscope.as_ref().map(|value| value.normalized()),
            // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) END ══════════
        }
    }

    /// 是否完全没有配置任何家族（用于旧 base_url 迁移提示）。
    pub fn is_empty(&self) -> bool {
        self.openai.is_none()
            && self.openai_response.is_none()
            && self.anthropic.is_none()
        // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) START ══════════
            && self.dashscope.is_none()
        // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 2 (config) END ══════════
    }
}

/// 段规范化：`"anthropic/"` → `"/anthropic"`、`"openai"` → `"/openai"`、`""` → `""`。
pub(crate) fn normalize_segment(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}
