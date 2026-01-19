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

export async function importKiroIdeTokens() {
  return await invoke<KiroAccountSummary[]>("kiro_import_ide");
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
