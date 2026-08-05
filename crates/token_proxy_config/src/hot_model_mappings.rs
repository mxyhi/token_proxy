use std::collections::{HashMap, HashSet};

const DEFAULT_HOT_MODEL_MAPPING_PAIRS: &[(&str, &str)] = &[
    ("openai/codex-auto-review", "codex-auto-review"),
    ("openai/gpt-5.6-sol", "gpt-5.6-sol"),
    ("openai/gpt-5.6-terra", "gpt-5.6-terra"),
    ("openai/gpt-5.6-luna", "gpt-5.6-luna"),
    ("openai/gpt-5.5-pro", "gpt-5.5-pro"),
    ("openai/gpt-5.5", "gpt-5.5"),
    ("openai/gpt-5.4-pro", "gpt-5.4-pro"),
    ("openai/gpt-5.4", "gpt-5.4"),
    ("openai/gpt-5.4-mini", "gpt-5.4-mini"),
    ("openai/gpt-5.4-nano", "gpt-5.4-nano"),
    ("openai/gpt-5.3-codex", "gpt-5.3-codex"),
    ("openai/gpt-5.3-chat", "gpt-5.3-chat"),
    ("openai/gpt-5.2-codex", "gpt-5.2-codex"),
    ("openai/gpt-5.2-chat", "gpt-5.2-chat"),
    ("openai/gpt-5.2-pro", "gpt-5.2-pro"),
    ("openai/gpt-5.2", "gpt-5.2"),
    ("openai/gpt-5.1", "gpt-5.1"),
    ("openai/gpt-5-pro", "gpt-5-pro"),
    ("openai/gpt-5", "gpt-5"),
    ("openai/gpt-5-mini", "gpt-5-mini"),
    ("openai/gpt-5-nano", "gpt-5-nano"),
    ("openai/gpt-4.1", "gpt-4.1"),
    ("openai/gpt-4.1-mini", "gpt-4.1-mini"),
    ("openai/gpt-4.1-nano", "gpt-4.1-nano"),
    ("openai/o4-mini", "o4-mini"),
    ("openai/o3", "o3"),
    ("openai/o3-mini", "o3-mini"),
    ("anthropic/claude-opus-5", "claude-opus-5"),
    ("anthropic/claude-opus-4-7", "claude-opus-4-7"),
    ("anthropic/claude-opus-4.7", "claude-opus-4-7"),
    ("anthropic/claude-opus-4.6", "claude-opus-4.6"),
    ("anthropic/claude-opus-4.6-fast", "claude-opus-4.6-fast"),
    ("anthropic/claude-sonnet-5", "claude-sonnet-5"),
    ("anthropic/claude-fable-5", "claude-fable-5"),
    ("anthropic/claude-sonnet-4.6", "claude-sonnet-4.6"),
    ("anthropic/claude-sonnet-4.5", "claude-sonnet-4.5"),
    ("anthropic/claude-haiku-4.5", "claude-haiku-4.5"),
    ("models/gemini-3.1-pro", "gemini-3.1-pro"),
    ("google/gemini-3.1-pro", "gemini-3.1-pro"),
    ("models/gemini-3.1-pro-preview", "gemini-3.1-pro-preview"),
    ("google/gemini-3.1-pro-preview", "gemini-3.1-pro-preview"),
    (
        "google/gemini-3.1-pro-preview-customtools",
        "gemini-3.1-pro-preview-customtools",
    ),
    ("models/gemini-3.5-flash", "gemini-3.5-flash"),
    ("google/gemini-3.5-flash", "gemini-3.5-flash"),
    ("models/gemini-3.5-flash-lite", "gemini-3.5-flash-lite"),
    ("google/gemini-3.5-flash-lite", "gemini-3.5-flash-lite"),
    ("models/gemini-3.6-flash", "gemini-3.6-flash"),
    ("google/gemini-3.6-flash", "gemini-3.6-flash"),
    ("models/gemini-3-pro", "gemini-3-pro"),
    ("google/gemini-3-pro", "gemini-3-pro"),
    (
        "google/gemini-3-pro-image-preview",
        "gemini-3-pro-image-preview",
    ),
    ("models/gemini-3-flash", "gemini-3-flash"),
    ("google/gemini-3-flash", "gemini-3-flash"),
    ("google/gemini-3-flash-preview", "gemini-3-flash-preview"),
    ("models/gemini-3.1-flash-lite", "gemini-3.1-flash-lite"),
    ("google/gemini-3.1-flash-lite", "gemini-3.1-flash-lite"),
    (
        "google/gemini-3.1-flash-lite-preview",
        "gemini-3.1-flash-lite-preview",
    ),
    (
        "google/gemini-3.1-flash-image-preview",
        "gemini-3.1-flash-image-preview",
    ),
    ("deepseek/deepseek-v4", "deepseek-v4"),
    ("deepseek/deepseek-v4-pro", "deepseek-v4-pro"),
    ("deepseek/deepseek-v4-flash", "deepseek-v4-flash"),
    ("qwen/qwen3.6-plus", "qwen3.6-plus"),
    ("qwen/qwen3.5-plus", "qwen3.5-plus"),
    ("qwenlm/qwen3.6-plus", "qwen3.6-plus"),
    ("qwenlm/qwen3.5-plus", "qwen3.5-plus"),
    ("x-ai/grok-4.5", "grok-4.5"),
    ("xai/grok-4.5", "grok-4.5"),
    ("x-ai/grok-4.5-high", "grok-4.5"),
    ("xai/grok-4.5-high", "grok-4.5"),
    ("x-ai/grok-4.3", "grok-4.3"),
    ("xai/grok-4.3", "grok-4.3"),
    ("x-ai/grok-4.20-multi-agent", "grok-4.20-multi-agent"),
    ("xai/grok-4.20-multi-agent", "grok-4.20-multi-agent"),
    ("x-ai/grok-4.20-multi-agent-0309", "grok-4.20-multi-agent"),
    ("xai/grok-4.20-multi-agent-0309", "grok-4.20-multi-agent"),
    ("x-ai/grok-4.20", "grok-4.20"),
    ("xai/grok-4.20", "grok-4.20"),
    ("x-ai/grok-4.20-0309", "grok-4.20"),
    ("xai/grok-4.20-0309", "grok-4.20"),
    ("x-ai/grok-4.20-0309-reasoning", "grok-4.20"),
    ("xai/grok-4.20-0309-reasoning", "grok-4.20"),
    ("x-ai/grok-4.20-0309-non-reasoning", "grok-4.20"),
    ("xai/grok-4.20-0309-non-reasoning", "grok-4.20"),
    ("x-ai/grok-4.1-fast", "grok-4.1-fast"),
    ("xai/grok-4.1-fast", "grok-4.1-fast"),
    ("x-ai/grok-4", "grok-4"),
    ("xai/grok-4", "grok-4"),
    ("x-ai/grok-4-fast", "grok-4-fast"),
    ("xai/grok-4-fast", "grok-4-fast"),
    ("x-ai/grok-build-0.1", "grok-build-0.1"),
    ("xai/grok-build-0.1", "grok-build-0.1"),
    ("composer-2.5", "grok-composer-2.5-fast"),
    ("x-ai/grok-composer-2.5-fast", "grok-composer-2.5-fast"),
    ("xai/grok-composer-2.5-fast", "grok-composer-2.5-fast"),
    ("moonshotai/kimi-k2.6", "kimi-k2.6"),
    ("moonshotai/kimi-k3", "kimi-k3"),
    ("moonshotai/kimi-k2.5", "kimi-k2.5"),
    ("moonshotai/kimi-k2", "kimi-k2"),
    ("z-ai/glm-5.2", "glm-5.2"),
    ("z-ai/glm-5.1", "glm-5.1"),
    ("z-ai/glm-5", "glm-5"),
    ("z-ai/glm-4.7", "glm-4.7"),
    ("z-ai/glm-4.6", "glm-4.6"),
    ("minimax/minimax-m2.7", "minimax-m2.7"),
    ("minimax/minimax-m2.5", "minimax-m2.5"),
    ("minimax/minimax-m2.1", "minimax-m2.1"),
    ("minimax/minimax-m2", "minimax-m2"),
    ("meta-llama/llama-4-maverick", "llama-4-maverick"),
    ("meta/llama-4-maverick", "llama-4-maverick"),
    ("meta-llama/llama-4-scout", "llama-4-scout"),
    ("meta/llama-4-scout", "llama-4-scout"),
    (
        "meta-llama/llama-3.3-70b-instruct",
        "llama-3.3-70b-instruct",
    ),
    ("meta/llama-3.3-70b-instruct", "llama-3.3-70b-instruct"),
];

