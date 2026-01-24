use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};
use time::OffsetDateTime;

const WAL_SUFFIX: &str = "-wal";
const SHM_SUFFIX: &str = "-shm";

pub(crate) async fn read_item(path: &Path, key: &str) -> Result<Option<String>, String> {
    let mut conn = open_connection(path, true).await?;
    let row = sqlx::query("SELECT value FROM ItemTable WHERE key = ?")
        .bind(key)
        .fetch_optional(&mut conn)
        .await
        .map_err(|err| format!("Failed to read Antigravity state: {err}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let value: Vec<u8> = row.try_get("value").unwrap_or_default();
    Ok(Some(String::from_utf8_lossy(&value).to_string()))
}

pub(crate) async fn write_item(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let mut conn = open_connection(path, false).await?;
    let mut tx = conn
        .begin()
        .await
        .map_err(|err| format!("Failed to start Antigravity DB transaction: {err}"))?;
    sqlx::query("INSERT INTO ItemTable(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        .bind(key)
        .bind(value)
        .execute(&mut *tx)
        .await
        .map_err(|err| format!("Failed to write Antigravity state: {err}"))?;
    tx.commit()
        .await
        .map_err(|err| format!("Failed to commit Antigravity state: {err}"))?;
    Ok(())
}

pub(crate) async fn delete_wal_shm(path: &Path) -> Result<(), String> {
    let wal = path_with_suffix(path, WAL_SUFFIX);
    let shm = path_with_suffix(path, SHM_SUFFIX);
    if tokio::fs::try_exists(&wal).await.unwrap_or(false) {
        tokio::fs::remove_file(&wal)
            .await
            .map_err(|err| format!("Failed to remove WAL: {err}"))?;
    }
    if tokio::fs::try_exists(&shm).await.unwrap_or(false) {
        tokio::fs::remove_file(&shm)
            .await
            .map_err(|err| format!("Failed to remove SHM: {err}"))?;
    }
    Ok(())
}

pub(crate) async fn backup_db(path: &Path) -> Result<PathBuf, String> {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp();
    let backup = path.with_extension(format!("vscdb.bak-{timestamp}"));
    tokio::fs::copy(path, &backup)
        .await
        .map_err(|err| format!("Failed to backup Antigravity DB: {err}"))?;
    Ok(backup)
}

pub(crate) async fn restore_db(original: &Path, backup: &Path) -> Result<(), String> {
    if tokio::fs::try_exists(backup).await.unwrap_or(false) {
        tokio::fs::copy(backup, original)
            .await
            .map_err(|err| format!("Failed to restore Antigravity DB: {err}"))?;
    }
    Ok(())
}

pub(crate) async fn cleanup_backup(path: &Path) -> Result<(), String> {
    if tokio::fs::try_exists(path).await.unwrap_or(false) {
        tokio::fs::remove_file(path)
            .await
            .map_err(|err| format!("Failed to cleanup backup: {err}"))?;
    }
    Ok(())
}

async fn open_connection(path: &Path, read_only: bool) -> Result<SqliteConnection, String> {
    if !tokio::fs::try_exists(path).await.unwrap_or(false) {
        return Err("Antigravity IDE database not found.".to_string());
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(read_only)
        .create_if_missing(false)
        .busy_timeout(Duration::from_secs(3));
    SqliteConnection::connect_with(&options)
        .await
        .map_err(|err| format!("Failed to open Antigravity DB: {err}"))
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let new_name = format!("{file_name}{suffix}");
    path.with_file_name(new_name)
}
