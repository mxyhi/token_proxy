use super::{compose_from_parts, version_segment};
use crate::my_url_compose::config::normalize_segment;
use crate::my_url_compose::EndpointCompose;

fn compose(base: &str, prefix: &str, suffix: &str, path: &str) -> String {
    let endpoint = EndpointCompose {
        prefix: prefix.to_string(),
        suffix: suffix.to_string(),
    };
    compose_from_parts(base, Some(&endpoint), path)
}

fn unconfigured(base: &str, path: &str) -> String {
    compose_from_parts(base, None, path)
}

// ── 恒等回退（未配置家族） ────────────────────────────────────────────

#[test]
fn unconfigured_family_is_identity_join() {
    assert_eq!(
        unconfigured("https://api.example.com", "/v1/chat/completions"),
        "https://api.example.com/v1/chat/completions"
    );
    assert_eq!(
        unconfigured("https://api.example.com", "/v1/models/gpt-5"),
        "https://api.example.com/v1/models/gpt-5"
    );
}

#[test]
fn configured_empty_suffix_falls_back_to_identity() {
    assert_eq!(
        compose("https://x.com", "", "", "/v1/chat/completions"),
        "https://x.com/v1/chat/completions"
    );
}

// ── 版本段替换 ───────────────────────────────────────────────────────

#[test]
fn v3_suffix_replaces_first_segment_only() {
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/chat/completions"),
        "https://ark.example.com/api/plan/v3/chat/completions"
    );
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/models"),
        "https://ark.example.com/api/plan/v3/models"
    );
    assert_eq!(
        compose("https://ark.example.com/api/plan", "", "/v3/chat/completions", "/v1/embeddings"),
        "https://ark.example.com/api/plan/v3/embeddings"
    );
}

#[test]
fn arbitrary_version_segment_like_abc() {
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/chat/completions"),
        "https://x.com/abc/chat/completions"
    );
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/abcdefg/xxx"),
        "https://x.com/abc/abcdefg/xxx"
    );
    assert_eq!(
        compose("https://x.com", "", "/abc/chat/completions", "/v1/models"),
        "https://x.com/abc/models"
    );
}

#[test]
fn anthropic_prefix_and_subpaths_follow() {
    assert_eq!(
        compose("https://x.com", "/anthropic", "/v1/messages", "/v1/messages"),
        "https://x.com/anthropic/v1/messages"
    );
    assert_eq!(
        compose("https://x.com", "/anthropic", "/v1/messages", "/v1/messages/count_tokens"),
        "https://x.com/anthropic/v1/messages/count_tokens"
    );
}

#[test]
fn plain_path_exactly_v1_is_replaced() {
    assert_eq!(
        compose("https://x.com", "", "/v3/models", "/v1"),
        "https://x.com/v3"
    );
}

#[test]
fn v10_is_not_matched_as_v1() {
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/v10/models"),
        "https://x.com/v10/models"
    );
}

#[test]
fn non_v1_first_segments_pass_through() {
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/v1beta/openai/models"),
        "https://x.com/v1beta/openai/models"
    );
    assert_eq!(
        compose("https://x.com", "", "/v3/chat/completions", "/alpha/search"),
        "https://x.com/alpha/search"
    );
}

// ── query / base 边界 ────────────────────────────────────────────────

#[test]
fn query_is_preserved_verbatim() {
    assert_eq!(
        compose("https://x.com", "", "/v3/models", "/v1/models?limit=100&b=2"),
        "https://x.com/v3/models?limit=100&b=2"
    );
}

#[test]
fn base_trailing_slash_is_trimmed() {
    assert_eq!(
        compose("https://x.com/", "", "/v1/chat/completions", "/v1/chat/completions"),
        "https://x.com/v1/chat/completions"
    );
}

#[test]
fn empty_base_joins_prefix_and_path() {
    assert_eq!(
        compose("", "/anthropic", "/v1/messages", "/v1/messages"),
        "/anthropic/v1/messages"
    );
}

// ── 版本段解析与规范化 ───────────────────────────────────────────────

#[test]
fn version_segment_defaults_and_extracts() {
    assert_eq!(version_segment(""), "/v1");
    assert_eq!(version_segment("   "), "/v1");
    assert_eq!(version_segment("/v3/chat/completions"), "/v3");
    assert_eq!(version_segment("/abc"), "/abc");
    assert_eq!(version_segment("v3"), "/v3");
    assert_eq!(version_segment("/v3/"), "/v3");
}

#[test]
fn segment_normalization_rules() {
    assert_eq!(normalize_segment("anthropic/"), "/anthropic");
    assert_eq!(normalize_segment("openai"), "/openai");
    assert_eq!(normalize_segment(" /v1 "), "/v1");
    assert_eq!(normalize_segment(""), "");
    assert_eq!(normalize_segment("/"), "");
}

#[test]
fn config_normalized_deep_copies() {
    use crate::my_url_compose::UrlComposeConfig;
    let mut config = UrlComposeConfig::default();
    config.anthropic = Some(EndpointCompose {
        prefix: "anthropic/".to_string(),
        suffix: " /v1/messages ".to_string(),
    });
    let normalized = config.normalized();
    let anthropic = normalized.anthropic.as_ref().unwrap();
    assert_eq!(anthropic.prefix, "/anthropic");
    assert_eq!(anthropic.suffix, "/v1/messages");
}

// ── bigmodel 特例迁移等价性 ──────────────────────────────────────────

#[test]
fn bigmodel_paas_base_via_compose() {
    // 官方特例（已删除）：base 含 /api/coding/paas/ 时把 /v1/chat/completions
    // 改写为 /chat/completions。等价写法：suffix = /chat/completions。
    assert_eq!(
        compose(
            "https://open.bigmodel.cn/api/coding/paas",
            "",
            "/v1/chat/completions",
            "/v1/chat/completions"
        ),
        "https://open.bigmodel.cn/api/coding/paas/v1/chat/completions"
    );
}
