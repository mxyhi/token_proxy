//! Account-as-Upstream Phase C2 生命周期编排集成测试。
//!
//! 禁止在日志/错误中输出 token / private key / api key / 完整 credential。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use token_proxy_account_codex::{CodexCredential, CodexLoginStatus, CodexTokenRecord};
use token_proxy_account_kiro::{KiroLoginStatus, KiroTokenRecord};
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_account_xai::{XaiLoginStatus, XaiTokenRecord};
use token_proxy_config::{
    AccountProvider, LogLevel, ProxyConfigFile, UpstreamConfig, UpstreamCredential,
    UpstreamOverrides,
};
use token_proxy_runtime::logging::LoggingState;

use crate::account_upstreams::{
    reconcile_account_upstreams, removed_account_bindings, AccountUpstreamRef,
};
use crate::app::TokenProxyApp;

use super::count_account_bindings;

fn open_app() -> (tempfile::TempDir, TokenProxyApp) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = TokenProxyPaths::from_app_data_dir(dir.path().to_path_buf()).expect("paths");
    let app = TokenProxyApp::open(paths, LoggingState::init(LogLevel::Silent), None).expect("app");
    (dir, app)
}

fn future_expires() -> String {
    "2999-01-01T00:00:00Z".to_string()
}

fn kiro_record(email: &str, access: &str) -> KiroTokenRecord {
    KiroTokenRecord {
        access_token: access.to_string(),
        refresh_token: format!("refresh-{access}"),
        profile_arn: None,
        expires_at: future_expires(),
        auth_method: "builder-id".to_string(),
        provider: "Google".to_string(),
        client_id: None,
        client_secret: None,
        email: Some(email.to_string()),
        last_refresh: None,
        start_url: None,
        region: None,
        status: token_proxy_account_kiro::KiroAccountStatus::Active,
        quota: Default::default(),
    }
}

fn codex_record(email: &str, access: &str) -> CodexTokenRecord {
    CodexTokenRecord {
        credential: CodexCredential::Oauth {
            access_token: access.to_string(),
            refresh_token: format!("refresh-{access}"),
            client_id: None,
            id_token: String::new(),
            auto_refresh_enabled: true,
            openai_device_id: None,
            expires_at: future_expires(),
            last_refresh: None,
        },
        status: token_proxy_account_codex::CodexAccountStatus::Active,
        account_id: Some(format!("chatgpt-{email}")),
        user_id: Some(format!("user-{email}")),
        email: Some(email.to_string()),
        quota: Default::default(),
    }
}

fn xai_record(email: &str, access: &str) -> XaiTokenRecord {
    XaiTokenRecord {
        access_token: access.to_string(),
        refresh_token: format!("refresh-{access}"),
        id_token: String::new(),
        token_type: "Bearer".to_string(),
        expires_at: future_expires(),
        last_refresh: None,
        email: Some(email.to_string()),
        subject: Some(format!("sub-{email}")),
        token_endpoint: None,
        auto_refresh_enabled: true,
        status: token_proxy_account_xai::XaiAccountStatus::Active,
        quota: Default::default(),
    }
}

fn xai_import_json(email: &str, access: &str) -> String {
    format!(
        r#"{{"type":"xai","auth_kind":"oauth","access_token":"{access}","refresh_token":"refresh-{access}","expired":"2999-01-01T00:00:00Z","email":"{email}","sub":"sub-{email}"}}"#
    )
}

/// 可靠 Codex text import fixture（含 email + expires_at，无需真实 JWT）。
fn codex_import_json(email: &str, access: &str) -> String {
    format!(
        r#"{{"access_token":"{access}","refresh_token":"refresh-{access}","expires_at":"2999-01-01T00:00:00Z","email":"{email}","account_id":"chatgpt-{email}","user_id":"user-{email}"}}"#
    )
}

fn sample_upstream(id: &str, providers: &[&str], credential: UpstreamCredential) -> UpstreamConfig {
    UpstreamConfig {
        id: id.to_string(),
        providers: providers.iter().map(|value| (*value).to_string()).collect(),
        base_url: String::new(),
        credential,
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }
}

async fn read_config(app: &TokenProxyApp) -> ProxyConfigFile {
    token_proxy_config::read_config(app.paths().as_ref())
        .await
        .expect("read config")
        .config
}

