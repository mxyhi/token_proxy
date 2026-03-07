use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CALLBACK_FILE_PREFIX: &str = ".oauth-kiro-";
const CALLBACK_FILE_SUFFIX: &str = ".oauth";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OAuthCallbackPayload {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
}

pub(crate) fn parse_callback_url(url: &str) -> Result<OAuthCallbackPayload, String> {
    let parsed = url::Url::parse(url).map_err(|err| format!("Invalid callback URL: {err}"))?;
    let mut payload = OAuthCallbackPayload {
        code: None,
        state: None,
        error: None,
    };
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => payload.code = Some(value.to_string()),
            "state" => payload.state = Some(value.to_string()),
            "error" => payload.error = Some(value.to_string()),
            _ => {}
        }
    }
    Ok(payload)
}

pub(crate) fn callback_file_path(dir: &Path, state: &str) -> PathBuf {
    dir.join(format!(
        "{CALLBACK_FILE_PREFIX}{state}{CALLBACK_FILE_SUFFIX}"
    ))
}

pub(crate) async fn write_callback_file(
    dir: &Path,
    payload: &OAuthCallbackPayload,
) -> Result<PathBuf, String> {
    let state = payload
        .state
        .as_deref()
        .ok_or_else(|| "Missing state in callback payload.".to_string())?;
    let path = callback_file_path(dir, state);
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|err| format!("Failed to create callback dir: {err}"))?;
    let content = serde_json::to_string(payload)
        .map_err(|err| format!("Failed to serialize callback payload: {err}"))?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|err| format!("Failed to write callback file: {err}"))?;
    Ok(path)
}

pub(crate) async fn read_callback_file(path: &Path) -> Result<OAuthCallbackPayload, String> {
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|err| format!("Failed to read callback file: {err}"))?;
    serde_json::from_str(&content).map_err(|err| format!("Failed to parse callback file: {err}"))
}
