use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CodexQuotaItem {
    pub name: String,
    pub percentage: f64,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub reset_at: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CodexQuotaCache {
    pub plan_type: Option<String>,
    #[serde(default)]
    pub quotas: Vec<CodexQuotaItem>,
    pub error: Option<String>,
    pub checked_at: Option<String>,
}

/// 账户认证健康状态（只读投影）。
/// 调度 enabled/priority/proxy 只属于 Upstream；此处不承载人工 disabled。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexAccountStatus {
    Active,
    Expired,
    Invalid,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexAuthMethod {
    Oauth,
    AgentIdentity,
}

impl CodexAuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::AgentIdentity => "agent_identity",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CodexCredential {
    Oauth {
        access_token: String,
        refresh_token: String,
        #[serde(default)]
        client_id: Option<String>,
        #[serde(default)]
        id_token: String,
        #[serde(default = "default_auto_refresh_enabled")]
        auto_refresh_enabled: bool,
        #[serde(default)]
        openai_device_id: Option<String>,
        expires_at: String,
        last_refresh: Option<String>,
    },
    AgentIdentity {
        agent_runtime_id: String,
        agent_private_key: String,
        #[serde(default)]
        task_id: Option<String>,
        #[serde(default)]
        plan_type: Option<String>,
        #[serde(default)]
        chatgpt_account_is_fedramp: bool,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CodexTokenRecord {
    pub credential: CodexCredential,
    #[serde(default = "default_account_status")]
    pub status: CodexAccountStatus,
    pub account_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    pub email: Option<String>,
    #[serde(default)]
    pub quota: CodexQuotaCache,
}

impl CodexTokenRecord {
    pub fn auth_method(&self) -> CodexAuthMethod {
        match self.credential {
            CodexCredential::Oauth { .. } => CodexAuthMethod::Oauth,
            CodexCredential::AgentIdentity { .. } => CodexAuthMethod::AgentIdentity,
        }
    }

    pub fn oauth(&self) -> Option<CodexOAuthCredentialRef<'_>> {
        let CodexCredential::Oauth {
            access_token,
            refresh_token,
            client_id,
            id_token,
            auto_refresh_enabled,
            openai_device_id,
            expires_at,
            last_refresh,
        } = &self.credential
        else {
            return None;
        };
        Some(CodexOAuthCredentialRef {
            access_token,
            refresh_token,
            client_id: client_id.as_deref(),
            id_token,
            auto_refresh_enabled: *auto_refresh_enabled,
            openai_device_id: openai_device_id.as_deref(),
            expires_at,
            last_refresh: last_refresh.as_deref(),
        })
    }

    pub fn oauth_mut(&mut self) -> Option<CodexOAuthCredentialMut<'_>> {
        let CodexCredential::Oauth {
            access_token,
            refresh_token,
            client_id,
            id_token,
            auto_refresh_enabled,
            openai_device_id,
            expires_at,
            last_refresh,
        } = &mut self.credential
        else {
            return None;
        };
        Some(CodexOAuthCredentialMut {
            access_token,
            refresh_token,
            client_id,
            id_token,
            auto_refresh_enabled,
            openai_device_id,
            expires_at,
            last_refresh,
        })
    }

    pub fn agent_identity(&self) -> Option<CodexAgentIdentityRef<'_>> {
        let CodexCredential::AgentIdentity {
            agent_runtime_id,
            agent_private_key,
            task_id,
            plan_type,
            chatgpt_account_is_fedramp,
        } = &self.credential
        else {
            return None;
        };
        Some(CodexAgentIdentityRef {
            agent_runtime_id,
            agent_private_key,
            task_id: task_id.as_deref(),
            plan_type: plan_type.as_deref(),
            chatgpt_account_is_fedramp: *chatgpt_account_is_fedramp,
        })
    }

    pub fn expires_at(&self) -> Option<OffsetDateTime> {
        let value = self.oauth()?.expires_at.trim();
        if value.is_empty() {
            return None;
        }
        OffsetDateTime::parse(value, &Rfc3339).ok()
    }

    pub fn expires_at_str(&self) -> Option<&str> {
        self.oauth()
            .map(|credential| credential.expires_at.trim())
            .filter(|value| !value.is_empty())
    }

    pub fn auto_refresh_enabled(&self) -> Option<bool> {
        self.oauth()
            .map(|credential| credential.auto_refresh_enabled)
    }

    pub fn is_expired(&self) -> bool {
        if self.auth_method() == CodexAuthMethod::AgentIdentity {
            return false;
        }
        let Some(expires_at) = self.expires_at() else {
            return true;
        };
        OffsetDateTime::now_utc() >= expires_at
    }

    /// 健康状态：Invalid 保留人工/鉴权失败标记；否则由 token 到期推导。
    pub fn effective_status(&self) -> CodexAccountStatus {
        if self.status == CodexAccountStatus::Invalid {
            return CodexAccountStatus::Invalid;
        }
        if self.is_expired() {
            CodexAccountStatus::Expired
        } else {
            CodexAccountStatus::Active
        }
    }

    pub fn is_usable(&self) -> bool {
        matches!(self.effective_status(), CodexAccountStatus::Active)
    }
}

pub struct CodexOAuthCredentialRef<'a> {
    pub access_token: &'a str,
    pub refresh_token: &'a str,
    pub client_id: Option<&'a str>,
    pub id_token: &'a str,
    pub auto_refresh_enabled: bool,
    pub openai_device_id: Option<&'a str>,
    pub expires_at: &'a str,
    pub last_refresh: Option<&'a str>,
}

pub struct CodexOAuthCredentialMut<'a> {
    pub access_token: &'a mut String,
    pub refresh_token: &'a mut String,
    pub client_id: &'a mut Option<String>,
    pub id_token: &'a mut String,
    pub auto_refresh_enabled: &'a mut bool,
    pub openai_device_id: &'a mut Option<String>,
    pub expires_at: &'a mut String,
    pub last_refresh: &'a mut Option<String>,
}

#[derive(Clone, Copy)]
pub struct CodexAgentIdentityRef<'a> {
    pub agent_runtime_id: &'a str,
    pub agent_private_key: &'a str,
    pub task_id: Option<&'a str>,
    pub plan_type: Option<&'a str>,
    pub chatgpt_account_is_fedramp: bool,
}

#[derive(Clone, Serialize)]
pub struct CodexAccountSummary {
    pub account_id: String,
    pub email: Option<String>,
    pub expires_at: Option<String>,
    pub status: CodexAccountStatus,
    pub auth_method: CodexAuthMethod,
    pub auto_refresh_enabled: Option<bool>,
}

fn default_auto_refresh_enabled() -> bool {
    true
}

fn default_account_status() -> CodexAccountStatus {
    CodexAccountStatus::Active
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodexLoginStatus {
    Waiting,
    Success,
    Error,
}

#[derive(Clone, Serialize)]
pub struct CodexLoginStartResponse {
    pub state: String,
    pub login_url: String,
    pub interval_seconds: u64,
    pub expires_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct CodexLoginPollResponse {
    pub state: String,
    pub status: CodexLoginStatus,
    pub error: Option<String>,
    pub account: Option<CodexAccountSummary>,
}

#[derive(Clone, Serialize)]
pub struct CodexQuotaSummary {
    pub account_id: String,
    pub plan_type: Option<String>,
    pub quotas: Vec<CodexQuotaItem>,
    pub error: Option<String>,
}
