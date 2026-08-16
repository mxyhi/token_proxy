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
pub const CLI_CLIENT_VERSION: &str = "0.2.93";
pub const CLI_USER_AGENT: &str = "xai-grok-workspace/0.2.93";
pub(crate) const CLI_BILLING_USER_AGENT: &str =
    "grok-pager/0.2.93 grok-shell/0.2.93 (macos; aarch64)";

/// CLI OAuth provider 不调用 `/models`，使用与当前参考实现一致的内建目录。
/// `grok-4.6-high` / `grok-4.6-xhigh` 是同模型 effort 别名，不单独列入上游 ID。
pub const BUILTIN_MODELS: &[&str] = &[
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
