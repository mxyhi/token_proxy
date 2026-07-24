import { describe, expect, it } from "vitest";

import { createEmptyUpstream } from "@/features/config/form";
import {
  coerceProviderSelection,
  isAccountBackedProviderSet,
  isAccountCredentialUpstream,
  resolveUpstreamIdForProviderChange,
} from "@/features/config/cards/upstreams/upstream-editor-helpers";

describe("upstreams/upstream-editor-helpers", () => {
  it("keeps id stable when editing and switching non-special provider", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "custom-1";
    upstream.providers = ["openai"];

    const id = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["gemini"],
      upstreams: [upstream],
      editingIndex: 0,
    });

    expect(id).toBe("custom-1");
  });

  it("keeps id stable when editing and switching to kiro/codex", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "custom-1";
    upstream.providers = ["openai"];

    const kiroId = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["kiro"],
      upstreams: [upstream],
      editingIndex: 0,
    });
    expect(kiroId).toBe("custom-1");

    const codexId = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["codex"],
      upstreams: [upstream],
      editingIndex: 0,
    });
    expect(codexId).toBe("custom-1");

    const antigravityId = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["antigravity"],
      upstreams: [upstream],
      editingIndex: 0,
    });
    expect(antigravityId).toBe("custom-1");
  });

  it("keeps id when editing and switching away from special provider", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "foo";
    upstream.providers = ["kiro"];

    const id = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["kiro"],
      nextProviders: ["openai"],
      upstreams: [upstream],
      editingIndex: 0,
    });

    expect(id).toBe("foo");
  });

  it("auto-generates id when creating and switching provider", () => {
    const upstream = createEmptyUpstream();
    upstream.id = "openai-1";
    upstream.providers = ["openai"];

    const id = resolveUpstreamIdForProviderChange({
      mode: "create",
      currentId: "openai-1",
      currentProviders: ["openai"],
      nextProviders: ["gemini"],
      upstreams: [upstream],
    });

    expect(id).toBe("gemini-1");
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
