//! 应用数据目录磁盘占用测量。
//!
//! 只统计 `TokenProxyPaths::data_dir()` 下文件，按数据库 / 配置 / 其它三分项汇总，
//! 供设置页展示；不做清空或 VACUUM。

use std::fs;
use std::path::Path;

use serde::Serialize;

use token_proxy_account_store::paths::{
    TokenProxyPaths, CONFIG_FILE_NAME, DB_FILE_NAME, DB_SHM_FILE_NAME, DB_WAL_FILE_NAME,
};

/// 数据目录占用快照（字节，前端再格式化）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataStorageUsage {
    /// 数据目录绝对路径。
    pub data_dir: String,
    /// 目录下全部文件合计。
    pub total_bytes: u64,
    /// `data.db` 及其 WAL/SHM 旁路文件。
    pub database_bytes: u64,
    /// `config.jsonc`。
    pub config_bytes: u64,
    /// auth 目录等其余文件（含旧版 Agent 节点遗留的 agent-node.json）。
    pub other_bytes: u64,
}

/// 扫描 `data_dir`，按分项汇总占用。目录不存在时返回全 0。
pub fn measure_data_storage(paths: &TokenProxyPaths) -> Result<DataStorageUsage, String> {
    let data_dir = paths.data_dir();
    let data_dir_display = data_dir.display().to_string();

    if !data_dir.exists() {
        tracing::info!(data_dir = %data_dir_display, "data dir missing; storage usage is zero");
        return Ok(DataStorageUsage {
            data_dir: data_dir_display,
            total_bytes: 0,
            database_bytes: 0,
            config_bytes: 0,
            other_bytes: 0,
        });
    }

    let mut database_bytes = 0_u64;
    let mut config_bytes = 0_u64;
    let mut other_bytes = 0_u64;

    visit_files(
        data_dir,
        &mut |file_path, size| match classify_file(file_path, data_dir) {
            StorageBucket::Database => database_bytes = database_bytes.saturating_add(size),
            StorageBucket::Config => config_bytes = config_bytes.saturating_add(size),
            StorageBucket::Other => other_bytes = other_bytes.saturating_add(size),
        },
    )?;

    let total_bytes = database_bytes
        .saturating_add(config_bytes)
        .saturating_add(other_bytes);

    tracing::info!(
        data_dir = %data_dir_display,
        total_bytes,
        database_bytes,
        config_bytes,
        other_bytes,
        "measured data storage usage"
    );

    Ok(DataStorageUsage {
        data_dir: data_dir_display,
        total_bytes,
        database_bytes,
        config_bytes,
        other_bytes,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StorageBucket {
    Database,
    Config,
    Other,
}

fn classify_file(path: &Path, data_dir: &Path) -> StorageBucket {
    let parent_is_root = path.parent() == Some(data_dir);
    if !parent_is_root {
        return StorageBucket::Other;
    }
    match path.file_name().and_then(|name| name.to_str()) {
        Some(DB_FILE_NAME) | Some(DB_WAL_FILE_NAME) | Some(DB_SHM_FILE_NAME) => {
            StorageBucket::Database
        }
        Some(CONFIG_FILE_NAME) => StorageBucket::Config,
        _ => StorageBucket::Other,
    }
}

fn visit_files(root: &Path, on_file: &mut dyn FnMut(&Path, u64)) -> Result<(), String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|err| format!("Failed to read data dir entry {}: {err}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                format!(
                    "Failed to read data dir entry under {}: {err}",
                    current.display()
                )
            })?;
            let path = entry.path();
            let metadata = entry
                .metadata()
                .map_err(|err| format!("Failed to read metadata for {}: {err}", path.display()))?;
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if metadata.is_file() {
                on_file(&path, metadata.len());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        let mut file = fs::File::create(path).expect("create file");
        file.write_all(bytes).expect("write file");
    }

    #[test]
    fn measure_data_storage_splits_database_config_and_other() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-proxy-storage-usage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&data_dir).expect("create data dir");
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");

        write_file(&data_dir.join(DB_FILE_NAME), &vec![0_u8; 100]);
        write_file(&data_dir.join(DB_WAL_FILE_NAME), &vec![0_u8; 20]);
        write_file(&data_dir.join(CONFIG_FILE_NAME), &vec![0_u8; 10]);
        write_file(
            &data_dir.join("kiro-auth").join("account.json"),
            &vec![0_u8; 7],
        );
        write_file(&data_dir.join("agent-node.json"), &vec![0_u8; 3]);

        let usage = measure_data_storage(&paths).expect("measure");
        let _ = fs::remove_dir_all(&data_dir);

        assert_eq!(usage.database_bytes, 120);
        assert_eq!(usage.config_bytes, 10);
        assert_eq!(usage.other_bytes, 10);
        assert_eq!(usage.total_bytes, 140);
        assert_eq!(PathBuf::from(&usage.data_dir), data_dir);
    }

    #[test]
    fn measure_data_storage_missing_dir_is_zero() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-proxy-storage-missing-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");
        let usage = measure_data_storage(&paths).expect("measure missing");
        assert_eq!(
            usage,
            DataStorageUsage {
                data_dir: data_dir.display().to_string(),
                total_bytes: 0,
                database_bytes: 0,
                config_bytes: 0,
                other_bytes: 0,
            }
        );
    }
}
