use axum::body::Bytes;
use serde_json::Value;

pub(crate) fn rewrite_request_model(bytes: &Bytes, model: &str) -> Option<Bytes> {
    let mut value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object_mut()?;
    if !object.contains_key("model") {
        return None;
    }
    object.insert("model".to_string(), Value::String(model.to_string()));
    serde_json::to_vec(&value).ok().map(Bytes::from)
}

pub(crate) fn rewrite_response_model(bytes: &Bytes, model: &str) -> Option<Bytes> {
    let mut value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object_mut()?;
    if object.contains_key("model") {
        object.insert("model".to_string(), Value::String(model.to_string()));
        return serde_json::to_vec(&value).ok().map(Bytes::from);
    }
    let Some(response) = object.get_mut("response").and_then(Value::as_object_mut) else {
        return None;
    };
    if !response.contains_key("model") {
        return None;
    }
    response.insert("model".to_string(), Value::String(model.to_string()));
    serde_json::to_vec(&value).ok().map(Bytes::from)
}
