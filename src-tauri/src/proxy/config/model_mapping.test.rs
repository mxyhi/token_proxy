use super::*;

#[test]
fn model_mapping_prefers_exact_then_prefix_then_wildcard() {
    let mut mappings = HashMap::new();
    mappings.insert("gpt-4".to_string(), "gpt-4.1".to_string());
    mappings.insert("gpt-4*".to_string(), "gpt-4.1-mini".to_string());
    mappings.insert("*".to_string(), "gpt-default".to_string());
    let rules = compile_model_mappings("demo", &mappings)
        .expect("compile")
        .expect("rules");

    assert_eq!(rules.map_model("gpt-4"), Some("gpt-4.1"));
    assert_eq!(rules.map_model("gpt-4-vision"), Some("gpt-4.1-mini"));
    assert_eq!(rules.map_model("other"), Some("gpt-default"));
}

#[test]
fn model_mapping_prefix_prefers_longer_prefix() {
    let mut mappings = HashMap::new();
    mappings.insert("gpt-4*".to_string(), "wide".to_string());
    mappings.insert("gpt-4.1*".to_string(), "narrow".to_string());
    let rules = compile_model_mappings("demo", &mappings)
        .expect("compile")
        .expect("rules");
    assert_eq!(rules.map_model("gpt-4.1-mini"), Some("narrow"));
}

#[test]
fn model_mapping_rejects_multiple_wildcards() {
    let mut mappings = HashMap::new();
    mappings.insert("*".to_string(), "a".to_string());
    mappings.insert(" * ".to_string(), "b".to_string());
    let err = compile_model_mappings("demo", &mappings).unwrap_err();
    assert!(err.contains("wildcard"));
}
