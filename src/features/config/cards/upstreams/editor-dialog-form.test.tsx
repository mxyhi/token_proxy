import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { UpstreamEditorFields } from "@/features/config/cards/upstreams/editor-dialog-form";
import { createEmptyUpstream } from "@/features/config/form";
import { m } from "@/paraglide/messages.js";

describe("upstreams/editor-dialog-form", () => {
  it("renders kiro account selector when provider is kiro", () => {
    const draft = createEmptyUpstream();
    draft.id = "kiro-default";
    draft.providers = ["kiro"];

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["kiro"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={vi.fn()}
      />
    );

    expect(screen.queryByText(m.field_kiro_account())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_proxy_url())).not.toBeInTheDocument();
    expect(screen.getByLabelText(m.field_id())).toBeDisabled();
    expect(screen.getByRole("button", { name: /kiro/i })).toBeDisabled();
  });

  it("renders codex account selector when provider is codex", () => {
    const draft = createEmptyUpstream();
    draft.id = "codex-default";
    draft.providers = ["codex"];

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["codex"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={vi.fn()}
      />
    );

    expect(screen.queryByText(m.field_codex_account())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_proxy_url())).not.toBeInTheDocument();
    expect(screen.getByLabelText(m.field_id())).toBeDisabled();
    expect(screen.getByRole("button", { name: /codex/i })).toBeDisabled();
  });
});
