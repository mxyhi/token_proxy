import { describe, expect, it } from "vitest";

import { createEmptyUpstream } from "@/features/config/form";
import {
  coerceProviderSelection,
  createAutoUpstreamId,
  isAccountBackedProviderSet,
  isAccountCredentialUpstream,
} from "@/features/config/cards/upstreams/upstream-editor-helpers";

describe("upstreams/upstream-editor-helpers", () => {
  it("derives auto id from base url host", () => {
    const draft = { baseUrl: "https://sub2api.example.com/v1", providers: ["openai"] };
    expect(createAutoUpstreamId(draft, [])).toBe("sub2api.example.com");
    expect(createAutoUpstreamId({ ...draft, baseUrl: "http://127.0.0.1:8080" }, [])).toBe(
      "127.0.0.1:8080",
    );
    // 用户省略协议时按 https 补全。
    expect(createAutoUpstreamId({ ...draft, baseUrl: "api.example.com" }, [])).toBe(
      "api.example.com",
    );
  });

  it("falls back to provider when base url is empty or invalid", () => {
    expect(createAutoUpstreamId({ baseUrl: "", providers: ["codex"] }, [])).toBe("codex");
    expect(createAutoUpstreamId({ baseUrl: "http://", providers: ["gemini"] }, [])).toBe("gemini");
    expect(createAutoUpstreamId({ baseUrl: "", providers: [] }, [])).toBe("upstream");
  });

  it("appends numeric suffix when auto id is taken", () => {
    const first = createEmptyUpstream();
    first.id = "api.example.com";
    const second = createEmptyUpstream();
    second.id = "api.example.com-2";
    const draft = { baseUrl: "https://api.example.com", providers: ["openai"] };

    expect(createAutoUpstreamId(draft, [first])).toBe("api.example.com-2");
    expect(createAutoUpstreamId(draft, [first, second])).toBe("api.example.com-3");
  });

  it("treats account-backed providers as single-provider selections", () => {
    expect(isAccountBackedProviderSet(["kiro"])).toBe(true);
    expect(isAccountBackedProviderSet(["codex"])).toBe(true);
    expect(isAccountBackedProviderSet(["xai"])).toBe(true);
    expect(isAccountBackedProviderSet(["antigravity"])).toBe(true);
    expect(isAccountBackedProviderSet(["openai"])).toBe(false);
    expect(isAccountBackedProviderSet(["antigravity", "openai"])).toBe(false);
  });

  it("coerces account-backed providers to exclusive selections", () => {
    expect(coerceProviderSelection(["openai", "antigravity"])).toEqual(["antigravity"]);
    expect(coerceProviderSelection(["codex", "antigravity"])).toEqual(["codex"]);
    expect(coerceProviderSelection(["openai", "xai"])).toEqual(["xai"]);
  });

  it("marks all account credential upstreams (copy disabled, delete allowed)", () => {
    const xaiDefault = createEmptyUpstream();
    xaiDefault.id = "xai-default";
    xaiDefault.providers = ["xai"];
    xaiDefault.accountId = "xai-1";
    const customXai = { ...xaiDefault, id: "xai-custom", accountId: "xai-2" };
    const customKiro = {
      ...xaiDefault,
      id: "kiro-custom",
      providers: ["kiro"] as string[],
      accountId: "kiro-1",
    };
    const openai = createEmptyUpstream();
    openai.providers = ["openai"];

    expect(isAccountCredentialUpstream(xaiDefault)).toBe(true);
    expect(isAccountCredentialUpstream(customXai)).toBe(true);
    expect(isAccountCredentialUpstream(customKiro)).toBe(true);
    expect(isAccountCredentialUpstream(openai)).toBe(false);
  });
});
