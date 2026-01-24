use serde_json::{json, Value};

pub(super) fn map_usage_responses_to_chat(usage: &Value) -> Option<Value> {
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
    Some(json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": total
    }))
}

pub(super) fn map_usage_chat_to_responses(usage: &Value) -> Option<Value> {
    let usage = usage.as_object()?;
    let prompt = usage.get("prompt_tokens").and_then(Value::as_u64);
    let completion = usage.get("completion_tokens").and_then(Value::as_u64);
    let total = usage.get("total_tokens").and_then(Value::as_u64).or_else(|| match (prompt, completion) {
        (Some(prompt), Some(completion)) => prompt.checked_add(completion),
        _ => None,
    });
    if prompt.is_none() && completion.is_none() && total.is_none() {
        return None;
    }
    Some(json!({
        "input_tokens": prompt,
        "output_tokens": completion,
        "total_tokens": total
    }))
}

