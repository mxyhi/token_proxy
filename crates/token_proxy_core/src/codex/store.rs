use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;
use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{
    expires_at_from_seconds, extract_chatgpt_account_id_from_jwt, extract_email_from_jwt,
    now_rfc3339, sanitize_id_part,
};
use crate::paths::TokenProxyPaths;

use super::oauth::CodexOAuthClient;
use super::types::{CodexAccountStatus, CodexAccountSummary, CodexTokenRecord};

const CODEX_AUTH_DIR_NAME: &str = "codex-auth";

pub struct CodexAccountStore {
    dir: PathBuf,
    cache: RwLock<HashMap<String, CodexTokenRecord>>,
    app_proxy: AppProxyState,
}

impl CodexAccountStore {
    pub fn new(paths: &TokenProxyPaths, app_proxy: AppProxyState) -> Result<Self, String> {
        let dir = paths.data_dir().join(CODEX_AUTH_DIR_NAME);
        Ok(Self {
            dir,
            cache: RwLock::new(HashMap::new()),
            app_proxy,
        })
    }

    pub async fn list_accounts(&self) -> Result<Vec<CodexAccountSummary>, String> {
        self.refresh_cache().await?;
        let cache = self.cache.read().await;
        let mut items: Vec<CodexAccountSummary> = cache
            .iter()
            .map(|(account_id, record)| CodexAccountSummary {
                account_id: account_id.clone(),
                email: record.email.clone(),
                expires_at: record.expires_at().map(|value| {
                    value
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_else(|_| record.expires_at.clone())
                }),
                status: record.status(),
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        Ok(items)
    }

    pub async fn import_file(&self, path: PathBuf) -> Result<Vec<CodexAccountSummary>, String> {
        if path.as_os_str().is_empty() {
            return Err("File path is required.".to_string());
        }
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err("Selected file not found.".to_string());
        }
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| format!("Failed to read JSON file: {err}"))?;
        let records = parse_import_records(&contents)?;
        let mut imported = Vec::new();
        for record in records {
            if let Ok(summary) = self.save_new_account(record).await {
                imported.push(summary);
            }
        }
        if imported.is_empty() {
            return Err("No valid Codex accounts found in JSON file.".to_string());
        }
        Ok(imported)
    }

    pub(crate) async fn get_account_record(
        &self,
        account_id: &str,
    ) -> Result<CodexTokenRecord, String> {
        let record = self.load_account(account_id).await?;
        self.refresh_if_needed(account_id, record).await
    }

