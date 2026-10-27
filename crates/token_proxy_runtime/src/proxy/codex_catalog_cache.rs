use reqwest::Client;
use serde_json::Value;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock};

pub(super) const CATALOG_URL: &str = "https://raw.githubusercontent.com/mxyhi/token_proxy/main/crates/token_proxy_runtime/resources/codex-models.json";
pub(super) const BUNDLED_CATALOG: &str = include_str!("../../resources/codex-models.json");
const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
// Codex 的完整响应限制为 1 MiB，给本地额外模型留出空间。
const MAX_CATALOG_BYTES: usize = 900 * 1024;

pub(super) struct CodexCatalogCache {
    url: Option<String>,
    catalog: RwLock<Value>,
    next_refresh: Mutex<Option<Instant>>,
}

impl CodexCatalogCache {
    pub(super) fn new(url: Option<String>) -> Self {
        Self {
            url,
            catalog: RwLock::new(
                parse_catalog(BUNDLED_CATALOG.as_bytes()).expect("valid bundled Codex catalog"),
            ),
            next_refresh: Mutex::new(None),
        }
    }

    pub(super) async fn get(&self, client: &Client) -> Value {
        if let Some(url) = &self.url {
            // 仅一个请求刷新；其余请求立即使用已有快照，不排队等待网络。
            if let Ok(mut next_refresh) = self.next_refresh.try_lock() {
                if next_refresh.is_none_or(|next| Instant::now() >= next) {
                    // 请求取消也保留退避，避免随后每个请求都重复拉取。
                    *next_refresh = Some(Instant::now() + RETRY_INTERVAL);
                    match fetch_catalog(client, url).await {
                        Ok(catalog) => {
                            let model_count = catalog["models"].as_array().map_or(0, Vec::len);
                            *self.catalog.write().await = catalog;
                            *next_refresh = Some(Instant::now() + REFRESH_INTERVAL);
                            tracing::info!(model_count, "refreshed Codex model catalog");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "Codex catalog refresh failed; retaining previous catalog")
                        }
                    }
                }
            }
        }
        self.catalog.read().await.clone()
    }
}

async fn fetch_catalog(client: &Client, url: &str) -> Result<Value, String> {
    // 固定公共资源地址，绝不转发客户端 Key、OAuth 或其它入站 header。
    let mut response = client
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("catalog request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("catalog HTTP error: {e}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_CATALOG_BYTES as u64)
    {
        return Err("catalog exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("catalog body failed: {e}"))?
    {
        if bytes.len() + chunk.len() > MAX_CATALOG_BYTES {
            return Err("catalog exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_catalog(&bytes)
}

fn parse_catalog(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_CATALOG_BYTES {
        return Err("catalog exceeds size limit".into());
    }
    let catalog: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid catalog JSON: {e}"))?;
    let models = catalog
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty())
        .ok_or("catalog requires nonempty models")?;
    let mut ids = HashSet::new();
    for model in models {
        let slug = model["slug"]
            .as_str()
            .filter(|id| !id.trim().is_empty() && id.len() <= 256)
            .ok_or("invalid catalog model ID")?;
        if !ids.insert(slug) {
            return Err("duplicate catalog model ID".into());
        }
        for key in ["display_name", "description", "shell_type", "visibility"] {
            if !model[key].is_string() {
                return Err(format!("catalog model missing {key}"));
            }
        }
        if !model["supported_in_api"].is_boolean() || model["priority"].as_i64().is_none() {
            return Err("invalid catalog model availability or priority".into());
        }
        let levels = model["supported_reasoning_levels"]
            .as_array()
            .ok_or("invalid catalog reasoning levels")?;
        if levels
            .iter()
            .any(|level| !level["effort"].is_string() || !level["description"].is_string())
        {
            return Err("invalid catalog reasoning level".into());
        }
        // Codex 会拒绝缺少指令模板的整个目录，不能将此类更新替换进缓存。
        if !model["model_messages"]["instructions_template"]
            .as_str()
            .or_else(|| model["base_instructions"].as_str())
            .is_some_and(|s| !s.trim().is_empty())
        {
            return Err("catalog model missing instructions".into());
        }
    }
    Ok(catalog)
}

#[cfg(test)]
#[path = "codex_catalog_cache.test.rs"]
mod tests;
