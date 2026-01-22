import { invoke } from "@tauri-apps/api/core";

import type {
  AntigravityAccountSummary,
  AntigravityIdeStatus,
  AntigravityLoginPollResponse,
  AntigravityLoginStartResponse,
  AntigravityQuotaSummary,
  AntigravityWarmupScheduleSummary,
} from "@/features/antigravity/types";

export async function listAntigravityAccounts() {
  return await invoke<AntigravityAccountSummary[]>("antigravity_list_accounts");
}

export async function startAntigravityLogin() {
  return await invoke<AntigravityLoginStartResponse>("antigravity_start_login");
}

export async function pollAntigravityLogin(state: string) {
  return await invoke<AntigravityLoginPollResponse>("antigravity_poll_login", { state });
}

export async function logoutAntigravityAccount(accountId: string) {
  return await invoke<void>("antigravity_logout", { accountId });
}

export async function importAntigravityIde(ideDbPath?: string) {
  return await invoke<AntigravityAccountSummary[]>("antigravity_import_ide", { ideDbPath });
}

export async function switchAntigravityIdeAccount(accountId: string, ideDbPath?: string) {
  return await invoke<AntigravityIdeStatus>("antigravity_switch_ide_account", {
    accountId,
    ideDbPath,
  });
}

export async function fetchAntigravityIdeStatus() {
  return await invoke<AntigravityIdeStatus>("antigravity_ide_status");
}

export async function fetchAntigravityQuotas() {
  return await invoke<AntigravityQuotaSummary[]>("antigravity_fetch_quotas");
}

export async function runAntigravityWarmup(
  accountId: string,
  model: string,
  stream = false
) {
  return await invoke<void>("antigravity_run_warmup", { accountId, model, stream });
}

export async function listAntigravityWarmupSchedules() {
  return await invoke<AntigravityWarmupScheduleSummary[]>(
    "antigravity_list_warmup_schedules"
  );
}

export async function setAntigravityWarmupSchedule(
  accountId: string,
  model: string,
  intervalMinutes: number,
  enabled: boolean
) {
  return await invoke<AntigravityWarmupScheduleSummary>(
    "antigravity_set_warmup_schedule",
    { accountId, model, intervalMinutes, enabled }
  );
}

export async function toggleAntigravityWarmupSchedule(
  accountId: string,
  model: string,
  enabled: boolean
) {
  return await invoke<void>("antigravity_toggle_warmup_schedule", {
    accountId,
    model,
    enabled,
  });
}
