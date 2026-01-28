import { afterEach, describe, expect, it, vi } from "vitest";

import { resolveDashboardRange, toDashboardTimeRange } from "@/features/dashboard/range";

describe("dashboard/range", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("parses dashboard time range values", () => {
    expect(toDashboardTimeRange("7d")).toBe("7d");
    expect(toDashboardTimeRange("unknown")).toBeNull();
  });

  it("resolves today range (from midnight to now)", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-28T12:34:56.789Z"));

    const result = resolveDashboardRange("today");
    const expectedStart = new Date();
    expectedStart.setHours(0, 0, 0, 0);

    expect(result.fromTsMs).toBe(expectedStart.getTime());
    expect(result.toTsMs).toBe(Date.now());
  });

  it("resolves rolling window ranges", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-28T12:00:00.000Z"));

    const now = Date.now();
    expect(resolveDashboardRange("7d")).toEqual({
      fromTsMs: now - 7 * 24 * 60 * 60 * 1000,
      toTsMs: now,
    });
  });
});

