//! Kiro account login, refresh, quota, state, and persistence behavior.

mod callback;
mod login;
mod oauth;
mod persistence;
mod quota;
mod sso_oidc;
mod store;
mod types;
mod util;

pub use login::{KiroLoginManager, KiroLoginPollClaim};
pub use quota::fetch_quotas;
#[cfg(any(test, feature = "test-support"))]
pub use store::ProviderGateProbe;
pub use store::{KiroAccountStore, KiroProviderMutation};
pub use types::{
    KiroAccountStatus, KiroAccountSummary, KiroLoginMethod, KiroLoginPollResponse,
    KiroLoginStartResponse, KiroLoginStatus, KiroQuotaCache, KiroQuotaItem, KiroQuotaSummary,
    KiroTokenRecord,
};
