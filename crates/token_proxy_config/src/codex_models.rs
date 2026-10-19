use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexReasoningEffort {
    #[default]
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// 仅声明额外模型在 Codex 中的能力，不改变上游模型或协议转换能力。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexModelCapabilities {
    pub context_window: u32,
    pub supports_vision: bool,
    pub reasoning_effort: CodexReasoningEffort,
}

impl Default for CodexModelCapabilities {
    fn default() -> Self {
        Self {
            context_window: 32768,
            supports_vision: false,
            reasoning_effort: CodexReasoningEffort::None,
        }
    }
}

pub(crate) fn normalize_capabilities(
    configured: HashMap<String, CodexModelCapabilities>,
) -> Result<HashMap<String, CodexModelCapabilities>, String> {
    if configured.len() > 64 {
        return Err("codex_model_capabilities supports at most 64 model IDs.".into());
    }
    let mut normalized = HashMap::new();
    for (id, capabilities) in configured {
        let id = id.trim();
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
            return Err("codex_model_capabilities contains an invalid model ID.".into());
        }
        if capabilities.context_window == 0 {
            return Err(
                "codex_model_capabilities context_window must be a positive integer.".into(),
            );
        }
        // 去空白后重复必须拒绝，避免 HashMap 遍历顺序决定最终能力。
        if normalized.insert(id.to_string(), capabilities).is_some() {
            return Err("codex_model_capabilities contains duplicate normalized model IDs.".into());
        }
    }
    Ok(normalized)
}
