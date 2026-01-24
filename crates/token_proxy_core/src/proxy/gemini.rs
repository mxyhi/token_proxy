pub(crate) const GEMINI_MODELS_PREFIX: &str = "/v1beta/models/";
const GEMINI_GENERATE_SUFFIX: &str = ":generateContent";
const GEMINI_STREAM_SUFFIX: &str = ":streamGenerateContent";

pub(crate) fn is_gemini_path(path: &str) -> bool {
    if !path.starts_with(GEMINI_MODELS_PREFIX) {
        return false;
    }
    path.ends_with(GEMINI_GENERATE_SUFFIX) || path.ends_with(GEMINI_STREAM_SUFFIX)
}

pub(crate) fn is_gemini_stream_path(path: &str) -> bool {
    path.starts_with(GEMINI_MODELS_PREFIX) && path.ends_with(GEMINI_STREAM_SUFFIX)
}

pub(crate) fn parse_gemini_model_from_path(path: &str) -> Option<String> {
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
    let rest = path.strip_prefix(GEMINI_MODELS_PREFIX)?;
    let (_, suffix) = rest.split_once(':')?;
    Some(format!("{GEMINI_MODELS_PREFIX}{model}:{suffix}"))
}
