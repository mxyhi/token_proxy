//! 最终发出图片请求的参数观察；不保存提示词、文件、URL 或图像内容。
use axum::body::Bytes;
use serde_json::{Map, Value};

const FIELDS: &[&str] = &[
    "n",
    "size",
    "quality",
    "background",
    "output_format",
    "output_compression",
    "moderation",
    "input_fidelity",
    "response_format",
    "aspect_ratio",
    "resolution",
    "partial_images",
];
const MAX_PARAMETER_BYTES: usize = 256;

fn retain_parameters(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .filter(|(name, value)| {
            FIELDS.contains(&name.as_str())
                && (value.is_boolean()
                    || value.as_u64().is_some()
                    || value
                        .as_str()
                        .is_some_and(|text| text.len() <= MAX_PARAMETER_BYTES))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

pub(super) async fn extract(path: &str, content_type: &str, body: &Bytes) -> Option<Value> {
    let path = path.split('?').next().unwrap_or(path);
    if !matches!(
        path,
        "/v1/images/generations" | "/v1/images/edits" | "/v1/responses"
    ) {
        return None;
    }
    let mut details = Map::new();
    if content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"))
    {
        let boundary = multer::parse_boundary(content_type).ok()?;
        let mut limits = multer::SizeLimit::new().whole_stream(body.len() as u64);
        for name in FIELDS {
            limits = limits.for_field(*name, MAX_PARAMETER_BYTES as u64);
        }
        let stream = futures_util::stream::once(std::future::ready(Ok::<
            _,
            std::convert::Infallible,
        >(body.clone())));
        let mut multipart = multer::Multipart::with_constraints(
            stream,
            boundary,
            multer::Constraints::new().size_limit(limits),
        );
        while let Some(field) = multipart.next_field().await.ok()? {
            if field.file_name().is_some() {
                continue;
            }
            let Some(name) = field
                .name()
                .filter(|name| FIELDS.contains(name))
                .map(str::to_owned)
            else {
                continue;
            };
            let bytes = field.bytes().await.ok()?;
            let text = std::str::from_utf8(&bytes).ok()?;
            let value = match name.as_str() {
                "n" | "output_compression" | "partial_images" => {
                    Value::from(text.parse::<u64>().ok()?)
                }
                _ => Value::String(text.to_string()),
            };
            details.insert(name, value);
        }
    } else {
        let value: Value = serde_json::from_slice(body).ok()?;
        let object = value.as_object()?;
        if path == "/v1/responses" {
            let tools: Vec<Value> = object
                .get("tools")?
                .as_array()?
                .iter()
                .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("image_generation"))
                .filter_map(Value::as_object)
                .map(retain_parameters)
                .filter(|parameters| !parameters.is_empty())
                .map(Value::Object)
                .collect();
            if !tools.is_empty() {
                details.insert("image_generation_tools".into(), Value::Array(tools));
            }
        } else {
            details = retain_parameters(object);
        }
    }
    (!details.is_empty()).then_some(Value::Object(details))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn image_json_observes_only_final_parameters() {
        let bytes = Bytes::from(json!({"size":"1024x1024","n":2,"output_compression":0,"prompt":"private","image":"data:image/png;base64,private"}).to_string());
        assert_eq!(
            extract("/v1/images/generations", "application/json", &bytes).await,
            Some(json!({"size":"1024x1024","n":2,"output_compression":0}))
        );
        assert!(extract("/v1/chat/completions", "application/json", &bytes)
            .await
            .is_none());
        let response = Bytes::from(json!({"input":"private","tools":[{"type":"image_generation","quality":"high"},{"type":"function","name":"private"}]}).to_string());
        assert_eq!(
            extract("/v1/responses", "application/json", &response).await,
            Some(json!({"image_generation_tools":[{"quality":"high"}]}))
        );
    }

    #[tokio::test]
    async fn multipart_skips_files_and_prompt_before_and_after_parameters() {
        let bytes = Bytes::from_static(b"--test\r\nContent-Disposition: form-data; name=\"image\"; filename=\"private.png\"\r\nContent-Type: image/png\r\n\r\n\xff\x00private\r\n--test\r\nContent-Disposition: form-data; name=\"size\"\r\n\r\n1024x1024\r\n--test\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nprivate prompt\r\n--test\r\nContent-Disposition: form-data; name=\"n\"\r\n\r\n2\r\n--test--\r\n");
        assert_eq!(
            extract(
                "/v1/images/edits",
                "multipart/form-data; boundary=\"test\"",
                &bytes
            )
            .await,
            Some(json!({"size":"1024x1024","n":2}))
        );
        assert!(extract("/v1/images/edits", "multipart/form-data", &bytes)
            .await
            .is_none());
    }
}