/// A. startup missing account creates exactly one upstream；重复 startup 幂等。
#[tokio::test]
async fn startup_reconcile_creates_one_upstream_and_is_idempotent() {
    let (_dir, app) = open_app();
    app.kiro_accounts()
        .save_record_for_test("kiro-user-a".to_string(), kiro_record("a@ex.com", "tok-a"))
        .await
        .expect("seed kiro");

    let faults = app.lifecycle_faults();
    faults.reset();

    let first = app
        .reconcile_existing_accounts_for_test()
        .await
        .expect("first reconcile");
    assert!(first.config_changed);
    assert_eq!(first.added_bindings, 1);
    assert_eq!(faults.config_write_count.load(Ordering::SeqCst), 1);

    let config = read_config(&app).await;
    assert_eq!(count_account_bindings(&config), 1);
    assert!(matches!(
        &config.upstreams[0].credential,
        UpstreamCredential::Account {
            provider: AccountProvider::Kiro,
            account_id
        } if account_id == "kiro-user-a"
    ));

    let second = app
        .reconcile_existing_accounts_for_test()
        .await
        .expect("second reconcile");
    assert!(!second.config_changed);
    assert_eq!(second.added_bindings, 0);
    assert_eq!(
        faults.config_write_count.load(Ordering::SeqCst),
        1,
        "idempotent reconcile must not rewrite config"
    );
}

/// B. repeated import preserves routing fields；config 不变时不 apply。
#[tokio::test]
async fn repeated_ensure_preserves_routing_and_skips_apply() {
    let (_dir, app) = open_app();
    app.codex_accounts()
        .save_record_for_test(
            "codex-user-b".to_string(),
            codex_record("b@ex.com", "tok-b"),
        )
        .await
        .expect("seed codex");

    let mut config = read_config(&app).await;
    let mut custom = sample_upstream(
        "custom-codex",
        &["codex"],
        UpstreamCredential::account(AccountProvider::Codex, "codex-user-b"),
    );
    custom.priority = Some(77);
    custom.enabled = false;
    custom.proxy_url = Some("socks5://127.0.0.1:1080".to_string());
    custom.available_models = vec!["gpt-5".to_string()];
    custom
        .model_mappings
        .insert("alias".to_string(), "gpt-5".to_string());
    custom.overrides = Some(UpstreamOverrides {
        header: HashMap::from([("X-Test".to_string(), Some("1".to_string()))]),
    });
    config.upstreams.push(custom);
    token_proxy_config::write_config(app.paths().as_ref(), config)
        .await
        .expect("seed config");

    let faults = app.lifecycle_faults();
    faults.reset();
    let before = serde_json::to_value(read_config(&app).await.upstreams).expect("ser");

    let summary = app
        .ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
            AccountProvider::Codex,
            "codex-user-b",
        )])
        .await
        .expect("ensure");
    assert!(!summary.config_changed);
    assert!(!summary.applied);
    assert_eq!(faults.config_write_count.load(Ordering::SeqCst), 0);
    assert_eq!(faults.apply_count.load(Ordering::SeqCst), 0);

    let after = serde_json::to_value(read_config(&app).await.upstreams).expect("ser");
    assert_eq!(before, after);

    let kept = &read_config(&app).await.upstreams[0];
    assert_eq!(kept.id, "custom-codex");
    assert_eq!(kept.priority, Some(77));
    assert!(!kept.enabled);
    assert_eq!(kept.proxy_url.as_deref(), Some("socks5://127.0.0.1:1080"));
    assert_eq!(kept.available_models, vec!["gpt-5".to_string()]);
}

/// C. config 写失败时恢复本次 import 写入的 credential。
#[tokio::test]
async fn config_write_failure_restores_imported_credential() {
    let (_dir, app) = open_app();
    let faults = app.lifecycle_faults();
    faults.reset();
    faults.fail_config_write.store(true, Ordering::SeqCst);

    let err = app
        .xai_import_text(&xai_import_json("c@ex.com", "tok-c"))
        .await
        .expect_err("config write must fail");
    assert!(err.contains("injected config write failure"));
    // 错误路径不得泄露完整 credential 字节。
    assert!(!err.contains("tok-c"));
    assert!(!err.contains("refresh-tok-c"));

    let accounts = app.xai_accounts().list_accounts().await.expect("list");
    assert!(
        accounts.is_empty(),
        "imported credential must be restored away"
    );
    assert_eq!(count_account_bindings(&read_config(&app).await), 0);
}

