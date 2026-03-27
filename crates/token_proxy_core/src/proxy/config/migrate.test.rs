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

    let changed = migrate_config_json(&mut value);
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

    let changed = migrate_config_json(&mut value);
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
fn migrate_api_key_to_api_keys() {
    let mut value = parse_json(
        r#"
        {
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

    let changed = migrate_config_json(&mut value);
    assert!(changed);

    let upstream = value["upstreams"][0]
        .as_object()
        .expect("upstream must be object");
    assert!(!upstream.contains_key("api_key"));
    let keys = upstream["api_keys"]
        .as_array()
        .expect("api_keys must be array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].as_str(), Some("key-1"));
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

    let changed = migrate_config_json(&mut value);
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

    let changed = migrate_config_json(&mut value);
    assert!(changed);

    assert_eq!(
        value["upstream_strategy"],
        serde_json::json!({
            "order": "round_robin",
            "dispatch": { "type": "serial" }
        })
    );
}
