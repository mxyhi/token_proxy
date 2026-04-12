import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { readDashboardSnapshot } from "@/features/dashboard/api";

describe("dashboard/api", () => {
  it("delegates to tauri invoke", async () => {
    const invokeMock = vi.mocked(invoke);
    invokeMock.mockResolvedValueOnce({
      summary: {
        totalRequests: 0,
        successRequests: 0,
        errorRequests: 0,
        totalTokens: 0,
        inputTokens: 0,
        outputTokens: 0,
        cachedTokens: 0,
        avgLatencyMs: 0,
        medianLatencyMs: 0,
      },
      providers: [],
      upstreams: [],
      accounts: [],
      series: [],
      recent: [],
      truncated: false,
    });

    const range = { fromTsMs: 1, toTsMs: 2 };
    await readDashboardSnapshot({
      range,
      offset: 10,
      upstreamId: "alpha",
      accountId: "codex-a.json",
      publicOnly: false,
    });

    expect(invokeMock).toHaveBeenCalledWith("read_dashboard_snapshot", {
      range,
      offset: 10,
      upstreamId: "alpha",
      accountId: "codex-a.json",
      publicOnly: false,
    });
  });
});
