use super::*;

fn section(current_ip: &str, current_format: &str, available: &[&str]) -> MyProxyEndpointSection {
    MyProxyEndpointSection {
        current_ip: Some(current_ip.to_string()),
        current_format: Some(current_format.to_string()),
        available_ips: available.iter().map(|ip| ip.to_string()).collect(),
    }
}

#[test]
fn scan_enabled_only_for_wildcard_hosts() {
    assert!(scan_enabled("0.0.0.0"));
    assert!(scan_enabled(" :: "));
    assert!(!scan_enabled("127.0.0.1"));
    assert!(!scan_enabled("localhost"));
    assert!(!scan_enabled(""));
    assert!(!scan_enabled("192.168.1.23"));
}

#[test]
fn normalize_display_host_defaults_to_loopback() {
    assert_eq!(normalize_display_host(""), "127.0.0.1");
    assert_eq!(normalize_display_host("  "), "127.0.0.1");
    assert_eq!(normalize_display_host("localhost"), "localhost");
    assert_eq!(normalize_display_host(" 127.0.0.1 "), "127.0.0.1");
}

#[test]
fn normalize_format_falls_back_to_openai() {
    assert_eq!(normalize_format(Some("anthropic")), FORMAT_ANTHROPIC);
    assert_eq!(normalize_format(Some("gemini")), FORMAT_GEMINI);
    assert_eq!(normalize_format(Some(" gemini ")), FORMAT_GEMINI);
    assert_eq!(normalize_format(Some("gemini-pro")), FORMAT_OPENAI);
    assert_eq!(normalize_format(Some("")), FORMAT_OPENAI);
    assert_eq!(normalize_format(None), FORMAT_OPENAI);
}

#[test]
fn normalize_available_filters_and_sorts() {
    let out = normalize_available([
        "10.0.0.5",
        "192.168.1.23",
        "172.20.0.3",
        "169.254.10.10",
        "192.168.1.23",
        "not-an-ip",
        "::1",
    ]);
    assert_eq!(
        out,
        vec!["192.168.1.23", "172.20.0.3", "10.0.0.5", "127.0.0.1"]
    );
}

#[test]
fn normalize_available_always_includes_loopback() {
    assert_eq!(normalize_available(Vec::<String>::new()), vec!["127.0.0.1"]);
}

#[test]
fn needs_scan_only_when_wildcard_and_empty() {
    assert!(needs_scan("0.0.0.0", None));
    assert!(needs_scan("0.0.0.0", Some(&section("127.0.0.1", "openai", &[]))));
    assert!(!needs_scan(
        "0.0.0.0",
        Some(&section("192.168.1.23", "openai", &["192.168.1.23"]))
    ));
    assert!(!needs_scan("127.0.0.1", None));
    assert!(!needs_scan("localhost", Some(&section("localhost", "openai", &[]))));
}

#[test]
fn resolve_keeps_current_ip_when_still_available() {
    // 已落盘快照是归一化后的形态（含 127.0.0.1、按 192→172→10→127 排序）。
    let existing = section(
        "192.168.1.23",
        "anthropic",
        &["192.168.1.23", "10.0.0.5", "127.0.0.1"],
    );
    let resolved = resolve("0.0.0.0", Some(&existing), None);
    assert_eq!(resolved.effective_ip, "192.168.1.23");
    assert_eq!(resolved.effective_format, FORMAT_ANTHROPIC);
    assert!(resolved.scan_enabled);
    assert!(!resolved.should_persist, "same values must not rewrite config");
}

#[test]
fn resolve_falls_back_192_then_172_then_loopback() {
    let stale = section("192.168.9.9", "openai", &["10.0.0.5", "172.20.0.3", "192.168.1.23"]);
    let resolved = resolve("0.0.0.0", Some(&stale), None);
    assert_eq!(resolved.effective_ip, "192.168.1.23");
    assert!(resolved.should_persist);

    let no_192 = section("192.168.9.9", "openai", &["10.0.0.5", "172.20.0.3"]);
    assert_eq!(resolve("0.0.0.0", Some(&no_192), None).effective_ip, "172.20.0.3");

    let only_10 = section("192.168.9.9", "openai", &["10.0.0.5", "127.0.0.1"]);
    assert_eq!(resolve("0.0.0.0", Some(&only_10), None).effective_ip, "127.0.0.1");
}

