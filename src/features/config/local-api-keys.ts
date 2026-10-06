import { invoke } from "@tauri-apps/api/core";

import type { LocalApiKey, LocalApiKeyUsage } from "./types";
import { m } from "@/paraglide/messages.js";

export async function readLocalApiKeyUsage(): Promise<ReadonlyMap<string, LocalApiKeyUsage>> {
  const usage = await invoke<LocalApiKeyUsage[] | null>("read_local_api_key_usage");
  return new Map((usage ?? []).map((item) => [item.keyId, item]));
}

export function createLocalApiKey(): LocalApiKey {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return {
    id: crypto.randomUUID(),
    name: "",
    key: `tp-${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`,
    enabled: true,
    scope: { type: "auto" },
  };
}

export function validateLocalApiKeys(
  keys: readonly LocalApiKey[],
): string | null {
  const ids = new Set<string>();
  const secrets = new Set<string>();
  for (const item of keys) {
    if (!item.id.trim() || !item.name.trim() || !item.key.trim())
      return m.api_keys_required();
    if (
      item.key !== item.key.trim() ||
      Array.from(item.key).some((char) => /\p{Cc}/u.test(char))
    ) {
      return m.api_keys_invalid_key();
    }
    if (ids.has(item.id) || secrets.has(item.key))
      return m.api_keys_duplicate();
    ids.add(item.id);
    secrets.add(item.key);
    if (item.scope.type === "selected") {
      const ids = item.scope.upstream_ids;
      if (
        !ids.length ||
        ids.some((id) => !id.trim()) ||
        new Set(ids).size !== ids.length
      ) {
        return m.api_keys_select_required();
      }
    }
  }
  return null;
}