/// C2. overwrite 场景：config 写失败时恢复旧 credential 字节。
#[tokio::test]
async fn config_write_failure_restores_overwritten_credential() {
    let (_dir, app) = open_app();
    let seeded = app
        .xai_accounts()
        .save_record_for_test(
            "xai-user-ow".to_string(),
            xai_record("ow@ex.com", "old-access"),
        )
        .await
        .expect("seed");
    assert_eq!(seeded.account_id, "xai-user-ow");

    app.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
        AccountProvider::Xai,
        "xai-user-ow",
    )])
    .await
    .expect("bind");

    let faults = app.lifecycle_faults();
    faults.reset();
    let mut config = read_config(&app).await;
    config.upstreams.clear();
    token_proxy_config::write_config(app.paths().as_ref(), config)
        .await
        .expect("clear bindings");

    faults.fail_config_write.store(true, Ordering::SeqCst);
    let err = app
        .xai_import_text(&xai_import_json("ow@ex.com", "new-access"))
        .await
        .expect_err("must fail");
    assert!(err.contains("injected config write failure"));
    assert!(!err.contains("old-access"));
    assert!(!err.contains("new-access"));

    let restored = app
        .xai_accounts()
        .snapshot_account_record("xai-user-ow")
        .await
        .expect("snap")
        .expect("account remains");
    assert_eq!(restored.access_token, "old-access");
}

/// D. credential 删除失败时保留旧 config 与全部 credentials。
#[tokio::test]
async fn credential_delete_failure_keeps_old_config_and_credentials() {
    let (_dir, app) = open_app();
    app.kiro_accounts()
        .save_record_for_test("kiro-del".to_string(), kiro_record("d@ex.com", "tok-d"))
        .await
        .expect("seed");
    app.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
        AccountProvider::Kiro,
        "kiro-del",
    )])
    .await
    .expect("bind");

    let old = read_config(&app).await;
    let mut new_config = old.clone();
    new_config.upstreams.clear();

    let faults = app.lifecycle_faults();
    faults.reset();
    faults.fail_credential_delete.store(true, Ordering::SeqCst);

    let err = app
        .save_proxy_config(new_config)
        .await
        .expect_err("delete fault");
    assert!(err.contains("injected credential delete failure"));

    let after = read_config(&app).await;
    assert_eq!(
        serde_json::to_value(&after.upstreams).unwrap(),
        serde_json::to_value(&old.upstreams).unwrap()
    );
    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);
}

/// E. apply 失败恢复旧 config 与 credential；rollback apply 可断言。
#[tokio::test]
async fn apply_failure_restores_config_and_credentials() {
    let (_dir, app) = open_app();
    app.codex_accounts()
        .save_record_for_test("codex-apply".to_string(), codex_record("e@ex.com", "tok-e"))
        .await
        .expect("seed");
    app.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
        AccountProvider::Codex,
        "codex-apply",
    )])
    .await
    .expect("bind");

    let old = read_config(&app).await;
    let mut new_config = old.clone();
    new_config.upstreams.clear();

    let faults = app.lifecycle_faults();
    faults.reset();
    faults.fail_apply.store(true, Ordering::SeqCst);

    let err = app
        .save_proxy_config(new_config)
        .await
        .expect_err("apply fault");
    assert!(err.contains("injected apply failure"));

    let after = read_config(&app).await;
    assert_eq!(count_account_bindings(&after), 1);
    assert_eq!(
        app.codex_accounts().list_accounts().await.unwrap().len(),
        1,
        "credential restored after apply failure"
    );
    assert!(faults.apply_count.load(Ordering::SeqCst) >= 1);
}

/// F. 并发 mutation 串行化：不丢 upstream、不重复 binding。
#[tokio::test]
async fn concurrent_mutations_serialize_without_duplicate_bindings() {
    let (_dir, app) = open_app();
    let app = Arc::new(app);

    app.kiro_accounts()
        .save_record_for_test("kiro-c1".to_string(), kiro_record("c1@ex.com", "t1"))
        .await
        .unwrap();
    app.codex_accounts()
        .save_record_for_test("codex-c2".to_string(), codex_record("c2@ex.com", "t2"))
        .await
        .unwrap();
    app.xai_accounts()
        .save_record_for_test("xai-c3".to_string(), xai_record("c3@ex.com", "t3"))
        .await
        .unwrap();

    let a = app.clone();
    let b = app.clone();
    let c = app.clone();
    let (r1, r2, r3) = tokio::join!(
        async move {
            a.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
                AccountProvider::Kiro,
                "kiro-c1",
            )])
            .await
        },
        async move {
            b.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
                AccountProvider::Codex,
                "codex-c2",
            )])
            .await
        },
        async move {
            c.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
                AccountProvider::Xai,
                "xai-c3",
            )])
            .await
        },
    );
    r1.expect("kiro");
    r2.expect("codex");
    r3.expect("xai");

    let config = read_config(&app).await;
    assert_eq!(count_account_bindings(&config), 3);
    token_proxy_config::write_config(app.paths().as_ref(), config.clone())
        .await
        .expect("unique bindings still valid");
}

