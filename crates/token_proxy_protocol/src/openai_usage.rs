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
        .and_then(map_responses_output_details_to_chat)
    {
        mapped.insert("completion_tokens_details".to_string(), details);
    }

    Some(Value::Object(mapped))
}

fn map_responses_input_details_to_chat(details: &Map<String, Value>) -> Option<Value> {
    let mut mapped = Map::new();
    insert_nonzero_u64(&mut mapped, "cached_tokens", details.get("cached_tokens"));
    insert_nonzero_u64(
        &mut mapped,
        "cached_creation_tokens",
        details.get("cached_creation_tokens"),
    );
    insert_nonzero_u64(&mut mapped, "audio_tokens", details.get("audio_tokens"));
    if mapped.contains_key("cached_creation_tokens") {
        tracing::debug!("preserved cache creation usage in Chat details");
    }
    (!mapped.is_empty()).then_some(Value::Object(mapped))
}

fn map_responses_output_details_to_chat(details: &Map<String, Value>) -> Option<Value> {
    let mut mapped = Map::new();
    insert_nonzero_u64(
        &mut mapped,
        "reasoning_tokens",
        details.get("reasoning_tokens"),
    );
    insert_nonzero_u64(&mut mapped, "audio_tokens", details.get("audio_tokens"));
    insert_nonzero_u64(
        &mut mapped,
        "accepted_prediction_tokens",
        details.get("accepted_prediction_tokens"),
    );
    insert_nonzero_u64(
        &mut mapped,
        "rejected_prediction_tokens",
        details.get("rejected_prediction_tokens"),
    );
    (!mapped.is_empty()).then_some(Value::Object(mapped))
}

fn insert_nonzero_u64(mapped: &mut Map<String, Value>, key: &str, value: Option<&Value>) {
    let Some(tokens) = value.and_then(Value::as_u64).filter(|tokens| *tokens > 0) else {
        return;
    };
    mapped.insert(key.to_string(), json!(tokens));
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

    // Preserve reasoning token details when converting Chat -> Responses.
    let reasoning_tokens = usage
        .get("completion_tokens_details")
        .and_then(Value::as_object)
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    mapped.insert(
        "output_tokens_details".to_string(),
        json!({ "reasoning_tokens": reasoning_tokens }),
    );

    Some(Value::Object(mapped))
}
