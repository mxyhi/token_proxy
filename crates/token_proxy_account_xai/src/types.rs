use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct XaiQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct XaiQuotaCache {
    pub plan_type: Option<String>,
    #[serde(default)]
    pub quotas: Vec<XaiQuotaItem>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
}

/// 账户认证健康状态（只读投影）。
/// 调度 enabled/priority/proxy 只属于 Upstream；此处不承载人工 disabled。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum XaiAccountStatus {
    Active,
    Expired,
    Invalid,
}

fn default_auto_refresh_enabled() -> bool {
    true
}

fn default_account_status() -> XaiAccountStatus {
    XaiAccountStatus::Active
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XaiTokenRecord {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub token_type: String,
    pub expires_at: String,
    pub last_refresh: Option<String>,
    pub email: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub token_endpoint: Option<String>,
    #[serde(default = "default_auto_refresh_enabled")]
    pub auto_refresh_enabled: bool,
    #[serde(default = "default_account_status")]
    pub status: XaiAccountStatus,
    #[serde(default)]
    pub quota: XaiQuotaCache,
}

impl XaiTokenRecord {
    pub fn expires_at(&self) -> Option<OffsetDateTime> {
        let value = self.expires_at.trim();
        if value.is_empty() {
            return None;
        }
        OffsetDateTime::parse(value, &Rfc3339).ok()
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at()
            .is_none_or(|expires_at| OffsetDateTime::now_utc() >= expires_at)
    }

    /// 健康状态：Invalid 保留鉴权失败；否则由 token 到期推导。
    pub fn effective_status(&self) -> XaiAccountStatus {
        match self.status {
            XaiAccountStatus::Invalid => XaiAccountStatus::Invalid,
            XaiAccountStatus::Active | XaiAccountStatus::Expired if self.is_expired() => {
                XaiAccountStatus::Expired
            }
            XaiAccountStatus::Active | XaiAccountStatus::Expired => XaiAccountStatus::Active,
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self.effective_status(), XaiAccountStatus::Active)
            || (matches!(self.effective_status(), XaiAccountStatus::Expired)
                && self.auto_refresh_enabled)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct XaiAccountSummary {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: XaiAccountStatus,
    pub auto_refresh_enabled: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum XaiLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct XaiLoginStartResponse {
    pub state: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval_seconds: u64,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct XaiLoginPollResponse {
    pub state: String,
    pub status: XaiLoginStatus,
    pub error: Option<String>,
    pub account: Option<XaiAccountSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct XaiQuotaSummary {
    pub account_id: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<XaiQuotaItem>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_status_preserves_invalid_auth_state() {
        let mut record = test_record("2999-01-01T00:00:00Z");
        record.status = XaiAccountStatus::Invalid;
        assert_eq!(record.effective_status(), XaiAccountStatus::Invalid);
    }

    #[test]
    fn effective_status_tracks_token_expiry() {
        assert_eq!(
            test_record("2000-01-01T00:00:00Z").effective_status(),
            XaiAccountStatus::Expired
        );
        assert_eq!(
            test_record("2999-01-01T00:00:00Z").effective_status(),
            XaiAccountStatus::Active
        );
    }

    fn test_record(expires_at: &str) -> XaiTokenRecord {
        XaiTokenRecord {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            id_token: String::new(),
            token_type: "Bearer".to_string(),
            expires_at: expires_at.to_string(),
            last_refresh: None,
            email: None,
            subject: None,
            token_endpoint: None,
            auto_refresh_enabled: true,
            status: XaiAccountStatus::Active,
            quota: XaiQuotaCache::default(),
        }
    }
}
