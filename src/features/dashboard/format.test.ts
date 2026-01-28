import { describe, expect, it } from "vitest";

import {
  createDashboardTimeFormatter,
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
});

