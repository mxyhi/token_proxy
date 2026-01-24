use super::{extract_plan_type, extract_project_id};
use serde_json::json;

#[test]
fn extract_project_id_supports_string_and_object() {
    let value = json!({ "cloudaicompanionProject": "project-123" });
    assert_eq!(extract_project_id(&value), Some("project-123".to_string()));

    let value = json!({ "cloudaicompanionProject": { "id": "project-456" } });
    assert_eq!(extract_project_id(&value), Some("project-456".to_string()));
}

#[test]
fn extract_plan_type_picks_default_allowed_tier() {
    let value = json!({
        "allowedTiers": [
            { "id": "FREE", "isDefault": true },
            { "id": "PRO", "isDefault": false }
        ]
    });
    assert_eq!(extract_plan_type(&value), Some("FREE".to_string()));
}
