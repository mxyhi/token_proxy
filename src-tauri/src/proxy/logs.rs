use serde::Serialize;
use sqlx::Row;
use tauri::AppHandle;

use super::sqlite;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestLogDetail {
    pub(crate) id: u64,
    pub(crate) request_headers: Option<String>,
    pub(crate) request_body: Option<String>,
    pub(crate) response_error: Option<String>,
}

pub(crate) async fn read_request_log_detail(
    app: AppHandle,
    id: u64,
) -> Result<RequestLogDetail, String> {
    let pool = sqlite::open_read_pool(&app).await?;
    let row = sqlx::query(
        r#"
SELECT
  id,
  request_headers,
  request_body,
  response_error
FROM request_logs
WHERE id = ?
LIMIT 1;
"#,
    )
    .bind(id as i64)
    .fetch_optional(&pool)
    .await
    .map_err(|err| format!("Failed to query request log detail: {err}"))?;

    let Some(row) = row else {
        return Err("Request log not found.".to_string());
    };

    let id = row.try_get::<i64, _>("id").unwrap_or_default();
    let request_headers = row.try_get::<Option<String>, _>("request_headers").ok().flatten();
    let request_body = row.try_get::<Option<String>, _>("request_body").ok().flatten();
    let response_error = row.try_get::<Option<String>, _>("response_error").ok().flatten();

    Ok(RequestLogDetail {
        id: id.max(0) as u64,
        request_headers,
        request_body,
        response_error,
    })
}
