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
});
