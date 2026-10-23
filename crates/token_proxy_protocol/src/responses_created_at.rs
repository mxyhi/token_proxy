//! 为缺失时间的 Responses 信封提供同一响应内稳定的回退值，不改上游有效时间。

use serde_json::{Map, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ResponsesCreatedAt {
    fallback: u64,
}

impl Default for ResponsesCreatedAt {
    fn default() -> Self {
        Self {
            fallback: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0),
        }
    }
}

impl ResponsesCreatedAt {
    pub fn normalize(&self, value: &mut Value) -> bool {
        let kind = value.get("type").and_then(Value::as_str);
        if matches!(
            kind,
            Some(
                "response.created"
                    | "response.in_progress"
                    | "response.completed"
                    | "response.incomplete"
                    | "response.failed"
            )
        ) {
            return value
                .get_mut("response")
                .and_then(Value::as_object_mut)
                .is_some_and(|response| self.normalize_object(response));
        }
        if value.get("object").and_then(Value::as_str) == Some("response") {
            return value
                .as_object_mut()
                .is_some_and(|response| self.normalize_object(response));
        }
        false
    }

    fn normalize_object(&self, response: &mut Map<String, Value>) -> bool {
        if response
            .get("created_at")
            .is_some_and(|value| !value.is_null())
        {
            return false;
        }
        response.insert("created_at".to_string(), Value::from(self.fallback));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_and_null_timestamps_share_one_fallback_without_overwriting_upstream() {
        let clock = ResponsesCreatedAt { fallback: 123 };
        for kind in [
            "response.created",
            "response.in_progress",
            "response.completed",
            "response.incomplete",
            "response.failed",
        ] {
            for response in [json!({}), json!({"created_at":null})] {
                let mut event = json!({"type":kind,"response":response});
                assert!(clock.normalize(&mut event));
                assert_eq!(event["response"]["created_at"], 123);
                assert!(!clock.normalize(&mut event));
            }
        }
        let mut valid = json!({"object":"response","created_at":42});
        assert!(!clock.normalize(&mut valid));
        assert_eq!(valid["created_at"], 42);
        let mut missing = json!({"object":"response","created_at":null});
        assert!(clock.normalize(&mut missing));
        assert_eq!(missing["created_at"], 123);
        let mut delta = json!({"type":"response.output_text.delta","delta":"text"});
        let original = delta.clone();
        assert!(!clock.normalize(&mut delta));
        assert_eq!(delta, original);
    }
}
