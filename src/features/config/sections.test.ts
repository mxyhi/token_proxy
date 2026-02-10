import { describe, expect, it } from "vitest";

import {
  DEFAULT_CONFIG_SECTION,
  getSectionIdFromPathname,
} from "@/features/config/sections";

describe("config/sections", () => {
  it("parses section id from config pathname", () => {
    expect(getSectionIdFromPathname("/config/upstreams")).toBe("upstreams");
    expect(getSectionIdFromPathname("/config/providers")).toBe("providers");
    expect(getSectionIdFromPathname("/config/settings/")).toBe("settings");
  });

  it("falls back to default section for invalid pathname", () => {
    expect(getSectionIdFromPathname("/config")).toBe(DEFAULT_CONFIG_SECTION);
    expect(getSectionIdFromPathname("/config/unknown")).toBe(DEFAULT_CONFIG_SECTION);
    expect(getSectionIdFromPathname("/other")).toBe(DEFAULT_CONFIG_SECTION);
  });
});

