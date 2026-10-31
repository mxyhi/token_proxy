use serde_json::{json, Map, Value};

pub fn map_usage_responses_to_chat(usage: &Value) -> Option<Value> {
    let usage = usage.as_object()?;
    let input = usage.get("input_tokens").and_then(Value::as_u64);
    let output = usage.get("output_tokens").and_then(Value::as_u64);
    let total = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| match (input, output) {
            (Some(input), Some(output)) => input.checked_add(output),
            _ => None,
        });
    if input.is_none() && output.is_none() && total.is_none() {
        return None;
    }

    let mut mapped = Map::new();
    mapped.insert("prompt_tokens".to_string(), json!(input));
    mapped.insert("completion_tokens".to_string(), json!(output));
    mapped.insert("total_tokens".to_string(), json!(total));

    if let Some(details) = usage
        .get("input_tokens_details")
        .and_then(Value::as_object)
        .and_then(map_responses_input_details_to_chat)
    {
        mapped.insert("prompt_tokens_details".to_string(), details);
    }

    if let Some(details) = usage
        .get("output_tokens_details")
        .and_then(Value::as_object)
        .filter(|details| !details.is_empty())
    {
        mapped.insert("completion_tokens_details".to_string(), json!(details));
    }

    Some(Value::Object(mapped))
}

fn map_responses_input_details_to_chat(details: &Map<String, Value>) -> Option<Value> {
    // 两种 OpenAI 协议的 token 明细共享语义；保留上游显式零和扩展字段。
    let mut mapped = details.clone();
    let cache_write_tokens = details.get("cache_write_tokens").and_then(Value::as_u64);
    if let Some(tokens) = cache_write_tokens {
        mapped.insert("cache_write_tokens".to_string(), json!(tokens));
        mapped.insert(
            "cached_creation_tokens".to_string(),
            // Legacy clients read this name, so it mirrors the native field.
            json!(tokens),
        );
    }
    if mapped.contains_key("cached_creation_tokens") {
        tracing::debug!("preserved cache creation usage in Chat details");
    }
    (!mapped.is_empty()).then_some(Value::Object(mapped))
}

