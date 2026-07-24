//! Account-as-Upstream 纯领域：账户引用与配置 reconcile（无 I/O、无 Tauri）。
//!
//! 目标：每个 Kiro/Codex/xAI account 在配置中恰好有一条 Account credential Upstream；
//! 已有绑定整体保留；缺失则追加确定性默认 Upstream。不删账户、不写盘、不 reload。

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};
use token_proxy_config::{AccountProvider, ProxyConfigFile, UpstreamConfig, UpstreamCredential};

/// 默认 Upstream id 使用 SHA-256(account_id) 前 16 hex，不可逆且稳定。
const ACCOUNT_UPSTREAM_ID_HASH_HEX_LEN: usize = 16;

/// 账户引用：provider + account_id（账户标识，非 OAuth token）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountUpstreamRef {
    pub provider: AccountProvider,
    pub account_id: String,
}

impl AccountUpstreamRef {
    pub fn new(provider: AccountProvider, account_id: impl Into<String>) -> Self {
        Self {
            provider,
            account_id: account_id.into(),
        }
    }
}

/// 已落盘/将落盘的 Account 绑定（含 upstream id；不含 secret）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountUpstreamBinding {
    pub provider: AccountProvider,
    pub account_id: String,
    pub upstream_id: String,
}

/// reconcile 结果：新配置 + 是否变更 + 本次新增绑定列表。
/// 不含 Debug：`ProxyConfigFile` 未实现 Debug（避免凭据误入日志）。
#[derive(Clone)]
pub struct ReconcileAccountUpstreamsResult {
    pub config: ProxyConfigFile,
    pub changed: bool,
    pub added: Vec<AccountUpstreamBinding>,
}

