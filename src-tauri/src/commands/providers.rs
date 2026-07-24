use std::sync::Arc;

use token_proxy_app::app::TokenProxyApp;

/// Providers 账户列表仍可读（UI Phase D 迁移前兼容）；批量删除路径已移除。
#[tauri::command]
pub async fn providers_list_accounts_page(
    paths: tauri::State<'_, Arc<token_proxy_account_store::paths::TokenProxyPaths>>,
    token_proxy_app: tauri::State<'_, TokenProxyApp>,
    page: u32,
    page_size: u32,
    provider_kind: Option<String>,
    status: Option<String>,
    search: Option<String>,
) -> Result<token_proxy_accounts::provider_accounts::ProviderAccountsPage, String> {
    let provider_kind = provider_kind
        .as_deref()
        .map(token_proxy_accounts::provider_accounts::ProviderAccountKind::parse)
        .transpose()?;
    let status = status
        .as_deref()
        .map(token_proxy_accounts::provider_accounts::ProviderAccountStatus::parse)
        .transpose()?;

    let mut items = token_proxy_accounts::provider_accounts::list_accounts_snapshot(
        paths.inner().as_ref(),
        token_proxy_accounts::provider_accounts::ProviderAccountsQueryParams {
            provider_kind,
            search: search.unwrap_or_default(),
        },
    )
    .await?;
    apply_runtime_account_cooldowns(token_proxy_app.inner().clone(), &mut items).await;
    let status_counts =
        token_proxy_accounts::provider_accounts::ProviderAccountStatusCounts::from_items(&items);
    if let Some(status) = status {
        items.retain(|item| item.status == status);
    }

    let page = page.max(1);
    let page_size = page_size.clamp(1, token_proxy_accounts::provider_accounts::MAX_PAGE_SIZE);
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    let start = usize::try_from((page - 1) * page_size).unwrap_or(usize::MAX);
    let end = start.saturating_add(usize::try_from(page_size).unwrap_or(usize::MAX));
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..items.len().min(end)].to_vec()
    };

    Ok(
        token_proxy_accounts::provider_accounts::ProviderAccountsPage {
            items,
            total,
            page,
            page_size,
            status_counts,
        },
    )
}

async fn apply_runtime_account_cooldowns(
    token_proxy_app: TokenProxyApp,
    items: &mut [token_proxy_accounts::provider_accounts::ProviderAccountListItem],
) {
    let kiro_account_ids = items
        .iter()
        .filter(|item| {
            item.provider_kind == token_proxy_accounts::provider_accounts::ProviderAccountKind::Kiro
                && item.status
                    == token_proxy_accounts::provider_accounts::ProviderAccountStatus::Active
        })
        .map(|item| item.account_id.clone())
        .collect::<Vec<_>>();
    let codex_account_ids = items
        .iter()
        .filter(|item| {
            item.provider_kind
                == token_proxy_accounts::provider_accounts::ProviderAccountKind::Codex
                && item.status
                    == token_proxy_accounts::provider_accounts::ProviderAccountStatus::Active
        })
        .map(|item| item.account_id.clone())
        .collect::<Vec<_>>();
    let xai_account_ids = items
        .iter()
        .filter(|item| {
            item.provider_kind == token_proxy_accounts::provider_accounts::ProviderAccountKind::Xai
                && item.status
                    == token_proxy_accounts::provider_accounts::ProviderAccountStatus::Active
        })
        .map(|item| item.account_id.clone())
        .collect::<Vec<_>>();
    // 三类账户冷却查询互不依赖，并行读取运行时状态，避免账户页延迟随 provider 数增长。
    let (cooling_kiro, cooling_codex, cooling_xai) = tokio::join!(
        token_proxy_app.cooling_account_ids("kiro", &kiro_account_ids),
        token_proxy_app.cooling_account_ids("codex", &codex_account_ids),
        token_proxy_app.cooling_account_ids("xai", &xai_account_ids),
    );

    for item in items.iter_mut() {
        if item.status != token_proxy_accounts::provider_accounts::ProviderAccountStatus::Active {
            continue;
        }
        let is_cooling = match item.provider_kind {
            token_proxy_accounts::provider_accounts::ProviderAccountKind::Kiro => {
                cooling_kiro.contains(&item.account_id)
            }
            token_proxy_accounts::provider_accounts::ProviderAccountKind::Codex => {
                cooling_codex.contains(&item.account_id)
            }
            token_proxy_accounts::provider_accounts::ProviderAccountKind::Xai => {
                cooling_xai.contains(&item.account_id)
            }
        };
        if is_cooling {
            item.status =
                token_proxy_accounts::provider_accounts::ProviderAccountStatus::CoolingDown;
        }
    }
}

// Phase C2: providers_delete_accounts 已删除；账户删除只能由删除 Upstream 触发。
