use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::RwLock;

use super::callback::{
    callback_file_path, parse_callback_url, read_callback_file, write_callback_file,
};
use super::oauth::{build_login_url, KiroOAuthClient};
use super::sso_oidc::{
    build_auth_code_url, CreateTokenResponse, RegisterClientResponse, SsoOidcClient,
    StartDeviceAuthResponse, TokenPollError,
};
use super::store::KiroAccountStore;
use super::types::{
    KiroAccountSummary, KiroLoginMethod, KiroLoginPollResponse, KiroLoginStartResponse,
    KiroLoginStatus, KiroTokenRecord,
};
use super::util::{expires_at_from_seconds, generate_pkce, generate_state, now_rfc3339};
use crate::app_proxy::AppProxyState;

const SOCIAL_CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const AUTH_CODE_TIMEOUT: Duration = Duration::from_secs(600);
const KIRO_REDIRECT_URI: &str = "kiro://kiro.kiroAgent/authenticate-success";

#[derive(Clone)]
pub struct KiroLoginManager {
    store: Arc<KiroAccountStore>,
    sessions: Arc<RwLock<HashMap<String, LoginSession>>>,
    app_proxy: AppProxyState,
}

#[derive(Clone)]
struct LoginSession {
    status: KiroLoginStatus,
    error: Option<String>,
    account: Option<KiroAccountSummary>,
    expires_at: Option<OffsetDateTime>,
}

impl KiroLoginManager {
    pub fn new(store: Arc<KiroAccountStore>, app_proxy: AppProxyState) -> Self {
        Self {
            store,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            app_proxy,
        }
    }

    pub async fn start_login(
        &self,
        method: KiroLoginMethod,
    ) -> Result<KiroLoginStartResponse, String> {
        let state = generate_state("kiro")?;
        let expires_at = Some(OffsetDateTime::now_utc() + time::Duration::seconds(600));
        self.insert_session(&state, expires_at).await;
        match method {
            KiroLoginMethod::Aws => self.start_device_code_login(state, method).await,
            KiroLoginMethod::AwsAuthcode => self.start_auth_code_login(state, method).await,
            KiroLoginMethod::Google => self.start_social_login(state, method).await,
        }
    }

    pub async fn poll_login(&self, state: &str) -> Result<KiroLoginPollResponse, String> {
        let mut guard = self.sessions.write().await;
        let session = guard
            .get_mut(state)
            .ok_or_else(|| "Login session not found.".to_string())?;
        if session.status != KiroLoginStatus::Success
            && session.status != KiroLoginStatus::Error
            && session
                .expires_at
                .map(|deadline| OffsetDateTime::now_utc() > deadline)
                .unwrap_or(false)
        {
            session.status = KiroLoginStatus::Error;
            session.error = Some("Login expired.".to_string());
        }
        Ok(KiroLoginPollResponse {
            state: state.to_string(),
            status: session.status.clone(),
            error: session.error.clone(),
            account: session.account.clone(),
        })
    }

    pub async fn logout(&self, account_id: &str) -> Result<(), String> {
        self.store.delete_account(account_id).await
    }

    pub async fn handle_callback_url(&self, url: &str) -> Result<(), String> {
        let payload = parse_callback_url(url)?;
        write_callback_file(self.store.dir(), &payload).await?;
        Ok(())
    }

    async fn start_device_code_login(
        &self,
        state: String,
        method: KiroLoginMethod,
    ) -> Result<KiroLoginStartResponse, String> {
        let proxy_url = self.app_proxy_url().await;
        let client = SsoOidcClient::new(proxy_url.as_deref())?;
        let reg = client.register_client().await?;
        let auth = client
            .start_device_authorization(&reg.client_id, &reg.client_secret)
            .await?;
        let manager = self.clone();
        let state_for_task = state.clone();
        let verification_uri = auth.verification_uri.clone();
        let verification_uri_complete = auth.verification_uri_complete.clone();
        let user_code = auth.user_code.clone();
        let interval_seconds = auth.interval as u64;
        let expires_at = expires_at_from_seconds(auth.expires_in);
        tokio::spawn(async move {
            run_device_code_login(manager, state_for_task, reg, auth).await;
        });
        Ok(KiroLoginStartResponse {
            state,
            method,
            login_url: None,
            verification_uri: Some(verification_uri),
            verification_uri_complete: Some(verification_uri_complete),
            user_code: Some(user_code),
            interval_seconds: Some(interval_seconds),
            expires_at: Some(expires_at),
        })
    }

