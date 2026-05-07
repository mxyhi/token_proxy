import { describe, expect, it } from "vitest";

import {
  createDashboardTimeFormatter,
  formatCompact,
  formatDashboardClockTime,
  formatDashboardProviderLabel,
  formatDashboardTimestamp,
  formatInteger,
  formatNanoUsdCost,
} from "@/features/dashboard/format";
import { setLocale } from "@/paraglide/runtime.js";

describe("dashboard/format", () => {
  it("formats integers with thousand separators", () => {
    expect(formatInteger(0)).toBe("0");
    expect(formatInteger(1234)).toBe("1,234");
    expect(formatInteger(1234.6)).toBe("1,235");
  });

  it("renders placeholder for invalid timestamps", () => {
    const formatter = createDashboardTimeFormatter("en-US");
    expect(formatDashboardTimestamp(Number.NaN, formatter)).toBe("—");
    expect(formatDashboardClockTime(Number.NaN)).toBe("—");
  });

  it("formats dashboard table timestamps as clock time only", () => {
    const timestamp = new Date(2026, 4, 2, 15, 28, 43).getTime();
    expect(formatDashboardClockTime(timestamp)).toBe("15:28:43");
  });

  it("formats compact numbers with K suffix for thousands", () => {
    expect(formatCompact(0)).toBe("0");
    expect(formatCompact(999)).toBe("999");
    expect(formatCompact(1000)).toBe("1K");
    expect(formatCompact(1500)).toBe("1.5K");
    expect(formatCompact(985856)).toBe("985.9K");
  });

  it("formats compact numbers with M suffix for millions", () => {
    expect(formatCompact(1000000)).toBe("1M");
    expect(formatCompact(1500000)).toBe("1.5M");
    expect(formatCompact(12345678)).toBe("12.3M");
  });

  it("formats compact numbers with B suffix for billions", () => {
    expect(formatCompact(1000000000)).toBe("1B");
    expect(formatCompact(2500000000)).toBe("2.5B");
  });

  it("formats cost amounts without a currency unit", () => {
    expect(formatNanoUsdCost(1_210_000_000)).toBe("1.21");
    expect(formatNanoUsdCost(4_325_000_000)).toBe("4.33");
    expect(formatNanoUsdCost(null)).toBe("—");
  });

  it("keeps provider when it adds new information", () => {
    expect(formatDashboardProviderLabel("primary", "openai", "team-account.json")).toBe(
      "primary · openai · team-account.json"
    );
  });

  it("omits provider when upstream or account already includes it", () => {
    expect(formatDashboardProviderLabel("codex-default", "codex", "codex-joane.json")).toBe(
      "codex-default · codex-joane.json"
    );
    expect(formatDashboardProviderLabel("fallback", "anthropic", "anthropic-main.json")).toBe(
      "fallback · anthropic-main.json"
    );
  });

  it("keeps provider when no account label is available", () => {
    expect(formatDashboardProviderLabel("fallback", "anthropic", null)).toBe(
      "fallback · anthropic"
    );
  });

  it("uses a dedicated label for local proxy request failures", () => {
    setLocale("en", { reload: false });
    expect(formatDashboardProviderLabel("local", "proxy", null)).toBe("Local proxy");
  });
});
