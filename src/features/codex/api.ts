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

export async function importCodexText(contents: string) {
  return await invoke<CodexAccountSummary[]>("codex_import_text", { contents });
}

export async function importCodexRefreshTokens(
  contents: string,
  clientKind: "codex" | "mobile"
) {
  return await invoke<CodexAccountSummary[]>("codex_import_refresh_tokens", {
    contents,
    clientKind,
  });
}

export async function startCodexLogin() {
  return await invoke<CodexLoginStartResponse>("codex_start_login");
}

export async function pollCodexLogin(state: string) {
  return await invoke<CodexLoginPollResponse>("codex_poll_login", { state });
}

export async function fetchCodexQuotas() {
  return await invoke<CodexQuotaSummary[]>("codex_fetch_quotas");
}

export async function refreshCodexQuotaCache(accountIds?: string[]) {
  return await invoke<string[]>("codex_refresh_quota_cache", {
    accountIds: accountIds ?? null,
  });
}

export async function refreshCodexQuotaNow(accountId: string) {
  return await invoke<void>("codex_refresh_quota_now", { accountId });
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
