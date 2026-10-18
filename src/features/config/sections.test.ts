import { describe, expect, it } from "vitest";

import {
  DEFAULT_CONFIG_SECTION,
  getSectionIdFromPathname,
} from "@/features/config/sections";

describe("config/sections", () => {
  it("parses section id from config pathname", () => {
    expect(getSectionIdFromPathname("/config/api-keys")).toBe("api-keys");
    expect(getSectionIdFromPathname("/config/upstreams")).toBe("upstreams");
    expect(getSectionIdFromPathname("/config/pricing")).toBe("pricing");
    expect(getSectionIdFromPathname("/config/settings/")).toBe("settings");
  });

  it("falls back to default section for invalid pathname including removed providers", () => {
    expect(getSectionIdFromPathname("/config")).toBe(DEFAULT_CONFIG_SECTION);
    expect(getSectionIdFromPathname("/config/unknown")).toBe(DEFAULT_CONFIG_SECTION);
    expect(getSectionIdFromPathname("/config/providers")).toBe(DEFAULT_CONFIG_SECTION);
    // Codex 模型菜单已并入 Agents，旧路由不再是独立分区。
    expect(getSectionIdFromPathname("/config/codex-models")).toBe(DEFAULT_CONFIG_SECTION);
    expect(getSectionIdFromPathname("/other")).toBe(DEFAULT_CONFIG_SECTION);
  });
});
