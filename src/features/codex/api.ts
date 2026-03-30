import { invoke } from "@tauri-apps/api/core";

import type {
  CodexAccountSummary,
  CodexLoginPollResponse,
  CodexLoginStartResponse,
  CodexQuotaSummary,
} from "@/features/codex/types";

export async function listCodexAccounts() {
  return await invoke<CodexAccountSummary[]>("codex_list_accounts");
}

export async function importCodexFile(path: string) {
  return await invoke<CodexAccountSummary[]>("codex_import_file", { path });
}

export async function startCodexLogin() {
  return await invoke<CodexLoginStartResponse>("codex_start_login");
}

export async function pollCodexLogin(state: string) {
  return await invoke<CodexLoginPollResponse>("codex_poll_login", { state });
}

export async function logoutCodexAccount(accountId: string) {
  return await invoke<void>("codex_logout", { accountId });
}

export async function fetchCodexQuotas() {
  return await invoke<CodexQuotaSummary[]>("codex_fetch_quotas");
}

export async function refreshCodexAccount(accountId: string) {
  return await invoke<void>("codex_refresh_account", { accountId });
}

export async function setCodexAutoRefresh(accountId: string, enabled: boolean) {
  return await invoke<CodexAccountSummary>("codex_set_auto_refresh", {
    accountId,
    enabled,
  });
}

export async function setCodexProxyUrl(accountId: string, proxyUrl: string | null) {
  return await invoke<CodexAccountSummary>("codex_set_proxy_url", {
    accountId,
    proxyUrl,
  });
}
