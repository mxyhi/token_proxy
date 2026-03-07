use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::RwLock;

use crate::app_proxy::AppProxyState;
use crate::oauth_util::{expires_at_from_seconds, generate_pkce, generate_state, now_rfc3339};

use super::oauth::CodexOAuthClient;
use super::store::CodexAccountStore;
use super::types::{
    CodexAccountSummary, CodexLoginPollResponse, CodexLoginStartResponse, CodexLoginStatus,
    CodexTokenRecord,
};

const AUTH_CODE_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL_SECONDS: u64 = 2;
const CODEX_CALLBACK_PORT: u16 = 1455;

#[derive(Clone)]
pub struct CodexLoginManager {
    store: Arc<CodexAccountStore>,
    sessions: Arc<RwLock<HashMap<String, LoginSession>>>,
    app_proxy: AppProxyState,
}

#[derive(Clone)]
struct LoginSession {
    status: CodexLoginStatus,
    error: Option<String>,
    account: Option<CodexAccountSummary>,
    expires_at: Option<OffsetDateTime>,
}

impl CodexLoginManager {
    pub fn new(store: Arc<CodexAccountStore>, app_proxy: AppProxyState) -> Self {
        Self {
            store,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            app_proxy,
        }
    }

    pub async fn start_login(&self) -> Result<CodexLoginStartResponse, String> {
        let state = generate_state("codex")?;
        let expires_at = Some(OffsetDateTime::now_utc() + time::Duration::seconds(600));
        self.insert_session(&state, expires_at).await;
        let (code_verifier, code_challenge) = generate_pkce()?;
        let callback = start_auth_code_callback(state.clone()).await?;
        let login_url =
            CodexOAuthClient::build_authorize_url(&callback.redirect_uri, &state, &code_challenge);
        let manager = self.clone();
        let state_for_task = state.clone();
        tokio::spawn(async move {
            run_auth_code_login(manager, state_for_task, code_verifier, callback).await;
        });
        Ok(CodexLoginStartResponse {
            state,
            login_url,
            interval_seconds: POLL_INTERVAL_SECONDS,
            expires_at: Some(expires_at_from_seconds(AUTH_CODE_TIMEOUT.as_secs() as i64)),
        })
    }

    pub async fn poll_login(&self, state: &str) -> Result<CodexLoginPollResponse, String> {
        let mut guard = self.sessions.write().await;
        let session = guard
            .get_mut(state)
            .ok_or_else(|| "Login session not found.".to_string())?;
        if session.status != CodexLoginStatus::Success
            && session.status != CodexLoginStatus::Error
            && session
                .expires_at
                .map(|deadline| OffsetDateTime::now_utc() > deadline)
                .unwrap_or(false)
        {
            session.status = CodexLoginStatus::Error;
            session.error = Some("Login expired.".to_string());
        }
        Ok(CodexLoginPollResponse {
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
            status: CodexLoginStatus::Waiting,
            error: None,
            account: None,
            expires_at,
        };
        let mut guard = self.sessions.write().await;
        guard.insert(state.to_string(), session);
    }

    async fn complete_session(&self, state: &str, account: CodexAccountSummary) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = CodexLoginStatus::Success;
            session.error = None;
            session.account = Some(account);
        }
    }

    async fn fail_session(&self, state: &str, message: String) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = CodexLoginStatus::Error;
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
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{CODEX_CALLBACK_PORT}"))
        .await
        .map_err(|err| format!("Failed to start callback server: {err}"))?;
    let redirect_uri = format!("http://localhost:{CODEX_CALLBACK_PORT}/auth/callback");
    let router = axum::Router::new().route(
        "/auth/callback",
        axum::routing::get(
            move |query: axum::extract::Query<HashMap<String, String>>| {
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
            },
        ),
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
    manager: CodexLoginManager,
    state: String,
    code_verifier: String,
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
    let client = match CodexOAuthClient::new(proxy_url.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let token = match client
        .exchange_code(&code, &code_verifier, &redirect_uri)
        .await
    {
        Ok(token) => token,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let record = CodexTokenRecord {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: token.id_token,
        account_id: None,
        email: None,
        expires_at: expires_at_from_seconds(token.expires_in),
        last_refresh: Some(now_rfc3339()),
    };
    match manager.store.save_new_account(record).await {
        Ok(account) => manager.complete_session(&state, account).await,
        Err(err) => manager.fail_session(&state, err).await,
    }
}

async fn wait_for_auth_code(callback: &mut AuthCodeCallback) -> Result<AuthCodeResult, String> {
    let shutdown = callback.shutdown.take();
    let result = tokio::time::timeout(AUTH_CODE_TIMEOUT, callback.receiver.recv()).await;
    if let Some(shutdown) = shutdown {
        let _ = shutdown.send(());
    }
    match result {
        Ok(Some(callback)) => Ok(callback),
        Ok(None) => Err("Authorization callback closed.".to_string()),
        Err(_) => Err("Authorization timed out.".to_string()),
    }
}

fn extract_auth_code(state: &str, callback_result: AuthCodeResult) -> Result<String, String> {
    if let Some(err) = callback_result.error {
        return Err(err);
    }
    if callback_result.state.as_deref() != Some(state) {
        return Err("OAuth state mismatch.".to_string());
    }
    callback_result
        .code
        .ok_or_else(|| "Authorization code missing.".to_string())
}
