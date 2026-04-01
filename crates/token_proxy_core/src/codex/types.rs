use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CodexQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CodexQuotaCache {
    pub plan_type: Option<String>,
    #[serde(default)]
    pub quotas: Vec<CodexQuotaItem>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexAccountStatus {
    Active,
    Disabled,
    Expired,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CodexTokenRecord {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    #[serde(default = "default_auto_refresh_enabled")]
    pub auto_refresh_enabled: bool,
    #[serde(default = "default_account_status")]
    pub status: CodexAccountStatus,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub expires_at: String,
    pub last_refresh: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub quota: CodexQuotaCache,
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

    pub fn effective_status(&self) -> CodexAccountStatus {
        if self.status == CodexAccountStatus::Disabled {
            return CodexAccountStatus::Disabled;
        }
        if self.is_expired() {
            CodexAccountStatus::Expired
        } else {
            CodexAccountStatus::Active
        }
    }

    pub fn is_schedulable(&self) -> bool {
        matches!(self.effective_status(), CodexAccountStatus::Active)
    }
}

#[derive(Clone, Serialize)]
pub struct CodexAccountSummary {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: CodexAccountStatus,
    pub auto_refresh_enabled: bool,
    pub proxy_url: Option<String>,
}

fn default_auto_refresh_enabled() -> bool {
    true
}

fn default_account_status() -> CodexAccountStatus {
    CodexAccountStatus::Active
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

#[derive(Clone, Serialize)]
pub struct CodexQuotaSummary {
    pub account_id: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<CodexQuotaItem>,
    pub error: Option<String>,
}