pub fn default_hot_model_mappings() -> HashMap<String, String> {
    DEFAULT_HOT_MODEL_MAPPING_PAIRS
        .iter()
        .map(|(alias, target)| ((*alias).to_string(), (*target).to_string()))
        .collect()
}

pub(crate) fn merge_hot_model_mappings(
    hot_model_mappings: &HashMap<String, String>,
    upstream_model_mappings: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = hot_model_mappings.clone();
    for (pattern, target) in upstream_model_mappings {
        merged.insert(pattern.clone(), target.clone());
    }
    merged
}

pub fn expand_model_ids_with_mappings(ids: &mut Vec<String>, mappings: &HashMap<String, String>) {
    if ids.is_empty() || mappings.is_empty() {
        return;
    }

    let source = ids
        .iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<HashSet<_>>();

    for model in source {
        if let Some(target) = mappings.get(&model) {
            push_unique_model_id(ids, target);
        }
        for (alias, target) in mappings {
            if alias.contains('*') || target.trim() != model {
                continue;
            }
            push_unique_model_id(ids, alias);
        }
    }
}

fn push_unique_model_id(ids: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() && ids.iter().all(|model| model != value) {
        ids.push(value.to_string());
    }
}

#[cfg(test)]
#[path = "hot_model_mappings.test.rs"]
mod tests;
