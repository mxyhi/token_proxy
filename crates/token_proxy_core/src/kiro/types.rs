use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct KiroQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
    pub is_trial: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct KiroQuotaCache {
    pub plan_type: Option<String>,
    #[serde(default)]
    pub quotas: Vec<KiroQuotaItem>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
}

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KiroAccountStatus {
    Active,
    Disabled,
    Expired,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KiroTokenRecord {
    pub access_token: String,
    pub refresh_token: String,
    pub profile_arn: Option<String>,
    pub expires_at: String,
    pub auth_method: String,
    pub provider: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub email: Option<String>,
    pub last_refresh: Option<String>,
    pub start_url: Option<String>,
    pub region: Option<String>,
    #[serde(default = "default_account_status")]
    pub status: KiroAccountStatus,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub quota: KiroQuotaCache,
}

impl KiroTokenRecord {
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

    pub fn effective_status(&self) -> KiroAccountStatus {
        if self.status == KiroAccountStatus::Disabled {
            return KiroAccountStatus::Disabled;
        }
        if self.is_expired() {
            KiroAccountStatus::Expired
        } else {
            KiroAccountStatus::Active
        }
    }

    pub fn is_schedulable(&self) -> bool {
        matches!(self.effective_status(), KiroAccountStatus::Active)
    }
}

#[derive(Clone, Serialize)]
pub struct KiroAccountSummary {
    pub account_id: String,
    pub provider: String,
    pub auth_method: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: KiroAccountStatus,
    pub proxy_url: Option<String>,
}

fn default_account_status() -> KiroAccountStatus {
    KiroAccountStatus::Active
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KiroLoginMethod {
    Aws,
    AwsAuthcode,
    Google,
}

impl std::str::FromStr for KiroLoginMethod {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "aws" | "builder-id" | "builder_id" => Ok(Self::Aws),
            "aws_authcode" | "aws-authcode" | "builder-authcode" | "builder_authcode" => {
                Ok(Self::AwsAuthcode)
            }
            "google" => Ok(Self::Google),
            other => Err(format!("Unsupported login method: {other}")),
        }
    }
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KiroLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub struct KiroLoginStartResponse {
    pub state: String,
    pub method: KiroLoginMethod,
    pub login_url: Option<String>,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub user_code: Option<String>,
    pub interval_seconds: Option<u64>,
    pub expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct KiroLoginPollResponse {
    pub state: String,
    pub status: KiroLoginStatus,
    pub error: Option<String>,
    pub account: Option<KiroAccountSummary>,
}

#[derive(Clone, Serialize)]
pub struct KiroQuotaSummary {
    pub account_id: String,
    pub provider: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<KiroQuotaItem>,
    pub error: Option<String>,
}
