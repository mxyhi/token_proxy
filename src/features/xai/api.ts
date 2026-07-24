import { invoke } from "@tauri-apps/api/core";

import type {
  XaiAccountSummary,
  XaiLoginPollResponse,
  XaiLoginStartResponse,
  XaiQuotaSummary,
} from "@/features/xai/types";

export async function listXaiAccounts() {
  return await invoke<XaiAccountSummary[]>("xai_list_accounts");
}

export async function importXaiFile(path: string) {
  return await invoke<XaiAccountSummary[]>("xai_import_file", { path });
}

export async function importXaiText(contents: string) {
  return await invoke<XaiAccountSummary[]>("xai_import_text", { contents });
}

export async function importXaiRefreshTokens(contents: string) {
  return await invoke<XaiAccountSummary[]>("xai_import_refresh_tokens", { contents });
}

export async function startXaiLogin() {
  return await invoke<XaiLoginStartResponse>("xai_start_login");
}

export async function pollXaiLogin(state: string) {
  return await invoke<XaiLoginPollResponse>("xai_poll_login", { state });
}

export async function cancelXaiLogin(state: string) {
  return await invoke<void>("xai_cancel_login", { state });
}

export async function fetchXaiQuotas() {
  return await invoke<XaiQuotaSummary[]>("xai_fetch_quotas");
}

export async function refreshXaiQuotaCache(accountIds?: string[]) {
  return await invoke<string[]>("xai_refresh_quota_cache", {
    accountIds: accountIds ?? null,
  });
}

export async function refreshXaiQuotaNow(accountId: string) {
  return await invoke<void>("xai_refresh_quota_now", { accountId });
}

export async function refreshXaiAccount(accountId: string) {
  return await invoke<void>("xai_refresh_account", { accountId });
}

export async function setXaiAutoRefresh(accountId: string, enabled: boolean) {
  return await invoke<XaiAccountSummary>("xai_set_auto_refresh", {
    accountId,
    enabled,
  });
}
