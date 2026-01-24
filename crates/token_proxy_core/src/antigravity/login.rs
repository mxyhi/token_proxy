use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{expires_at_from_seconds, generate_state};

use super::oauth::AntigravityOAuthClient;
use super::project;
use super::store::AntigravityAccountStore;
use super::types::{
    AntigravityAccountSummary, AntigravityLoginPollResponse, AntigravityLoginStartResponse,
    AntigravityLoginStatus, AntigravityTokenRecord,
};

const AUTH_CODE_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL_SECONDS: u64 = 2;
const CALLBACK_PORT: u16 = 51121;

#[derive(Clone)]
pub struct AntigravityLoginManager {
    store: Arc<AntigravityAccountStore>,
    sessions: Arc<RwLock<HashMap<String, LoginSession>>>,
    app_proxy: AppProxyState,
}

#[derive(Clone)]
struct LoginSession {
    status: AntigravityLoginStatus,
    error: Option<String>,
    account: Option<AntigravityAccountSummary>,
    expires_at: Option<OffsetDateTime>,
}

impl AntigravityLoginManager {
    pub fn new(store: Arc<AntigravityAccountStore>, app_proxy: AppProxyState) -> Self {
        Self {
            store,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            app_proxy,
        }
    }

    pub async fn start_login(&self) -> Result<AntigravityLoginStartResponse, String> {
        let state = generate_state("antigravity")?;
        let expires_at = Some(OffsetDateTime::now_utc() + time::Duration::seconds(300));
        self.insert_session(&state, expires_at).await;
        let callback = start_auth_code_callback(state.clone()).await?;
        let login_url = AntigravityOAuthClient::build_authorize_url(&callback.redirect_uri, &state);
        let manager = self.clone();
        let state_for_task = state.clone();
        tokio::spawn(async move {
            run_auth_code_login(manager, state_for_task, callback).await;
        });
        Ok(AntigravityLoginStartResponse {
            state,
            login_url,
            interval_seconds: POLL_INTERVAL_SECONDS,
            expires_at: Some(expires_at_from_seconds(AUTH_CODE_TIMEOUT.as_secs() as i64)),
        })
    }

    pub async fn poll_login(
        &self,
        state: &str,
    ) -> Result<AntigravityLoginPollResponse, String> {
        let mut guard = self.sessions.write().await;
        let session = guard
            .get_mut(state)
            .ok_or_else(|| "Login session not found.".to_string())?;
        if session.status != AntigravityLoginStatus::Success
            && session.status != AntigravityLoginStatus::Error
            && session
                .expires_at
                .map(|deadline| OffsetDateTime::now_utc() > deadline)
                .unwrap_or(false)
        {
            session.status = AntigravityLoginStatus::Error;
            session.error = Some("Login expired.".to_string());
        }
        Ok(AntigravityLoginPollResponse {
            state: state.to_string(),
            status: session.status.clone(),
            error: session.error.clone(),
            account: session.account.clone(),
        })
    }

    pub async fn logout(&self, account_id: &str) -> Result<(), String> {
        self.store.delete_account(account_id).await
    }

    async fn insert_session(&self, state: &str, expires_at: Option<OffsetDateTime>) {
        let session = LoginSession {
            status: AntigravityLoginStatus::Waiting,
            error: None,
            account: None,
            expires_at,
        };
        let mut guard = self.sessions.write().await;
        guard.insert(state.to_string(), session);
    }

    async fn complete_session(&self, state: &str, account: AntigravityAccountSummary) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = AntigravityLoginStatus::Success;
            session.error = None;
            session.account = Some(account);
        }
    }

    async fn fail_session(&self, state: &str, message: String) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = AntigravityLoginStatus::Error;
            session.error = Some(message);
        }
    }

    async fn app_proxy_url(&self) -> Option<String> {
        self.app_proxy.read().await.clone()
    }
}

struct AuthCodeCallback {
    redirect_uri: String,
    receiver: tokio::sync::mpsc::Receiver<AuthCodeResult>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Clone)]
