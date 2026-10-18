use super::*;
use token_proxy_config::{LocalApiKey, LocalApiKeyScope};

const QWEN: &str = "qwen3.8-27b-uncensored";

async fn catalog_request(state: ProxyStateHandle, method: Method, key: Option<&str>) -> Response {
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {key}")).unwrap(),
        );
    }
    proxy_request(
        State(state),
        method,
        Uri::from_static("/codex/models.json?client_version=0.163.0"),
        headers,
        Body::empty(),
    )
    .await
}

#[test]
fn codex_catalog_preserves_official_metadata_and_only_appends_selected_models() {
    run_async(async {
        // 不建 Codex OAuth 账户；官方目录和显式本地模型无需实时上游即可读取。
        let mut config =
            config_with_upstreams(&[(PROVIDER_RESPONSES, 0, "qwen", FORMATS_RESPONSES)]);
        config.codex_extra_models = vec![
            QWEN.into(),
            QWEN.into(),
            "not-available".into(),
            "gpt-6-astra".into(),
        ];
        config.upstreams.get_mut(PROVIDER_RESPONSES).unwrap().groups[0].items[0]
            .advertised_model_ids = vec![QWEN.into(), "other-model".into(), "gpt-6-astra".into()];
        let dir = next_test_data_dir("codex-catalog");
        let state = build_test_state_handle(config, dir.clone()).await;
        let response = catalog_request(state.clone(), Method::GET, None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(
            body.len() < 1024 * 1024,
            "Codex remote catalog has a 1 MiB limit"
        );
        let result: Value = serde_json::from_slice(&body).unwrap();
        let official: Value =
            serde_json::from_str(include_str!("../../../resources/codex-models-0.163.0.json"))
                .unwrap();
        let original = official["models"].as_array().unwrap();
        let models = result["models"].as_array().unwrap();
        assert_eq!(
            &models[..original.len()],
            original.as_slice(),
            "official metadata and order must remain unchanged"
        );
        assert_eq!(models.len(), original.len() + 1);
        assert_eq!(models.last().unwrap()["slug"], QWEN);
        assert_eq!(models.last().unwrap()["visibility"], "list");
        assert_eq!(models.last().unwrap()["support_verbosity"], false);
        assert_eq!(models.last().unwrap()["use_responses_lite"], false);
        // ModelsResponse 在反序列化时强制要求指令模板；缺失会让官方模型也消失。
        for model in models {
            assert!(model["model_messages"]["instructions_template"]
                .as_str()
                .or_else(|| model["base_instructions"].as_str())
                .is_some_and(|instructions| !instructions.is_empty()));
        }
        let head = catalog_request(state.clone(), Method::HEAD, None).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(to_bytes(head.into_body(), usize::MAX)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            catalog_request(state, Method::POST, None).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let _ = std::fs::remove_dir_all(dir);
    });
}

#[test]
fn codex_catalog_auth_and_scope_hide_unselected_upstream_models() {
    run_async(async {
        let mut config =
            config_with_upstreams(&[(PROVIDER_RESPONSES, 0, "qwen", FORMATS_RESPONSES)]);
        config.codex_extra_models = vec![QWEN.into()];
        config.upstreams.get_mut(PROVIDER_RESPONSES).unwrap().groups[0].items[0]
            .advertised_model_ids = vec![QWEN.into()];
        config.local_api_keys = vec![LocalApiKey {
            id: "restricted".into(),
            name: "restricted".into(),
            key: "test-secret".into(),
            enabled: true,
            scope: LocalApiKeyScope::Selected {
                upstream_ids: vec!["other".into()],
            },
        }];
        let dir = next_test_data_dir("codex-catalog-auth");
        let state = build_test_state_handle(config, dir.clone()).await;
        assert_eq!(
            catalog_request(state.clone(), Method::GET, None)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            catalog_request(state.clone(), Method::GET, Some("invalid"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = catalog_request(state, Method::GET, Some("test-secret")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let result: Value = serde_json::from_slice(&body).unwrap();
        assert!(!result["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["slug"] == QWEN));
        let _ = std::fs::remove_dir_all(dir);
    });
}
