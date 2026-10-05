//! xAI OAuth 账户领域：设备登录、凭证刷新、账户存储、配额与代理身份合同。

mod error;
mod login;
mod oauth;
mod persistence;
mod quota;
mod store;
mod types;

pub use login::{XaiLoginManager, XaiLoginPollClaim};
pub use quota::fetch_quotas;
#[cfg(any(test, feature = "test-support"))]
pub use store::ProviderGateProbe;
pub use store::{XaiAccountStore, XaiProviderMutation};
pub use types::{
    XaiAccountStatus, XaiAccountSummary, XaiLoginPollResponse, XaiLoginStartResponse,
    XaiLoginStatus, XaiQuotaCache, XaiQuotaItem, XaiQuotaSummary, XaiTokenRecord,
};

pub const CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
pub const OFFICIAL_API_BASE_URL: &str = "https://api.x.ai/v1";
pub const CLI_TOKEN_AUTH_HEADER: &str = "x-xai-token-auth";
pub const CLI_TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
pub const CLI_CLIENT_VERSION_HEADER: &str = "x-grok-client-version";
/// cli-chat-proxy 会以 426 拒绝低于 1.0.13 的客户端；固定为已抓包核对的官方 CLI 版本。
pub const CLI_CLIENT_VERSION: &str = "1.0.46";

/// 官方交互式 Grok CLI 主请求的身份头（1.0.46 抓包，对齐 Sub2API v0.2.13）。
/// 推理、额度探测与模型目录共用同一身份；User-Agent 由 [`cli_user_agent`] 按平台生成。
pub const CLI_IDENTITY_HEADERS: [(&str, &str); 5] = [
    (CLI_TOKEN_AUTH_HEADER, CLI_TOKEN_AUTH_VALUE),
    (CLI_CLIENT_VERSION_HEADER, CLI_CLIENT_VERSION),
    ("x-grok-client-identifier", "grok-pager"),
    ("x-grok-client-mode", "interactive"),
    ("x-authenticateresponse", "authenticate-response"),
];

/// 官方 CLI UA，平台与架构使用 Rust 的命名（macos/linux/windows、aarch64/x86_64）。
pub fn cli_user_agent() -> &'static str {
    static USER_AGENT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        format!(
            "grok-pager/{CLI_CLIENT_VERSION} grok-shell/{CLI_CLIENT_VERSION} ({}; {})",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    });
    USER_AGENT.as_str()
}

/// CLI OAuth provider 不调用 `/models`，使用与当前参考实现一致的内建目录。
/// `grok-4.7-high` / `grok-4.7-xhigh` 与 `grok-4.6` 的 effort 别名同价，不单独列入上游 ID。
pub const BUILTIN_MODELS: &[&str] = &[
    "grok-4.7",
    "grok-4.6",
    "grok-4.5",
    "grok-4.3",
    "grok-build-0.1",
    "grok-composer-2.5-fast",
    "grok-4.20-0309-reasoning",
    "grok-4.20-0309-non-reasoning",
    "grok-4.20-multi-agent-0309",
    "grok-imagine",
    "grok-imagine-image",
    "grok-imagine-image-2.0",
    "grok-imagine-image-quality",
    "grok-imagine-edit",
    "grok-imagine-video",
    "grok-imagine-video-1.5",
];

#[cfg(test)]
mod tests {
    use super::BUILTIN_MODELS;

    #[test]
    fn builtin_models_include_grok_imagine_image_2() {
        assert!(BUILTIN_MODELS.contains(&"grok-imagine-image-2.0"));
    }
}
