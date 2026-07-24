use super::*;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use token_proxy_account_store::paths::TokenProxyPaths;

#[test]
fn parse_config_file_migrates_legacy_upstream_strategy_string() {
    let parsed = parse_config_file(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstream_strategy": "priority_fill_first",
          "upstreams": []
        }
        "#,
        Path::new("/tmp/config.jsonc"),
    )
    .expect("legacy config should migrate");

    assert!(parsed.migrated);
    assert_eq!(
        parsed.config.upstream_strategy.order,
        crate::UpstreamOrderStrategy::FillFirst
    );
    assert_eq!(
        parsed.config.upstream_strategy.dispatch,
        crate::UpstreamDispatchStrategy::Serial
    );
}

/// 测试专用临时数据目录（唯一路径，用完清理）。
fn test_data_dir(label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("token-proxy-config-io-{label}-{nanos}"))
}

#[tokio::test]
async fn load_config_file_migrates_legacy_flat_credential_and_writes_byte_identical_backup() {
    let data_dir = test_data_dir("migrate-ok");
    tokio::fs::create_dir_all(&data_dir)
        .await
        .expect("create temp data dir");
    let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");
    let config_path = paths.config_file().to_path_buf();
    let backup_path = build_backup_path(&config_path);

    // 使用占位 key，避免测试中出现真实密钥形态。
    let original = concat!(
        "// token-proxy test config\n",
        "{\n",
        "  \"host\": \"127.0.0.1\",\n",
        "  \"port\": 9208,\n",
        "  \"hot_model_mappings\": {},\n",
        "  \"upstreams\": [\n",
        "    {\n",
        "      \"id\": \"legacy-openai\",\n",
        "      \"providers\": [\"openai\"],\n",
        "      \"base_url\": \"https://example.com/v1\",\n",
        "      \"api_keys\": [\"test-key-placeholder\"],\n",
        "      \"enabled\": true\n",
        "    }\n",
        "  ]\n",
        "}\n",
    );
    tokio::fs::write(&config_path, original)
        .await
        .expect("write original config");

    let loaded = load_config_file(&paths)
        .await
        .expect("legacy config should migrate and load");

    assert_eq!(loaded.upstreams.len(), 1);
    let upstream = &loaded.upstreams[0];
    match &upstream.credential {
        crate::UpstreamCredential::ApiKeys { api_keys } => {
            assert_eq!(api_keys.as_slice(), ["test-key-placeholder"]);
        }
        other => panic!("expected api_keys credential, got {other:?}"),
    }

    // 写回后主配置序列化不得再含旧平铺字段。
    let rewritten = tokio::fs::read_to_string(&config_path)
        .await
        .expect("read rewritten config");
    let rewritten_value: serde_json::Value = {
        let sanitized = crate::jsonc::sanitize_jsonc(&rewritten);
        serde_json::from_str(&sanitized).expect("parse rewritten")
    };
    let rewritten_upstream = &rewritten_value["upstreams"][0];
    assert!(rewritten_upstream.get("api_keys").is_none());
    assert!(rewritten_upstream.get("kiro_account_id").is_none());
    assert!(rewritten_upstream.get("codex_account_id").is_none());
    assert!(rewritten_upstream.get("xai_account_id").is_none());
    assert_eq!(
        rewritten_upstream["credential"]["type"].as_str(),
        Some("api_keys")
    );
    assert_eq!(
        rewritten_upstream["credential"]["api_keys"],
        serde_json::json!(["test-key-placeholder"])
    );

    // 备份字节必须与原始 JSONC 完全一致（含注释与换行）。
    let backup = tokio::fs::read(&backup_path)
        .await
        .expect("backup must exist after successful migration");
    assert_eq!(backup.as_slice(), original.as_bytes());

    let _ = tokio::fs::remove_dir_all(&data_dir).await;
}

#[tokio::test]
async fn load_config_file_conflict_leaves_main_file_unchanged_and_skips_backup() {
    let data_dir = test_data_dir("migrate-conflict");
    tokio::fs::create_dir_all(&data_dir)
        .await
        .expect("create temp data dir");
    let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");
    let config_path = paths.config_file().to_path_buf();
    let backup_path = build_backup_path(&config_path);

    let original = concat!(
        "// conflict fixture\n",
        "{\n",
        "  \"host\": \"127.0.0.1\",\n",
        "  \"port\": 9208,\n",
        "  \"hot_model_mappings\": {},\n",
        "  \"upstreams\": [\n",
        "    {\n",
        "      \"id\": \"conflict-u\",\n",
        "      \"providers\": [\"openai\"],\n",
        "      \"base_url\": \"https://example.com/v1\",\n",
        "      \"credential\": { \"type\": \"passthrough\" },\n",
        "      \"api_keys\": [\"must-not-apply\"],\n",
        "      \"enabled\": true\n",
        "    }\n",
        "  ]\n",
        "}\n",
    );
    tokio::fs::write(&config_path, original)
        .await
        .expect("write conflict config");

    let error = match load_config_file(&paths).await {
        Ok(_) => panic!("credential + flat conflict must fail load"),
        Err(message) => message,
    };
    assert!(
        error.contains("cannot combine credential with legacy flat credential fields")
            || error.contains("Failed to migrate"),
        "unexpected error: {error}"
    );

    // 主文件字节不变。
    let after = tokio::fs::read(&config_path)
        .await
        .expect("read main config after failed migrate");
    assert_eq!(after.as_slice(), original.as_bytes());

    // 不得生成/覆盖 backup。
    assert!(
        !tokio::fs::try_exists(&backup_path).await.unwrap_or(true),
        "backup must not be written when migration fails"
    );

    let _ = tokio::fs::remove_dir_all(&data_dir).await;
}

/// 原子写：rename 失败时旧主文件字节保持，JSONC header 不丢。
#[tokio::test]
async fn atomic_write_rename_failure_preserves_existing_config_bytes() {
    let data_dir = test_data_dir("atomic-rename-fail");
    tokio::fs::create_dir_all(&data_dir)
        .await
        .expect("create temp data dir");
    let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("paths");
    let config_path = paths.config_file().to_path_buf();

    let original = concat!(
        "// preserve-me header\n",
        "{\n",
        "  \"host\": \"127.0.0.1\",\n",
        "  \"port\": 9208,\n",
        "  \"hot_model_mappings\": {},\n",
        "  \"upstreams\": []\n",
        "}\n",
    );
    tokio::fs::write(&config_path, original)
        .await
        .expect("seed original");

    set_fail_rename_after_temp_write(true);
    let mut next = ProxyConfigFile::default();
    next.port = 9333;
    let err = save_config_file(&paths, &next)
        .await
        .expect_err("rename seam must fail");
    set_fail_rename_after_temp_write(false);

    assert!(
        err.contains("injected atomic config rename failure"),
        "unexpected error: {err}"
    );

    let after = tokio::fs::read_to_string(&config_path)
        .await
        .expect("read preserved config");
    assert_eq!(after, original, "old main file must remain byte-identical");
    assert!(after.contains("preserve-me header"));
    assert!(!after.contains("9333"));

    // 成功路径仍可写，且保留 header。
    next.port = 9444;
    save_config_file(&paths, &next)
        .await
        .expect("atomic save ok");
    let rewritten = tokio::fs::read_to_string(&config_path)
        .await
        .expect("read rewritten");
    assert!(rewritten.contains("preserve-me header"));
    assert!(rewritten.contains("9444"));

    let _ = tokio::fs::remove_dir_all(&data_dir).await;
}
