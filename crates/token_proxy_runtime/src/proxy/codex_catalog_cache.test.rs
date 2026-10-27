use super::*;
use axum::{routing::get, Router};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[tokio::test]
async fn refresh_caches_valid_catalog_and_keeps_it_after_invalid_updates() {
    let hits = Arc::new(AtomicUsize::new(0));
    let mut changed: Value = serde_json::from_str(BUNDLED_CATALOG).unwrap();
    changed["models"][0]["display_name"] = "Updated from GitHub".into();
    changed["models"][0]["future_metadata"] = serde_json::json!({"preserve": true});
    let body = Arc::new(RwLock::new(changed.to_string()));
    let app = Router::new().route(
        "/models.json",
        get({
            let hits = hits.clone();
            let body = body.clone();
            move |headers: axum::http::HeaderMap| {
                let hits = hits.clone();
                let body = body.clone();
                async move {
                    assert!(!headers.contains_key("authorization"));
                    hits.fetch_add(1, Ordering::SeqCst);
                    body.read().await.clone()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/models.json", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cache = CodexCatalogCache::new(Some(url));
    let client = Client::builder().no_proxy().build().unwrap();
    assert_eq!(cache.get(&client).await, changed);
    assert_eq!(cache.get(&client).await, changed);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "fresh cache skips network");
    *body.write().await = "{\"models\":[]}".into();
    *cache.next_refresh.lock().await = None;
    assert_eq!(
        cache.get(&client).await,
        changed,
        "invalid update retains last good metadata"
    );
    assert_eq!(cache.get(&client).await, changed);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "failure applies retry backoff"
    );
    let guard = cache.next_refresh.lock().await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(100), cache.get(&client))
            .await
            .unwrap(),
        changed,
        "concurrent reader does not wait for refresh"
    );
    drop(guard);
    server.abort();
    *cache.next_refresh.lock().await = None;
    assert_eq!(
        cache.get(&client).await,
        changed,
        "network failure retains last good catalog"
    );
    let offline = CodexCatalogCache::new(cache.url.clone());
    assert_eq!(
        offline.get(&client).await,
        parse_catalog(BUNDLED_CATALOG.as_bytes()).unwrap()
    );
}

#[test]
fn validates_remote_catalog_before_replacing_cache() {
    let base = parse_catalog(BUNDLED_CATALOG.as_bytes()).unwrap();
    let mut duplicate = base.clone();
    let first = duplicate["models"][0].clone();
    duplicate["models"].as_array_mut().unwrap().push(first);
    assert!(parse_catalog(duplicate.to_string().as_bytes()).is_err());
    for key in [
        "slug",
        "display_name",
        "supported_reasoning_levels",
        "priority",
        "supported_in_api",
    ] {
        let mut broken = base.clone();
        broken["models"][0][key] = Value::Null;
        assert!(
            parse_catalog(broken.to_string().as_bytes()).is_err(),
            "{key}"
        );
    }
    let mut broken = base.clone();
    broken["models"][0]["model_messages"] = Value::Null;
    broken["models"][0]["base_instructions"] = Value::Null;
    assert!(parse_catalog(broken.to_string().as_bytes()).is_err());
    assert!(parse_catalog(&vec![b' '; MAX_CATALOG_BYTES + 1]).is_err());
    assert!(parse_catalog(b"not JSON").is_err());
    let visible: Vec<_> = base["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["visibility"] == "list")
        .map(|m| m["slug"].as_str().unwrap())
        .collect();
    assert_eq!(
        visible,
        [
            "gpt-6.1-sol",
            "gpt-6.1-terra",
            "gpt-6-astra",
            "gpt-6-sol",
            "gpt-6-luna",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna"
        ]
    );
    assert!(!base["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["slug"] == "gpt-5.5"));
}

#[tokio::test]
async fn download_rejects_http_errors_oversized_streams_and_slow_servers() {
    use axum::{
        body::{Body, Bytes},
        http::StatusCode,
        response::Response,
    };
    let app = Router::new()
        .route("/error", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
        .route(
            "/oversized",
            get(|| async {
                // 不提供 Content-Length，验证实际流式读取仍受大小上限约束。
                Response::new(Body::from_stream(futures_util::stream::iter([
                    Ok::<_, std::io::Error>(Bytes::from(vec![b' '; MAX_CATALOG_BYTES])),
                    Ok(Bytes::from_static(b"x")),
                ])))
            }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                BUNDLED_CATALOG
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::builder().no_proxy().build().unwrap();
    assert!(fetch_catalog(&client, &format!("{base}/error"))
        .await
        .is_err());
    assert!(fetch_catalog(&client, &format!("{base}/oversized"))
        .await
        .unwrap_err()
        .contains("size limit"));
    assert!(tokio::time::timeout(
        Duration::from_secs(5),
        fetch_catalog(&client, &format!("{base}/slow"))
    )
    .await
    .expect("download must time out within its budget")
    .is_err());
    server.abort();
}