#[test]
fn resolve_non_scan_clears_snapshot_and_fixes_host() {
    let stale = section("192.168.1.23", "openai", &["192.168.1.23", "10.0.0.5"]);
    let resolved = resolve("127.0.0.1", Some(&stale), None);
    assert!(!resolved.scan_enabled);
    assert_eq!(resolved.effective_ip, "127.0.0.1");
    assert_eq!(resolved.available_ips, Vec::<String>::new());
    assert!(resolved.should_persist, "stale snapshot must be cleared");

    let localhost = resolve("localhost", None, None);
    assert_eq!(localhost.effective_ip, "localhost");
    assert!(!localhost.should_persist, "absent section must stay absent");
}

#[test]
fn resolve_persists_only_after_scan_when_section_absent() {
    let resolved = resolve("0.0.0.0", None, Some(vec!["10.0.0.5".into()]));
    assert!(resolved.should_persist);
    assert_eq!(resolved.effective_ip, "127.0.0.1");
    assert_eq!(
        resolved.available_ips,
        vec!["10.0.0.5".to_string(), "127.0.0.1".to_string()]
    );
    assert_eq!(resolved.section.current_format.as_deref(), Some("openai"));
}

#[test]
fn resolve_ignores_scan_result_when_host_is_not_wildcard() {
    let resolved = resolve("127.0.0.1", None, Some(vec!["192.168.1.23".into()]));
    assert!(!resolved.scan_enabled);
    assert!(resolved.available_ips.is_empty());
    assert_eq!(resolved.effective_ip, "127.0.0.1");
    assert!(!resolved.should_persist);
}

#[test]
fn validate_selection_rejects_bad_input() {
    let available = vec!["192.168.1.23".to_string(), "127.0.0.1".to_string()];
    assert!(validate_selection("0.0.0.0", &available, "192.168.1.23", "anthropic").is_ok());
    assert!(validate_selection("0.0.0.0", &available, "192.168.1.23", "gemini").is_ok());
    assert!(validate_selection("0.0.0.0", &available, "10.0.0.9", "openai").is_err());
    assert!(validate_selection("0.0.0.0", &available, "192.168.1.23", "gemini-pro").is_err());
    assert!(validate_selection("127.0.0.1", &[], "127.0.0.1", "openai").is_ok());
    assert!(validate_selection("127.0.0.1", &[], "192.168.1.23", "openai").is_err());
    assert!(validate_selection("localhost", &[], "localhost", "openai").is_ok());
    assert!(validate_selection("", &[], "127.0.0.1", "openai").is_ok());
}

#[test]
fn validate_selection_normalizes_format_whitespace() {
    let available = vec!["192.168.1.23".to_string()];
    let (ip, format) = validate_selection("0.0.0.0", &available, " 192.168.1.23 ", " gemini ").unwrap();
    assert_eq!(ip, "192.168.1.23");
    assert_eq!(format, "gemini");
}

#[test]
fn section_serialization_omits_empty_fields() {
    let empty = serde_json::to_string(&MyProxyEndpointSection::default()).unwrap();
    assert_eq!(empty, "{}");

    let filled = serde_json::to_string(&section("192.168.1.23", "openai", &["192.168.1.23"]))
        .unwrap();
    assert_eq!(
        filled,
        r#"{"current_ip":"192.168.1.23","current_format":"openai","available_ips":["192.168.1.23"]}"#
    );
}

#[derive(Debug, serde::Deserialize)]
struct Holder {
    #[serde(default, deserialize_with = "super::de_lenient")]
    my_proxy_endpoint: Option<MyProxyEndpointSection>,
}

#[test]
fn de_lenient_accepts_valid_and_degrades_invalid() {
    let ok: Holder = serde_json::from_str(
        r#"{"my_proxy_endpoint":{"current_ip":"10.0.0.5","current_format":"gemini","available_ips":["10.0.0.5"]}}"#,
    )
    .unwrap();
    let section = ok.my_proxy_endpoint.expect("valid section");
    assert_eq!(section.current_ip.as_deref(), Some("10.0.0.5"));
    assert_eq!(section.current_format.as_deref(), Some("gemini"));

    let null: Holder = serde_json::from_str(r#"{"my_proxy_endpoint":null}"#).unwrap();
    assert!(null.my_proxy_endpoint.is_none());

    let string: Holder = serde_json::from_str(r#"{"my_proxy_endpoint":"oops"}"#).unwrap();
    assert!(string.my_proxy_endpoint.is_none());

    let array: Holder = serde_json::from_str(r#"{"my_proxy_endpoint":["oops"]}"#).unwrap();
    assert!(array.my_proxy_endpoint.is_none());

    let bad_type: Holder =
        serde_json::from_str(r#"{"my_proxy_endpoint":{"current_format":123}}"#).unwrap();
    assert!(bad_type.my_proxy_endpoint.is_none());

    let missing: Holder = serde_json::from_str(r#"{}"#).unwrap();
    assert!(missing.my_proxy_endpoint.is_none());
}
