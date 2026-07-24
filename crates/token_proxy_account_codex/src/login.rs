use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::RwLock;

use token_proxy_account_store::app_proxy::AppProxyState;
use token_proxy_account_store::oauth_util::{
    expires_at_from_seconds, generate_pkce, generate_state, now_rfc3339,
};

use super::oauth::CodexOAuthClient;
use super::store::CodexAccountStore;
use super::types::{
    CodexAccountSummary, CodexCredential, CodexLoginPollResponse, CodexLoginStartResponse,
    CodexLoginStatus, CodexQuotaCache, CodexTokenRecord,
};

const AUTH_CODE_TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL_SECONDS: u64 = 2;
const CODEX_CALLBACK_PORT: u16 = 1455;

#[derive(Clone)]
pub struct CodexLoginManager {
    /// 保留构造对称；credential 落库改由编排层 store 提交。
    #[allow(dead_code)]
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
    /// OAuth 完成后待编排层在 mutation 锁内提交的 credential。
    pending_record: Option<CodexTokenRecord>,
    /// 编排层正在提交 credential+config，禁止取消/重复 claim。
    committing: bool,
}

/// poll 结果：仍等待 / 终态 / 可提交的 prepared credential。
pub enum CodexLoginPollClaim {
    /// 无需提交（Waiting / 已 Success / Error）。
    Response(CodexLoginPollResponse),
    /// 已从 session 取出 pending，调用方必须 commit 或 fail。
    Prepared {
        state: String,
        record: CodexTokenRecord,
    },
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
        // 只读会话状态；有 pending 时对外仍为 Waiting，真正提交走 claim_prepared_login。
        let mut guard = self.sessions.write().await;
        let session = guard
            .get_mut(state)
            .ok_or_else(|| "Login session not found.".to_string())?;
        if session.status != CodexLoginStatus::Success
            && session.status != CodexLoginStatus::Error
            && !session.committing
            && session.pending_record.is_none()
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

    /// 编排层在 mutation 锁内调用：取出 prepared credential 或返回当前 poll 响应。
    pub async fn claim_prepared_login(&self, state: &str) -> Result<CodexLoginPollClaim, String> {
        let mut guard = self.sessions.write().await;
        let session = guard
            .get_mut(state)
            .ok_or_else(|| "Login session not found.".to_string())?;
        if session.status != CodexLoginStatus::Success
            && session.status != CodexLoginStatus::Error
            && !session.committing
            && session.pending_record.is_none()
            && session
                .expires_at
                .map(|deadline| OffsetDateTime::now_utc() > deadline)
                .unwrap_or(false)
        {
            session.status = CodexLoginStatus::Error;
            session.error = Some("Login expired.".to_string());
        }
        // 取消安全：clone 不 take；complete/fail 才清 pending，中断后可重 claim。
        if let Some(record) = session.pending_record.clone() {
            session.committing = true;
            tracing::debug!("codex login prepared credential claimed for orchestration commit");
            return Ok(CodexLoginPollClaim::Prepared {
                state: state.to_string(),
                record,
            });
        }
        Ok(CodexLoginPollClaim::Response(CodexLoginPollResponse {
            state: state.to_string(),
            status: session.status.clone(),
            error: session.error.clone(),
            account: session.account.clone(),
        }))
    }

    /// 编排提交成功。
    pub async fn complete_login_commit(&self, state: &str, account: CodexAccountSummary) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = CodexLoginStatus::Success;
            session.error = None;
            session.account = Some(account);
            session.pending_record = None;
            session.committing = false;
            tracing::info!("codex login commit completed");
        }
    }

    /// 编排提交失败（credential 已由调用方补偿）。
    pub async fn fail_login_commit(&self, state: &str, message: String) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = CodexLoginStatus::Error;
            session.error = Some(message);
            session.account = None;
            session.pending_record = None;
            session.committing = false;
        }
    }

    /// 测试：注入 prepared login，不走真实 OAuth。
    #[cfg(any(test, feature = "test-support"))]
    pub async fn inject_prepared_login_for_test(&self, state: &str, record: CodexTokenRecord) {
        let session = LoginSession {
            status: CodexLoginStatus::Waiting,
            error: None,
            account: None,
            expires_at: Some(OffsetDateTime::now_utc() + time::Duration::seconds(600)),
            pending_record: Some(record),
            committing: false,
        };
        self.sessions
            .write()
            .await
            .insert(state.to_string(), session);
        tracing::debug!("codex prepared login injected for test");
    }

    async fn insert_session(&self, state: &str, expires_at: Option<OffsetDateTime>) {
        let session = LoginSession {
            status: CodexLoginStatus::Waiting,
            error: None,
            account: None,
            expires_at,
            pending_record: None,
            committing: false,
        };
        let mut guard = self.sessions.write().await;
        guard.insert(state.to_string(), session);
    }

    async fn prepare_session(&self, state: &str, record: CodexTokenRecord) {
        // 只暂存 credential，最终落库由 token_proxy_app 编排锁提交。
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.pending_record = Some(record);
            session.status = CodexLoginStatus::Waiting;
            session.error = None;
            session.account = None;
            session.committing = false;
            tracing::info!("codex login credential prepared; awaiting orchestration commit");
        }
    }

    async fn fail_session(&self, state: &str, message: String) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = CodexLoginStatus::Error;
            session.error = Some(message);
            session.account = None;
            session.pending_record = None;
            session.committing = false;
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
        credential: CodexCredential::Oauth {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            client_id: Some(
                super::oauth::CodexRefreshTokenClient::Codex
                    .client_id()
                    .to_string(),
            ),
            id_token: token.id_token,
            auto_refresh_enabled: true,
            openai_device_id: None,
            expires_at: expires_at_from_seconds(token.expires_in),
            last_refresh: Some(now_rfc3339()),
        },
        status: super::types::CodexAccountStatus::Active,
        account_id: None,
        user_id: None,
        email: None,
        quota: CodexQuotaCache::default(),
    };
    // 不在 OAuth 任务内落库；交给 poll 路径的编排小事务提交。
    manager.prepare_session(&state, record).await;
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
