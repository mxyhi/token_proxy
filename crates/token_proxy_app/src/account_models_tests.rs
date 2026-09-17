use super::*;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn account_requests_use_official_endpoints_and_account_identity() {
    let client = Client::new();
    let codex = codex_models_request(&client, "Bearer test-token", Some("test-account"))
        .build()
        .unwrap();
    assert_eq!(codex.url().host_str(), Some("chatgpt.com"));
    assert_eq!(codex.url().path(), "/backend-api/codex/models");
    assert!(codex
        .url()
        .query_pairs()
        .any(|(key, value)| key == "client_version"
            && value == codex::enforce_minimum_client_version("")));
    assert_eq!(codex.headers()["chatgpt-account-id"], "test-account");
    assert_eq!(codex.headers()["Authorization"], "Bearer test-token");
    assert_eq!(codex.headers()["Accept"], "application/json");

    let agent = codex_models_request(&client, "AgentAssertion test-assertion", None)
        .build()
        .unwrap();
    assert_eq!(
        agent.headers()["Authorization"],
        "AgentAssertion test-assertion"
    );
    assert!(!agent.headers().contains_key("chatgpt-account-id"));

    let kiro = kiro_models_request(
        &client,
        "test-token",
        Some("arn:aws:codewhisperer:eu-west-1:123:profile/test"),
        Some("us-east-1"),
    )
    .unwrap()
    .build()
    .unwrap();
    assert_eq!(kiro.url().host_str(), Some("q.eu-west-1.amazonaws.com"));
    assert_eq!(kiro.url().path(), "/ListAvailableModels");
    assert!(kiro
        .url()
        .query_pairs()
        .any(|(key, value)| key == "origin" && value == "AI_EDITOR"));
    assert!(kiro
        .url()
        .query_pairs()
        .any(|(key, value)| key == "profileArn" && value.ends_with("profile/test")));
    assert!(kiro_models_request(&client, "test-token", None, Some("evil.com/")).is_err());

    let xai = xai_models_request(&client, "test-token").build().unwrap();
    assert_eq!(
        xai.url().as_str(),
        "https://cli-chat-proxy.grok.com/v1/models"
    );
    assert_eq!(
        xai.headers()[xai::CLI_TOKEN_AUTH_HEADER],
        xai::CLI_TOKEN_AUTH_VALUE
    );
    assert_eq!(xai.headers()["Authorization"], "Bearer test-token");
}

#[test]
fn catalog_ids_use_provider_protocol_fields_instead_of_display_names() {
    assert_eq!(
        extract_account_model_ids(
            "codex",
            &json!({"models": [
                {"slug": " gpt-live ", "display_name": "GPT Live"}, {"slug": ""}
            ]})
        ),
        ["gpt-live"]
    );
    assert_eq!(
        extract_account_model_ids(
            "kiro",
            &json!({"models": [
                {"modelId": "claude-live", "modelName": "Claude Live"}
            ]})
        ),
        ["claude-live"]
    );
    assert_eq!(
        extract_account_model_ids(
            "xai",
            &json!({"data": [
                {"model": "grok-live", "id": "display-id", "name": "Grok Live"},
                {"_meta": {"modelId": "grok-meta"}, "name": "Grok Meta"},
                {"model_id": "grok-id"}, {"name": "grok-fallback"}
            ]})
        ),
        ["grok-live", "grok-meta", "grok-id", "grok-fallback"]
    );
}

#[test]
fn account_model_proxy_matches_channel_override_and_app_fallback() {
    let app_proxy = Some("http://127.0.0.1:7890");
    for channel_proxy in [None, Some(""), Some(" $app_proxy_url ")] {
        assert_eq!(
            resolve_model_proxy(channel_proxy, app_proxy)
                .unwrap()
                .as_deref(),
            app_proxy
        );
    }
    assert_eq!(
        resolve_model_proxy(Some("socks5://127.0.0.1:1080"), app_proxy)
            .unwrap()
            .as_deref(),
        Some("socks5://127.0.0.1:1080")
    );
    assert!(resolve_model_proxy(Some("$app_proxy_url"), None)
        .unwrap()
        .is_none());
    assert!(resolve_model_proxy(Some("file:///tmp/proxy"), app_proxy).is_err());
}

// 用本地 HTTP 回读实际请求，覆盖分页与错误边界，不访问真实账户或供应商。
async fn serve_catalog(
    pages: Vec<(u16, &'static str)>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/models", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in pages {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            requests.push(String::from_utf8(request).unwrap());
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, server)
}

#[tokio::test]
async fn live_catalog_follows_kiro_pages_and_deduplicates() {
    let (url, server) = serve_catalog(vec![
        (
            200,
            r#"{"models":[{"modelId":"model-b"}],"nextToken":"page 2"}"#,
        ),
        (
            200,
            r#"{"models":[{"modelId":"model-a"},{"modelId":"model-b"}]}"#,
        ),
    ])
    .await;
    let models = fetch_model_pages(
        "kiro",
        Client::new().get(url).query(&[("origin", "AI_EDITOR")]),
    )
    .await
    .unwrap();
    assert_eq!(models, ["model-a", "model-b"]);
    let requests = server.await.unwrap();
    assert!(!requests[0].contains("nextToken="));
    assert!(requests[1].contains("origin=AI_EDITOR&nextToken=page+2"));
}

#[tokio::test]
async fn failed_catalog_does_not_return_static_models_or_provider_body() {
    let (url, server) = serve_catalog(vec![(403, "private-provider-response")]).await;
    let error = fetch_model_pages("codex", Client::new().get(url))
        .await
        .unwrap_err();
    assert!(error.contains("403"));
    assert!(!error.contains("private-provider-response"));
    server.await.unwrap();
}

#[tokio::test]
async fn repeated_kiro_page_token_fails_instead_of_hanging() {
    let page = (200, r#"{"models":[],"nextToken":"repeated"}"#);
    let (url, server) = serve_catalog(vec![page, page]).await;
    assert!(fetch_model_pages("kiro", Client::new().get(url))
        .await
        .unwrap_err()
        .contains("repeated"));
    server.await.unwrap();
}
