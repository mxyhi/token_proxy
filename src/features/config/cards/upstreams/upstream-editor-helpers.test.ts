import { describe, expect, it } from "vitest";

import { createEmptyUpstream } from "@/features/config/form";
import { resolveUpstreamIdForProviderChange } from "@/features/config/cards/upstreams/upstream-editor-helpers";

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
      kiroAccountId: "",
      codexAccountId: "",
      antigravityAccountId: "",
    });

    expect(id).toBe("custom-1");
  });

  it("updates id to account_id when editing and switching to kiro/codex/antigravity", () => {
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
      kiroAccountId: "foo.json",
      codexAccountId: "",
      antigravityAccountId: "",
    });
    expect(kiroId).toBe("foo");

    const codexId = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["codex"],
      upstreams: [upstream],
      editingIndex: 0,
      kiroAccountId: "",
      codexAccountId: "bar.json",
      antigravityAccountId: "",
    });
    expect(codexId).toBe("bar");

    const antigravityId = resolveUpstreamIdForProviderChange({
      mode: "edit",
      currentId: upstream.id,
      currentProviders: ["openai"],
      nextProviders: ["antigravity"],
      upstreams: [upstream],
      editingIndex: 0,
      kiroAccountId: "",
      codexAccountId: "",
      antigravityAccountId: "baz.json",
    });
    expect(antigravityId).toBe("baz");
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
      kiroAccountId: "",
      codexAccountId: "",
      antigravityAccountId: "",
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
      kiroAccountId: "",
      codexAccountId: "",
      antigravityAccountId: "",
    });

    expect(id).toBe("gemini-1");
  });
});

