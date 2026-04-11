pub mod codex;
pub mod config;
pub mod dashboard;
pub mod kiro;
pub mod logs;
pub mod providers;
pub mod proxy;

pub use codex::{
    codex_fetch_quotas, codex_import_file, codex_list_accounts, codex_logout, codex_poll_login,
    codex_refresh_account, codex_refresh_quota_cache, codex_refresh_quota_now,
    codex_set_auto_refresh, codex_set_priority, codex_set_proxy_url, codex_set_status,
    codex_start_login,
};
pub use config::{
    preview_client_setup, read_proxy_config, save_proxy_config, write_claude_code_settings,
    write_codex_config, write_opencode_config,
};
pub use dashboard::read_dashboard_snapshot;
pub use kiro::{
    kiro_fetch_quotas, kiro_handle_callback, kiro_import_ide, kiro_import_kam, kiro_list_accounts,
    kiro_logout, kiro_poll_login, kiro_refresh_quota_cache, kiro_refresh_quota_now,
    kiro_set_priority, kiro_set_proxy_url, kiro_set_status, kiro_start_login,
};
pub use logs::{read_request_detail_capture, read_request_log_detail, set_request_detail_capture};
pub use providers::{providers_delete_accounts, providers_list_accounts_page};
pub use proxy::{
    prepare_relaunch, proxy_reload, proxy_restart, proxy_start, proxy_status, proxy_stop,
};

#[derive(Clone, Copy)]
pub(crate) enum ManualAccountStatus {
    Active,
    Disabled,
}

impl From<ManualAccountStatus> for crate::kiro::KiroAccountStatus {
    fn from(value: ManualAccountStatus) -> Self {
        match value {
            ManualAccountStatus::Active => Self::Active,
            ManualAccountStatus::Disabled => Self::Disabled,
        }
    }
}

impl From<ManualAccountStatus> for crate::codex::CodexAccountStatus {
    fn from(value: ManualAccountStatus) -> Self {
        match value {
            ManualAccountStatus::Active => Self::Active,
            ManualAccountStatus::Disabled => Self::Disabled,
        }
    }
}

pub(crate) fn parse_manual_account_status(value: &str) -> Result<ManualAccountStatus, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "active" => Ok(ManualAccountStatus::Active),
        "disabled" => Ok(ManualAccountStatus::Disabled),
        other => Err(format!("Unsupported manual account status: {other}")),
    }
}
