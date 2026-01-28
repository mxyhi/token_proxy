use super::{
    config::InboundApiFormat,
    gemini,
    openai_compat::{self, ApiFormat},
    server_helpers::is_anthropic_path,
};

/// 从请求 path 推断入站 API 格式。
///
/// 说明：
/// - `None` 表示“非格式化路由”（例如 /v1/models、健康检查等），不参与格式转换/过滤。
/// - `Some(..)` 表示需要按格式做 provider/upstream 的 eligibility 过滤。
pub(crate) fn detect_inbound_api_format(path: &str) -> Option<InboundApiFormat> {
    if gemini::is_gemini_path(path) {
        return Some(InboundApiFormat::Gemini);
    }
    if is_anthropic_path(path) {
        return Some(InboundApiFormat::AnthropicMessages);
    }
    match openai_compat::inbound_format(path)? {
        ApiFormat::ChatCompletions => Some(InboundApiFormat::OpenaiChat),
        ApiFormat::Responses => Some(InboundApiFormat::OpenaiResponses),
    }
}

