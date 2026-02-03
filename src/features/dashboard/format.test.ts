import { describe, expect, it } from "vitest";

import {
  createDashboardTimeFormatter,
  formatCompact,
  formatDashboardTimestamp,
  formatInteger,
} from "@/features/dashboard/format";

describe("dashboard/format", () => {
  it("formats integers with thousand separators", () => {
    expect(formatInteger(0)).toBe("0");
    expect(formatInteger(1234)).toBe("1,234");
    expect(formatInteger(1234.6)).toBe("1,235");
  });

  it("renders placeholder for invalid timestamps", () => {
    const formatter = createDashboardTimeFormatter("en-US");
    expect(formatDashboardTimestamp(Number.NaN, formatter)).toBe("—");
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
});

