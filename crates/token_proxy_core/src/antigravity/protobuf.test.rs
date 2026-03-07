use super::extract_token_record;
use super::inject_token_record;
use super::AntigravityTokenRecord;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[test]
fn extract_empty_returns_none() {
    let result = extract_token_record("").expect("empty base64 should be valid");
    assert!(result.is_none());
}

#[test]
fn inject_and_extract_roundtrip() {
    let expires_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("timestamp");
    let expires_at_text = expires_at.format(&Rfc3339).expect("format");
    let record = AntigravityTokenRecord {
        access_token: "ya29.test-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expired: Some(expires_at_text.clone()),
        expires_in: None,
        timestamp: None,
        email: None,
        token_type: Some("Bearer".to_string()),
        project_id: None,
        source: None,
    };

    let encoded = inject_token_record("", &record).expect("inject");
    let extracted = extract_token_record(&encoded)
        .expect("extract")
        .expect("record");

    assert_eq!(extracted.access_token, record.access_token);
    assert_eq!(extracted.refresh_token, record.refresh_token);
    assert_eq!(extracted.token_type, record.token_type);
    assert_eq!(extracted.expired, Some(expires_at_text));
    assert_eq!(extracted.source.as_deref(), Some("ide"));
}
