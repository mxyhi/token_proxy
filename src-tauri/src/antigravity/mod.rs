mod ide;
mod ide_db;
mod login;
mod oauth;
pub(crate) mod endpoints;
pub(crate) mod project;
mod protobuf;
mod quota;
mod store;
mod types;
mod warmup;

pub(crate) use ide::{ide_status, import_from_ide, switch_ide_account};
pub(crate) use login::AntigravityLoginManager;
pub(crate) use quota::fetch_quotas;
pub(crate) use store::AntigravityAccountStore;
pub(crate) use types::{
    AntigravityAccountSummary,
    AntigravityLoginPollResponse,
    AntigravityLoginStartResponse,
    AntigravityIdeStatus,
    AntigravityQuotaSummary,
    AntigravityWarmupScheduleSummary,
};
pub(crate) use warmup::AntigravityWarmupScheduler;
