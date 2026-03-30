use serde_json::Value;

const CODEX_RELOGIN_REQUIRED_MESSAGE: &str = "Codex 登录已失效，请重新登录该账户。";

pub(super) fn format_oauth_status_error(status: u16, body: &str) -> String {
    relogin_required_message(body)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Codex OAuth request failed (status {status}): {body}"))
}

pub(super) fn format_usage_status_error(status: u16, body: &str) -> String {
    relogin_required_message(body)
        .map(str::to_string)
        .unwrap_or_else(|| format!("status {status}: {body}"))
}

fn relogin_required_message(body: &str) -> Option<&'static str> {
    let error = serde_json::from_str::<Value>(body).ok()?;
    let code = [
        error.pointer("/error/code").and_then(Value::as_str),
        error.pointer("/code").and_then(Value::as_str),
        error.pointer("/detail/code").and_then(Value::as_str),
    ]
    .into_iter()
    .flatten()
    .find(|value| !value.trim().is_empty())?;

    if is_relogin_required_code(code) {
        return Some(CODEX_RELOGIN_REQUIRED_MESSAGE);
    }

    None
}

fn is_relogin_required_code(code: &str) -> bool {
    matches!(
        code.trim().to_ascii_lowercase().as_str(),
        "refresh_token_reused" | "token_invalidated" | "token_revoked" | "invalid_grant"
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