/// G. 删除 upstream 级联删 credential；rename 同 binding 不删。
#[tokio::test]
async fn cascade_delete_on_removed_binding_and_rename_preserves_credential() {
    let (_dir, app) = open_app();
    app.kiro_accounts()
        .save_record_for_test("kiro-g1".to_string(), kiro_record("g1@ex.com", "tg1"))
        .await
        .unwrap();
    app.codex_accounts()
        .save_record_for_test("codex-g2".to_string(), codex_record("g2@ex.com", "tg2"))
        .await
        .unwrap();
    app.ensure_account_bindings_for_test(vec![
        AccountUpstreamRef::new(AccountProvider::Kiro, "kiro-g1"),
        AccountUpstreamRef::new(AccountProvider::Codex, "codex-g2"),
    ])
    .await
    .unwrap();

    let old = read_config(&app).await;
    let mut new_config = old.clone();
    for upstream in &mut new_config.upstreams {
        if let UpstreamCredential::Account {
            provider: AccountProvider::Kiro,
            ..
        } = &upstream.credential
        {
            upstream.id = "kiro-renamed".to_string();
        }
    }
    new_config.upstreams.retain(|u| {
        !matches!(
            u.credential,
            UpstreamCredential::Account {
                provider: AccountProvider::Codex,
                ..
            }
        )
    });

    let removed = removed_account_bindings(&old, &new_config);
    assert_eq!(
        removed,
        vec![AccountUpstreamRef::new(AccountProvider::Codex, "codex-g2")]
    );

    app.save_proxy_config(new_config).await.expect("save");

    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);
    assert!(
        app.codex_accounts()
            .list_accounts()
            .await
            .unwrap()
            .is_empty(),
        "codex credential cascaded"
    );
    let final_config = read_config(&app).await;
    assert_eq!(count_account_bindings(&final_config), 1);
    assert_eq!(final_config.upstreams[0].id, "kiro-renamed");
}

/// H. 三 provider import 入口真实成功并创建 binding。
#[tokio::test]
async fn three_provider_import_entries_create_bindings() {
    let (dir, app) = open_app();

    let kiro_dir = dir.path().join("kiro-ide");
    std::fs::create_dir_all(&kiro_dir).unwrap();
    std::fs::write(
        kiro_dir.join("token.json"),
        r#"{"accessToken":"kiro-access","refreshToken":"kiro-refresh","expiresAt":"2999-01-01T00:00:00Z","email":"kiro@ex.com","provider":"Google"}"#,
    )
    .unwrap();
    let kiro = app.kiro_import_ide(kiro_dir).await.expect("kiro import");
    assert_eq!(kiro.len(), 1);

    let codex = app
        .codex_import_text(&codex_import_json("codex@ex.com", "codex-access"))
        .await
        .expect("codex import must succeed without seed fallback");
    assert_eq!(codex.len(), 1);

    let xai = app
        .xai_import_text(&xai_import_json("xai@ex.com", "xai-access"))
        .await
        .expect("xai import");
    assert_eq!(xai.len(), 1);

    let config = read_config(&app).await;
    assert_eq!(count_account_bindings(&config), 3);

    let again = reconcile_account_upstreams(
        &config,
        vec![
            AccountUpstreamRef::new(AccountProvider::Kiro, &kiro[0].account_id),
            AccountUpstreamRef::new(AccountProvider::Codex, &codex[0].account_id),
            AccountUpstreamRef::new(AccountProvider::Xai, &xai[0].account_id),
        ],
    )
    .expect("pure reconcile");
    assert!(!again.changed);
}

/// poll_login 会话缺失必须返回错误且 config 不变。
#[tokio::test]
async fn poll_login_missing_session_does_not_mutate_config() {
    let (_dir, app) = open_app();
    let before = serde_json::to_value(read_config(&app).await).unwrap();
    let err = match app.kiro_poll_login("missing-state").await {
        Ok(_) => panic!("missing session must err"),
        Err(message) => message,
    };
    assert!(
        err.to_ascii_lowercase().contains("not found")
            || err.to_ascii_lowercase().contains("session"),
        "unexpected error: {err}"
    );
    assert!(!err.contains("access_token"));
    let after = serde_json::to_value(read_config(&app).await).unwrap();
    assert_eq!(before, after);
}

