//! 仅本地性能夹具：真实 runtime、SQLite 和 socket；不访问供应商或价格服务。
use axum::{
    body::{Body, Bytes},
    response::Response,
    routing::{get, post},
    Router,
};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use token_proxy_account_store::{app_proxy, paths::TokenProxyPaths};
use token_proxy_config::{LogLevel, ProxyConfigFile};
use token_proxy_runtime::{
    logging::LoggingState,
    proxy::{
        request_detail::RequestDetailCapture,
        service::{ProxyContext, ProxyServiceHandle},
        token_rate::TokenRateTracker,
    },
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).ok_or("data directory required")?);
    let detail = args.get(2).is_some_and(|v| v == "on");
    let frames: usize = args.get(3).ok_or("frame count required")?.parse()?;
    let frame = Bytes::from(format!(
        "data: {}\n\n",
        serde_json::json!({"id":"perf","object":"chat.completion.chunk","model":"perf-model","choices":[{"index":0,"delta":{"content":"x".repeat(16_384)},"finish_reason":null}]})
    ));
    let end = Bytes::from_static(b"data: {\"id\":\"perf\",\"object\":\"chat.completion.chunk\",\"model\":\"perf-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\ndata: [DONE]\n\n");
    let expected_bytes = frame.len() * frames + end.len();
    let app = Router::new()
        .route(
            "/v1/chat/completions",
            post(move || {
                let frame = frame.clone();
                let end = end.clone();
                async move {
                    let stream = futures_util::stream::unfold(0usize, move |index| {
                        let frame = frame.clone();
                        let end = end.clone();
                        async move {
                            if index > frames {
                                return None;
                            }
                            // 给长流稳定的时间轴，同时避免 mock 自己积累正文。
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            Some((
                                Ok::<_, io::Error>(if index == frames { end } else { frame }),
                                index + 1,
                            ))
                        }
                    });
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }),
        )
        .route(
            "/v1/models",
            get(|| async {
                axum::Json(serde_json::json!({"data":[{"id":"perf-model","object":"model"}]}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let upstream_addr = listener.local_addr()?;
    let mock = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Service 的配置端口不支持回读动态端口，因此本地先预留再立即绑定。
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = reservation.local_addr()?.port();
    let paths = Arc::new(TokenProxyPaths::from_app_data_dir(dir.clone())?);
    let mut config = ProxyConfigFile {
        port,
        log_level: LogLevel::Error,
        ..Default::default()
    };
    config.upstreams.push(serde_json::from_value(serde_json::json!({"id":"perf","providers":["openai"],"base_url":format!("http://{upstream_addr}/v1"),"credential":{"type":"api_keys","api_keys":["local-perf"]},"enabled":true,"available_models":["perf-model"]}))?);
    tokio::fs::create_dir_all(&dir).await?;
    token_proxy_config::write_config(&paths, config).await?;
    let app_proxy = app_proxy::new_state();
    let capture = Arc::new(RequestDetailCapture::default());
    if detail {
        capture.arm();
    }
    let ctx = ProxyContext {
        paths: paths.clone(),
        logging: LoggingState::init(LogLevel::Error),
        request_detail: capture,
        token_rate: TokenRateTracker::new(),
        kiro_accounts: Arc::new(token_proxy_account_kiro::KiroAccountStore::new(
            &paths,
            app_proxy.clone(),
        )?),
        codex_accounts: Arc::new(token_proxy_account_codex::CodexAccountStore::new(
            &paths,
            app_proxy.clone(),
        )?),
        xai_accounts: Arc::new(token_proxy_account_xai::XaiAccountStore::new(
            &paths, app_proxy,
        )?),
    };
    let service = ProxyServiceHandle::new();
    drop(reservation);
    service.start(&ctx).await?;
    println!(
        "{}",
        serde_json::json!({"pid":std::process::id(),"port":port,"expected_bytes":expected_bytes})
    );
    io::stdout().flush()?;
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        io::stdin().read_line(&mut line)
    })
    .await??;
    service.stop().await?;
    mock.abort();
    Ok(())
}
