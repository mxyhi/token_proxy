import { invoke } from "@tauri-apps/api/core";

import type {
  DashboardSnapshot,
  DashboardSnapshotQuery,
} from "@/features/dashboard/types";

export async function readDashboardSnapshot(query: DashboardSnapshotQuery) {
  return await invoke<DashboardSnapshot>("read_dashboard_snapshot", query);
}
