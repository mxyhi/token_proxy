use super::*;

fn parse_json(input: &str) -> serde_json::Value {
    serde_json::from_str(input).expect("test json must be valid")
}

#[test]
fn migrate_removes_legacy_fields_and_sets_providers() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "enable_api_format_conversion": true,
          "upstreams": [
            { "id": "u1", "provider": "openai", "base_url": "https://example.com", "enabled": true }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let obj = value.as_object().expect("root must be object");
    assert!(!obj.contains_key("enable_api_format_conversion"));

    let upstreams = obj
        .get("upstreams")
        .and_then(|v| v.as_array())
        .expect("upstreams must be array");
    let upstream = upstreams[0].as_object().expect("upstream must be object");
    assert!(!upstream.contains_key("provider"));
    assert_eq!(
        upstream
            .get("providers")
            .and_then(|v| v.as_array())
            .and_then(|items| items[0].as_str())
            .unwrap_or(""),
        "openai"
    );
    assert!(upstream.contains_key("convert_from_map"));
    assert_eq!(upstream["credential"]["type"].as_str(), Some("passthrough"));
}

#[test]
fn migrate_default_legacy_enable_true_when_missing() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstreams": [
            { "id": "u1", "provider": "openai-response", "base_url": "https://example.com", "enabled": true }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let obj = value.as_object().expect("root must be object");
    let upstream = obj["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    let map = upstream["convert_from_map"]
        .as_object()
        .expect("convert_from_map must be object");
    let formats = map["openai-response"]
        .as_array()
        .expect("formats must be array");
    assert!(formats.iter().any(|v| v.as_str() == Some("openai_chat")));
    assert!(formats
        .iter()
        .any(|v| v.as_str() == Some("anthropic_messages")));
}

#[test]
fn migrate_api_key_to_credential_api_keys() {
    let mut value = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "u1",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "api_key": "key-1",
              "enabled": true
            }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("api_key"));
    assert!(!upstream.contains_key("api_keys"));
    assert_eq!(upstream["credential"]["type"].as_str(), Some("api_keys"));
    let keys = upstream["credential"]["api_keys"]
        .as_array()
        .expect("credential.api_keys must be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].as_str(), Some("key-1"));
}

#[test]
fn migrate_flat_api_keys_to_credential() {
    let mut value = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "u1",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "api_keys": ["key-a", "key-b"],
              "enabled": true
            }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("api_keys"));
    assert_eq!(upstream["credential"]["type"].as_str(), Some("api_keys"));
    assert_eq!(
        upstream["credential"]["api_keys"],
        serde_json::json!(["key-a", "key-b"])
    );
}

#[test]
fn migrate_kiro_account_to_credential() {
    let mut value = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "kiro-u",
              "providers": ["kiro"],
              "base_url": "",
              "kiro_account_id": "kiro-acc-1",
              "enabled": true
            }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("kiro_account_id"));
    assert_eq!(
        upstream["credential"],
        serde_json::json!({
            "type": "account",
            "provider": "kiro",
            "account_id": "kiro-acc-1"
        })
    );
}

#[test]
fn migrate_codex_account_to_credential() {
    let mut value = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "codex-u",
              "providers": ["codex"],
              "base_url": "",
              "codex_account_id": "codex-acc-1",
              "enabled": true
            }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("codex_account_id"));
    assert_eq!(upstream["credential"]["provider"].as_str(), Some("codex"));
    assert_eq!(
        upstream["credential"]["account_id"].as_str(),
        Some("codex-acc-1")
    );
}

#[test]
fn migrate_xai_account_to_credential() {
    let mut value = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "xai-u",
              "providers": ["xai"],
              "base_url": "",
              "xai_account_id": "xai-acc-1",
              "enabled": true
            }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("xai_account_id"));
    assert_eq!(upstream["credential"]["type"].as_str(), Some("account"));
    assert_eq!(upstream["credential"]["provider"].as_str(), Some("xai"));
}

#[test]
fn migrate_rejects_api_keys_and_account_conflict_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "conflict-u",
              "providers": ["codex"],
              "base_url": "",
              "api_keys": ["key-a"],
              "codex_account_id": "codex-acc",
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("conflict must fail");
    assert!(error.contains("cannot combine api_keys with account_id"));
    // 冲突时传入 Value 字节语义不变。
    assert_eq!(value, original);
}

#[test]
fn migrate_rejects_multiple_account_ids_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "multi-acc",
              "providers": ["kiro"],
              "base_url": "",
              "kiro_account_id": "kiro-1",
              "codex_account_id": "codex-1",
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("multi account must fail");
    assert!(error.contains("cannot pin multiple account ids"));
    assert_eq!(value, original);
}

#[test]
fn migrate_rejects_api_keys_non_array_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "bad-keys",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "api_keys": "not-an-array",
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("non-array api_keys must fail");
    assert!(error.contains("api_keys"));
    assert!(error.contains("must be an array"));
    assert_eq!(value, original);
}

#[test]
fn migrate_rejects_api_keys_non_string_element_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "bad-key-elem",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "api_keys": [1, "ok"],
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("non-string api_keys element must fail");
    assert!(error.contains("api_keys"));
    assert!(error.contains("only strings"));
    assert_eq!(value, original);
}

#[test]
fn migrate_rejects_account_id_non_string_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "bad-acc",
              "providers": ["codex"],
              "base_url": "",
              "codex_account_id": 42,
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("non-string account_id must fail");
    assert!(error.contains("codex_account_id"));
    assert!(error.contains("must be a string or null"));
    assert_eq!(value, original);
}