    async fn start_auth_code_login(
        &self,
        state: String,
        method: KiroLoginMethod,
    ) -> Result<KiroLoginStartResponse, String> {
        let (code_verifier, code_challenge) = generate_pkce()?;
        let callback = start_auth_code_callback(state.clone()).await?;
        let proxy_url = self.app_proxy_url().await;
        let client = SsoOidcClient::new(proxy_url.as_deref())?;
        let reg = client
            .register_client_for_auth_code(&callback.redirect_uri)
            .await?;
        let login_url = build_auth_code_url(
            &reg.client_id,
            &callback.redirect_uri,
            &state,
            &code_challenge,
        );
        let manager = self.clone();
        let state_for_task = state.clone();
        tokio::spawn(async move {
            run_auth_code_login(manager, state_for_task, reg, code_verifier, callback).await;
        });
        Ok(KiroLoginStartResponse {
            state,
            method,
            login_url: Some(login_url),
            verification_uri: None,
            verification_uri_complete: None,
            user_code: None,
            interval_seconds: None,
            expires_at: Some(expires_at_from_seconds(AUTH_CODE_TIMEOUT.as_secs() as i64)),
        })
    }

    async fn start_social_login(
        &self,
        state: String,
        method: KiroLoginMethod,
    ) -> Result<KiroLoginStartResponse, String> {
        let provider = match method {
            KiroLoginMethod::Google => "Google",
            _ => "",
        };
        let (code_verifier, code_challenge) = generate_pkce()?;
        let login_url = build_login_url(provider, KIRO_REDIRECT_URI, &code_challenge, &state);
        let manager = self.clone();
        let state_for_task = state.clone();
        tokio::spawn(async move {
            run_social_login(manager, state_for_task, provider.to_string(), code_verifier).await;
        });
        Ok(KiroLoginStartResponse {
            state,
            method,
            login_url: Some(login_url),
            verification_uri: None,
            verification_uri_complete: None,
            user_code: None,
            interval_seconds: None,
            expires_at: Some(expires_at_from_seconds(
                SOCIAL_CALLBACK_TIMEOUT.as_secs() as i64
            )),
        })
    }

    async fn insert_session(&self, state: &str, expires_at: Option<OffsetDateTime>) {
        let session = LoginSession {
            status: KiroLoginStatus::Waiting,
            error: None,
            account: None,
            expires_at,
        };
        let mut guard = self.sessions.write().await;
        guard.insert(state.to_string(), session);
    }

