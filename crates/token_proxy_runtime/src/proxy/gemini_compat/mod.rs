mod request;
mod response;
mod stream;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};

const RESPONSES_CARRIER_PREFIX: &str = "cpa-gemini-responses-carrier-v1:next:function:";
const GEMINI_SIGNATURE_BYPASS: &str = "skip_thought_signature_validator";
const MAX_GEMINI_SIGNATURE_BYTES: usize = 64 * 1024;
mod tools {
    pub(super) use token_proxy_protocol::gemini_tools::*;
}

pub(crate) use request::chat_request_to_gemini;
pub(crate) use request::chat_request_to_gemini_with_summary_visibility;
pub(crate) use request::gemini_request_to_chat;
pub(crate) use request::gemini_request_to_chat_with_summary_visibility;
pub(crate) use response::chat_response_to_gemini;
pub(crate) use response::gemini_response_to_chat;
pub(crate) use stream::stream_gemini_to_chat;
pub(crate) use stream::{gemini_error_sse, stream_chat_to_gemini};

pub(crate) fn encode_function_signature_carrier(signature: &str) -> Option<String> {
    let signature = signature.trim();
    if signature.is_empty()
        || signature == GEMINI_SIGNATURE_BYPASS
        || signature.len() > MAX_GEMINI_SIGNATURE_BYTES
    {
        return None;
    }
    Some(format!(
        "{RESPONSES_CARRIER_PREFIX}{}",
        STANDARD_NO_PAD.encode(signature)
    ))
}

pub(crate) fn decode_function_signature_carrier(carrier: &str) -> Option<String> {
    let encoded = carrier.trim().strip_prefix(RESPONSES_CARRIER_PREFIX)?;
    if encoded.is_empty() || encoded.len() > (MAX_GEMINI_SIGNATURE_BYTES * 4 / 3) + 4 {
        return None;
    }
    let decoded = STANDARD_NO_PAD.decode(encoded).ok()?;
    if decoded.is_empty() || decoded.len() > MAX_GEMINI_SIGNATURE_BYTES {
        return None;
    }
    let signature = String::from_utf8(decoded).ok()?;
    let signature = signature.trim();
    if signature.is_empty()
        || signature == GEMINI_SIGNATURE_BYPASS
        || signature.starts_with("cpa-gemini-responses-carrier-v1:")
    {
        return None;
    }
    Some(signature.to_string())
}

#[cfg(test)]
mod signature_tests {
    use super::*;

    #[test]
    fn function_signature_carrier_is_provider_scoped() {
        let carrier = encode_function_signature_carrier("gemini-signature").expect("carrier");
        assert_eq!(
            decode_function_signature_carrier(&carrier).as_deref(),
            Some("gemini-signature")
        );
        assert!(decode_function_signature_carrier("claude-signature").is_none());
        assert!(encode_function_signature_carrier(GEMINI_SIGNATURE_BYPASS).is_none());
    }
}
