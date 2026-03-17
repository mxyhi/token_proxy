import type { ColumnVisibility, UpstreamColumnDefinition } from "@/features/config/cards/upstreams/types";
import { m } from "@/paraglide/messages.js";

export const UPSTREAM_COLUMNS: readonly UpstreamColumnDefinition[] = [
  {
    id: "id",
    label: () => m.upstreams_column_id(),
    defaultVisible: true,
    headerClassName: "w-[12rem]",
    cellClassName: "w-[12rem] max-w-[12rem]",
  },
  {
    id: "provider",
    label: () => m.upstreams_column_provider(),
    defaultVisible: true,
    headerClassName: "w-[10rem]",
    cellClassName: "w-[10rem] max-w-[10rem]",
  },
  {
    id: "account",
    label: () => m.upstreams_column_account(),
    defaultVisible: true,
    headerClassName: "w-[7.5rem]",
    cellClassName: "w-[7.5rem] max-w-[7.5rem]",
  },
  { id: "baseUrl", label: () => m.upstreams_column_base_url(), defaultVisible: false, cellClassName: "min-w-[18rem]" },
  { id: "apiKeys", label: () => m.upstreams_column_api_key(), defaultVisible: false, cellClassName: "min-w-[18rem]" },
  { id: "proxyUrl", label: () => m.upstreams_column_proxy_url(), defaultVisible: false, cellClassName: "min-w-[18rem]" },
  {
    id: "priority",
    label: () => m.upstreams_column_priority(),
    defaultVisible: true,
    headerClassName: "w-[6rem]",
    cellClassName: "w-[6rem]",
  },
  {
    id: "status",
    label: () => m.upstreams_column_status(),
    defaultVisible: true,
    headerClassName: "w-[8rem]",
    cellClassName: "w-[8rem]",
  },
];

export function createDefaultColumnVisibility() {
  const visibility: ColumnVisibility = {
    id: true,
    provider: true,
    account: true,
    baseUrl: false,
    apiKeys: false,
    proxyUrl: false,
    priority: true,
    status: true,
  };
  for (const column of UPSTREAM_COLUMNS) {
    visibility[column.id] = column.defaultVisible;
  }
  return visibility;
}

const DEFAULT_PROVIDER_OPTIONS = [
  "openai",
  "openai-response",
  "anthropic",
  "gemini",
  "kiro",
  "codex",
  "antigravity",
] as const;

export function mergeProviderOptions(values: readonly string[]) {
  const seen = new Set<string>();
  const merged: string[] = [];
  for (const option of DEFAULT_PROVIDER_OPTIONS) {
    if (!seen.has(option)) {
      seen.add(option);
      merged.push(option);
    }
  }
  for (const option of values) {
    if (!seen.has(option)) {
      seen.add(option);
      merged.push(option);
    }
  }
  return merged;
}

export function toMaskedApiKey(value: string) {
  return value.trim() ? "••••••••" : "";
}

export function toMaskedProxyUrl(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return "";
  }
  if (trimmed === "$app_proxy_url") {
    return trimmed;
  }
  try {
    const url = new URL(trimmed);
    return `${url.protocol}//${url.host}`;
  } catch (_) {
    return "••••••••";
  }
}

export function toStatusLabel(enabled: boolean) {
  return enabled ? m.common_enabled() : m.common_disabled();
}

export function getUpstreamLabel(index: number) {
  return m.upstreams_upstream_n({ number: String(index + 1) });
}
