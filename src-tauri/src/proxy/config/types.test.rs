use super::*;

#[test]
fn test_strip_overlapping_prefix() {
    // 标准 OpenAI 兼容格式：base_url 包含 /v1
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai/v1", "/v1/chat/completions"),
        "/chat/completions"
    );
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/v1", "/v1/chat/completions"),
        "/chat/completions"
    );

    // 无重叠情况：base_url 不包含路径
    assert_eq!(
        strip_overlapping_prefix("https://api.openai.com", "/v1/chat/completions"),
        "/v1/chat/completions"
    );

    // 无重叠情况：base_url 路径与请求路径无公共后缀
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai/", "/v1/chat/completions"),
        "/v1/chat/completions"
    );
    assert_eq!(
        strip_overlapping_prefix("https://api.example.com/openai", "/v1/chat/completions"),
        "/v1/chat/completions"
    );

    // 多层路径重叠
    assert_eq!(
        strip_overlapping_prefix("https://example.com/api/openai/v1", "/v1/models"),
        "/models"
    );

    // 完整路径重叠
    assert_eq!(
        strip_overlapping_prefix("https://example.com/openai/v1", "/openai/v1/completions"),
        "/completions"
    );

    // 带尾斜杠的 base_url
    assert_eq!(
        strip_overlapping_prefix("https://example.com/v1/", "/v1/chat/completions"),
        "/chat/completions"
    );

    // 无效 URL 回退
    assert_eq!(
        strip_overlapping_prefix("not-a-valid-url", "/v1/chat/completions"),
        "/v1/chat/completions"
    );
}

#[test]
fn test_upstream_url() {
    // openai provider: /v1/chat/completions
    let upstream = UpstreamRuntime {
        id: "test".to_string(),
        base_url: "https://api.example.com/openai/v1".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        model_mappings: None,
        header_overrides: None,
    };
    assert_eq!(
        upstream.upstream_url("/v1/chat/completions"),
        "https://api.example.com/openai/v1/chat/completions"
    );

    // openai-response provider: /v1/responses
    let upstream_responses = UpstreamRuntime {
        id: "test".to_string(),
        base_url: "https://api.example.com/openai/v1".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        model_mappings: None,
        header_overrides: None,
    };
    assert_eq!(
        upstream_responses.upstream_url("/v1/responses"),
        "https://api.example.com/openai/v1/responses"
    );

    // 无路径前缀的 base_url
    let upstream_no_path = UpstreamRuntime {
        id: "test".to_string(),
        base_url: "https://api.openai.com".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        model_mappings: None,
        header_overrides: None,
    };
    assert_eq!(
        upstream_no_path.upstream_url("/v1/chat/completions"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        upstream_no_path.upstream_url("/v1/responses"),
        "https://api.openai.com/v1/responses"
    );

    // 带尾斜杠的 base_url
    let upstream_trailing_slash = UpstreamRuntime {
        id: "test".to_string(),
        base_url: "https://api.example.com/openai/v1/".to_string(),
        api_key: None,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        kiro_account_id: None,
        codex_account_id: None,
        antigravity_account_id: None,
        kiro_preferred_endpoint: None,
        proxy_url: None,
        priority: 0,
        model_mappings: None,
        header_overrides: None,
    };
    // openai: /v1/chat/completions
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/chat/completions"),
        "https://api.example.com/openai/v1/chat/completions"
    );
    // openai-response: /v1/responses
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/responses"),
        "https://api.example.com/openai/v1/responses"
    );
    // anthropic: /v1/messages
    assert_eq!(
        upstream_trailing_slash.upstream_url("/v1/messages"),
        "https://api.example.com/openai/v1/messages"
    );
}
