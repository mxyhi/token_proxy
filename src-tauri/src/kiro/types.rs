use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct KiroTokenRecord {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) profile_arn: Option<String>,
    pub(crate) expires_at: String,
    pub(crate) auth_method: String,
    pub(crate) provider: String,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) last_refresh: Option<String>,
    pub(crate) start_url: Option<String>,
    pub(crate) region: Option<String>,
}

impl KiroTokenRecord {
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

    pub(crate) fn status(&self) -> KiroAccountStatus {
        if self.is_expired() {
            KiroAccountStatus::Expired
        } else {
            KiroAccountStatus::Active
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KiroAccountStatus {
    Active,
    Expired,
}

#[derive(Clone, Serialize)]
pub(crate) struct KiroAccountSummary {
    pub(crate) account_id: String,
    pub(crate) provider: String,
    pub(crate) auth_method: String,
    pub(crate) email: Option<String>,
    pub(crate) expires_at: Option<String>,
    pub(crate) status: KiroAccountStatus,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KiroLoginMethod {
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
pub(crate) enum KiroLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub(crate) struct KiroLoginStartResponse {
    pub(crate) state: String,
    pub(crate) method: KiroLoginMethod,
    pub(crate) login_url: Option<String>,
    pub(crate) verification_uri: Option<String>,
    pub(crate) verification_uri_complete: Option<String>,
    pub(crate) user_code: Option<String>,
    pub(crate) interval_seconds: Option<u64>,
    pub(crate) expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct KiroLoginPollResponse {
    pub(crate) state: String,
    pub(crate) status: KiroLoginStatus,
    pub(crate) error: Option<String>,
    pub(crate) account: Option<KiroAccountSummary>,
}
