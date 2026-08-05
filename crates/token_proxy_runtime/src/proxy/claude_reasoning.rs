/// Marker used when Responses \`encrypted_content\` carries redacted Claude data,
/// not a replayable thinking signature.
pub(crate) const REDACTED_THINKING_PREFIX: &str = "claude-redacted-thinking:";

/// Wrap redacted Claude data in the Responses carrier without changing payload bytes.
pub(crate) fn redacted_thinking_carrier(data: &str) -> Option<String> {
    (!data.is_empty()).then(|| format!("{REDACTED_THINKING_PREFIX}{data}"))
}

/// Return redacted Claude data only when the carrier has the explicit marker.
pub(crate) fn redacted_thinking_data(encrypted_content: &str) -> Option<&str> {
    encrypted_content
        .strip_prefix(REDACTED_THINKING_PREFIX)
        .filter(|data| !data.is_empty())
}
