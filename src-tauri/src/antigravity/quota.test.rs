use super::extract_quota_items;
use serde_json::json;

#[test]
fn extract_quota_items_from_array_models() {
    let value = json!({
        "models": [
            {
                "name": "gemini-3-pro",
                "quotaInfo": { "remainingFraction": 0.5, "resetTime": "2026-01-01T00:00:00Z" }
            }
        ]
    });
    let items = extract_quota_items(&value);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "gemini-3-pro");
    assert_eq!(items[0].percentage, 50.0);
}

#[test]
fn extract_quota_items_from_map_models() {
    let value = json!({
        "models": {
            "claude-opus-4": { "quotaInfo": { "remainingFraction": 0.25 } },
            "text-embedding": { "quotaInfo": { "remainingFraction": 0.9 } }
        }
    });
    let items = extract_quota_items(&value);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "claude-opus-4");
    assert_eq!(items[0].percentage, 25.0);
}
