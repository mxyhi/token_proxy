use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::build_reqwest_client;

use super::endpoints;
use super::project;
use super::store::AntigravityAccountStore;
use super::types::{AntigravityWarmupSchedule, AntigravityWarmupScheduleSummary};

const GENERATE_PATH: &str = "/v1internal:generateContent";
const STREAM_PATH: &str = "/v1internal:streamGenerateContent";

#[derive(Clone)]
pub struct AntigravityWarmupScheduler {
    store: Arc<AntigravityAccountStore>,
    app_proxy: AppProxyState,
    schedules: Arc<RwLock<HashMap<String, AntigravityWarmupSchedule>>>,
    runner: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl AntigravityWarmupScheduler {
    pub fn new(store: Arc<AntigravityAccountStore>, app_proxy: AppProxyState) -> Self {
        Self {
            store,
            app_proxy,
            schedules: Arc::new(RwLock::new(HashMap::new())),
            runner: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start(&self) {
        let mut guard = self.runner.lock().await;
        if guard.is_some() {
            return;
        }
        let scheduler = self.clone();
        let handle = tokio::spawn(async move {
            scheduler.run_loop().await;
        });
        *guard = Some(handle);
    }

    pub async fn list_schedules(&self) -> Vec<AntigravityWarmupScheduleSummary> {
        let guard = self.schedules.read().await;
        let mut items: Vec<AntigravityWarmupScheduleSummary> = guard
            .values()
            .map(|item| AntigravityWarmupScheduleSummary {
                account_id: item.account_id.clone(),
                model: item.model.clone(),
                interval_minutes: item.interval_minutes,
                next_run_at: item.next_run_at.clone(),
                enabled: item.enabled,
            })
            .collect();
        items.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        items
    }

    pub async fn set_schedule(
        &self,
        account_id: String,
        model: String,
        interval_minutes: u64,
        enabled: bool,
    ) -> Result<AntigravityWarmupScheduleSummary, String> {
        if account_id.trim().is_empty() || model.trim().is_empty() {
            return Err("Account ID and model are required.".to_string());
        }
        let interval_minutes = interval_minutes.max(1);
        let mut guard = self.schedules.write().await;
        let key = schedule_key(&account_id, &model);
        let next_run_at = if enabled {
            Some(format_next_run(interval_minutes))
        } else {
            None
        };
        let schedule = AntigravityWarmupSchedule {
            account_id: account_id.clone(),
            model: model.clone(),
            interval_minutes,
            next_run_at: next_run_at.clone(),
            enabled,
        };
        guard.insert(key, schedule.clone());
        Ok(AntigravityWarmupScheduleSummary {
            account_id,
            model,
            interval_minutes,
            next_run_at,
            enabled,
        })
    }

    pub async fn toggle_schedule(
        &self,
        account_id: String,
        model: String,
        enabled: bool,
    ) -> Result<(), String> {
        let mut guard = self.schedules.write().await;
        let key = schedule_key(&account_id, &model);
        let Some(schedule) = guard.get_mut(&key) else {
            return Err("Warmup schedule not found.".to_string());
        };
        schedule.enabled = enabled;
        schedule.next_run_at = if enabled {
            Some(format_next_run(schedule.interval_minutes))
        } else {
            None
        };
        Ok(())
    }

    pub async fn run_warmup(
        &self,
        account_id: &str,
        model: &str,
        stream: bool,
    ) -> Result<(), String> {
        let record = self.store.get_account_record(account_id).await?;
        let proxy_url = self.app_proxy.read().await.clone();
        let client = build_reqwest_client(proxy_url.as_deref(), Duration::from_secs(20))?;
        let mut project_id = record.project_id.clone();
        if project_id.is_none() {
            if let Ok(info) =
                project::load_code_assist(&record.access_token, proxy_url.as_deref()).await
            {
                if let Some(value) = info.project_id.clone() {
                    let _ = self
                        .store
                        .update_project_id(account_id, value.clone())
                        .await;
                    project_id = Some(value);
                } else if let Some(tier_id) = info.plan_type.as_deref() {
                    if let Ok(Some(value)) =
                        project::onboard_user(&record.access_token, proxy_url.as_deref(), tier_id)
                            .await
                    {
                        let _ = self
                            .store
                            .update_project_id(account_id, value.clone())
                            .await;
                        project_id = Some(value);
                    }
                }
            }
        }
        let user_agent = endpoints::default_user_agent();
        let payload = build_warmup_payload(model, project_id.as_deref(), &user_agent);
        let path = if stream { STREAM_PATH } else { GENERATE_PATH };
        let mut last_error: Option<String> = None;
        for base in endpoints::BASE_URLS {
            let url = format!("{}{}", base, path);
            let response = client
                .post(url)
                .header(AUTHORIZATION, format!("Bearer {}", record.access_token))
                .header(USER_AGENT, user_agent.as_str())
                .header(CONTENT_TYPE, "application/json")
                .json(&payload)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    last_error = Some(format!("Warmup request failed: {err}"));
                    continue;
                }
            };
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                let message = format!("Warmup failed: {status} {body}");
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                    last_error = Some(message);
                    continue;
                }
                return Err(message);
            }
            return Ok(());
        }
        Err(last_error.unwrap_or_else(|| "Warmup failed.".to_string()))
    }

    async fn run_loop(&self) {
        loop {
            let due = self.collect_due().await;
            for item in due {
                let _ = self.run_warmup(&item.account_id, &item.model, false).await;
                self.bump_schedule(&item.account_id, &item.model).await;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }

    async fn collect_due(&self) -> Vec<AntigravityWarmupSchedule> {
        let now = OffsetDateTime::now_utc();
        let guard = self.schedules.read().await;
        guard
            .values()
            .filter_map(|schedule| {
                if !schedule.enabled {
                    return None;
                }
                let next_run = schedule
                    .next_run_at
                    .as_deref()
                    .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok());
                if next_run.is_some_and(|value| value > now) {
                    return None;
                }
                Some(schedule.clone())
            })
            .collect()
    }

    async fn bump_schedule(&self, account_id: &str, model: &str) {
        let mut guard = self.schedules.write().await;
        let key = schedule_key(account_id, model);
        if let Some(schedule) = guard.get_mut(&key) {
            schedule.next_run_at = Some(format_next_run(schedule.interval_minutes));
        }
    }
}

fn format_next_run(interval_minutes: u64) -> String {
    let next = OffsetDateTime::now_utc() + time::Duration::minutes(interval_minutes as i64);
    next.format(&Rfc3339)
        .unwrap_or_else(|_| next.unix_timestamp().to_string())
}

fn schedule_key(account_id: &str, model: &str) -> String {
    format!("{}::{}", account_id.trim(), model.trim())
}

fn build_warmup_payload(
    model: &str,
    project_id: Option<&str>,
    user_agent: &str,
) -> serde_json::Value {
    let project = project_id.unwrap_or_default();
    let request_id = format!("agent-{}", OffsetDateTime::now_utc().unix_timestamp());
    serde_json::json!({
        "project": project,
        "request": {
            "contents": [
                { "role": "user", "parts": [{ "text": "ping" }] }
            ],
            "generationConfig": { "maxOutputTokens": 1 },
            "toolConfig": { "functionCallingConfig": { "mode": "NONE" } },
            "sessionId": format!("-{}", OffsetDateTime::now_utc().unix_timestamp())
        },
        "model": model,
        "requestId": request_id,
        "userAgent": user_agent,
        "requestType": "agent"
    })
}

pub(crate) async fn run_blocking<F, R>(task: F) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(task).await
}
