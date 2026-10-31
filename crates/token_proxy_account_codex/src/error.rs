use serde_json::Value;

const CODEX_RELOGIN_REQUIRED_MESSAGE: &str = "Codex 登录已失效，请重新登录该账户。";

pub(super) fn format_oauth_status_error(status: u16, body: &str) -> String {
    if status == 401 || relogin_required_message(body).is_some() {
        return CODEX_RELOGIN_REQUIRED_MESSAGE.to_string();
    }
    // OAuth 错误正文可能回显 access/refresh token；只展示受限诊断字段。
    let code = error_code(body).filter(|code| {
        matches!(
            code.as_str(),
            "temporarily_unavailable"
                | "server_error"
                | "invalid_request"
                | "unsupported_grant_type"
                | "invalid_scope"
                | "access_denied"
        )
    });
    match code {
        Some(code) => format!("Codex OAuth request failed (status {status}, code {code})."),
        None => format!("Codex OAuth request failed (status {status})."),
    }
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

pub(super) fn oauth_status_is_retryable(status: u16, body: &str) -> bool {
    status != 401
        && relogin_required_message(body).is_none()
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
            | "invalid_client"
            | "invalid_grant"
    )
}

#[cfg(test)]
mod tests {
    use super::{format_oauth_status_error, format_usage_status_error};

    #[test]
    fn oauth_terminal_grants_and_401_request_relogin_without_echoing_secrets() {
        for code in ["invalid_grant", "invalid_client", "refresh_token_reused"] {
            let body = serde_json::json!({"error":{"code":code,"message":"refresh_token=secret"}})
                .to_string();
            assert_eq!(
                format_oauth_status_error(400, &body),
                "Codex 登录已失效，请重新登录该账户。"
            );
        }
        assert_eq!(
            format_oauth_status_error(401, "secret"),
            "Codex 登录已失效，请重新登录该账户。"
        );
        let diagnostic = format_oauth_status_error(
            503,
            r#"{"error":{"code":"secret_refresh_token","message":"secret_access_token"}}"#,
        );
        assert_eq!(diagnostic, "Codex OAuth request failed (status 503).");
    }

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
