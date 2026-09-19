//! my_stats_api —— 只读行级统计 HTTP API 守护（fork 本地增强）。
//!
//! 恢复指南见 `crates/token_proxy_config/src/my_stats_api/MY-STATS-API-PATCHES.md`。
//! 职责：在 CLI/GUI 汇聚的装配层（`TokenProxyApp::open` 末尾）启动一个独立线程 +
//! 自建 current_thread tokio runtime 的 axum 服务，暴露三个只读端点：
//! `GET /max_id`、`GET /rows?afterId&limit`、`GET /attribution?cid`。
//! 设置与鉴权判定在 `token_proxy_config::my_stats_api`（token reload 即生效）；
//! 数据源为 `request_logs` 表本身（含错误行与 `is_billable=0` 行，过滤口径在消费方）。

use axum::{
    extract::{Request, Query, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use token_proxy_account_store::paths::TokenProxyPaths;
use token_proxy_config::my_stats_api::{auth_ok, current_settings};

const DEFAULT_ROW_LIMIT: i64 = 500;
const MAX_ROW_LIMIT: i64 = 2000;

/// PATCH 3 注入点：`TokenProxyApp::open` 末尾调用。
///
/// 使用 `std::thread::spawn` 独立线程 + 线程内自建 current_thread runtime——
/// `open` 是同步函数，GUI 侧在 tauri setup（无 tokio runtime 上下文）中调用，
/// `tokio::spawn` 会 panic；CLI 侧（tokio main）同样兼容。线程失败不 panic、
/// 不影响代理主功能。
pub fn spawn(paths: Arc<TokenProxyPaths>) {
    let spawned = std::thread::Builder::new()
        .name("my-stats-api".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(error = %err, "my-stats-api tokio runtime 创建失败，服务未启动");
                    return;
                }
            };
            runtime.block_on(serve(paths));
        });
    if spawned.is_err() {
        tracing::error!("my-stats-api 线程创建失败，服务未启动");
    }
}

async fn serve(paths: Arc<TokenProxyPaths>) {
    // open 返回时全局设置尚未同步（CLI 在 open 后才 read_config；GUI 更晚）——自行加载。
    let config = match token_proxy_config::read_config(&paths).await {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "my-stats-api 配置读取失败，服务未启动");
            return;
        }
    };
    let Some(settings) = token_proxy_config::my_stats_api::sync_settings(&config.config) else {
        tracing::debug!("my-stats-api 段缺失，服务未启动");
        return;
    };
    if !settings.enabled {
        tracing::debug!("my-stats-api 未启用（enabled=false），服务未启动");
        return;
    }
    let pool = match token_proxy_storage::sqlite::open_read_pool(&paths.sqlite_db_path()).await {
        Ok(pool) => pool,
        Err(err) => {
            tracing::error!(error = %err, "my-stats-api 读池打开失败，服务未启动");
            return;
        }
    };
    let router = Router::new()
        .route("/max_id", get(max_id))
        .route("/rows", get(rows))
        .route("/attribution", get(attribution))
        .layer(middleware::from_fn(auth_middleware))
        .with_state(pool);
    match tokio::net::TcpListener::bind((settings.host.as_str(), settings.port)).await {
        Ok(listener) => {
            tracing::info!(addr = %settings.host, port = settings.port, "my-stats-api 已启动");
            if let Err(err) = axum::serve(listener, router).await {
                tracing::error!(error = %err, "my-stats-api 服务异常退出");
            }
        }
        Err(err) => tracing::error!(error = %err, "my-stats-api 端口绑定失败，服务未启动"),
    }
}