/// 新登录：ensure/config 失败时删除新写入的 credential。
#[tokio::test]
async fn new_login_failure_removes_new_credential() {
    let (_dir, app) = open_app();
    let faults = app.lifecycle_faults();
    faults.reset();
    faults.fail_config_write.store(true, Ordering::SeqCst);

    app.kiro_login()
        .inject_prepared_login_for_test("login-new", kiro_record("new@ex.com", "login-new-tok"))
        .await;

    let err = match app.kiro_poll_login("login-new").await {
        Ok(_) => panic!("login commit must fail on config write"),
        Err(message) => message,
    };
    assert!(err.contains("injected config write failure"));
    assert!(!err.contains("login-new-tok"));

    let accounts = app.kiro_accounts().list_accounts().await.expect("list");
    assert!(
        accounts.is_empty(),
        "new login credential must be deleted after ensure failure"
    );
    assert_eq!(count_account_bindings(&read_config(&app).await), 0);
}

/// 重登录：覆盖已有 credential 后 config 失败，必须恢复旧 record。
#[tokio::test]
async fn re_login_failure_restores_old_credential() {
    let (_dir, app) = open_app();
    let seeded = app
        .xai_accounts()
        .save_record_for_test(
            "xai-relogin".to_string(),
            xai_record("re@ex.com", "old-login-tok"),
        )
        .await
        .expect("seed");
    assert_eq!(seeded.account_id, "xai-relogin");

    // 已有 binding：credential-only re-login 不会写 config；先清 binding 迫使 ensure 写 config。
    let mut config = read_config(&app).await;
    config.upstreams.clear();
    token_proxy_config::write_config(app.paths().as_ref(), config)
        .await
        .expect("clear");

    let faults = app.lifecycle_faults();
    faults.reset();
    faults.fail_config_write.store(true, Ordering::SeqCst);

    app.xai_login()
        .inject_prepared_login_for_test("login-re", xai_record("re@ex.com", "new-login-tok"))
        .await;

    let err = match app.xai_poll_login("login-re").await {
        Ok(_) => panic!("re-login ensure must fail"),
        Err(message) => message,
    };
    assert!(err.contains("injected config write failure"));
    assert!(!err.contains("old-login-tok"));
    assert!(!err.contains("new-login-tok"));

    let restored = app
        .xai_accounts()
        .snapshot_account_record("xai-relogin")
        .await
        .expect("snap")
        .expect("account");
    assert_eq!(restored.access_token, "old-login-tok");
    assert_eq!(count_account_bindings(&read_config(&app).await), 0);
}

/// 登录成功：三 provider 编排路径落库并创建 binding。
#[tokio::test]
async fn login_success_paths_commit_credential_and_binding() {
    let (_dir, app) = open_app();

    app.kiro_login()
        .inject_prepared_login_for_test("ok-kiro", kiro_record("ok-k@ex.com", "ok-kiro-tok"))
        .await;
    let kiro = app.kiro_poll_login("ok-kiro").await.expect("kiro login");
    assert_eq!(kiro.status, KiroLoginStatus::Success);
    assert!(kiro.account.is_some());

    app.codex_login()
        .inject_prepared_login_for_test("ok-codex", codex_record("ok-c@ex.com", "ok-codex-tok"))
        .await;
    let codex = app.codex_poll_login("ok-codex").await.expect("codex login");
    assert_eq!(codex.status, CodexLoginStatus::Success);
    assert!(codex.account.is_some());

    app.xai_login()
        .inject_prepared_login_for_test("ok-xai", xai_record("ok-x@ex.com", "ok-xai-tok"))
        .await;
    let xai = app.xai_poll_login("ok-xai").await.expect("xai login");
    assert_eq!(xai.status, XaiLoginStatus::Success);
    assert!(xai.account.is_some());

    assert_eq!(count_account_bindings(&read_config(&app).await), 3);
    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);
    assert_eq!(app.codex_accounts().list_accounts().await.unwrap().len(), 1);
    assert_eq!(app.xai_accounts().list_accounts().await.unwrap().len(), 1);
}

/// save no-op：语义相同不写、不删、不 apply。
#[tokio::test]
async fn save_proxy_config_noop_when_semantically_equal() {
    let (_dir, app) = open_app();
    app.kiro_accounts()
        .save_record_for_test("kiro-noop".to_string(), kiro_record("n@ex.com", "tn"))
        .await
        .unwrap();
    app.ensure_account_bindings_for_test(vec![AccountUpstreamRef::new(
        AccountProvider::Kiro,
        "kiro-noop",
    )])
    .await
    .unwrap();

    let config = read_config(&app).await;
    let faults = app.lifecycle_faults();
    faults.reset();

    let outcome = app
        .save_proxy_config(config.clone())
        .await
        .expect("noop save");
    assert!(!outcome.config_changed);
    assert_eq!(outcome.removed_credentials, 0);
    assert_eq!(faults.config_write_count.load(Ordering::SeqCst), 0);
    assert_eq!(faults.apply_count.load(Ordering::SeqCst), 0);
    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);
}

