//! Codex account OAuth, refresh, quota, state, and persistence behavior.

mod agent_identity;
mod error;
mod identity;
mod login;
mod oauth;
mod persistence;
mod quota;
mod store;
mod types;

pub use identity::{
    enforce_minimum_client_version, is_official_originator, official_originator_from_user_agent,
    supported_official_user_agent, DEFAULT_ORIGINATOR, USER_AGENT,
};
pub use login::{CodexLoginManager, CodexLoginPollClaim};
pub use oauth::CodexRefreshTokenClient;
pub use quota::fetch_quotas;
#[cfg(any(test, feature = "test-support"))]
pub use store::ProviderGateProbe;
pub use store::{CodexAccountStore, CodexProviderMutation};
pub use types::{
    CodexAccountStatus, CodexAccountSummary, CodexAgentIdentityRef, CodexAuthMethod,
    CodexCredential, CodexLoginPollResponse, CodexLoginStartResponse, CodexLoginStatus,
    CodexOAuthCredentialRef, CodexQuotaCache, CodexQuotaItem, CodexQuotaSummary, CodexTokenRecord,
};