struct AuthCodeResult {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn start_auth_code_callback(state: String) -> Result<AuthCodeCallback, String> {
    let (tx, rx) = tokio::sync::mpsc::channel::<AuthCodeResult>(1);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{CALLBACK_PORT}"))
        .await
        .map_err(|err| format!("Failed to start callback server: {err}"))?;
    let redirect_uri = format!("http://localhost:{CALLBACK_PORT}/oauth-callback");
    let router = axum::Router::new().route(
        "/oauth-callback",
        axum::routing::get(move |query: axum::extract::Query<HashMap<String, String>>| {
            let expected_state = state.clone();
            let tx = tx.clone();
            async move {
                let code = query.get("code").cloned();
                let state = query.get("state").cloned();
                let error = query.get("error").cloned();
                let has_error = error.is_some();
                let state_matches = state.as_deref() == Some(&expected_state);
                let _ = tx.send(AuthCodeResult { code, state, error }).await;
                let body = if has_error || !state_matches {
                    "Login failed. You can close this window."
                } else {
                    "Login successful. You can close this window."
                };
                axum::response::Html(body)
            }
        }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Ok(AuthCodeCallback {
        redirect_uri,
        receiver: rx,
        shutdown: Some(shutdown_tx),
    })
}

async fn run_auth_code_login(
    manager: AntigravityLoginManager,
    state: String,
    mut callback: AuthCodeCallback,
) {
    let redirect_uri = callback.redirect_uri.clone();
    let callback_result = match wait_for_auth_code(&mut callback).await {
        Ok(result) => result,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let code = match extract_auth_code(&state, callback_result) {
        Ok(code) => code,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let proxy_url = manager.app_proxy_url().await;
    let client = AntigravityOAuthClient::new(proxy_url.clone());
    let token = match client.exchange_code(&code, &redirect_uri).await {
        Ok(token) => token,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let email = match client.fetch_user_email(&token.access_token).await {
        Ok(email) => email,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };

    let project_id = match project::load_code_assist(&token.access_token, proxy_url.as_deref()).await
    {
        Ok(info) => {
            let mut project_id = info.project_id.clone();
            if project_id.is_none() {
                if let Some(tier_id) = info.plan_type.as_deref() {
                    match project::onboard_user(
                        &token.access_token,
                        proxy_url.as_deref(),
                        tier_id,
                    )
                    .await
                    {
                        Ok(Some(value)) => project_id = Some(value),
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(error = %err, "antigravity onboardUser failed");
                        }
                    }
                }
            }
            project_id
        }
        Err(err) => {
            tracing::warn!(error = %err, "antigravity loadCodeAssist failed");
            None
        }
    };

    let record = AntigravityTokenRecord {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expired: Some(expires_at_from_seconds(token.expires_in)),
        expires_in: Some(token.expires_in),
        timestamp: Some(OffsetDateTime::now_utc().unix_timestamp() * 1000),
        email: email.clone(),
        token_type: token.token_type.clone(),
        project_id,
        source: Some("oauth".to_string()),
    };
    let summary = match manager.store.save_new_account(record).await {
        Ok(summary) => summary,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    manager.complete_session(&state, summary).await;
}

async fn wait_for_auth_code(callback: &mut AuthCodeCallback) -> Result<AuthCodeResult, String> {
    let timeout = tokio::time::sleep(AUTH_CODE_TIMEOUT);
    tokio::pin!(timeout);
    tokio::select! {
        _ = &mut timeout => Err("Login timed out.".to_string()),
        result = callback.receiver.recv() => {
            let _ = callback.shutdown.take().map(|sender| sender.send(()));
            result.ok_or_else(|| "Login failed.".to_string())
        }
    }
}

fn extract_auth_code(state: &str, result: AuthCodeResult) -> Result<String, String> {
    if result.error.is_some() {
        return Err("Login failed.".to_string());
    }
    if result.state.as_deref() != Some(state) {
        return Err("Login failed: state mismatch.".to_string());
    }
    let code = result.code.unwrap_or_default();
    if code.trim().is_empty() {
        return Err("Login failed: code missing.".to_string());
    }
    Ok(code)
}