/// credential-only 重导入已有 binding：不 apply。
#[tokio::test]
async fn credential_only_reimport_skips_apply() {
    let (_dir, app) = open_app();
    let imported = app
        .xai_import_text(&xai_import_json("only@ex.com", "first-tok"))
        .await
        .expect("first import");
    assert_eq!(imported.len(), 1);

    let faults = app.lifecycle_faults();
    faults.reset();

    let again = app
        .xai_import_text(&xai_import_json("only@ex.com", "second-tok"))
        .await
        .expect("reimport");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].account_id, imported[0].account_id);
    assert_eq!(
        faults.config_write_count.load(Ordering::SeqCst),
        0,
        "existing binding reimport must not rewrite config"
    );
    assert_eq!(faults.apply_count.load(Ordering::SeqCst), 0);

    let record = app
        .xai_accounts()
        .snapshot_account_record(&again[0].account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.access_token, "second-tok");
}

/// config 变更成功只 apply 一次。
#[tokio::test]
async fn config_change_applies_once() {
    let (_dir, app) = open_app();
    let faults = app.lifecycle_faults();
    faults.reset();

    app.xai_import_text(&xai_import_json("once@ex.com", "once-tok"))
        .await
        .expect("import");
    assert_eq!(faults.config_write_count.load(Ordering::SeqCst), 1);
    assert_eq!(faults.apply_count.load(Ordering::SeqCst), 1);
}

/// Codex file import 编排路径。
#[tokio::test]
async fn codex_import_file_creates_binding() {
    let (dir, app) = open_app();
    let path = dir.path().join("codex-auth.json");
    std::fs::write(&path, codex_import_json("file@ex.com", "file-tok")).unwrap();
    let imported = app.codex_import_file(path).await.expect("file import");
    assert_eq!(imported.len(), 1);
    assert_eq!(count_account_bindings(&read_config(&app).await), 1);
}

/// xAI file import 编排路径。
#[tokio::test]
async fn xai_import_file_creates_binding() {
    let (dir, app) = open_app();
    let path = dir.path().join("xai-auth.json");
    std::fs::write(&path, xai_import_json("filex@ex.com", "filex-tok")).unwrap();
    let imported = app.xai_import_file(path).await.expect("file import");
    assert_eq!(imported.len(), 1);
    assert_eq!(count_account_bindings(&read_config(&app).await), 1);
}

async fn wait_critical_idle(app: &TokenProxyApp) {
    let faults = app.lifecycle_faults();
    for _ in 0..200 {
        if faults.critical_in_flight.load(Ordering::SeqCst) == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "critical mutation still in flight: {}",
        faults.critical_in_flight.load(Ordering::SeqCst)
    );
}

/// P0-1：取消/丢弃 poll future 后 worker 仍提交；下次 poll 得终态，无半状态。
#[tokio::test]
async fn login_poll_cancel_still_commits_or_recovers() {
    let (_dir, app) = open_app();
    let faults = app.lifecycle_faults();
    faults.reset();
    faults.delay_after_lock_ms.store(150, Ordering::SeqCst);

    app.kiro_login()
        .inject_prepared_login_for_test(
            "cancel-login",
            kiro_record("cancel@ex.com", "cancel-login-tok"),
        )
        .await;

    let app_task = app.clone();
    let handle = tokio::spawn(async move { app_task.kiro_poll_login("cancel-login").await });
    // 窗口内取消调用方等待；worker 必须继续。
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    handle.abort();
    let _ = handle.await;

    wait_critical_idle(&app).await;
    faults.delay_after_lock_ms.store(0, Ordering::SeqCst);

    // 下次 poll：应已 Success（worker 完成）或可恢复后成功。
    let poll = app
        .kiro_poll_login("cancel-login")
        .await
        .expect("recovering poll");
    assert_eq!(poll.status, KiroLoginStatus::Success);
    assert!(poll.account.is_some());
    if let Some(err) = poll.error.as_deref() {
        assert!(!err.contains("cancel-login-tok"));
    }

    let accounts = app.kiro_accounts().list_accounts().await.expect("list");
    assert_eq!(
        accounts.len(),
        1,
        "credential must exist after cancelled poll"
    );
    assert_eq!(count_account_bindings(&read_config(&app).await), 1);
    // 无半状态：有 binding 则必有 credential，且 config 可读。
    let config = read_config(&app).await;
    token_proxy_config::write_config(app.paths().as_ref(), config)
        .await
        .expect("config remains valid");
}

