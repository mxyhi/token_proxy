use serde_json::{Map, Value};

use super::super::constants::KIRO_MAX_OUTPUT_TOKENS;
use super::super::types::KiroInferenceConfig;

pub(super) fn build_inference_config(object: &Map<String, Value>) -> Option<KiroInferenceConfig> {
    let mut max_tokens = object
        .get("max_output_tokens")
        .or_else(|| object.get("max_tokens"))
        .and_then(Value::as_i64);
    if let Some(value) = max_tokens {
        if value == -1 {
            max_tokens = Some(KIRO_MAX_OUTPUT_TOKENS);
        }
    }
    let temperature = object.get("temperature").and_then(Value::as_f64);
    let top_p = object.get("top_p").and_then(Value::as_f64);

    if max_tokens.is_none() && temperature.is_none() && top_p.is_none() {
        return None;
    }

    Some(KiroInferenceConfig {
        max_tokens,
        temperature,
        top_p,
    })
}
