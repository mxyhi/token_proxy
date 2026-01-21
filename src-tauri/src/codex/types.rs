use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CodexTokenRecord {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) id_token: String,
    pub(crate) account_id: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) expires_at: String,
    pub(crate) last_refresh: Option<String>,
}

impl CodexTokenRecord {
    pub(crate) fn expires_at(&self) -> Option<OffsetDateTime> {
        let value = self.expires_at.trim();
        if value.is_empty() {
            return None;
        }
        OffsetDateTime::parse(value, &Rfc3339).ok()
    }

    pub(crate) fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at() else {
            return true;
        };
        OffsetDateTime::now_utc() >= expires_at
    }

    pub(crate) fn status(&self) -> CodexAccountStatus {
        if self.is_expired() {
            CodexAccountStatus::Expired
        } else {
            CodexAccountStatus::Active
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodexAccountStatus {
    Active,
    Expired,
}

#[derive(Clone, Serialize)]
pub(crate) struct CodexAccountSummary {
    pub(crate) account_id: String,
    pub(crate) email: Option<String>,
    pub(crate) expires_at: Option<String>,
    pub(crate) status: CodexAccountStatus,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodexLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub(crate) struct CodexLoginStartResponse {
    pub(crate) state: String,
    pub(crate) login_url: String,
    pub(crate) interval_seconds: u64,
    pub(crate) expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct CodexLoginPollResponse {
    pub(crate) state: String,
    pub(crate) status: CodexLoginStatus,
    pub(crate) error: Option<String>,
    pub(crate) account: Option<CodexAccountSummary>,
}
