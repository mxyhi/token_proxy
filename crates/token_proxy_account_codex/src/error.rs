use serde_json::Value;

const CODEX_RELOGIN_REQUIRED_MESSAGE: &str = "Codex 登录已失效，请重新登录该账户。";
const CODEX_INVALID_GRANT_MESSAGE: &str = "Codex OAuth refresh rejected (invalid_grant).";

pub(super) fn format_oauth_status_error(status: u16, body: &str) -> String {
    if error_code(body).as_deref() == Some("invalid_grant") {
        return CODEX_INVALID_GRANT_MESSAGE.to_string();
    }
    relogin_required_message(body)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Codex OAuth request failed (status {status}): {body}"))
}

pub(super) fn format_usage_status_error(status: u16, body: &str) -> String {
    relogin_required_message(body)
        .map(str::to_string)
        .unwrap_or_else(|| format!("status {status}: {body}"))
}

pub(super) fn usage_status_requires_relogin(body: &str) -> bool {
    relogin_required_message(body).is_some()
}

pub(super) fn error_requires_relogin(message: &str) -> bool {
    message.trim() == CODEX_RELOGIN_REQUIRED_MESSAGE
}

pub(super) fn error_is_invalid_grant(message: &str) -> bool {
    message == CODEX_INVALID_GRANT_MESSAGE
}

pub(super) fn oauth_status_is_retryable(status: u16, body: &str) -> bool {
    relogin_required_message(body).is_none()
        && error_code(body).as_deref() != Some("invalid_grant")
        && (status == 408 || status == 429 || status >= 500)
}

fn relogin_required_message(body: &str) -> Option<&'static str> {
    if is_relogin_required_code(&error_code(body)?) {
        return Some(CODEX_RELOGIN_REQUIRED_MESSAGE);
    }
    None
}

fn error_code(body: &str) -> Option<String> {
    let error = serde_json::from_str::<Value>(body).ok()?;
    let code = [
        error.pointer("/error/code").and_then(Value::as_str),
        error.pointer("/code").and_then(Value::as_str),
        error.pointer("/detail/code").and_then(Value::as_str),
        error.get("error").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())?;

    Some(code.trim().to_ascii_lowercase())
}

fn is_relogin_required_code(code: &str) -> bool {
    matches!(
        code.trim().to_ascii_lowercase().as_str(),
        "refresh_token_reused"
            | "refresh_token_invalidated"
            | "refresh_token_expired"
            | "refresh_token_revoked"
            | "token_invalidated"
            | "token_revoked"
    )
}

#[cfg(test)]
mod tests {
    use super::{format_oauth_status_error, format_usage_status_error};

    #[test]
    fn oauth_refresh_token_reused_maps_to_relogin_message() {
        let message = format_oauth_status_error(
            401,
            r#"{"error":{"message":"Your refresh token has already been used to generate a new access token. Please try signing in again.","type":"invalid_request_error","param":null,"code":"refresh_token_reused"}}"#,
        );

        assert_eq!(message, "Codex 登录已失效，请重新登录该账户。");
    }

    #[test]
    fn oauth_token_invalidated_maps_to_relogin_message() {
        let message = format_oauth_status_error(
            401,
            r#"{"error":{"message":"Your authentication token has been invalidated. Please try signing in again.","type":"invalid_request_error","code":"token_invalidated","param":null}}"#,
        );

        assert_eq!(message, "Codex 登录已失效，请重新登录该账户。");
    }

    #[test]
    fn usage_token_revoked_maps_to_relogin_message() {
        let message = format_usage_status_error(
            401,
            r#"{"error":{"message":"Your authentication token has been revoked. Please try signing in again.","type":"invalid_request_error","code":"token_revoked","param":null}}"#,
        );

        assert_eq!(message, "Codex 登录已失效，请重新登录该账户。");
    }
}
