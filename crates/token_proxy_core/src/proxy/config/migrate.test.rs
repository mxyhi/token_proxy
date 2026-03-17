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
fn migrate_legacy_enable_false_keeps_conversion_disabled_except_antigravity_messages() {
    let mut value = parse_json(
        r#"
        {
          "enable_api_format_conversion": false,
          "upstreams": [
            { "id": "u1", "provider": "openai", "base_url": "https://example.com", "enabled": true },
            { "id": "u2", "provider": "antigravity", "base_url": "", "enabled": true }
          ]
        }
        "#,
    );

    let changed = migrate_config_json(&mut value);
    assert!(changed);

    let obj = value.as_object().expect("root must be object");
    let u1 = obj["upstreams"][0].as_object().expect("u1 must be object");
    assert!(!u1.contains_key("convert_from_map"));

    let u2 = obj["upstreams"][1].as_object().expect("u2 must be object");
    let map = u2
        .get("convert_from_map")
        .and_then(|v| v.as_object())
        .expect("antigravity upstream must have convert_from_map");
    let list = map
        .get("antigravity")
        .and_then(|v| v.as_array())
        .expect("antigravity list must be array");
    assert!(list
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