#[test]
fn migrate_rejects_credential_with_legacy_flat_fields_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "mixed",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "credential": { "type": "passthrough" },
              "api_keys": ["should-not-override"],
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("credential + flat must fail");
    assert!(error.contains("cannot combine credential with legacy flat credential fields"));
    assert_eq!(value, original);
    // 确认未静默覆盖为 api_keys credential。
    assert_eq!(
        value["upstreams"][0]["credential"]["type"].as_str(),
        Some("passthrough")
    );
    assert!(value["upstreams"][0].get("api_keys").is_some());
}

#[test]
fn migrate_rejects_credential_with_legacy_account_id_without_mutating_root() {
    let original = parse_json(
        r#"
        {
          "hot_model_mappings": {},
          "upstreams": [
            {
              "id": "mixed-acc",
              "providers": ["xai"],
              "base_url": "",
              "credential": {
                "type": "account",
                "provider": "xai",
                "account_id": "keep-me"
              },
              "xai_account_id": "must-not-override",
              "enabled": true
            }
          ]
        }
        "#,
    );
    let mut value = original.clone();

    let error = migrate_config_json(&mut value).expect_err("credential + account flat must fail");
    assert!(error.contains("cannot combine credential with legacy flat credential fields"));
    assert_eq!(value, original);
}

#[test]
fn migrate_new_format_is_noop() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "model_list_prefix": true,
          "model_list_prefix_default_on_migrated": true,
          "hot_model_mappings": {
            "custom/alias": "custom-target"
          },
          "upstreams": [
            {
              "id": "modern",
              "providers": ["openai"],
              "base_url": "https://example.com",
              "credential": {
                "type": "api_keys",
                "api_keys": ["key-1"]
              },
              "enabled": true
            }
          ]
        }
        "#,
    );
    let before = value.clone();

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(!changed);
    assert_eq!(value, before);
}

#[test]
fn migrate_legacy_upstream_strategy_string_to_structured_fill_first_serial() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstream_strategy": "priority_fill_first",
          "upstreams": []
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    assert_eq!(
        value["upstream_strategy"],
        serde_json::json!({
            "order": "fill_first",
            "dispatch": { "type": "serial" }
        })
    );
}

#[test]
fn migrate_legacy_upstream_strategy_string_to_structured_round_robin_serial() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstream_strategy": "priority_round_robin",
          "upstreams": []
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    assert_eq!(
        value["upstream_strategy"],
        serde_json::json!({
            "order": "round_robin",
            "dispatch": { "type": "serial" }
        })
    );
}

#[test]
fn migrate_adds_default_hot_model_mappings_when_missing() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstreams": []
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    assert_eq!(
        value["hot_model_mappings"]["openai/gpt-5.6-sol"].as_str(),
        Some("gpt-5.6-sol")
    );
    assert_eq!(
        value["hot_model_mappings"]["openai/gpt-5.5"].as_str(),
        Some("gpt-5.5")
    );
    assert_eq!(
        value["hot_model_mappings"]["models/gemini-3.1-pro-preview"].as_str(),
        Some("gemini-3.1-pro-preview")
    );
    assert!(value.get("model_discovery_refresh_secs").is_none());
}

#[test]
fn migrate_preserves_custom_hot_model_mappings() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "model_list_prefix": true,
          "model_list_prefix_default_on_migrated": true,
          "hot_model_mappings": {
            "custom/alias": "custom-target"
          },
          "upstreams": []
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(!changed);

    assert_eq!(
        value["hot_model_mappings"]["custom/alias"].as_str(),
        Some("custom-target")
    );
    assert!(value["hot_model_mappings"].get("openai/gpt-5.5").is_none());
    assert!(value.get("model_discovery_refresh_secs").is_none());
}

#[test]
fn migrate_enables_model_list_prefix_when_missing_or_false() {
    let mut missing = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "hot_model_mappings": { "a": "b" },
          "upstreams": []
        }
        "#,
    );
    let changed = migrate_config_json(&mut missing).expect("migrate missing");
    assert!(changed);
    assert_eq!(missing["model_list_prefix"], serde_json::json!(true));
    assert_eq!(
        missing["model_list_prefix_default_on_migrated"],
        serde_json::json!(true)
    );

    let mut explicit_false = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "model_list_prefix": false,
          "hot_model_mappings": { "a": "b" },
          "upstreams": []
        }
        "#,
    );
    let changed = migrate_config_json(&mut explicit_false).expect("migrate false");
    assert!(changed);
    assert_eq!(explicit_false["model_list_prefix"], serde_json::json!(true));
    assert_eq!(
        explicit_false["model_list_prefix_default_on_migrated"],
        serde_json::json!(true)
    );
}

#[test]
fn migrate_does_not_reforce_model_list_prefix_after_user_disables() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "model_list_prefix": false,
          "model_list_prefix_default_on_migrated": true,
          "hot_model_mappings": { "a": "b" },
          "upstreams": []
        }
        "#,
    );
    let before = value.clone();
    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(!changed);
    assert_eq!(value, before);
    assert_eq!(value["model_list_prefix"], serde_json::json!(false));
}

#[test]
fn migrate_removes_legacy_model_discovery_refresh_secs() {
    let mut value = parse_json(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "model_discovery_refresh_secs": 900,
          "hot_model_mappings": {
            "custom/alias": "custom-target"
          },
          "upstreams": []
        }
        "#,
    );

    let changed = migrate_config_json(&mut value).expect("migrate");
    assert!(changed);

    assert!(value.get("model_discovery_refresh_secs").is_none());
}