    async fn complete_session(&self, state: &str, account: KiroAccountSummary) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = KiroLoginStatus::Success;
            session.error = None;
            session.account = Some(account);
        }
    }

    async fn fail_session(&self, state: &str, message: String) {
        let mut guard = self.sessions.write().await;
        if let Some(session) = guard.get_mut(state) {
            session.status = KiroLoginStatus::Error;
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| format!("Failed to start callback server: {err}"))?;
    let port = listener
        .local_addr()
        .map_err(|err| format!("Failed to read callback port: {err}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback");
    let router = axum::Router::new().route(
        "/oauth/callback",
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

async fn run_device_code_login(
    manager: KiroLoginManager,
    state: String,
    reg: RegisterClientResponse,
    auth: StartDeviceAuthResponse,
) {
    let mut interval = Duration::from_secs(auth.interval.max(1) as u64);
    let deadline = OffsetDateTime::now_utc() + time::Duration::seconds(auth.expires_in.max(1));
    let proxy_url = manager.app_proxy_url().await;
    let client = match SsoOidcClient::new(proxy_url.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };

    while OffsetDateTime::now_utc() < deadline {
        tokio::time::sleep(interval).await;
        match client
            .create_token_device_code(&reg.client_id, &reg.client_secret, &auth.device_code)
            .await
        {
            Ok(token) => {
                handle_builder_success(manager, state, token, reg).await;
                return;
            }
            Err(TokenPollError::Pending) => continue,
            Err(TokenPollError::SlowDown) => {
                interval += Duration::from_secs(5);
                continue;
            }
            Err(TokenPollError::Other(err)) => {
                manager.fail_session(&state, err).await;
                return;
            }
        }
    }
    manager
        .fail_session(&state, "Authorization timed out.".to_string())
        .await;
}

async fn run_auth_code_login(
    manager: KiroLoginManager,
    state: String,
    reg: RegisterClientResponse,
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
    let client = match SsoOidcClient::new(proxy_url.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let token = match client
        .create_token_auth_code(
            &reg.client_id,
            &reg.client_secret,
            &code,
            &code_verifier,
            &redirect_uri,
        )
        .await
    {
        Ok(token) => token,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    handle_builder_success(manager, state, token, reg).await;
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

async fn run_social_login(
    manager: KiroLoginManager,
    state: String,
    provider: String,
    code_verifier: String,
) {
    let callback_path = callback_file_path(manager.store.dir(), &state);
    let deadline = OffsetDateTime::now_utc()
        + time::Duration::seconds(SOCIAL_CALLBACK_TIMEOUT.as_secs() as i64);
    loop {
        if OffsetDateTime::now_utc() > deadline {
            manager
                .fail_session(&state, "OAuth flow timed out.".to_string())
                .await;
            return;
        }
        if tokio::fs::try_exists(&callback_path).await.unwrap_or(false) {
            match read_callback_file(&callback_path).await {
                Ok(payload) => {
                    let _ = tokio::fs::remove_file(&callback_path).await;
                    if let Some(err) = payload.error {
                        manager.fail_session(&state, err).await;
                        return;
                    }
                    if payload.state.as_deref() != Some(&state) {
                        manager
                            .fail_session(&state, "OAuth state mismatch.".to_string())
                            .await;
                        return;
                    }
                    let Some(code) = payload.code else {
                        manager
                            .fail_session(&state, "Authorization code missing.".to_string())
                            .await;
                        return;
                    };
                    handle_social_success(manager, state, provider.clone(), code, code_verifier)
                        .await;
                    return;
                }
                Err(err) => {
                    manager.fail_session(&state, err).await;
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn handle_builder_success(
    manager: KiroLoginManager,
    state: String,
    token: CreateTokenResponse,
    reg: RegisterClientResponse,
) {
    let proxy_url = manager.app_proxy_url().await;
    let profile_arn = match SsoOidcClient::new(proxy_url.as_deref()) {
        Ok(client) => client.fetch_profile_arn(&token.access_token).await,
        Err(_) => None,
    };
    let record = KiroTokenRecord {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        profile_arn,
        expires_at: expires_at_from_seconds(token.expires_in),
        auth_method: "builder-id".to_string(),
        provider: "AWS".to_string(),
        client_id: Some(reg.client_id),
        client_secret: Some(reg.client_secret),
        email: None,
        last_refresh: Some(now_rfc3339()),
        start_url: None,
        region: None,
    };
    match manager.store.save_new_account(record).await {
        Ok(account) => manager.complete_session(&state, account).await,
        Err(err) => manager.fail_session(&state, err).await,
    }
}

async fn handle_social_success(
    manager: KiroLoginManager,
    state: String,
    provider: String,
    code: String,
    code_verifier: String,
) {
    let proxy_url = manager.app_proxy_url().await;
    let client = match KiroOAuthClient::new(proxy_url.as_deref()) {
        Ok(client) => client,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let token = match client
        .exchange_code(&code, &code_verifier, KIRO_REDIRECT_URI)
        .await
    {
        Ok(token) => token,
        Err(err) => {
            manager.fail_session(&state, err).await;
            return;
        }
    };
    let record = KiroTokenRecord {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        profile_arn: token.profile_arn,
        expires_at: expires_at_from_seconds(token.expires_in),
        auth_method: "social".to_string(),
        provider,
        client_id: None,
        client_secret: None,
        email: None,
        last_refresh: Some(now_rfc3339()),
        start_url: None,
        region: None,
    };
    match manager.store.save_new_account(record).await {
        Ok(account) => manager.complete_session(&state, account).await,
        Err(err) => manager.fail_session(&state, err).await,
    }
}
