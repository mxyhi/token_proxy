//! 编辑器按渠道绑定的账户获取实时模型候选，不应用路由白名单或内置目录回退。

use std::{collections::BTreeSet, time::Duration};

use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use token_proxy_account_codex as codex;
use token_proxy_account_store::oauth_util::{
    build_reqwest_client_no_redirect, normalize_proxy_url,
};
use token_proxy_account_xai as xai;

use crate::app::TokenProxyApp;

impl TokenProxyApp {
    /// 获取一个绑定账户的完整目录；账户存储负责已有的 token 到期刷新。
    pub async fn fetch_account_models(
        &self,
        provider: &str,
        account_id: &str,
        proxy_url: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let account_id = account_id.trim();
        if account_id.is_empty() {
            return Err("Account is required to fetch channel models.".to_string());
        }
        let app_proxy = self.app_proxy().read().await.clone();
        let proxy_url = resolve_model_proxy(proxy_url, app_proxy.as_deref())?;
        let client =
            build_reqwest_client_no_redirect(proxy_url.as_deref(), Duration::from_secs(30))?;
        let request = match provider {
            "codex" => {
                let (_, record) = self
                    .codex_accounts
                    .resolve_pinned_account_record(account_id)
                    .await?;
                let authorization = self.codex_accounts.authorization_header(account_id).await?;
                codex_models_request(&client, &authorization, record.account_id.as_deref())
            }
            "kiro" => {
                let (_, record) = self
                    .kiro_accounts
                    .resolve_pinned_account_record(account_id)
                    .await?;
                kiro_models_request(
                    &client,
                    &record.access_token,
                    record.profile_arn.as_deref(),
                    record.region.as_deref(),
                )?
            }
            "xai" => {
                let (_, record) = self
                    .xai_accounts
                    .resolve_pinned_account_record(account_id)
                    .await?;
                xai_models_request(&client, &record.access_token)
            }
            _ => return Err(format!("Unsupported account provider: {provider}")),
        };

        tracing::debug!(provider, "fetching bound account model catalog");
        let result = fetch_model_pages(provider, request).await;
        match &result {
            Ok(models) => tracing::info!(
                provider,
                model_count = models.len(),
                "fetched account model candidates"
            ),
            Err(error) => {
                tracing::warn!(provider, %error, "failed to fetch account model candidates")
            }
        }
        result
    }
}

fn resolve_model_proxy(
    proxy_url: Option<&str>,
    app_proxy: Option<&str>,
) -> Result<Option<String>, String> {
    let proxy_url = proxy_url.map(str::trim).filter(|value| !value.is_empty());
    let effective = match proxy_url {
        None | Some("$app_proxy_url") => app_proxy,
        explicit => explicit,
    };
    normalize_proxy_url(effective)
}

fn codex_models_request(
    client: &Client,
    authorization: &str,
    account_id: Option<&str>,
) -> RequestBuilder {
    // 官方 codex-api/endpoint/models.rs：GET models?client_version，模型键为 slug。
    let version = codex::enforce_minimum_client_version("");
    let request = client
        .get("https://chatgpt.com/backend-api/codex/models")
        .query(&[("client_version", version)])
        .header("Authorization", authorization)
        .header("Accept", "application/json")
        .header("originator", codex::DEFAULT_ORIGINATOR)
        .header("User-Agent", codex::USER_AGENT)
        .header("version", version);
    match account_id.filter(|value| !value.is_empty()) {
        Some(account_id) => request.header("chatgpt-account-id", account_id),
        None => request,
    }
}

fn kiro_models_request(
    client: &Client,
    token: &str,
    profile_arn: Option<&str>,
    region: Option<&str>,
) -> Result<RequestBuilder, String> {
    // CLIProxyAPI 的 Kiro ListAvailableModels 合同：区域取自 profile ARN，返回 models[].modelId。
    let region = profile_arn
        .and_then(|arn| arn.split(':').nth(3))
        .filter(|value| !value.is_empty())
        .or(region.filter(|value| !value.is_empty()))
        .unwrap_or("us-east-1");
    // 区域来自导入文件，只允许区域名字符，固定凭据目的主机。
    if !region
        .bytes()
        .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == b'-')
    {
        return Err("Invalid Kiro account region.".to_string());
    }
    let mut request = client
        .get(format!(
            "https://q.{region}.amazonaws.com/ListAvailableModels"
        ))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("User-Agent", "KiroIDE")
        .header("x-amz-user-agent", "aws-sdk-js/3.738.0 KiroIDE")
        .query(&[("origin", "AI_EDITOR")]);
    if let Some(profile_arn) = profile_arn.filter(|value| !value.is_empty()) {
        request = request.query(&[("profileArn", profile_arn)]);
    }
    Ok(request)
}

fn xai_models_request(client: &Client, token: &str) -> RequestBuilder {
    client
        .get(format!("{}/models", xai::CLI_BASE_URL))
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header(xai::CLI_TOKEN_AUTH_HEADER, xai::CLI_TOKEN_AUTH_VALUE)
        .header(xai::CLI_CLIENT_VERSION_HEADER, xai::CLI_CLIENT_VERSION)
        .header("User-Agent", xai::CLI_USER_AGENT)
}

async fn fetch_model_pages(provider: &str, request: RequestBuilder) -> Result<Vec<String>, String> {
    let mut models = BTreeSet::new();
    let mut next_token = None;
    let mut seen_tokens = BTreeSet::new();
    // Kiro 的 nextToken 需要继续读取；限制重复/过多页，避免刷新一直处于等待状态。
    for _ in 0..100 {
        let mut page = request
            .try_clone()
            .ok_or_else(|| "Unable to clone model request.".to_string())?;
        if let Some(token) = &next_token {
            page = page.query(&[("nextToken", token)]);
        }
        let response = page
            .send()
            .await
            .map_err(|error| format!("Model catalog request failed: {}", error.without_url()))?;
        if !response.status().is_success() {
            return Err(format!(
                "Model catalog returned HTTP {}.",
                response.status()
            ));
        }
        // 仅报告状态或解码错误，不回显供应商正文、账户身份或鉴权头。
        let body: Value = response
            .json()
            .await
            .map_err(|error| format!("Invalid model catalog: {}", error.without_url()))?;
        models.extend(extract_account_model_ids(provider, &body));
        next_token = if provider == "kiro" {
            string_field(&body, "nextToken").map(str::to_owned)
        } else {
            None
        };
        let Some(token) = &next_token else {
            return Ok(models.into_iter().collect());
        };
        if !seen_tokens.insert(token.clone()) {
            return Err("Model catalog repeated a pagination token.".to_string());
        }
    }
    Err("Model catalog exceeded the pagination limit.".to_string())
}

fn extract_account_model_ids(provider: &str, body: &Value) -> Vec<String> {
    let items = body.as_array().into_iter().flatten().chain(
        ["data", "models"]
            .into_iter()
            .filter_map(|key| body.get(key).and_then(Value::as_array))
            .flatten(),
    );
    items
        .filter_map(|item| match provider {
            "codex" => string_field(item, "slug"),
            "kiro" => string_field(item, "modelId"),
            "xai" => ["model", "modelId", "model_id", "id"]
                .into_iter()
                .find_map(|key| string_field(item, key))
                .or_else(|| {
                    item.get("_meta").and_then(|meta| {
                        ["model", "modelId", "model_id", "id", "name"]
                            .into_iter()
                            .find_map(|key| string_field(meta, key))
                    })
                })
                .or_else(|| string_field(item, "name")),
            _ => None,
        })
        .map(str::to_owned)
        .collect()
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "account_models_tests.rs"]
mod tests;
