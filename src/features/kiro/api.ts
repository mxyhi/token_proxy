import { invoke } from "@tauri-apps/api/core";

import type {
  KiroAccountSummary,
  KiroLoginMethod,
  KiroLoginPollResponse,
  KiroLoginStartResponse,
  KiroQuotaSummary,
} from "@/features/kiro/types";

export async function listKiroAccounts() {
  return await invoke<KiroAccountSummary[]>("kiro_list_accounts");
}

export async function startKiroLogin(method: KiroLoginMethod) {
  return await invoke<KiroLoginStartResponse>("kiro_start_login", { method });
}

export async function pollKiroLogin(state: string) {
  return await invoke<KiroLoginPollResponse>("kiro_poll_login", { state });
}

export async function importKiroIdeTokens(directory: string) {
  return await invoke<KiroAccountSummary[]>("kiro_import_ide", { directory });
}

export async function importKiroKamTokens(path: string) {
  return await invoke<KiroAccountSummary[]>("kiro_import_kam", { path });
}

export async function logoutKiroAccount(accountId: string) {
  await invoke<void>("kiro_logout", { accountId });
}

export async function handleKiroCallback(url: string) {
  await invoke<void>("kiro_handle_callback", { url });
}

export async function fetchKiroQuotas() {
  return await invoke<KiroQuotaSummary[]>("kiro_fetch_quotas");
}

export async function setKiroProxyUrl(accountId: string, proxyUrl: string | null) {
  return await invoke<KiroAccountSummary>("kiro_set_proxy_url", {
    accountId,
    proxyUrl,
  });
}
