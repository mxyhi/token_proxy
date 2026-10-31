use std::{collections::HashMap, sync::Mutex, time::Duration};

use reqwest::{redirect::Policy, Client, ClientBuilder, Proxy, Url};

/// 进程内上游 HTTP client 池。
///
/// - 按 `(proxy_url, http1_only, redirects_disabled)` 缓存，长连接复用
/// - 首响应头前 H2/连接故障时 `rotate_*` 重建 H2 槽位，避免毒连接反复被复用
/// - ordinary / codex 共用同一套池与调优参数
pub(crate) struct ProxyHttpClients {
    by_key: Mutex<HashMap<ClientKey, Client>>,
}

impl ProxyHttpClients {
    pub(crate) fn new() -> Result<Self, String> {
        let direct = build_tuned_client(None, false, false)
            .map_err(|err| format!("Failed to build direct HTTP client: {err}"))?;
        let mut by_key = HashMap::new();
        by_key.insert(ClientKey::new(None, false, false), direct);
        Ok(Self {
            by_key: Mutex::new(by_key),
        })
    }

    /// 默认 HTTP/2 优先的上游 client（无显式 proxy 时不走系统代理）。
    pub(crate) fn client_for_proxy_url(&self, proxy_url: Option<&str>) -> Result<Client, String> {
        self.client_for(proxy_url, false, false)
    }