    pub(crate) async fn save_record(
        &self,
        account_id: String,
        record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        self.ensure_dir().await?;
        let path = self.account_path(&account_id);
        let payload = serde_json::to_string_pretty(&record)
            .map_err(|err| format!("Failed to serialize token record: {err}"))?;
        tokio::fs::write(&path, payload)
            .await
            .map_err(|err| format!("Failed to write token record: {err}"))?;
        let mut cache = self.cache.write().await;
        cache.insert(account_id.clone(), record.clone());
        Ok(CodexAccountSummary {
            account_id,
            email: record.email.clone(),
            expires_at: record.expires_at().map(|value| {
                value
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| record.expires_at.clone())
            }),
            status: record.status(),
        })
    }

    pub(crate) async fn save_new_account(
        &self,
        mut record: CodexTokenRecord,
    ) -> Result<CodexAccountSummary, String> {
        fill_record_from_jwt(&mut record);
        let id_part_source = record
            .email
            .as_deref()
            .or(record.account_id.as_deref())
            .unwrap_or_default();
        let mut id_part = sanitize_id_part(id_part_source);
        if id_part.is_empty() {
            id_part = format!("{}", OffsetDateTime::now_utc().unix_timestamp());
        }
        let account_id = self.unique_account_id(&id_part).await?;
        self.save_record(account_id, record).await
    }

    pub(crate) async fn delete_account(&self, account_id: &str) -> Result<(), String> {
        let path = self.account_path(account_id);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|err| format!("Failed to delete token record: {err}"))?;
        }
        let mut cache = self.cache.write().await;
        cache.remove(account_id);
        Ok(())
    }

    async fn refresh_if_needed(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        if !record.is_expired() {
            return Ok(record);
        }
        self.refresh_record(account_id, record).await
    }

    async fn refresh_record(
        &self,
        account_id: &str,
        record: CodexTokenRecord,
    ) -> Result<CodexTokenRecord, String> {
        let proxy_url = self.app_proxy_url().await;
        let client = CodexOAuthClient::new(proxy_url.as_deref())?;
        let response = client.refresh_token(&record.refresh_token).await?;
        let mut refreshed = CodexTokenRecord {
            access_token: response.access_token,
            refresh_token: if response.refresh_token.trim().is_empty() {
                record.refresh_token.clone()
            } else {
                response.refresh_token
            },
            id_token: if response.id_token.trim().is_empty() {
                record.id_token.clone()
            } else {
                response.id_token
            },
            account_id: record.account_id.clone(),
            email: record.email.clone(),
            expires_at: expires_at_from_seconds(response.expires_in),
            last_refresh: Some(now_rfc3339()),
        };
        fill_record_from_jwt(&mut refreshed);
        let summary = self
            .save_record(account_id.to_string(), refreshed.clone())
            .await?;
        if matches!(summary.status, CodexAccountStatus::Expired) {
            return Err("Codex token refresh failed.".to_string());
        }
        Ok(refreshed)
    }

    async fn load_account(&self, account_id: &str) -> Result<CodexTokenRecord, String> {
        if let Some(record) = self.cache.read().await.get(account_id).cloned() {
            return Ok(record);
        }
        self.refresh_cache().await?;
        self.cache
            .read()
            .await
            .get(account_id)
            .cloned()
            .ok_or_else(|| format!("Codex account not found: {account_id}"))
    }

    pub(crate) async fn app_proxy_url(&self) -> Option<String> {
        self.app_proxy.read().await.clone()
    }

    async fn refresh_cache(&self) -> Result<(), String> {
        let mut cache = HashMap::new();
        let dir = self.dir.clone();
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut guard = self.cache.write().await;
                guard.clear();
                return Ok(());
            }
            Err(err) => return Err(format!("Failed to read Codex auth directory: {err}")),
        };

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|err| format!("Failed to read Codex auth entry: {err}"))?
        {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let file_name = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            let contents = match tokio::fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            let record: CodexTokenRecord = match serde_json::from_str(&contents) {
                Ok(record) => record,
                Err(_) => continue,
            };
            cache.insert(file_name, record);
        }

        let mut guard = self.cache.write().await;
        *guard = cache;
        Ok(())
    }

    async fn ensure_dir(&self) -> Result<(), String> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|err| format!("Failed to create Codex auth dir: {err}"))
    }

    async fn unique_account_id(&self, id_part: &str) -> Result<String, String> {
        self.ensure_dir().await?;
        let mut suffix = 0u32;
        loop {
            let candidate = if suffix == 0 {
                format!("codex-{id_part}.json")
            } else {
                format!("codex-{id_part}-{suffix}.json")
            };
            if !tokio::fs::try_exists(self.account_path(&candidate))
                .await
                .unwrap_or(false)
            {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    fn account_path(&self, account_id: &str) -> PathBuf {
        self.dir.join(account_id)
    }
}

fn fill_record_from_jwt(record: &mut CodexTokenRecord) {
    if record.account_id.is_none() {
        record.account_id = extract_chatgpt_account_id_from_jwt(&record.id_token);
    }
    if record.email.is_none() {
        record.email = extract_email_from_jwt(&record.id_token);
    }
}

fn parse_import_records(contents: &str) -> Result<Vec<CodexTokenRecord>, String> {
    let value: Value = serde_json::from_str(contents)
        .map_err(|err| format!("Invalid Codex account JSON file: {err}"))?;
    let mut records = Vec::new();
    collect_import_records(&value, &mut records);
    Ok(records)
}

fn collect_import_records(value: &Value, records: &mut Vec<CodexTokenRecord>) {
    if let Some(record) = parse_import_record(value) {
        records.push(record);
        return;
    }

    if let Some(items) = value.as_array() {
        for item in items {
            collect_import_records(item, records);
        }
        return;
    }

    for key in ["accounts", "auths", "items", "data"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            collect_import_records(item, records);
        }
    }
}

fn parse_import_record(value: &Value) -> Option<CodexTokenRecord> {
    let provider = find_string(value, &[&["type"], &["provider"], &["kind"]]);
    if let Some(provider) = provider {
        if !provider.eq_ignore_ascii_case("codex") {
            return None;
        }
    }

    let access_token = find_string(value, &[&["access_token"], &["token", "access_token"]])?;
    let refresh_token = find_string(value, &[&["refresh_token"], &["token", "refresh_token"]])?;
    let id_token = find_string(value, &[&["id_token"], &["token", "id_token"]])?;
    let expires_at = find_rfc3339_or_unix_timestamp(
        value,
        &[
            &["expires_at"],
            &["expired"],
            &["token", "expires_at"],
            &["token", "expired"],
        ],
    )
    .or_else(|| {
        find_i64(value, &[&["expires_in"], &["token", "expires_in"]]).map(expires_at_from_seconds)
    })?;

    let account_id = find_string(
        value,
        &[
            &["account_id"],
            &["chatgpt_account_id"],
            &["account", "uuid"],
            &["account", "id"],
        ],
    );
    let email = find_string(
        value,
        &[
            &["email"],
            &["account", "email_address"],
            &["account", "email"],
            &["user", "email"],
        ],
    );
    let last_refresh = find_string(
        value,
        &[
            &["last_refresh"],
            &["lastRefresh"],
            &["last_refreshed_at"],
            &["lastRefreshedAt"],
        ],
    )
    .or_else(|| Some(now_rfc3339()));

    Some(CodexTokenRecord {
        access_token,
        refresh_token,
        id_token,
        account_id,
        email,
        expires_at,
        last_refresh,
    })
}

fn find_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let Some(candidate) = value_at_path(value, path) else {
            continue;
        };
        let Some(text) = candidate.as_str() else {
            continue;
        };
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn find_i64(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    for path in paths {
        let Some(candidate) = value_at_path(value, path) else {
            continue;
        };
        if let Some(number) = candidate.as_i64() {
            return Some(number);
        }
        if let Some(text) = candidate.as_str() {
            if let Ok(number) = text.trim().parse::<i64>() {
                return Some(number);
            }
        }
    }
    None
}

fn find_rfc3339_or_unix_timestamp(value: &Value, paths: &[&[&str]]) -> Option<String> {
    if let Some(text) = find_string(value, paths) {
        return Some(text);
    }
    find_i64(value, paths).and_then(format_unix_timestamp)
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn format_unix_timestamp(value: i64) -> Option<String> {
    let (seconds, nanos) = if value >= 10_000_000_000 {
        let secs = value / 1000;
        let ms = value % 1000;
        (secs, ms * 1_000_000)
    } else {
        (value, 0)
    };
    let total_nanos = i128::from(seconds)
        .checked_mul(1_000_000_000)?
        .checked_add(i128::from(nanos))?;
    OffsetDateTime::from_unix_timestamp_nanos(total_nanos)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_proxy;
    use crate::paths::TokenProxyPaths;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rand::random;
    use serde_json::json;
    use std::future::Future;
    use std::path::PathBuf;
    use time::format_description::well_known::Rfc3339;

    fn run_async(test: impl Future<Output = ()>) {
        tokio::runtime::Runtime::new()
            .expect("runtime")
            .block_on(test);
    }

    fn create_test_store() -> (CodexAccountStore, PathBuf) {
        let data_dir =
            std::env::temp_dir().join(format!("token-proxy-codex-store-test-{}", random::<u64>()));
        std::fs::create_dir_all(&data_dir).expect("create test data dir");
        let paths = TokenProxyPaths::from_app_data_dir(data_dir.clone()).expect("test paths");
        let store = CodexAccountStore::new(&paths, app_proxy::new_state()).expect("codex store");
        (store, data_dir)
    }

    fn build_id_token(email: &str, account_id: &str) -> String {
        let payload = json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
            }
        });
        let encoded =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("serialize payload"));
        format!("header.{encoded}.signature")
    }

    fn future_rfc3339(hours: i64) -> String {
        (OffsetDateTime::now_utc() + time::Duration::hours(hours))
            .format(&Rfc3339)
            .expect("format expires_at")
    }

    #[test]
    fn import_file_parses_token_proxy_codex_record() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let id_token = build_id_token("alice@example.com", "acct-token-proxy");
            let expires_at = future_rfc3339(6);
            let input_path = data_dir.join("token-proxy-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": id_token,
                    "expires_at": expires_at,
                    "last_refresh": "2026-03-27T01:02:03Z",
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("alice@example.com"));
            assert_eq!(imported[0].expires_at.as_deref(), Some(expires_at.as_str()));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-token-proxy"));
            assert_eq!(record.email.as_deref(), Some("alice@example.com"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_parses_cliproxy_codex_record_with_expired_alias() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let expires_at = future_rfc3339(8);
            let input_path = data_dir.join("cliproxy-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "type": "codex",
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": build_id_token("bob@example.com", "acct-cliproxy"),
                    "account_id": "acct-cliproxy",
                    "email": "bob@example.com",
                    "expired": expires_at,
                    "last_refresh": "2026-03-27T02:03:04Z",
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("bob@example.com"));
            assert_eq!(imported[0].expires_at.as_deref(), Some(expires_at.as_str()));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.expires_at, expires_at);
            assert_eq!(record.account_id.as_deref(), Some("acct-cliproxy"));

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }

    #[test]
    fn import_file_parses_sub2api_oauth_token_response() {
        run_async(async {
            let (store, data_dir) = create_test_store();
            let input_path = data_dir.join("sub2api-codex.json");
            tokio::fs::write(
                &input_path,
                serde_json::to_string_pretty(&json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "id_token": build_id_token("carol@example.com", "acct-sub2api"),
                    "token_type": "Bearer",
                    "expires_in": 7200,
                }))
                .expect("serialize test json"),
            )
            .await
            .expect("write input");

            let imported = store
                .import_file(input_path)
                .await
                .expect("import should succeed");

            assert_eq!(imported.len(), 1);
            assert_eq!(imported[0].email.as_deref(), Some("carol@example.com"));

            let record = store
                .get_account_record(&imported[0].account_id)
                .await
                .expect("record should exist");
            assert_eq!(record.account_id.as_deref(), Some("acct-sub2api"));
            assert_eq!(record.email.as_deref(), Some("carol@example.com"));
            assert!(record.expires_at().is_some());
            assert!(!record.is_expired());

            let _ = std::fs::remove_dir_all(data_dir);
        });
    }
}