/// P0-2：cascade delete 途中取消调用方；worker 仍完成，无旧 config/无 credential 半状态。
#[tokio::test]
async fn save_proxy_config_cancel_still_completes_cascade() {
    let (_dir, app) = open_app();
    app.kiro_accounts()
        .save_record_for_test(
            "kiro-cancel-cascade".to_string(),
            kiro_record("casc@ex.com", "casc-tok"),
        )
        .await
        .unwrap();
    app.codex_accounts()
        .save_record_for_test(
            "codex-cancel-cascade".to_string(),
            codex_record("casc2@ex.com", "casc2-tok"),
        )
        .await
        .unwrap();
    app.ensure_account_bindings_for_test(vec![
        AccountUpstreamRef::new(AccountProvider::Kiro, "kiro-cancel-cascade"),
        AccountUpstreamRef::new(AccountProvider::Codex, "codex-cancel-cascade"),
    ])
    .await
    .unwrap();

    let old = read_config(&app).await;
    let mut new_config = old.clone();
    new_config.upstreams.retain(|u| {
        !matches!(
            u.credential,
            UpstreamCredential::Account {
                provider: AccountProvider::Codex,
                ..
            }
        )
    });
    assert_eq!(count_account_bindings(&new_config), 1);

    let faults = app.lifecycle_faults();
    faults.reset();
    faults.delay_after_lock_ms.store(150, Ordering::SeqCst);

    let app_task = app.clone();
    let handle = tokio::spawn(async move { app_task.save_proxy_config(new_config).await });
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    handle.abort();
    let _ = handle.await;

    wait_critical_idle(&app).await;
    faults.delay_after_lock_ms.store(0, Ordering::SeqCst);

    // worker 完成后：codex credential 已删，kiro 保留，config 只剩 1 binding。
    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);
    assert!(
        app.codex_accounts()
            .list_accounts()
            .await
            .unwrap()
            .is_empty(),
        "cascade delete must complete after caller cancel"
    );
    let final_config = read_config(&app).await;
    assert_eq!(count_account_bindings(&final_config), 1);
    // 无「新 config 无 credential」或「旧 config 无 credential」半状态。
    assert!(faults.critical_completed.load(Ordering::SeqCst) >= 1);
}

/// P1-3：provider gate 下并发 save 与 lifecycle restore 不丢更新、不半恢复。
#[tokio::test]
async fn provider_gate_serializes_save_and_lifecycle_restore() {
    let (_dir, app) = open_app();
    let store = app.kiro_accounts();
    store
        .save_record_for_test(
            "kiro-gate-1".to_string(),
            kiro_record("gate@ex.com", "gate-v1"),
        )
        .await
        .unwrap();

    let snapshot = store.snapshot_all_records().await.expect("snap");
    assert_eq!(snapshot.len(), 1);

    // 并发：A 覆盖 token；B 全量 restore 到 v1；最终 cache/DB 一致且只有合法完整态。
    let store_a = store.clone();
    let store_b = store.clone();
    let snap_b = snapshot.clone();
    let (r_save, r_restore) = tokio::join!(
        async move {
            store_a
                .save_record_for_test(
                    "kiro-gate-1".to_string(),
                    kiro_record("gate@ex.com", "gate-v2"),
                )
                .await
        },
        async move { store_b.restore_all_records(snap_b).await },
    );
    r_save.expect("save");
    r_restore.expect("restore");

    let final_record = store
        .snapshot_account_record("kiro-gate-1")
        .await
        .expect("snap")
        .expect("exists");
    // 串行后必为 v1 或 v2 之一，不得缺失或半写。
    assert!(
        final_record.access_token == "gate-v1" || final_record.access_token == "gate-v2",
        "token must be one complete value"
    );
    assert_eq!(store.list_accounts().await.unwrap().len(), 1);
}

