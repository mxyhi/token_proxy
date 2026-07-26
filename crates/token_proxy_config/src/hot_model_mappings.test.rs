use super::*;

#[test]
fn default_hot_model_mappings_include_popular_namespaced_aliases() {
    let mappings = default_hot_model_mappings();

    assert_eq!(
        mappings.get("openai/gpt-5.6-sol"),
        Some(&"gpt-5.6-sol".to_string())
    );
    assert_eq!(
        mappings.get("openai/gpt-5.6-terra"),
        Some(&"gpt-5.6-terra".to_string())
    );
    assert_eq!(
        mappings.get("openai/gpt-5.6-luna"),
        Some(&"gpt-5.6-luna".to_string())
    );
    assert_eq!(
        mappings.get("openai/gpt-5.5-pro"),
        Some(&"gpt-5.5-pro".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-opus-4.6-fast"),
        Some(&"claude-opus-4.6-fast".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-opus-4-7"),
        Some(&"claude-opus-4-7".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-opus-4.7"),
        Some(&"claude-opus-4-7".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-opus-5"),
        Some(&"claude-opus-5".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-sonnet-5"),
        Some(&"claude-sonnet-5".to_string())
    );
    assert_eq!(
        mappings.get("anthropic/claude-fable-5"),
        Some(&"claude-fable-5".to_string())
    );
    assert_eq!(
        mappings.get("google/gemini-3.1-pro-preview-customtools"),
        Some(&"gemini-3.1-pro-preview-customtools".to_string())
    );
    assert_eq!(
        mappings.get("google/gemini-3.5-flash"),
        Some(&"gemini-3.5-flash".to_string())
    );
    assert_eq!(
        mappings.get("models/gemini-3.5-flash"),
        Some(&"gemini-3.5-flash".to_string())
    );
    assert_eq!(
        mappings.get("deepseek/deepseek-v4-pro"),
        Some(&"deepseek-v4-pro".to_string())
    );
    assert!(!mappings.contains_key("deepseek/deepseek-r1"));
    assert!(!mappings.contains_key("deepseek/deepseek-v3"));
    assert_eq!(
        mappings.get("qwen/qwen3.6-plus"),
        Some(&"qwen3.6-plus".to_string())
    );
    assert!(!mappings.contains_key("qwen/qwen3-coder"));
    assert!(!mappings.contains_key("google/gemini-2.5-pro"));
    assert_eq!(
        mappings.get("x-ai/grok-4.20"),
        Some(&"grok-4.20".to_string())
    );
    assert_eq!(
        mappings.get("x-ai/grok-4.20-0309-reasoning"),
        Some(&"grok-4.20".to_string())
    );
    assert_eq!(
        mappings.get("x-ai/grok-4.20-0309-non-reasoning"),
        Some(&"grok-4.20".to_string())
    );
    assert_eq!(mappings.get("x-ai/grok-4.5"), Some(&"grok-4.5".to_string()));
    assert_eq!(
        mappings.get("x-ai/grok-4.5-high"),
        Some(&"grok-4.5".to_string())
    );
    assert_eq!(mappings.get("x-ai/grok-4.3"), Some(&"grok-4.3".to_string()));
    assert_eq!(
        mappings.get("x-ai/grok-4.20-multi-agent-0309"),
        Some(&"grok-4.20-multi-agent".to_string())
    );
    assert_eq!(
        mappings.get("x-ai/grok-build-0.1"),
        Some(&"grok-build-0.1".to_string())
    );
    assert_eq!(
        mappings.get("x-ai/grok-composer-2.5-fast"),
        Some(&"grok-composer-2.5-fast".to_string())
    );
    assert_eq!(
        mappings.get("composer-2.5"),
        Some(&"grok-composer-2.5-fast".to_string())
    );
    assert!(!mappings.contains_key("x-ai/grok-3"));
    assert_eq!(
        mappings.get("moonshotai/kimi-k2.6"),
        Some(&"kimi-k2.6".to_string())
    );
    assert_eq!(mappings.get("z-ai/glm-5.2"), Some(&"glm-5.2".to_string()));
    assert_eq!(mappings.get("z-ai/glm-5.1"), Some(&"glm-5.1".to_string()));
    assert_eq!(
        mappings.get("minimax/minimax-m2.7"),
        Some(&"minimax-m2.7".to_string())
    );
}

#[test]
fn upstream_model_mappings_override_hot_model_mappings() {
    let hot = default_hot_model_mappings();
    let upstream = HashMap::from([(
        "openai/gpt-5.5".to_string(),
        "vendor-special-gpt-5.5".to_string(),
    )]);

    let merged = merge_hot_model_mappings(&hot, &upstream);

    assert_eq!(
        merged.get("openai/gpt-5.5"),
        Some(&"vendor-special-gpt-5.5".to_string())
    );
}

#[test]
fn model_catalog_expansion_adds_aliases_only_for_present_targets() {
    let mappings = default_hot_model_mappings();
    let mut ids = vec!["gpt-5.5".to_string(), "vendor-only".to_string()];

    expand_model_ids_with_mappings(&mut ids, &mappings);

    assert!(ids.contains(&"gpt-5.5".to_string()));
    assert!(ids.contains(&"openai/gpt-5.5".to_string()));
    assert!(ids.contains(&"vendor-only".to_string()));
    assert!(!ids.contains(&"openai/gpt-5.4".to_string()));
}
