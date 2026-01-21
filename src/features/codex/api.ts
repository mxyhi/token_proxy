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
