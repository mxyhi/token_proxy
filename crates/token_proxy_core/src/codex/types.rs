use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub struct CodexTokenRecord {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub expires_at: String,
    pub last_refresh: Option<String>,
}

impl CodexTokenRecord {
    pub fn expires_at(&self) -> Option<OffsetDateTime> {
        let value = self.expires_at.trim();
        if value.is_empty() {
            return None;
        }
        OffsetDateTime::parse(value, &Rfc3339).ok()
    }

    pub fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at() else {
            return true;
        };
        OffsetDateTime::now_utc() >= expires_at
    }

    pub fn status(&self) -> CodexAccountStatus {
        if self.is_expired() {
            CodexAccountStatus::Expired
        } else {
            CodexAccountStatus::Active
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexAccountStatus {
    Active,
    Expired,
}

#[derive(Clone, Serialize)]
pub struct CodexAccountSummary {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: CodexAccountStatus,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub struct CodexLoginStartResponse {
    pub state: String,
    pub login_url: String,
    pub interval_seconds: u64,
    pub expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct CodexLoginPollResponse {
    pub state: String,
    pub status: CodexLoginStatus,
    pub error: Option<String>,
    pub account: Option<CodexAccountSummary>,
}
