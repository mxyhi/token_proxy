use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub struct AntigravityTokenRecord {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl AntigravityTokenRecord {
    pub fn expires_at(&self) -> Option<OffsetDateTime> {
        if let Some(expired) = self.expired.as_deref() {
            if let Ok(value) = OffsetDateTime::parse(expired.trim(), &Rfc3339) {
                return Some(value);
            }
        }
        let expires_in = self.expires_in.unwrap_or_default();
        let timestamp = self.timestamp.unwrap_or_default();
        if expires_in <= 0 || timestamp <= 0 {
            return None;
        }
        let expires_at = (timestamp / 1000) + expires_in;
        OffsetDateTime::from_unix_timestamp(expires_at).ok()
    }

    pub fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at() else {
            return true;
        };
        OffsetDateTime::now_utc() >= expires_at
    }

    pub fn status(&self) -> AntigravityAccountStatus {
        if self.is_expired() {
            AntigravityAccountStatus::Expired
        } else {
            AntigravityAccountStatus::Active
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AntigravityAccountStatus {
    Active,
    Expired,
}

#[derive(Clone, Serialize)]
pub struct AntigravityAccountSummary {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: AntigravityAccountStatus,
    pub source: Option<String>,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AntigravityLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub struct AntigravityLoginStartResponse {
    pub state: String,
    pub login_url: String,
    pub interval_seconds: u64,
    pub expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct AntigravityLoginPollResponse {
    pub state: String,
    pub status: AntigravityLoginStatus,
    pub error: Option<String>,
    pub account: Option<AntigravityAccountSummary>,
}

#[derive(Clone, Serialize)]
pub struct AntigravityQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub reset_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct AntigravityQuotaSummary {
    pub account_id: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<AntigravityQuotaItem>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct AntigravityIdeStatus {
    pub database_available: bool,
    pub ide_running: bool,
    pub active_email: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AntigravityWarmupSchedule {
    pub account_id: String,
    pub model: String,
    pub interval_minutes: u64,
    pub next_run_at: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct AntigravityWarmupScheduleSummary {
    pub account_id: String,
    pub model: String,
    pub interval_minutes: u64,
    pub next_run_at: Option<String>,
    pub enabled: bool,
}
