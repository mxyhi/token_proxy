import type { ColumnVisibility, UpstreamColumnDefinition } from "@/features/config/cards/upstreams/types";
import { m } from "@/paraglide/messages.js";

export const UPSTREAM_COLUMNS: readonly UpstreamColumnDefinition[] = [
  {
    id: "id",
    label: () => m.upstreams_column_id(),
    defaultVisible: true,
    headerClassName: "w-[14rem]",
    cellClassName: "w-[14rem] max-w-[14rem]",
  },
  {
    id: "provider",
    label: () => m.upstreams_column_provider(),
    defaultVisible: true,
    headerClassName: "w-[12rem]",
    cellClassName: "w-[12rem] max-w-[12rem]",
  },
  { id: "baseUrl", label: () => m.upstreams_column_base_url(), defaultVisible: false, cellClassName: "min-w-[18rem]" },
  { id: "apiKey", label: () => m.upstreams_column_api_key(), defaultVisible: false, cellClassName: "min-w-[18rem]" },
  {
    id: "priority",
    label: () => m.upstreams_column_priority(),
    defaultVisible: true,
    headerClassName: "w-[8rem]",
    cellClassName: "w-[8rem]",
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
    baseUrl: false,
    apiKey: false,
    priority: true,
    status: true,
  };
  for (const column of UPSTREAM_COLUMNS) {
    visibility[column.id] = column.defaultVisible;
  }
  return visibility;
}

const DEFAULT_PROVIDER_OPTIONS = ["openai", "openai-response", "anthropic", "gemini"] as const;

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

export function toStatusLabel(enabled: boolean) {
  return enabled ? m.common_enabled() : m.common_disabled();
}

export function getUpstreamLabel(index: number) {
  return m.upstreams_upstream_n({ number: String(index + 1) });
}
