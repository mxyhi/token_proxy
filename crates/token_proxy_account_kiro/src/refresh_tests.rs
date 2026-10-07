fn refresh_fixture() -> KiroTokenRecord {
    serde_json::from_value(json!({
        "access_token":"old", "refresh_token":"old-refresh", "expires_at":future_rfc3339(6),
        "auth_method":"social", "provider":"google", "status":"active"
    })).unwrap()
}

#[test]
fn late_kiro_refresh_cannot_overwrite_relogin_or_restore_deleted_account() {
    run_async(async {
        for deleted in [false, true] {
            let (store, data_dir) = create_test_store();
            let original = refresh_fixture();
            store.save_record("account".into(), original.clone()).await.unwrap();
            let result = store.refresh_record_with("account", original, |mut old| async {
                if deleted {
                    store.delete_account("account").await.unwrap();
                } else {
                    let mut imported = old.clone();
                    imported.access_token = "relogin".into();
                    store.save_record("account".into(), imported).await.unwrap();
                }
                old.access_token = "late-refresh".into();
                Ok(old)
            }).await;
            if deleted {
                assert!(result.is_err(), "deleted account must not be resurrected");
                assert!(store.load_account("account").await.is_err());
            } else {
                assert_eq!(result.unwrap().access_token, "relogin");
                assert_eq!(store.load_account("account").await.unwrap().access_token, "relogin");
            }
            let _ = std::fs::remove_dir_all(data_dir);
        }
    });
}

#[test]
fn concurrent_kiro_refresh_exchanges_once_and_cancel_releases_permit() {
    run_async(async {
        let (store, dir) = create_test_store();
        let store = Arc::new(store);
        let original = refresh_fixture();
        store.save_record("account".into(), original.clone()).await.unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let exchange = |mut record: KiroTokenRecord| async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::task::yield_now().await;
            record.access_token = "refreshed".into();
            Ok(record)
        };
        let (a,b) = tokio::join!(
            store.refresh_record_with("account", original.clone(), exchange),
            store.refresh_record_with("account", original, exchange),
        );
        assert_eq!(a.unwrap().access_token, "refreshed");
        assert_eq!(b.unwrap().access_token, "refreshed");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let entered = Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn({
            let store = store.clone(); let entered = entered.clone();
            async move {
                let current = store.load_account("account").await.unwrap();
                store.refresh_record_with("account", current, |_| async {
                    entered.notify_one();
                    std::future::pending::<Result<KiroTokenRecord, String>>().await
                }).await
            }
        });
        entered.notified().await;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(store.token_refreshing.lock().unwrap().is_empty());
        let current = store.load_account("account").await.unwrap();
        let final_record = store.refresh_record_with("account", current, |mut record| async {
            record.access_token = "after-cancel".into(); Ok(record)
        }).await.unwrap();
        assert_eq!(final_record.access_token, "after-cancel");
        let _ = std::fs::remove_dir_all(dir);
    });
}