async fn auth_middleware(req: Request, next: Next) -> Response {
    let settings = current_settings().unwrap_or_default();
    let provided = bearer_token(&req).or_else(|| query_token(&req));
    if !auth_ok(&settings, provided.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

fn bearer_token(req: &Request) -> Option<String> {
    req.headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

/// `?token=<value>` 兼容形式（注意会进访问日志，优先使用 Bearer 头）。
fn query_token(req: &Request) -> Option<String> {
    req.uri().query().and_then(|query| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == "token").then(|| value.to_string())
        })
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RowsQuery {
    after_id: Option<i64>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
struct AttributionQuery {
    cid: String,
}

/// 行结构对齐统计侧行映射需求；全部 token 分量与归因列按 `Option` 建模
/// （旧行 / 未回填行为 NULL，消费方按 0 计）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsRow {
    id: i64,
    ts_ms: i64,
    provider: String,
    upstream_id: String,
    model: Option<String>,
    mapped_model: Option<String>,
    status: i64,
    is_billable: bool,
    client_request_id: Option<String>,
    attempt_index: Option<i64>,
    uncached_input_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cache_write_5m_tokens: Option<i64>,
    cache_write_1h_tokens: Option<i64>,
    image_input_tokens: Option<i64>,
    image_output_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

fn map_row(row: sqlx::sqlite::SqliteRow) -> Option<StatsRow> {
    use sqlx::Row;
    Some(StatsRow {
        id: row.try_get::<i64, _>("id").ok()?,
        ts_ms: row.try_get::<i64, _>("ts_ms").ok()?,
        provider: row.try_get::<String, _>("provider").ok()?,
        upstream_id: row.try_get::<String, _>("upstream_id").ok()?,
        model: row.try_get::<Option<String>, _>("model").ok()?,
        mapped_model: row.try_get::<Option<String>, _>("mapped_model").ok()?,
        status: row.try_get::<i64, _>("status").unwrap_or(0),
        is_billable: row.try_get::<i64, _>("is_billable").map(|v| v == 1).unwrap_or(false),
        client_request_id: row.try_get::<Option<String>, _>("client_request_id").ok()?,
        attempt_index: row.try_get::<Option<i64>, _>("attempt_index").ok()?,
        uncached_input_tokens: row.try_get::<Option<i64>, _>("uncached_input_tokens").ok()?,
        cache_read_tokens: row.try_get::<Option<i64>, _>("cache_read_tokens").ok()?,
        cache_write_tokens: row.try_get::<Option<i64>, _>("cache_write_tokens").ok()?,
        cache_write_5m_tokens: row.try_get::<Option<i64>, _>("cache_write_5m_tokens").ok()?,
        cache_write_1h_tokens: row.try_get::<Option<i64>, _>("cache_write_1h_tokens").ok()?,
        image_input_tokens: row.try_get::<Option<i64>, _>("image_input_tokens").ok()?,
        image_output_tokens: row.try_get::<Option<i64>, _>("image_output_tokens").ok()?,
        output_tokens: row.try_get::<Option<i64>, _>("output_tokens").ok()?,
    })
}

async fn max_id(State(pool): State<sqlx::SqlitePool>) -> Response {
    match sqlx::query("SELECT COALESCE(MAX(id), 0) AS max_id FROM request_logs")
        .fetch_one(&pool)
        .await
    {
        Ok(row) => {
            let max_id: i64 = sqlx::Row::try_get(&row, "max_id").unwrap_or(0);
            Json(json!({ "maxId": max_id })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

async fn rows(
    State(pool): State<sqlx::SqlitePool>,
    Query(query): Query<RowsQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROW_LIMIT);
    let after_id = query.after_id.unwrap_or(0);
    let list = sqlx::query(
        "SELECT id, ts_ms, provider, upstream_id, model, mapped_model, status, is_billable, \
         client_request_id, attempt_index, uncached_input_tokens, cache_read_tokens, \
         cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, image_input_tokens, \
         image_output_tokens, output_tokens \
         FROM request_logs WHERE id > ?1 ORDER BY id LIMIT ?2",
    )
    .bind(after_id)
    .bind(limit)
    .fetch_all(&pool)
    .await;
    match list {
        Ok(list) => {
            let rows: Vec<StatsRow> = list.into_iter().filter_map(map_row).collect();
            // maxId 恒为源表当前 MAX(id)（与本批次行数无关），消费方拉空即知已追平。
            let max_id = sqlx::query("SELECT COALESCE(MAX(id), 0) AS max_id FROM request_logs")
                .fetch_one(&pool)
                .await
                .ok()
                .and_then(|row| sqlx::Row::try_get::<i64, _>(&row, "max_id").ok())
                .unwrap_or(0);
            Json(json!({ "maxId": max_id, "rows": rows })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

async fn attribution(
    State(pool): State<sqlx::SqlitePool>,
    Query(query): Query<AttributionQuery>,
) -> Response {
    let row = sqlx::query(
        "SELECT id, ts_ms, provider, upstream_id, model, mapped_model, status, is_billable, \
         client_request_id, attempt_index, uncached_input_tokens, cache_read_tokens, \
         cache_write_tokens, cache_write_5m_tokens, cache_write_1h_tokens, image_input_tokens, \
         image_output_tokens, output_tokens \
         FROM request_logs WHERE client_request_id = ?1 \
         ORDER BY attempt_index DESC, id DESC LIMIT 1",
    )
    .bind(&query.cid)
    .fetch_optional(&pool)
    .await;
    match row {
        Ok(row) => {
            let row = row.and_then(map_row);
            Json(json!({ "row": row })).into_response()
        }
        Err(err) => internal_error(err),
    }
}

fn internal_error(err: sqlx::Error) -> Response {
    tracing::error!(error = %err, "my-stats-api 查询失败");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "query failed"}))).into_response()
}