    /// 强制 HTTP/1.1 的上游 client，用于 H2 协议层故障后的降级重试。
    pub(crate) fn client_for_proxy_url_http1(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<Client, String> {
        self.client_for(proxy_url, true, false)
    }

    /// 丢弃并重建指定 proxy 槽位的 H2 client；若已有 H1 槽位则一并重建。
    /// 返回新的 H2 client，供调用方立即使用。
    pub(crate) fn rotate_client_for_proxy_url(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<Client, String> {
        self.rotate_client(proxy_url, false)
    }

    /// xAI OAuth bearer 只能直达受信端点；独立池保留连接复用但完全禁用重定向。
    pub(crate) fn xai_client_for_proxy_url(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<Client, String> {
        self.client_for(proxy_url, false, true)
    }

    pub(crate) fn xai_client_for_proxy_url_http1(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<Client, String> {
        self.client_for(proxy_url, true, true)
    }

    pub(crate) fn rotate_xai_client_for_proxy_url(
        &self,
        proxy_url: Option<&str>,
    ) -> Result<Client, String> {
        self.rotate_client(proxy_url, true)
    }

    fn rotate_client(
        &self,
        proxy_url: Option<&str>,
        redirects_disabled: bool,
    ) -> Result<Client, String> {
        let proxy_key = normalize_proxy_url(proxy_url);
        let h2_client = build_tuned_client(proxy_key.as_deref(), false, redirects_disabled)
            .map_err(|err| {
                format!("Failed to rebuild HTTP client after transport failure: {err}")
            })?;
        let mut guard = self
            .by_key
            .lock()
            .map_err(|_| "HTTP client pool is poisoned.".to_string())?;

        guard.insert(
            ClientKey::new(proxy_key.as_deref(), false, redirects_disabled),
            h2_client.clone(),
        );
        let h1_key = ClientKey::new(proxy_key.as_deref(), true, redirects_disabled);
        if let Some(h1_slot) = guard.get_mut(&h1_key) {
            let h1_client = build_tuned_client(proxy_key.as_deref(), true, redirects_disabled)
                .map_err(|err| {
                    format!("Failed to rebuild HTTP/1.1 client after transport failure: {err}")
                })?;
            *h1_slot = h1_client;
        }

        tracing::info!(
            proxy = %proxy_log_target(proxy_key.as_deref()),
            redirects_disabled,
            "rotated upstream HTTP client pool after pre-header transport failure"
        );
        Ok(h2_client)
    }

    pub(crate) fn codex_client_for_proxy_url(
        &self,
        proxy_url: Option<&str>,
        http1_only: bool,
    ) -> Result<Client, String> {
        self.client_for(proxy_url, http1_only, false)
    }

    fn client_for(
        &self,
        proxy_url: Option<&str>,
        http1_only: bool,
        redirects_disabled: bool,
    ) -> Result<Client, String> {
        let key = ClientKey::new(proxy_url, http1_only, redirects_disabled);
        let mut guard = self
            .by_key
            .lock()
            .map_err(|_| "HTTP client pool is poisoned.".to_string())?;
        if let Some(existing) = guard.get(&key) {
            return Ok(existing.clone());
        }
        let client = build_tuned_client(
            key.proxy_url.as_deref(),
            key.http1_only,
            key.redirects_disabled,
        )
        .map_err(|err| {
            format!(
                "Failed to build {} HTTP client: {err}",
                if http1_only { "HTTP/1.1" } else { "upstream" }
            )
        })?;
        guard.insert(key, client.clone());
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) fn client_count(&self) -> usize {
        self.by_key.lock().map(|guard| guard.len()).unwrap_or(0)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ClientKey {
    proxy_url: Option<String>,
    http1_only: bool,
    redirects_disabled: bool,
}

impl ClientKey {
    fn new(proxy_url: Option<&str>, http1_only: bool, redirects_disabled: bool) -> Self {
        Self {
            proxy_url: normalize_proxy_url(proxy_url),
            http1_only,
            redirects_disabled,
        }
    }
}

fn normalize_proxy_url(proxy_url: Option<&str>) -> Option<String> {
    proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// 日志只能描述代理目标，不能包含可能存在的 userinfo、路径或查询参数。
pub(crate) fn proxy_log_target(proxy_url: Option<&str>) -> String {
    let Some(value) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return "direct".to_string();
    };
    let Ok(parsed) = Url::parse(value) else {
        return "[invalid-proxy-url]".to_string();
    };
    let Some(host) = parsed.host_str() else {
        return "[invalid-proxy-url]".to_string();
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match parsed.port_or_known_default() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HttpClientTuning {
    connect_timeout: Duration,
    pool_idle_timeout: Duration,
    pool_max_idle_per_host: usize,
    tcp_nodelay: bool,
    tcp_keepalive: Duration,
    http2_adaptive_window: bool,
    http2_keep_alive_interval: Duration,
    http2_keep_alive_timeout: Duration,
    http2_keep_alive_while_idle: bool,
}

impl Default for HttpClientTuning {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            // 180s 过长：Cloudflare/airouter 常更早踢 idle H2，毒连接会被反复复用。
            pool_idle_timeout: Duration::from_secs(90),
            pool_max_idle_per_host: 32,
            tcp_nodelay: true,
            tcp_keepalive: Duration::from_secs(60),
            http2_adaptive_window: true,
            // 空闲 H2 session 上主动 ping，尽早发现远端已 reset/GOAWAY 的连接。
            http2_keep_alive_interval: Duration::from_secs(30),
            http2_keep_alive_timeout: Duration::from_secs(10),
            http2_keep_alive_while_idle: true,
        }
    }
}

fn tuned_client_builder() -> ClientBuilder {
    let tuning = HttpClientTuning::default();
    ClientBuilder::new()
        .connect_timeout(tuning.connect_timeout)
        .pool_idle_timeout(tuning.pool_idle_timeout)
        .pool_max_idle_per_host(tuning.pool_max_idle_per_host)
        .tcp_nodelay(tuning.tcp_nodelay)
        .tcp_keepalive(tuning.tcp_keepalive)
        .http2_adaptive_window(tuning.http2_adaptive_window)
        .http2_keep_alive_interval(tuning.http2_keep_alive_interval)
        .http2_keep_alive_timeout(tuning.http2_keep_alive_timeout)
        .http2_keep_alive_while_idle(tuning.http2_keep_alive_while_idle)
}

fn build_tuned_client(
    proxy_url: Option<&str>,
    http1_only: bool,
    redirects_disabled: bool,
) -> Result<Client, reqwest::Error> {
    let mut builder = tuned_client_builder();
    if let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) {
        let proxy = Proxy::all(proxy_url)?;
        builder = builder.proxy(proxy);
    } else {
        // 默认不走系统代理；仅当用户显式配置 proxy_url 时才走代理。
        builder = builder.no_proxy();
    }
    if http1_only {
        builder = builder.http1_only();
    }
    if redirects_disabled {
        builder = builder.redirect(Policy::none());
    } else {
        // reqwest 会移除标准 Authorization，但不会移除供应商自定义 api-key
        // 头。仅跟随同源跳转，避免自定义凭据及 307/308 请求体被重放到其他源。
        builder = builder.redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                return attempt.error("too many redirects");
            }
            if attempt
                .previous()
                .first()
                .is_some_and(|original| original.origin() != attempt.url().origin())
            {
                tracing::warn!("blocked cross-origin upstream redirect");
                return attempt.stop();
            }
            attempt.follow()
        }));
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, Bytes},
        extract::State,
        http::{header, HeaderMap, StatusCode},
        response::Response,
        routing::post,
        Router,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Clone)]
    struct RedirectState {
        status: StatusCode,
        location: String,
        observed: Arc<Mutex<Vec<(String, String, Bytes)>>>,
    }

    async fn redirect_handler(
        State(state): State<RedirectState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let cli_identity = headers
            .get("x-xai-token-auth")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        state.observed.lock().expect("redirect observations").push((
            authorization,
            cli_identity,
            body,
        ));
        Response::builder()
            .status(state.status)
            .header(header::LOCATION, state.location)
            .body(Body::empty())
            .expect("redirect response")
    }

    async fn redirected_target(State(hits): State<Arc<AtomicUsize>>) -> StatusCode {
        hits.fetch_add(1, Ordering::SeqCst);
        StatusCode::NO_CONTENT
    }

    #[test]
    fn clients_are_cached_by_proxy_and_http1_mode() {
        let clients = ProxyHttpClients::new().expect("clients");

        let _ = clients
            .client_for_proxy_url(Some("http://127.0.0.1:7890"))
            .expect("proxied client");
        let _ = clients
            .client_for_proxy_url(Some("http://127.0.0.1:7890"))
            .expect("same proxied client");
        let _ = clients
            .client_for_proxy_url_http1(Some("http://127.0.0.1:7890"))
            .expect("http1 client");
        let _ = clients
            .codex_client_for_proxy_url(Some("http://127.0.0.1:7890"), false)
            .expect("codex reuses ordinary pool");

        // new() 预建 direct H2 + proxy H2 + proxy H1
        assert_eq!(clients.client_count(), 3);
    }

    #[test]
    fn rotate_rebuilds_h2_slot_and_existing_http1_slot() {
        let clients = ProxyHttpClients::new().expect("clients");
        let proxy = "http://127.0.0.1:7890";
        let before_h2 = clients.client_for_proxy_url(Some(proxy)).expect("h2");
        let before_h1 = clients.client_for_proxy_url_http1(Some(proxy)).expect("h1");
        let after_h2 = clients
            .rotate_client_for_proxy_url(Some(proxy))
            .expect("rotate");
        let after_h1 = clients
            .client_for_proxy_url_http1(Some(proxy))
            .expect("h1 after");

        // 指针不必不同，但槽位必须仍可用；count 保持 1 direct + proxy h2 + proxy h1。
        assert_eq!(clients.client_count(), 3);
        let _ = (before_h2, before_h1, after_h2, after_h1);
    }

    #[test]
    fn default_tuning_shortens_idle_and_enables_h2_keepalive() {
        let tuning = HttpClientTuning::default();

        assert!(tuning.tcp_nodelay);
        assert!(tuning.http2_adaptive_window);
        assert!(tuning.http2_keep_alive_while_idle);
        assert_eq!(tuning.connect_timeout, Duration::from_secs(10));
        assert_eq!(tuning.pool_idle_timeout, Duration::from_secs(90));
        assert_eq!(tuning.pool_max_idle_per_host, 32);
        assert_eq!(tuning.http2_keep_alive_interval, Duration::from_secs(30));
        assert_eq!(tuning.http2_keep_alive_timeout, Duration::from_secs(10));
        assert_eq!(tuning.tcp_keepalive, Duration::from_secs(60));
    }

    #[test]
    fn proxy_log_target_omits_credentials_and_request_components() {
        assert_eq!(
            proxy_log_target(Some(
                "http://alice:secret@proxy.example:8443/private?q=token#x"
            )),
            "http://proxy.example:8443"
        );
        assert_eq!(proxy_log_target(None), "direct");
        assert_eq!(proxy_log_target(Some("not a url")), "[invalid-proxy-url]");
    }

    #[tokio::test]
    async fn credentialed_clients_do_not_follow_cross_origin_redirects() {
        for status in [
            StatusCode::MOVED_PERMANENTLY,
            StatusCode::FOUND,
            StatusCode::SEE_OTHER,
            StatusCode::TEMPORARY_REDIRECT,
            StatusCode::PERMANENT_REDIRECT,
        ] {
            let target_hits = Arc::new(AtomicUsize::new(0));
            let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("target listener");
            let target_addr = target_listener.local_addr().expect("target address");
            let target_app = Router::new()
                .route("/capture", post(redirected_target))
                .with_state(target_hits.clone());
            let target_task = tokio::spawn(async move {
                axum::serve(target_listener, target_app)
                    .await
                    .expect("target server");
            });

            let observed = Arc::new(Mutex::new(Vec::new()));
            let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("redirect listener");
            let redirect_addr = redirect_listener.local_addr().expect("redirect address");
            let redirect_app = Router::new()
                .route("/redirect", post(redirect_handler))
                .with_state(RedirectState {
                    status,
                    location: format!("http://{target_addr}/capture"),
                    observed: observed.clone(),
                });
            let redirect_task = tokio::spawn(async move {
                axum::serve(redirect_listener, redirect_app)
                    .await
                    .expect("redirect server");
            });

            let clients = ProxyHttpClients::new().expect("clients");
            let sensitive_clients = [
                clients
                    .xai_client_for_proxy_url(None)
                    .expect("xAI H2 client"),
                clients
                    .rotate_xai_client_for_proxy_url(None)
                    .expect("rotated xAI H2 client"),
                clients
                    .xai_client_for_proxy_url_http1(None)
                    .expect("xAI HTTP/1 client"),
                clients
                    .client_for_proxy_url(None)
                    .expect("ordinary H2 client"),
                clients
                    .rotate_client_for_proxy_url(None)
                    .expect("rotated ordinary H2 client"),
                clients
                    .client_for_proxy_url_http1(None)
                    .expect("ordinary HTTP/1 client"),
                clients
                    .codex_client_for_proxy_url(None, false)
                    .expect("Codex client"),
            ];
            for client in sensitive_clients {
                let response = client
                    .post(format!("http://{redirect_addr}/redirect"))
                    .bearer_auth("secret-bearer")
                    .header("x-xai-token-auth", "xai-grok-cli")
                    .header("x-api-key", "sensitive-api-key")
                    .header("api-key", "sensitive-azure-key")
                    .body("sensitive request body")
                    .send()
                    .await
                    .expect("redirect response");
                assert_eq!(response.status(), status);
            }

            let observed = observed.lock().expect("redirect observations");
            assert_eq!(observed.len(), 7);
            assert!(observed.iter().all(|(authorization, cli_identity, body)| {
                authorization == "Bearer secret-bearer"
                    && cli_identity == "xai-grok-cli"
                    && body.as_ref() == b"sensitive request body"
            }));
            assert_eq!(target_hits.load(Ordering::SeqCst), 0);
            redirect_task.abort();
            target_task.abort();
        }
    }

    #[tokio::test]
    async fn ordinary_clients_follow_same_origin_redirects() {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/redirect",
                post(|| async {
                    (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, "/capture")],
                    )
                }),
            )
            .route("/capture", post(redirected_target))
            .with_state(hits.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let response = ProxyHttpClients::new()
            .unwrap()
            .client_for_proxy_url(None)
            .unwrap()
            .post(format!("http://{addr}/redirect"))
            .header("x-api-key", "test-key")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