/// 为每个账户保证恰好一条 Account credential Upstream。
///
/// - 输入 accounts：trim + 去重；空 `account_id` 硬失败。
/// - 已存在相同 (provider, account_id) 绑定：该 Upstream 全部字段原样保留。
/// - 缺失：追加默认 Upstream（`base_url=""` 由 normalize 解析官方端点）。
/// - 不删除任何现有 Upstream（级联删除留给 Phase C2）。
pub fn reconcile_account_upstreams(
    config: &ProxyConfigFile,
    accounts: impl IntoIterator<Item = AccountUpstreamRef>,
) -> Result<ReconcileAccountUpstreamsResult, String> {
    let accounts = normalize_account_refs(accounts)?;
    let mut next = config.clone();
    let mut taken_ids = existing_upstream_ids(&next.upstreams);
    let mut added = Vec::new();

    for account in &accounts {
        if find_account_binding_index(&next.upstreams, account.provider, &account.account_id)
            .is_some()
        {
            continue;
        }

        let upstream_id =
            allocate_default_upstream_id(account.provider, &account.account_id, &taken_ids);
        taken_ids.insert(upstream_id.clone());
        let upstream =
            default_account_upstream(&upstream_id, account.provider, account.account_id.as_str());
        next.upstreams.push(upstream);
        added.push(AccountUpstreamBinding {
            provider: account.provider,
            account_id: account.account_id.clone(),
            upstream_id,
        });
    }

    let changed = !added.is_empty();
    if changed {
        // 仅变更时 debug；account_id 为引用标识，非 token。
        tracing::debug!(
            added_count = added.len(),
            added = ?added
                .iter()
                .map(|binding| {
                    (
                        binding.provider.as_str(),
                        binding.account_id.as_str(),
                        binding.upstream_id.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            "reconciled account upstreams"
        );
    }

    Ok(ReconcileAccountUpstreamsResult {
        config: next,
        changed,
        added,
    })
}

/// 计算 old → new 中被移除的 Account 绑定（provider+account_id 精确比较）。
/// 供 Phase C2 级联删除账户侧资源；本函数不做 I/O。
pub fn removed_account_bindings(
    old_config: &ProxyConfigFile,
    new_config: &ProxyConfigFile,
) -> Vec<AccountUpstreamRef> {
    let new_keys: HashSet<AccountUpstreamRef> =
        collect_account_refs(new_config).into_iter().collect();
    let mut removed = Vec::new();
    let mut seen = HashSet::new();
    for reference in collect_account_refs(old_config) {
        if new_keys.contains(&reference) {
            continue;
        }
        if seen.insert(reference.clone()) {
            removed.push(reference);
        }
    }
    removed
}

/// trim + 去重；空 account_id 硬失败。保留首次出现顺序。
fn normalize_account_refs(
    accounts: impl IntoIterator<Item = AccountUpstreamRef>,
) -> Result<Vec<AccountUpstreamRef>, String> {
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for account in accounts {
        let account_id = account.account_id.trim();
        if account_id.is_empty() {
            return Err(format!(
                "account_id cannot be empty for provider {}.",
                account.provider.as_str()
            ));
        }
        let reference = AccountUpstreamRef {
            provider: account.provider,
            account_id: account_id.to_string(),
        };
        if !seen.insert(reference.clone()) {
            continue;
        }
        output.push(reference);
    }
    Ok(output)
}

fn collect_account_refs(config: &ProxyConfigFile) -> Vec<AccountUpstreamRef> {
    let mut output = Vec::new();
    for upstream in &config.upstreams {
        let Some((provider, account_id)) = upstream.credential.account_binding() else {
            continue;
        };
        let account_id = account_id.trim();
        if account_id.is_empty() {
            continue;
        }
        output.push(AccountUpstreamRef {
            provider,
            account_id: account_id.to_string(),
        });
    }
    output
}

fn existing_upstream_ids(upstreams: &[UpstreamConfig]) -> HashSet<String> {
    upstreams
        .iter()
        .map(|upstream| upstream.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect()
}

fn find_account_binding_index(
    upstreams: &[UpstreamConfig],
    provider: AccountProvider,
    account_id: &str,
) -> Option<usize> {
    upstreams.iter().position(|upstream| {
        matches!(
            &upstream.credential,
            UpstreamCredential::Account {
                provider: bound_provider,
                account_id: bound_id,
            } if *bound_provider == provider && bound_id.trim() == account_id
        )
    })
}

/// 确定性默认 id：`{provider}-{sha256(account_id)[0..16 hex]}`；碰撞则稳定递增 suffix。
fn allocate_default_upstream_id(
    provider: AccountProvider,
    account_id: &str,
    taken: &HashSet<String>,
) -> String {
    let base = default_upstream_id_base(provider, account_id);
    if !taken.contains(&base) {
        return base;
    }
    // 与前端 copy id 一致：碰撞后从 -2 起递增，永不覆盖。
    let mut suffix = 2_u64;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
        if suffix == 0 {
            // 理论上不可达；避免死循环。
            return format!("{base}-overflow");
        }
    }
}

fn default_upstream_id_base(provider: AccountProvider, account_id: &str) -> String {
    let digest = {
        let mut hasher = Sha256::new();
        hasher.update(account_id.as_bytes());
        hasher.finalize()
    };
    // sha2 0.11 Output 无 LowerHex，手动拼固定宽度 hex。
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}",
        provider.as_str(),
        &hex[..ACCOUNT_UPSTREAM_ID_HASH_HEX_LEN]
    )
}

fn default_account_upstream(
    id: &str,
    provider: AccountProvider,
    account_id: &str,
) -> UpstreamConfig {
    UpstreamConfig {
        id: id.to_string(),
        providers: vec![provider.as_str().to_string()],
        base_url: String::new(),
        credential: UpstreamCredential::account(provider, account_id),
        filter_prompt_cache_retention: false,
        filter_safety_identifier: false,
        use_chat_completions_for_responses: false,
        rewrite_developer_role_to_system: false,
        preferred_endpoint: None,
        proxy_url: None,
        priority: Some(0),
        enabled: true,
        available_models: Vec::new(),
        model_mappings: HashMap::new(),
        convert_from_map: HashMap::new(),
        overrides: None,
    }
}

// 供测试直接断言默认 id 算法，不暴露为公共 API。
#[cfg(test)]
fn default_upstream_id_for_test(provider: AccountProvider, account_id: &str) -> String {
    default_upstream_id_base(provider, account_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use token_proxy_config::UpstreamOverrides;

    fn sample_upstream(
        id: &str,
        providers: &[&str],
        base_url: &str,
        credential: UpstreamCredential,
    ) -> UpstreamConfig {
        UpstreamConfig {
            id: id.to_string(),
            providers: providers.iter().map(|value| (*value).to_string()).collect(),
            base_url: base_url.to_string(),
            credential,
            filter_prompt_cache_retention: false,
            filter_safety_identifier: false,
            use_chat_completions_for_responses: false,
            rewrite_developer_role_to_system: false,
            preferred_endpoint: None,
            proxy_url: None,
            priority: Some(0),
            enabled: true,
            available_models: Vec::new(),
            model_mappings: HashMap::new(),
            convert_from_map: HashMap::new(),
            overrides: None,
        }
    }

    fn empty_config() -> ProxyConfigFile {
        ProxyConfigFile::default()
    }

    #[test]
    fn reconcile_generates_default_fields_and_stable_ids_for_three_providers() {
        let config = empty_config();
        let accounts = vec![
            AccountUpstreamRef::new(AccountProvider::Kiro, "kiro-user-1"),
            AccountUpstreamRef::new(AccountProvider::Codex, "codex-user-1"),
            AccountUpstreamRef::new(AccountProvider::Xai, "xai-user-1"),
        ];

        let result = reconcile_account_upstreams(&config, accounts).expect("reconcile");
        assert!(result.changed);
        assert_eq!(result.added.len(), 3);
        assert_eq!(result.config.upstreams.len(), 3);

        for (upstream, expected_provider, expected_account) in [
            (
                &result.config.upstreams[0],
                AccountProvider::Kiro,
                "kiro-user-1",
            ),
            (
                &result.config.upstreams[1],
                AccountProvider::Codex,
                "codex-user-1",
            ),
            (
                &result.config.upstreams[2],
                AccountProvider::Xai,
                "xai-user-1",
            ),
        ] {
            let expected_id = default_upstream_id_for_test(expected_provider, expected_account);
            assert_eq!(upstream.id, expected_id);
            assert_eq!(
                upstream.providers,
                vec![expected_provider.as_str().to_string()]
            );
            assert_eq!(upstream.base_url, "");
            assert_eq!(upstream.priority, Some(0));
            assert!(upstream.enabled);
            assert!(upstream.proxy_url.is_none());
            assert!(upstream.available_models.is_empty());
            assert!(upstream.model_mappings.is_empty());
            assert!(upstream.convert_from_map.is_empty());
            assert!(upstream.overrides.is_none());
            assert!(!upstream.filter_prompt_cache_retention);
            assert!(!upstream.filter_safety_identifier);
            assert!(!upstream.use_chat_completions_for_responses);
            assert!(!upstream.rewrite_developer_role_to_system);
            assert_eq!(
                upstream.credential,
                UpstreamCredential::account(expected_provider, expected_account)
            );
        }

        // 二次 reconcile：id 与字段保持稳定。
        let again = reconcile_account_upstreams(
            &result.config,
            vec![
                AccountUpstreamRef::new(AccountProvider::Kiro, "kiro-user-1"),
                AccountUpstreamRef::new(AccountProvider::Codex, "codex-user-1"),
                AccountUpstreamRef::new(AccountProvider::Xai, "xai-user-1"),
            ],
        )
        .expect("second reconcile");
        assert!(!again.changed);
        assert!(again.added.is_empty());
        assert_eq!(
            serde_json::to_value(&again.config.upstreams).expect("ser"),
            serde_json::to_value(&result.config.upstreams).expect("ser")
        );
    }

    #[test]
    fn reconcile_dedupes_duplicate_accounts_and_trims() {
        let config = empty_config();
        let result = reconcile_account_upstreams(
            &config,
            vec![
                AccountUpstreamRef::new(AccountProvider::Kiro, "  acc-a  "),
                AccountUpstreamRef::new(AccountProvider::Kiro, "acc-a"),
                AccountUpstreamRef::new(AccountProvider::Kiro, "acc-a"),
            ],
        )
        .expect("reconcile");

        assert!(result.changed);
        assert_eq!(result.added.len(), 1);
        assert_eq!(result.config.upstreams.len(), 1);
        assert_eq!(
            result.config.upstreams[0].credential,
            UpstreamCredential::account(AccountProvider::Kiro, "acc-a")
        );
    }

    #[test]
    fn reconcile_rejects_empty_account_id() {
        let error = reconcile_account_upstreams(
            &empty_config(),
            vec![AccountUpstreamRef::new(AccountProvider::Codex, "   ")],
        )
        .err()
        .expect("empty account_id must fail");
        assert!(error.contains("account_id cannot be empty"));
        assert!(error.contains("codex"));
    }

    #[test]
    fn reconcile_preserves_existing_binding_routing_bytes() {
        let mut existing = sample_upstream(
            "custom-kiro",
            &["kiro"],
            "https://example.invalid/kiro",
            UpstreamCredential::account(AccountProvider::Kiro, "keep-me"),
        );
        existing.priority = Some(42);
        existing.enabled = false;
        existing.proxy_url = Some("socks5://127.0.0.1:1080".to_string());
        existing.available_models = vec!["m1".to_string()];
        existing
            .model_mappings
            .insert("alias".to_string(), "m1".to_string());
        existing.filter_prompt_cache_retention = true;
        existing.rewrite_developer_role_to_system = true;
        existing.overrides = Some(UpstreamOverrides {
            header: HashMap::from([("X-Custom".to_string(), Some("v".to_string()))]),
        });

        let mut config = empty_config();
        config.upstreams = vec![
            sample_upstream(
                "openai-key",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::api_keys(["sk-live-secret"]),
            ),
            existing.clone(),
            sample_upstream(
                "openai-pass",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::Passthrough,
            ),
        ];
        let before = serde_json::to_value(&config.upstreams).expect("before");

        let result = reconcile_account_upstreams(
            &config,
            vec![AccountUpstreamRef::new(AccountProvider::Kiro, "keep-me")],
        )
        .expect("reconcile");

        assert!(!result.changed);
        assert!(result.added.is_empty());
        let after = serde_json::to_value(&result.config.upstreams).expect("after");
        assert_eq!(before, after);

        // 路由字段逐项核对（含禁用/自定义 base_url/proxy）。
        let kept = &result.config.upstreams[1];
        assert_eq!(kept.id, "custom-kiro");
        assert_eq!(kept.base_url, "https://example.invalid/kiro");
        assert_eq!(kept.priority, Some(42));
        assert!(!kept.enabled);
        assert_eq!(kept.proxy_url.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(kept.available_models, vec!["m1".to_string()]);
        assert_eq!(
            kept.model_mappings.get("alias").map(String::as_str),
            Some("m1")
        );
        assert!(kept.filter_prompt_cache_retention);
        assert!(kept.rewrite_developer_role_to_system);
        assert!(kept.overrides.is_some());
    }

    #[test]
    fn reconcile_uses_stable_suffix_on_id_collision() {
        let account_id = "collide-me";
        let base = default_upstream_id_for_test(AccountProvider::Xai, account_id);
        let mut config = empty_config();
        // 占住 base 与 base-2，新绑定应落到 base-3。
        config.upstreams = vec![
            sample_upstream(
                &base,
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::api_keys(["sk-other"]),
            ),
            sample_upstream(
                &format!("{base}-2"),
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::Passthrough,
            ),
        ];

        let result = reconcile_account_upstreams(
            &config,
            vec![AccountUpstreamRef::new(AccountProvider::Xai, account_id)],
        )
        .expect("reconcile");

        assert!(result.changed);
        assert_eq!(result.added.len(), 1);
        let expected_id = format!("{base}-3");
        assert_eq!(result.added[0].upstream_id, expected_id);
        assert_eq!(
            result.config.upstreams.last().map(|u| u.id.as_str()),
            Some(expected_id.as_str())
        );
        // 既有 id 不被覆盖。
        assert_eq!(result.config.upstreams[0].id, base);
        assert_eq!(result.config.upstreams[1].id, format!("{base}-2"));
    }

    #[test]
    fn removed_account_bindings_is_exact_provider_and_account_id() {
        let mut old = empty_config();
        old.upstreams = vec![
            sample_upstream(
                "k1",
                &["kiro"],
                "",
                UpstreamCredential::account(AccountProvider::Kiro, "a"),
            ),
            sample_upstream(
                "c1",
                &["codex"],
                "",
                UpstreamCredential::account(AccountProvider::Codex, "b"),
            ),
            sample_upstream(
                "x1",
                &["xai"],
                "",
                UpstreamCredential::account(AccountProvider::Xai, "c"),
            ),
            sample_upstream(
                "api",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::api_keys(["sk-1"]),
            ),
        ];
        let mut new = empty_config();
        new.upstreams = vec![
            // 同 account 不同 upstream id：不算 removed。
            sample_upstream(
                "k1-renamed",
                &["kiro"],
                "",
                UpstreamCredential::account(AccountProvider::Kiro, "a"),
            ),
            // codex b 消失；xai c 仍在；新增 codex d。
            sample_upstream(
                "x1",
                &["xai"],
                "",
                UpstreamCredential::account(AccountProvider::Xai, "c"),
            ),
            sample_upstream(
                "c2",
                &["codex"],
                "",
                UpstreamCredential::account(AccountProvider::Codex, "d"),
            ),
            sample_upstream(
                "api",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::api_keys(["sk-1"]),
            ),
        ];

        let removed = removed_account_bindings(&old, &new);
        assert_eq!(
            removed,
            vec![AccountUpstreamRef::new(AccountProvider::Codex, "b")]
        );
    }

    #[test]
    fn reconcile_leaves_api_key_and_passthrough_untouched_when_adding_account() {
        let mut config = empty_config();
        config.upstreams = vec![
            sample_upstream(
                "openai-key",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::api_keys(["sk-live-secret"]),
            ),
            sample_upstream(
                "openai-pass",
                &["openai"],
                "https://api.openai.com/v1",
                UpstreamCredential::Passthrough,
            ),
        ];
        let before_key = serde_json::to_value(&config.upstreams[0]).expect("key");
        let before_pass = serde_json::to_value(&config.upstreams[1]).expect("pass");

        let result = reconcile_account_upstreams(
            &config,
            vec![AccountUpstreamRef::new(AccountProvider::Codex, "codex-1")],
        )
        .expect("reconcile");

        assert!(result.changed);
        assert_eq!(result.config.upstreams.len(), 3);
        assert_eq!(
            serde_json::to_value(&result.config.upstreams[0]).expect("key after"),
            before_key
        );
        assert_eq!(
            serde_json::to_value(&result.config.upstreams[1]).expect("pass after"),
            before_pass
        );
        assert!(matches!(
            result.config.upstreams[2].credential,
            UpstreamCredential::Account {
                provider: AccountProvider::Codex,
                ..
            }
        ));
    }
}
