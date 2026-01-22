use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AntigravityTokenRecord {
    pub(crate) access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expired: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) timestamp: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
}

impl AntigravityTokenRecord {
    pub(crate) fn expires_at(&self) -> Option<OffsetDateTime> {
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

    pub(crate) fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at() else {
            return true;
        };
        OffsetDateTime::now_utc() >= expires_at
    }

    pub(crate) fn status(&self) -> AntigravityAccountStatus {
        if self.is_expired() {
            AntigravityAccountStatus::Expired
        } else {
            AntigravityAccountStatus::Active
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AntigravityAccountStatus {
    Active,
    Expired,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityAccountSummary {
    pub(crate) account_id: String,
    pub(crate) email: Option<String>,
    pub(crate) expires_at: Option<String>,
    pub(crate) status: AntigravityAccountStatus,
    pub(crate) source: Option<String>,
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AntigravityLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityLoginStartResponse {
    pub(crate) state: String,
    pub(crate) login_url: String,
    pub(crate) interval_seconds: u64,
    pub(crate) expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityLoginPollResponse {
    pub(crate) state: String,
    pub(crate) status: AntigravityLoginStatus,
    pub(crate) error: Option<String>,
    pub(crate) account: Option<AntigravityAccountSummary>,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityQuotaItem {
    pub(crate) name: String,
    pub(crate) percentage: f64,
    pub(crate) reset_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityQuotaSummary {
    pub(crate) account_id: String,
    pub(crate) plan_type: Option<String>,
    pub(crate) quotas: Vec<AntigravityQuotaItem>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityIdeStatus {
    pub(crate) database_available: bool,
    pub(crate) ide_running: bool,
    pub(crate) active_email: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AntigravityWarmupSchedule {
    pub(crate) account_id: String,
    pub(crate) model: String,
    pub(crate) interval_minutes: u64,
    pub(crate) next_run_at: Option<String>,
    #[serde(default)]
    pub(crate) enabled: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct AntigravityWarmupScheduleSummary {
    pub(crate) account_id: String,
    pub(crate) model: String,
    pub(crate) interval_minutes: u64,
    pub(crate) next_run_at: Option<String>,
    pub(crate) enabled: bool,
}
