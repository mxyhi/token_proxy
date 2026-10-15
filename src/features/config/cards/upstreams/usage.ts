import { invoke } from "@tauri-apps/api/core";

import type { UpstreamUsage } from "@/features/config/types";

/** 读取各渠道累计用量，按渠道 ID 建索引。 */
export async function readUpstreamUsage(): Promise<ReadonlyMap<string, UpstreamUsage>> {
  const usage = await invoke<UpstreamUsage[] | null>("read_upstream_usage");
  return new Map((usage ?? []).map((item) => [item.upstreamId, item]));
}
