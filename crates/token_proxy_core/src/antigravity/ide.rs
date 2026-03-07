use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sysinfo::ProcessesToUpdate;

use crate::paths::TokenProxyPaths;

use super::ide_db;
use super::protobuf;
use super::store::AntigravityAccountStore;
use super::types::{AntigravityAccountSummary, AntigravityIdeStatus, AntigravityTokenRecord};

#[derive(Clone)]
pub(crate) struct AntigravityIdeConfig {
    pub(crate) ide_db_path: Option<PathBuf>,
    pub(crate) app_paths: Vec<PathBuf>,
    pub(crate) process_names: Vec<String>,
}

#[cfg(target_os = "macos")]
const DEFAULT_PROCESS_NAMES: [&str; 2] =
    ["com.google.antigravity", "com.todesktop.230313mzl4w4u92"];

#[cfg(not(target_os = "macos"))]
const DEFAULT_PROCESS_NAMES: [&str; 0] = [];

#[cfg(target_os = "macos")]
fn default_app_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Antigravity.app"),
        home.join("Applications").join("Antigravity.app"),
    ]
}

#[cfg(not(target_os = "macos"))]
fn default_app_paths(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn default_db_path(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("Antigravity")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

#[cfg(not(target_os = "macos"))]
fn default_db_path(_home: &Path) -> PathBuf {
    PathBuf::new()
}

pub async fn import_from_ide(
    paths: &TokenProxyPaths,
    store: &AntigravityAccountStore,
    path_override: Option<PathBuf>,
) -> Result<Vec<AntigravityAccountSummary>, String> {
    let config = resolve_ide_config(paths).await?;
    let db_path = resolve_db_path(path_override, &config)?;
    let state = ide_db::read_item(&db_path, "jetskiStateSync.agentManagerInitState")
        .await?
        .ok_or_else(|| "Antigravity IDE state not found.".to_string())?;
    let mut record = match protobuf::extract_token_record(&state)? {
        Some(record) => record,
        None => return Err("Failed to extract Antigravity token from IDE.".to_string()),
    };
    record.email = read_auth_email(&db_path).await?;
    record.source = Some("ide".to_string());
    let summary = store.save_new_account(record).await?;
    Ok(vec![summary])
}

pub async fn switch_ide_account(
    paths: &TokenProxyPaths,
    store: &AntigravityAccountStore,
    account_id: &str,
    path_override: Option<PathBuf>,
) -> Result<AntigravityIdeStatus, String> {
    let config = resolve_ide_config(paths).await?;
    let db_path = resolve_db_path(path_override, &config)?;
    let record = store.get_account_record(account_id).await?;
    ensure_ide_closed(&config).await?;
    ide_db::delete_wal_shm(&db_path).await?;
    let backup = ide_db::backup_db(&db_path).await?;
    let result = apply_account_to_db(&db_path, &record).await;
    if let Err(err) = result {
        let _ = ide_db::restore_db(&db_path, &backup).await;
        let _ = ide_db::cleanup_backup(&backup).await;
        return Err(err);
    }
    let _ = ide_db::cleanup_backup(&backup).await;
    restart_ide(&config).await?;
    ide_status_with_config(config).await
}

pub async fn ide_status(paths: &TokenProxyPaths) -> Result<AntigravityIdeStatus, String> {
    let config = resolve_ide_config(paths).await?;
    ide_status_with_config(config).await
}

async fn ide_status_with_config(
    config: AntigravityIdeConfig,
) -> Result<AntigravityIdeStatus, String> {
    let database_available = config
        .ide_db_path
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false);
    let active_email = if database_available {
        let db_path = config.ide_db_path.as_ref().expect("db path");
        read_auth_email(db_path).await?
    } else {
        None
    };
    let ide_running = is_ide_running(&config).await;
    Ok(AntigravityIdeStatus {
        database_available,
        ide_running,
        active_email,
    })
}

async fn apply_account_to_db(path: &Path, record: &AntigravityTokenRecord) -> Result<(), String> {
    let state = ide_db::read_item(path, "jetskiStateSync.agentManagerInitState")
        .await?
        .ok_or_else(|| "Antigravity IDE state not found.".to_string())?;
    let injected = protobuf::inject_token_record(&state, record)?;
    ide_db::write_item(path, "jetskiStateSync.agentManagerInitState", &injected).await?;
    ide_db::write_item(path, "antigravityOnboarding", "true").await?;
    if let Some(email) = record.email.as_deref() {
        let payload = serde_json::json!({ "email": email });
        let payload = serde_json::to_string(&payload).unwrap_or_default();
        let _ = ide_db::write_item(path, "antigravityAuthStatus", &payload).await;
    }
    Ok(())
}

async fn read_auth_email(path: &Path) -> Result<Option<String>, String> {
    let raw = ide_db::read_item(path, "antigravityAuthStatus").await?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Ok(None);
    };
    if let Some(email) = value.get("email").and_then(Value::as_str) {
        let trimmed = email.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }
    Ok(None)
}