/// C2：import transaction 从 snapshot 到 binding 失败 rollback 全程持 gate；
/// 无 sleep 确定性：并发 save 在持锁期间未完成，释放后写入，最终不被旧 snapshot 覆盖。
#[tokio::test]
async fn provider_import_transaction_holds_gate_until_binding_rollback() {
    let (dir, app) = open_app();
    let store = app.kiro_accounts();
    store
        .save_record_for_test(
            "kiro-gate-seed".to_string(),
            kiro_record("seed@ex.com", "gate-v1"),
        )
        .await
        .expect("seed");

    // IDE import 新账户，ensure binding 会写 config。
    let import_dir = dir.path().join("kiro-import-gate");
    std::fs::create_dir_all(&import_dir).expect("import dir");
    std::fs::write(
        import_dir.join("new.json"),
        r#"{"accessToken":"import-access","refreshToken":"import-refresh","email":"new@ex.com","provider":"Google","authMethod":"social","expiresAt":"2999-01-01T00:00:00Z"}"#,
    )
    .expect("write import fixture");

    let faults = app.lifecycle.faults();
    faults.fail_config_write.store(true, Ordering::SeqCst);

    // 第一枚 probe 只用于确认 import 已持 gate（begin 的 acquired）。
    let import_probe = store.install_provider_gate_probe();
    let import_acquired = import_probe.acquired.notified();

    let app_import = app.clone();
    let import_path = import_dir.clone();
    let import_handle = tokio::spawn(async move { app_import.kiro_import_ide(import_path).await });

    // import transaction 已持 provider_mutation。
    import_acquired.await;

    // 丢弃 import begin 可能残留的 Notify permit，换全新 probe 专测并发 save。
    // 旧 probe 的 about_to_lock 在无 waiter 时 notify_one 会留 permit，造成假阳性。
    store.clear_provider_gate_probe();
    let save_probe = store.install_provider_gate_probe();
    // waiter 必须在 spawn save 前建立，避免丢失 notify。
    let save_about_to_lock = save_probe.about_to_lock.notified();
    let mut save_acquired = std::pin::pin!(save_probe.acquired.notified());

    let (save_done_tx, mut save_done_rx) = tokio::sync::oneshot::channel();
    let store_save = store.clone();
    let save_handle = tokio::spawn(async move {
        let result = store_save
            .save_record_for_test(
                "kiro-gate-seed".to_string(),
                kiro_record("seed@ex.com", "gate-v2"),
            )
            .await;
        let _ = save_done_tx.send(result);
    });

    // 新 probe 上的 about_to_lock：证明并发 save 真实进入 acquire。
    save_about_to_lock.await;
    assert!(
        save_done_rx.try_recv().is_err(),
        "concurrent save must still be blocked while import transaction holds gate"
    );
    // transaction 结束前 save 不得 acquired（biased：先 poll acquired，Pending 则走 ready 分支）。
    tokio::select! {
        biased;
        _ = &mut save_acquired => {
            panic!("save must not acquire gate while import transaction holds it");
        }
        () = std::future::ready(()) => {}
    }

    // binding 失败 → rollback snapshot；import 返回 Err。
    let import_result = import_handle.await.expect("import join");
    let import_err = match import_result {
        Ok(_) => panic!("binding fault must fail import"),
        Err(err) => err,
    };
    assert!(
        import_err.contains("injected config write failure"),
        "unexpected import error: {import_err}"
    );

    // transaction drop 后 save 必须真正 acquired，再完成写入。
    save_acquired.await;
    let save_result = save_done_rx.await.expect("save oneshot");
    save_result.expect("concurrent save after gate release");
    save_handle.await.expect("save join");

    let seed = store
        .snapshot_account_record("kiro-gate-seed")
        .await
        .expect("snap")
        .expect("seed exists");
    assert_eq!(
        seed.access_token, "gate-v2",
        "concurrent save must win over rolled-back import snapshot"
    );

    // 导入的新账户应被 rollback 清掉；seed 保留。
    let accounts = store.list_accounts().await.expect("list");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].account_id, "kiro-gate-seed");

    // Codex / xAI 接线：begin_provider_mutation 可构造并立即释放（编译期接线验证）。
    let codex = app.codex_accounts();
    let codex_txn = codex.begin_provider_mutation().await;
    drop(codex_txn);
    let xai = app.xai_accounts();
    let xai_txn = xai.begin_provider_mutation().await;
    drop(xai_txn);

    store.clear_provider_gate_probe();
    faults.fail_config_write.store(false, Ordering::SeqCst);
}

/// Kiro 同身份重登录覆盖，不新建重复 credential；失败可恢复 previous。
#[tokio::test]
async fn kiro_re_login_overwrites_same_identity() {
    let (_dir, app) = open_app();
    let first = app
        .kiro_accounts()
        .save_record_for_test(
            "kiro-google-sameexcom.json".to_string(),
            kiro_record("same@ex.com", "kiro-old"),
        )
        .await
        .expect("seed");

    app.kiro_login()
        .inject_prepared_login_for_test("kiro-re", kiro_record("same@ex.com", "kiro-new"))
        .await;
    let poll = app.kiro_poll_login("kiro-re").await.expect("re-login");
    assert_eq!(poll.status, KiroLoginStatus::Success);
    let account = poll.account.expect("account");
    assert_eq!(account.account_id, first.account_id);
    assert_eq!(app.kiro_accounts().list_accounts().await.unwrap().len(), 1);

    let record = app
        .kiro_accounts()
        .snapshot_account_record(&account.account_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.access_token, "kiro-new");
}
