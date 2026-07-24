use serde::{Deserialize, Serialize};
use token_proxy_account_codex::{
    CodexAccountStatus, CodexQuotaCache, CodexQuotaItem, CodexTokenRecord,
};
use token_proxy_account_kiro::{KiroAccountStatus, KiroQuotaCache, KiroQuotaItem, KiroTokenRecord};
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_store::records::{self, StoredAccountRow};
use token_proxy_account_xai::{XaiAccountStatus, XaiQuotaCache, XaiQuotaItem, XaiTokenRecord};

pub use token_proxy_account_store::records::ProviderKind as ProviderAccountKind;

const STATUS_ACTIVE: &str = "active";
const STATUS_EXPIRED: &str = "expired";
const STATUS_INVALID: &str = "invalid";
const STATUS_COOLING_DOWN: &str = "cooling_down";

pub const MAX_PAGE_SIZE: u32 = 100;

/// 跨 provider 账户列表健康状态（只读）。不含人工 disabled 调度字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAccountStatus {
    Active,
    Expired,
    Invalid,
    CoolingDown,
}

impl ProviderAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => STATUS_ACTIVE,
            Self::Expired => STATUS_EXPIRED,
            Self::Invalid => STATUS_INVALID,
            Self::CoolingDown => STATUS_COOLING_DOWN,
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            STATUS_ACTIVE => Ok(Self::Active),
            STATUS_EXPIRED => Ok(Self::Expired),
            STATUS_INVALID => Ok(Self::Invalid),
            STATUS_COOLING_DOWN => Ok(Self::CoolingDown),
            other => Err(format!("Unsupported status filter: {other}")),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountListItem {
    pub provider_kind: ProviderAccountKind,
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: ProviderAccountStatus,
    pub auth_method: Option<String>,
    pub provider_name: Option<String>,
    pub auto_refresh_enabled: Option<bool>,
    pub quota: ProviderAccountQuotaSnapshot,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProviderAccountQuotaSnapshot {
    pub plan_type: Option<String>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
    pub items: Vec<ProviderAccountQuotaItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
    pub is_trial: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderAccountsPage {
    pub items: Vec<ProviderAccountListItem>,
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
    pub status_counts: ProviderAccountStatusCounts,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProviderAccountStatusCounts {
    pub all: u32,
    pub active: u32,
    pub expired: u32,
    pub invalid: u32,
    pub cooling_down: u32,
}

impl ProviderAccountStatusCounts {
    pub fn from_items(items: &[ProviderAccountListItem]) -> Self {
        let mut counts = Self::default();
        for item in items {
            counts.all = counts.all.saturating_add(1);
            match item.status {
                ProviderAccountStatus::Active => {
                    counts.active = counts.active.saturating_add(1);
                }
                ProviderAccountStatus::Expired => {
                    counts.expired = counts.expired.saturating_add(1);
                }
                ProviderAccountStatus::Invalid => {
                    counts.invalid = counts.invalid.saturating_add(1);
                }
                ProviderAccountStatus::CoolingDown => {
                    counts.cooling_down = counts.cooling_down.saturating_add(1);
                }
            }
        }
        counts
    }
}

#[derive(Clone, Debug)]
pub struct ProviderAccountsQueryParams {
    pub provider_kind: Option<ProviderAccountKind>,
    pub search: String,
}

pub async fn delete_accounts(
    paths: &TokenProxyPaths,
    account_ids: &[String],
) -> Result<(), String> {
    records::delete_rows(paths, account_ids).await
}

pub async fn list_accounts_snapshot(
    paths: &TokenProxyPaths,
    params: ProviderAccountsQueryParams,
) -> Result<Vec<ProviderAccountListItem>, String> {
    let rows = records::list_rows(paths, params.provider_kind, &params.search).await?;
    rows.into_iter()
        .map(build_list_item)
        .collect::<Result<Vec<_>, String>>()
}

fn build_list_item(row: StoredAccountRow) -> Result<ProviderAccountListItem, String> {
    match row.provider_kind {
        ProviderAccountKind::Kiro => build_kiro_list_item(
            row.account_id,
            row.email,
            row.expires_at,
            row.auth_method,
            row.provider_name,
            &row.record_json,
        ),
        ProviderAccountKind::Codex => build_codex_list_item(
            row.account_id,
            row.email,
            row.expires_at,
            row.provider_name,
            &row.record_json,
        ),
        ProviderAccountKind::Xai => build_xai_list_item(
            row.account_id,
            row.email,
            row.expires_at,
            row.provider_name,
            &row.record_json,
        ),
    }
}

fn build_kiro_list_item(
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    auth_method: Option<String>,
    provider_name: Option<String>,
    record_json: &str,
) -> Result<ProviderAccountListItem, String> {
    let record = serde_json::from_str::<KiroTokenRecord>(record_json)
        .map_err(|err| format!("Failed to parse kiro record_json: {err}"))?;
    Ok(ProviderAccountListItem {
        provider_kind: ProviderAccountKind::Kiro,
        account_id,
        email,
        expires_at,
        status: provider_status_from_kiro(&record),
        auth_method,
        provider_name,
        auto_refresh_enabled: None,
        quota: provider_quota_snapshot_from_kiro(&record.quota),
    })
}

fn build_codex_list_item(
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    provider_name: Option<String>,
    record_json: &str,
) -> Result<ProviderAccountListItem, String> {
    let record = serde_json::from_str::<CodexTokenRecord>(record_json)
        .map_err(|err| format!("Failed to parse codex record_json: {err}"))?;
    Ok(ProviderAccountListItem {
        provider_kind: ProviderAccountKind::Codex,
        account_id,
        email,
        expires_at,
        status: provider_status_from_codex(&record),
        auth_method: Some(record.auth_method().as_str().to_string()),
        provider_name,
        auto_refresh_enabled: record.auto_refresh_enabled(),
        quota: provider_quota_snapshot_from_codex(&record.quota),
    })
}

fn build_xai_list_item(
    account_id: String,
    email: Option<String>,
    expires_at: Option<String>,
    provider_name: Option<String>,
    record_json: &str,
) -> Result<ProviderAccountListItem, String> {
    let record = serde_json::from_str::<XaiTokenRecord>(record_json)
        .map_err(|err| format!("Failed to parse xai record_json: {err}"))?;
    Ok(ProviderAccountListItem {
        provider_kind: ProviderAccountKind::Xai,
        account_id,
        email,
        expires_at,
        status: provider_status_from_xai(&record),
        auth_method: Some("oauth".to_string()),
        provider_name,
        auto_refresh_enabled: Some(record.auto_refresh_enabled),
        quota: provider_quota_snapshot_from_xai(&record.quota),
    })
}

fn provider_status_from_kiro(record: &KiroTokenRecord) -> ProviderAccountStatus {
    match record.effective_status() {
        KiroAccountStatus::Active => ProviderAccountStatus::Active,
        KiroAccountStatus::Expired => ProviderAccountStatus::Expired,
    }
}

fn provider_status_from_codex(record: &CodexTokenRecord) -> ProviderAccountStatus {
    match record.effective_status() {
        CodexAccountStatus::Active => ProviderAccountStatus::Active,
        CodexAccountStatus::Expired => ProviderAccountStatus::Expired,
        CodexAccountStatus::Invalid => ProviderAccountStatus::Invalid,
    }
}

fn provider_status_from_xai(record: &XaiTokenRecord) -> ProviderAccountStatus {
    match record.effective_status() {
        XaiAccountStatus::Active => ProviderAccountStatus::Active,
        XaiAccountStatus::Expired => ProviderAccountStatus::Expired,
        XaiAccountStatus::Invalid => ProviderAccountStatus::Invalid,
    }
}

fn provider_quota_snapshot_from_kiro(quota: &KiroQuotaCache) -> ProviderAccountQuotaSnapshot {
    ProviderAccountQuotaSnapshot {
        plan_type: quota.plan_type.clone(),
        error: quota.error.clone(),
        checked_at: quota.checked_at.clone(),
        items: quota
            .quotas
            .iter()
            .map(provider_quota_item_from_kiro)
            .collect(),
    }
}

fn provider_quota_snapshot_from_codex(quota: &CodexQuotaCache) -> ProviderAccountQuotaSnapshot {
    ProviderAccountQuotaSnapshot {
        plan_type: quota.plan_type.clone(),
        error: quota.error.clone(),
        checked_at: quota.checked_at.clone(),
        items: quota
            .quotas
            .iter()
            .map(provider_quota_item_from_codex)
            .collect(),
    }
}

fn provider_quota_snapshot_from_xai(quota: &XaiQuotaCache) -> ProviderAccountQuotaSnapshot {
    ProviderAccountQuotaSnapshot {
        plan_type: quota.plan_type.clone(),
        error: quota.error.clone(),
        checked_at: quota.checked_at.clone(),
        items: quota
            .quotas
            .iter()
            .map(provider_quota_item_from_xai)
            .collect(),
    }
}

fn provider_quota_item_from_kiro(item: &KiroQuotaItem) -> ProviderAccountQuotaItem {
    ProviderAccountQuotaItem {
        name: item.name.clone(),
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        reset_at: item.reset_at.clone(),
        is_trial: item.is_trial,
    }
}

fn provider_quota_item_from_codex(item: &CodexQuotaItem) -> ProviderAccountQuotaItem {
    ProviderAccountQuotaItem {
        name: item.name.clone(),
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        reset_at: item.reset_at.clone(),
        is_trial: false,
    }
}

fn provider_quota_item_from_xai(item: &XaiQuotaItem) -> ProviderAccountQuotaItem {
    ProviderAccountQuotaItem {
        name: item.name.clone(),
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        reset_at: item.reset_at.clone(),
        is_trial: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::random;
    use token_proxy_account_codex::{
        CodexAccountStatus, CodexCredential, CodexQuotaCache, CodexTokenRecord,
    };
    use token_proxy_account_store::records::AccountRecordMetadata;
    use token_proxy_account_xai::{XaiAccountStatus, XaiQuotaCache, XaiTokenRecord};

    fn account_with_status(status: ProviderAccountStatus) -> ProviderAccountListItem {
        ProviderAccountListItem {
            provider_kind: ProviderAccountKind::Codex,
            account_id: format!("codex-{}", status.as_str()),
            email: None,
            expires_at: None,
            status,
            auth_method: None,
            provider_name: None,
            auto_refresh_enabled: Some(true),
            quota: ProviderAccountQuotaSnapshot::default(),
        }
    }

    #[test]
    fn status_counts_include_invalid_accounts() {
        let items = [
            account_with_status(ProviderAccountStatus::Active),
            account_with_status(ProviderAccountStatus::Expired),
            account_with_status(ProviderAccountStatus::Invalid),
            account_with_status(ProviderAccountStatus::CoolingDown),
        ];

        let counts = ProviderAccountStatusCounts::from_items(&items);

        assert_eq!(counts.all, 4);
        assert_eq!(counts.active, 1);
        assert_eq!(counts.expired, 1);
        assert_eq!(counts.invalid, 1);
        assert_eq!(counts.cooling_down, 1);
    }

    #[tokio::test]
    async fn list_snapshot_reads_persisted_invalid_codex_status() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-proxy-provider-accounts-smoke-{}",
            random::<u64>()
        ));
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        for (account_id, email, status) in [
            (
                "codex-valid",
                "valid@example.com",
                CodexAccountStatus::Active,
            ),
            (
                "codex-invalid",
                "invalid@example.com",
                CodexAccountStatus::Invalid,
            ),
        ] {
            let record = CodexTokenRecord {
                credential: CodexCredential::Oauth {
                    access_token: format!("access-{account_id}"),
                    refresh_token: format!("refresh-{account_id}"),
                    client_id: None,
                    id_token: String::new(),
                    auto_refresh_enabled: true,
                    openai_device_id: None,
                    expires_at: "2099-01-01T00:00:00Z".to_string(),
                    last_refresh: None,
                },
                status,
                account_id: Some(account_id.to_string()),
                user_id: None,
                email: Some(email.to_string()),
                quota: CodexQuotaCache::default(),
            };
            records::upsert_record(
                &paths,
                ProviderAccountKind::Codex,
                AccountRecordMetadata {
                    account_id,
                    email: Some(email),
                    expires_at: record.expires_at_str(),
                    expires_at_ms: record.expires_at().map(records::unix_millis),
                    auth_method: None,
                    provider_name: None,
                },
                &record,
            )
            .await
            .expect("seed codex account");
        }

        let items = list_accounts_snapshot(
            &paths,
            ProviderAccountsQueryParams {
                provider_kind: Some(ProviderAccountKind::Codex),
                search: String::new(),
            },
        )
        .await
        .expect("list provider accounts");
        let counts = ProviderAccountStatusCounts::from_items(&items);

        let _ = std::fs::remove_dir_all(data_dir);

        assert_eq!(items.len(), 2);
        assert_eq!(counts.all, 2);
        assert_eq!(counts.active, 1);
        assert_eq!(counts.invalid, 1);
        assert!(items.iter().any(|item| item.account_id == "codex-invalid"
            && item.status == ProviderAccountStatus::Invalid));
    }

    #[tokio::test]
    async fn list_snapshot_reads_persisted_xai_account_fields() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-proxy-provider-accounts-xai-smoke-{}",
            random::<u64>()
        ));
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        let account_id = "xai-user@example.com";
        let record = XaiTokenRecord {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            id_token: String::new(),
            token_type: "Bearer".to_string(),
            expires_at: "2099-01-01T00:00:00Z".to_string(),
            last_refresh: None,
            email: Some("user@example.com".to_string()),
            subject: Some("subject-1".to_string()),
            token_endpoint: Some("https://auth.x.ai/oauth/token".to_string()),
            auto_refresh_enabled: true,
            status: XaiAccountStatus::Active,
            quota: XaiQuotaCache::default(),
        };
        records::upsert_record(
            &paths,
            ProviderAccountKind::Xai,
            AccountRecordMetadata {
                account_id,
                email: record.email.as_deref(),
                expires_at: Some(record.expires_at.as_str()),
                expires_at_ms: record.expires_at().map(records::unix_millis),
                auth_method: Some("oauth"),
                provider_name: Some("xai"),
            },
            &record,
        )
        .await
        .expect("seed xai account");

        let items = list_accounts_snapshot(
            &paths,
            ProviderAccountsQueryParams {
                provider_kind: Some(ProviderAccountKind::Xai),
                search: "user@example.com".to_string(),
            },
        )
        .await
        .expect("list xai accounts");

        let _ = std::fs::remove_dir_all(data_dir);

        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.provider_kind, ProviderAccountKind::Xai);
        assert_eq!(item.email.as_deref(), Some("user@example.com"));
        assert_eq!(item.status, ProviderAccountStatus::Active);
        assert_eq!(item.auth_method.as_deref(), Some("oauth"));
        assert_eq!(item.provider_name.as_deref(), Some("xai"));
        assert_eq!(item.auto_refresh_enabled, Some(true));
    }
}