pub fn map_usage_chat_to_responses(usage: &Value) -> Option<Value> {
    let usage = usage.as_object()?;
    let prompt = usage.get("prompt_tokens").and_then(Value::as_u64);
    let completion = usage.get("completion_tokens").and_then(Value::as_u64);
    let total = usage
        .get("total_tokens")
        .and_then(Value::as_u64)
        .or_else(|| match (prompt, completion) {
            (Some(prompt), Some(completion)) => prompt.checked_add(completion),
            _ => None,
        });
    if prompt.is_none() && completion.is_none() && total.is_none() {
        return None;
    }

    let mut mapped = Map::new();
    mapped.insert("input_tokens".to_string(), json!(prompt));
    mapped.insert("output_tokens".to_string(), json!(completion));
    mapped.insert("total_tokens".to_string(), json!(total));

    // Gemini 等组合转换需要保留缓存输入明细，避免后续计费把缓存计为普通输入。
    if let Some(details) = usage
        .get("prompt_tokens_details")
        .filter(|value| value.is_object())
    {
        mapped.insert("input_tokens_details".to_string(), details.clone());
    }

    // 保留完整输出明细。reasoning 仍优先使用嵌套字段，且不重复加到输出总数。
    let mut output_details = usage
        .get("completion_tokens_details")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let reasoning_tokens = output_details
        .get("reasoning_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("reasoning_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    output_details.insert("reasoning_tokens".to_string(), json!(reasoning_tokens));
    mapped.insert(
        "output_tokens_details".to_string(),
        Value::Object(output_details),
    );

    Some(Value::Object(mapped))
}

#[cfg(test)]
mod tests {
    use super::{map_usage_chat_to_responses, map_usage_responses_to_chat};
    use serde_json::json;

    #[test]
    fn openai_usage_conversion_preserves_details_and_explicit_zero() {
        let input =
            json!({"cached_tokens":0,"image_tokens":2,"audio_tokens":0,"cache_write_tokens":0});
        let output = json!({"reasoning_tokens":0,"image_tokens":3,"audio_tokens":4,"accepted_prediction_tokens":0,"provider_detail":{"a":1}});
        let chat = map_usage_responses_to_chat(&json!({
            "input_tokens":10,"output_tokens":8,
            "input_tokens_details":input,"output_tokens_details":output
        }))
        .unwrap();
        assert_eq!(chat["prompt_tokens_details"]["cached_tokens"], 0);
        assert_eq!(chat["prompt_tokens_details"]["image_tokens"], 2);
        assert_eq!(chat["completion_tokens_details"], output);
        let responses = map_usage_chat_to_responses(&chat).unwrap();
        assert_eq!(responses["output_tokens_details"], output);
        assert_eq!(responses["output_tokens"], 8);
    }

    #[test]
    fn chat_reasoning_tokens_use_nested_value_then_flat_without_double_counting() {
        for (details, expected) in [
            (json!({}), 4),
            (json!({"reasoning_tokens":0}), 0),
            (json!({"reasoning_tokens":2}), 2),
        ] {
            let mapped = map_usage_chat_to_responses(&json!({
                "prompt_tokens":10,"completion_tokens":6,"reasoning_tokens":4,
                "completion_tokens_details":details
            }))
            .unwrap();
            assert_eq!(
                mapped["output_tokens_details"]["reasoning_tokens"],
                expected
            );
            assert_eq!(mapped["output_tokens"], 6);
            assert_eq!(mapped["total_tokens"], 16);
        }
    }

    #[test]
    fn responses_cache_write_maps_to_both_chat_cache_fields() {
        let mapped = map_usage_responses_to_chat(&json!({
            "input_tokens": 10,
            "output_tokens": 2,
            "input_tokens_details": { "cache_write_tokens": 3 }
        }))
        .expect("usage mapping");

        assert_eq!(
            mapped["prompt_tokens_details"]["cache_write_tokens"],
            json!(3)
        );
        assert_eq!(
            mapped["prompt_tokens_details"]["cached_creation_tokens"],
            json!(3)
        );
    }

    #[test]
    fn cache_write_tokens_overrides_conflicting_legacy_cache_creation_tokens() {
        let mapped = map_usage_responses_to_chat(&json!({
            "input_tokens": 10,
            "output_tokens": 2,
            "input_tokens_details": {
                "cache_write_tokens": 3,
                "cached_creation_tokens": 9
            }
        }))
        .expect("usage mapping");

        assert_eq!(
            mapped["prompt_tokens_details"]["cache_write_tokens"],
            json!(3)
        );
        assert_eq!(
            mapped["prompt_tokens_details"]["cached_creation_tokens"],
            json!(3)
        );
    }

    #[test]
    fn zero_cache_write_tokens_overrides_conflicting_legacy_cache_creation_tokens() {
        let mapped = map_usage_responses_to_chat(&json!({
            "input_tokens": 10,
            "output_tokens": 2,
            "input_tokens_details": {
                "cache_write_tokens": 0,
                "cached_creation_tokens": 9
            }
        }))
        .expect("usage mapping");

        assert_eq!(
            mapped["prompt_tokens_details"]["cache_write_tokens"],
            json!(0)
        );
        assert_eq!(
            mapped["prompt_tokens_details"]["cached_creation_tokens"],
            json!(0)
        );
    }

    #[test]
    fn chat_usage_mapping_keeps_existing_shape() {
        let mapped = map_usage_chat_to_responses(&json!({
            "prompt_tokens": 4,
            "completion_tokens": 1,
            "total_tokens": 5
        }))
        .expect("usage mapping");

        assert_eq!(mapped["input_tokens"], json!(4));
        assert_eq!(mapped["output_tokens"], json!(1));
    }
}