async fn resolve_ide_config(paths: &TokenProxyPaths) -> Result<AntigravityIdeConfig, String> {
    let config = crate::proxy::config::read_config(paths).await?.config;
    let home = resolve_home_dir()?;
    let ide_db_path = config
        .antigravity_ide_db_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let default = default_db_path(&home);
            if default.as_os_str().is_empty() {
                None
            } else {
                Some(default)
            }
        });
    let app_paths = if !config.antigravity_app_paths.is_empty() {
        config
            .antigravity_app_paths
            .iter()
            .map(|value| PathBuf::from(value))
            .collect()
    } else {
        default_app_paths(&home)
    };
    let process_names = if !config.antigravity_process_names.is_empty() {
        config
            .antigravity_process_names
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect()
    } else {
        DEFAULT_PROCESS_NAMES
            .iter()
            .map(|value| value.to_string())
            .collect()
    };
    Ok(AntigravityIdeConfig {
        ide_db_path,
        app_paths,
        process_names,
    })
}

fn resolve_db_path(
    override_path: Option<PathBuf>,
    config: &AntigravityIdeConfig,
) -> Result<PathBuf, String> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    config
        .ide_db_path
        .clone()
        .ok_or_else(|| "Antigravity IDE database path is not configured.".to_string())
}

async fn is_ide_running(config: &AntigravityIdeConfig) -> bool {
    if config.process_names.is_empty() {
        return false;
    }
    let targets: Vec<String> = config
        .process_names
        .iter()
        .map(|value| value.to_lowercase())
        .collect();
    super::warmup::run_blocking(move || {
        let mut system = sysinfo::System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);
        system.processes().values().any(|process| {
            let name = process.name().to_string_lossy().to_lowercase();
            targets.iter().any(|target| name.contains(target))
        })
    })
    .await
    .unwrap_or(false)
}

async fn ensure_ide_closed(config: &AntigravityIdeConfig) -> Result<(), String> {
    if config.process_names.is_empty() {
        return Ok(());
    }
    let targets = config.process_names.clone();
    super::warmup::run_blocking(move || {
        let mut system = sysinfo::System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);
        for process in system.processes().values() {
            let name = process.name().to_string_lossy().to_lowercase();
            if targets
                .iter()
                .any(|target| name.contains(&target.to_lowercase()))
            {
                let _ = process.kill();
            }
        }
    })
    .await
    .map_err(|_| "Failed to terminate Antigravity IDE.".to_string())?;
    Ok(())
}

async fn restart_ide(config: &AntigravityIdeConfig) -> Result<(), String> {
    if config.app_paths.is_empty() {
        return Ok(());
    }
    for path in &config.app_paths {
        if !path.exists() {
            continue;
        }
        #[cfg(target_os = "macos")]
        {
            let result = Command::new("open").arg(path).spawn();
            if result.is_ok() {
                return Ok(());
            }
        }
        #[cfg(target_os = "windows")]
        {
            let result = Command::new("cmd")
                .args(["/C", "start", ""])
                .arg(path)
                .spawn();
            if result.is_ok() {
                return Ok(());
            }
        }
        #[cfg(target_os = "linux")]
        {
            let result = Command::new("xdg-open").arg(path).spawn();
            if result.is_ok() {
                return Ok(());
            }
        }
    }
    Ok(())
}

fn resolve_home_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(dir);
    }
    if cfg!(target_os = "windows") {
        if let Some(dir) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
            return Ok(dir);
        }
    }
    Err("Failed to resolve user home directory.".to_string())
}
