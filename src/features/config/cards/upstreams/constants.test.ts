import { describe, expect, it } from "vitest";

import {
  UPSTREAM_COLUMNS,
  mergeProviderOptions,
} from "@/features/config/cards/upstreams/constants";

describe("upstreams/constants", () => {
  it("adjusts id, provider, and priority column widths", () => {
    const idColumn = UPSTREAM_COLUMNS.find((column) => column.id === "id");
    const providerColumn = UPSTREAM_COLUMNS.find((column) => column.id === "provider");
    const priorityColumn = UPSTREAM_COLUMNS.find((column) => column.id === "priority");

    expect(idColumn?.headerClassName).toBe("w-[12rem]");
    expect(idColumn?.cellClassName).toBe("w-[12rem] max-w-[12rem]");
    expect(providerColumn?.headerClassName).toBe("w-[10rem]");
    expect(providerColumn?.cellClassName).toBe("w-[10rem] max-w-[10rem]");
    expect(priorityColumn?.headerClassName).toBe("w-[6rem]");
    expect(priorityColumn?.cellClassName).toBe("w-[6rem]");
  });

  it("exposes public API provider options first by default", () => {
    expect(mergeProviderOptions([])).toEqual([
      "openai",
      "openai-response",
      "anthropic",
      "gemini",
    ]);
  });

  it("merges non-account providers from config but excludes Kiro/Codex/xAI", () => {
    // 账户型不进入普通创建选项；antigravity/legacy 等仍保留。
    expect(
      mergeProviderOptions(["kiro", "codex", "xai", "antigravity", "legacy-provider"]),
    ).toEqual([
      "openai",
      "openai-response",
      "anthropic",
      "gemini",
      "antigravity",
      "legacy-provider",
    ]);
  });
});
