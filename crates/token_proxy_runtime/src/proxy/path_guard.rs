const MAX_PATH_SEGMENT_BYTES: usize = 128;
const MAX_RESPONSES_SUFFIX_SEGMENTS: usize = 8;

pub(crate) const INVALID_UPSTREAM_PATH_MESSAGE: &str = "Invalid upstream URL path.";
pub(crate) const REDACTED_INVALID_PATH: &str = "[invalid-upstream-path]";

/// 校验客户端可控、可能原样参与上游 URL 拼接的路径。
pub(crate) fn validate_sensitive_path(path_with_query: &str) -> Result<(), &'static str> {
    let path = path_with_query
        .split_once('?')
        .map_or(path_with_query, |(path, _)| path);

    if let Some(suffix) = strip_path_suffix(path, &["/v1/responses", "/responses"]) {
        return validate_suffix(suffix, MAX_RESPONSES_SUFFIX_SEGMENTS);
    }
    if let Some(suffix) = strip_path_suffix(path, &["/v1beta/models"]) {
        return validate_gemini_model_path(suffix);
    }
    if let Some(suffix) = strip_path_suffix(path, &["/v1/videos"]) {
        return validate_video_path(suffix);
    }
    Ok(())
}

/// 模型映射值在插入路径前单独校验，避免 `?` 被误解释为查询串边界。
pub(crate) fn validate_path_segment(segment: &str) -> Result<(), &'static str> {
    if is_safe_path_segment(segment) {
        Ok(())
    } else {
        Err(INVALID_UPSTREAM_PATH_MESSAGE)
    }
}

fn strip_path_suffix<'a>(path: &'a str, roots: &[&str]) -> Option<&'a str> {
    roots.iter().find_map(|root| {
        if path == *root {
            Some("")
        } else {
            path.strip_prefix(root)?.strip_prefix('/')
        }
    })
}

fn validate_suffix(suffix: &str, max_segments: usize) -> Result<(), &'static str> {
    if suffix.is_empty() {
        return Ok(());
    }
    let mut count = 0;
    for segment in suffix.split('/') {
        count += 1;
        if count > max_segments || !is_safe_path_segment(segment) {
            return Err(INVALID_UPSTREAM_PATH_MESSAGE);
        }
    }
    Ok(())
}

fn validate_gemini_model_path(suffix: &str) -> Result<(), &'static str> {
    if suffix.is_empty() {
        return Ok(());
    }
    let Some((model, action)) = suffix.split_once(':') else {
        return validate_path_segment(suffix);
    };
    if action.contains(':') {
        return Err(INVALID_UPSTREAM_PATH_MESSAGE);
    }
    validate_path_segment(model)?;
    validate_path_segment(action)
}

fn validate_video_path(suffix: &str) -> Result<(), &'static str> {
    if suffix.is_empty() {
        return Ok(());
    }
    let mut segments = suffix.split('/');
    let request_id = segments.next().unwrap_or_default();
    if !is_safe_path_segment(request_id) {
        return Err(INVALID_UPSTREAM_PATH_MESSAGE);
    }
    match (segments.next(), segments.next()) {
        (None, None) | (Some("content"), None) => Ok(()),
        _ => Err(INVALID_UPSTREAM_PATH_MESSAGE),
    }
}

fn is_safe_path_segment(segment: &str) -> bool {
    if segment.is_empty() || segment.len() > MAX_PATH_SEGMENT_BYTES {
        return false;
    }
    let mut dots_only = true;
    for byte in segment.bytes() {
        if !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')) {
            return false;
        }
        dots_only &= byte == b'.';
    }
    !dots_only
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nonconforming_sensitive_path_segments() {
        for path in [
            "/v1/responses/..",
            "/v1/responses/compact/../detail",
            "/v1/responses/compact%2f..",
            "/v1/responses/模型",
            "/v1beta/models/gemini-3.5-pro%2f..:generateContent",
            "/v1/videos/../content",
            "/v1/videos/video-1/delete",
        ] {
            assert!(validate_sensitive_path(path).is_err(), "path={path}");
        }
    }

    #[test]
    fn accepts_supported_sensitive_paths() {
        for path in [
            "/v1/responses",
            "/v1/responses/resp_123/cancel",
            "/responses/compact",
            "/v1beta/models",
            "/v1beta/models/gemini-3.5-pro:generateContent",
            "/v1beta/models/gemini-3.5-pro",
            "/v1/videos",
            "/v1/videos/video-123",
            "/v1/videos/video-123/content?download=1",
        ] {
            assert_eq!(validate_sensitive_path(path), Ok(()), "path={path}");
        }
    }

    #[test]
    fn enforces_segment_and_depth_bounds() {
        let long_segment = "a".repeat(MAX_PATH_SEGMENT_BYTES + 1);
        assert!(validate_sensitive_path(&format!("/v1/responses/{long_segment}")).is_err());
        assert!(validate_sensitive_path("/v1/responses/a/a/a/a/a/a/a/a/a").is_err());
        assert!(validate_path_segment("gemini-3.5-pro?alt=sse").is_err());
    }
}
