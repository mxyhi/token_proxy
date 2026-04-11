use std::sync::Arc;

use crate::proxy;

async fn apply_runtime_account_cooldowns(
    proxy_service: proxy::service::ProxyServiceHandle,
    items: &mut [token_proxy_core::provider_accounts::ProviderAccountListItem],
) {
    let kiro_account_ids = items
        .iter()
        .filter(|item| {
            item.provider_kind == token_proxy_core::provider_accounts::ProviderAccountKind::Kiro
                && item.status == token_proxy_core::provider_accounts::ProviderAccountStatus::Active
        })
        .map(|item| item.account_id.clone())
        .collect::<Vec<_>>();
    let codex_account_ids = items
        .iter()
        .filter(|item| {
            item.provider_kind == token_proxy_core::provider_accounts::ProviderAccountKind::Codex
                && item.status == token_proxy_core::provider_accounts::ProviderAccountStatus::Active
        })
        .map(|item| item.account_id.clone())
        .collect::<Vec<_>>();
    let cooling_kiro = proxy_service
        .cooling_account_ids("kiro", &kiro_account_ids)
        .await;
    let cooling_codex = proxy_service
        .cooling_account_ids("codex", &codex_account_ids)
        .await;

    for item in items.iter_mut() {
        if item.status != token_proxy_core::provider_accounts::ProviderAccountStatus::Active {
            continue;
        }
        let is_cooling = match item.provider_kind {
            token_proxy_core::provider_accounts::ProviderAccountKind::Kiro => {
                cooling_kiro.contains(&item.account_id)
            }
            token_proxy_core::provider_accounts::ProviderAccountKind::Codex => {
                cooling_codex.contains(&item.account_id)
            }
        };
        if is_cooling {
            item.status = token_proxy_core::provider_accounts::ProviderAccountStatus::CoolingDown;
        }
    }
}

#[tauri::command]
pub async fn providers_list_accounts_page(
    paths: tauri::State<'_, Arc<token_proxy_core::paths::TokenProxyPaths>>,
    proxy_service: tauri::State<'_, proxy::service::ProxyServiceHandle>,
    page: u32,
    page_size: u32,
    provider_kind: Option<String>,
    status: Option<String>,
    search: Option<String>,
) -> Result<token_proxy_core::provider_accounts::ProviderAccountsPage, String> {
    let provider_kind = provider_kind
        .as_deref()
        .map(token_proxy_core::provider_accounts::ProviderAccountKind::parse)
        .transpose()?;
    let status = status
        .as_deref()
        .map(token_proxy_core::provider_accounts::ProviderAccountStatus::parse)
        .transpose()?;

    let mut items = token_proxy_core::provider_accounts::list_accounts_snapshot(
        paths.inner().as_ref(),
        token_proxy_core::provider_accounts::ProviderAccountsQueryParams {
            provider_kind,
            search: search.unwrap_or_default(),
        },
    )
    .await?;
    apply_runtime_account_cooldowns(proxy_service.inner().clone(), &mut items).await;
    if let Some(status) = status {
        items.retain(|item| item.status == status);
    }

    let page = page.max(1);
    let page_size = page_size.clamp(1, token_proxy_core::provider_accounts::MAX_PAGE_SIZE);
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    let start = usize::try_from((page - 1) * page_size).unwrap_or(usize::MAX);
    let end = start.saturating_add(usize::try_from(page_size).unwrap_or(usize::MAX));
    let items = if start >= items.len() {
        Vec::new()
    } else {
        items[start..items.len().min(end)].to_vec()
    };

    Ok(token_proxy_core::provider_accounts::ProviderAccountsPage {
        items,
        total,
        page,
        page_size,
    })
}

#[tauri::command]
pub async fn providers_delete_accounts(
    paths: tauri::State<'_, Arc<token_proxy_core::paths::TokenProxyPaths>>,
    account_ids: Vec<String>,
) -> Result<(), String> {
    token_proxy_core::provider_accounts::delete_accounts(paths.inner().as_ref(), &account_ids).await
}
