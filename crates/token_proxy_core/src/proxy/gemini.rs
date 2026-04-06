use url::form_urlencoded;

pub(crate) const GEMINI_MODELS_ROOT: &str = "/v1beta/models";
pub(crate) const GEMINI_MODELS_PREFIX: &str = "/v1beta/models/";
const GEMINI_OPENAI_COMPAT_MODELS_ROOT: &str = "/v1beta/openai/models";
const GEMINI_CACHED_CONTENTS_ROOT: &str = "/v1beta/cachedContents";
const GEMINI_FILES_ROOT: &str = "/v1beta/files";
const GEMINI_TUNED_MODELS_ROOT: &str = "/v1beta/tunedModels";
const GEMINI_UPLOAD_FILES_ROOT: &str = "/upload/v1beta/files";
const GEMINI_GENERATE_SUFFIX: &str = ":generateContent";
const GEMINI_STREAM_SUFFIX: &str = ":streamGenerateContent";

pub(crate) fn is_gemini_path(path: &str) -> bool {
    is_gemini_generate_path(path) || is_gemini_stream_path(path)
}

pub(crate) fn is_gemini_generate_path(path: &str) -> bool {
    let path = strip_query(path);
    path.starts_with(GEMINI_MODELS_PREFIX) && path.ends_with(GEMINI_GENERATE_SUFFIX)
}

pub(crate) fn is_gemini_stream_path(path: &str) -> bool {
    let path = strip_query(path);
    path.starts_with(GEMINI_MODELS_PREFIX) && path.ends_with(GEMINI_STREAM_SUFFIX)
}

pub(crate) fn is_gemini_stream_request(path: &str) -> bool {
    is_gemini_stream_path(path) || (is_gemini_generate_path(path) && has_alt_sse_query(path))
}

pub(crate) fn is_gemini_models_action_path(path: &str) -> bool {
    let path = strip_query(path);
    let Some(rest) = path.strip_prefix(GEMINI_MODELS_PREFIX) else {
        return false;
    };
    let Some((model, action)) = rest.split_once(':') else {
        return false;
    };
    !model.trim().is_empty() && !action.trim().is_empty()
}

pub(crate) fn is_gemini_model_catalog_path(path: &str) -> bool {
    let path = strip_query(path);
    if path == GEMINI_MODELS_ROOT {
        return true;
    }
    let Some(rest) = path.strip_prefix(GEMINI_MODELS_PREFIX) else {
        return false;
    };
    !rest.is_empty() && !rest.contains(':')
}

pub(crate) fn is_gemini_native_path(path: &str) -> bool {
    let path = strip_query(path);
    if path.starts_with(GEMINI_OPENAI_COMPAT_MODELS_ROOT) {
        return false;
    }
    is_gemini_models_action_path(path)
        || is_gemini_model_catalog_path(path)
        || path.starts_with(GEMINI_CACHED_CONTENTS_ROOT)
        || path.starts_with(GEMINI_FILES_ROOT)
        || path.starts_with(GEMINI_TUNED_MODELS_ROOT)
        || path.starts_with(GEMINI_UPLOAD_FILES_ROOT)
}

pub(crate) fn has_alt_sse_query(path: &str) -> bool {
    let Some((_, query)) = path.split_once('?') else {
        return false;
    };
    form_urlencoded::parse(query.as_bytes())
        .any(|(key, value)| key == "alt" && value.eq_ignore_ascii_case("sse"))
}

pub(crate) fn parse_gemini_model_from_path(path: &str) -> Option<String> {
    let path = strip_query(path);
    let rest = path.strip_prefix(GEMINI_MODELS_PREFIX)?;
    let (model, _) = rest.split_once(':')?;
    let model = model.trim();
    if model.is_empty() {
        None
    } else {
        Some(model.to_string())
    }
}

pub(crate) fn replace_gemini_model_in_path(path: &str, model: &str) -> Option<String> {
    let path = strip_query(path);
    let rest = path.strip_prefix(GEMINI_MODELS_PREFIX)?;
    let (_, suffix) = rest.split_once(':')?;
    Some(format!("{GEMINI_MODELS_PREFIX}{model}:{suffix}"))
}

fn strip_query(path: &str) -> &str {
    path.split_once('?').map(|(path, _)| path).unwrap_or(path)
}
