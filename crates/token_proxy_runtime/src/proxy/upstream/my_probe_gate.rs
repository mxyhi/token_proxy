//! MY-URL-COMPOSE R1：模型探测按凭据类型门控。
//!
//! 仅账户型（OAuth：kiro / codex / xai）上游参与自发探测；api_keys 与
//! passthrough 手动渠道不再被探测，避免不兼容上游在 Dashboard 反复出现
//! 探测错误。客户端主动发起的模型列表请求（`GET /v1/models`）不经过本
//! 门控，行为不变。

use token_proxy_config::UpstreamRuntime;

pub(super) fn should_probe_upstream(upstream: &UpstreamRuntime) -> bool {
    upstream.kiro_account_id.is_some()
        || upstream.codex_account_id.is_some()
        || upstream.xai_account_id.is_some()
}

#[cfg(test)]
mod tests {
    use super::should_probe_upstream;
    use token_proxy_config::UpstreamRuntime;

    fn upstream(
        kiro: Option<&str>,
        codex: Option<&str>,
        xai: Option<&str>,
    ) -> UpstreamRuntime {
        UpstreamRuntime {
            id: "probe-u".to_string(),
            selector_key: "probe-u".to_string(),
            base_url: "https://example.com".to_string(),
            api_key: None,
            api_key_headers: None,
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            rewrite_developer_role_to_system: false,
            kiro_account_id: kiro.map(str::to_string),
            codex_account_id: codex.map(str::to_string),
            xai_account_id: xai.map(str::to_string),
            kiro_preferred_endpoint: None,
            proxy_url: None,
            priority: 0,
            available_models: Vec::new(),
            advertised_model_ids: Vec::new(),
            model_capabilities: Default::default(),
            model_mappings: None,
            header_overrides: None,
            allowed_inbound_formats: Default::default(),
            url_compose: Default::default(),
        }
    }

    #[test]
    fn api_key_upstream_is_not_probed() {
        let mut upstream = upstream(None, None, None);
        upstream.api_key = Some("sk-test".to_string());
        assert!(!should_probe_upstream(&upstream));
    }

    #[test]
    fn passthrough_upstream_is_not_probed() {
        assert!(!should_probe_upstream(&upstream(None, None, None)));
    }

    #[test]
    fn account_upstreams_are_probed() {
        assert!(should_probe_upstream(&upstream(Some("k-acct"), None, None)));
        assert!(should_probe_upstream(&upstream(None, Some("c-acct"), None)));
        assert!(should_probe_upstream(&upstream(None, None, Some("x-acct"))));
    }
}
