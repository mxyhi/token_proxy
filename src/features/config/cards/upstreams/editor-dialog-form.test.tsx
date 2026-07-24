import { cleanup, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UpstreamEditorFields } from "@/features/config/cards/upstreams/editor-dialog-form";
import { createEmptyUpstream } from "@/features/config/form";
import { m } from "@/paraglide/messages.js";

vi.mock("@tanstack/react-router", () => ({
  Link: ({ children }: { children: React.ReactNode }) => <span>{children}</span>,
}));

afterEach(() => {
  cleanup();
  vi.mocked(invoke).mockReset();
});

describe("upstreams/editor-dialog-form", () => {
  it("shows connection, model access, and collapsed advanced sections", () => {
    const draft = createEmptyUpstream();

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={vi.fn()}
      />
    );

    expect(screen.getByText(m.upstreams_section_connection())).toBeInTheDocument();
    expect(screen.getByText(m.upstreams_section_models())).toBeInTheDocument();
    expect(screen.getByText(m.upstreams_section_advanced())).toBeInTheDocument();
    expect(screen.getByText(m.available_models_all_desc())).toBeInTheDocument();
  });

  it("switches from all models to selected-model mode", async () => {
    const user = userEvent.setup();
    const draft = createEmptyUpstream();
    const onChangeDraft = vi.fn();

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={onChangeDraft}
      />
    );

    await user.click(screen.getByText(m.available_models_selected()));

    expect(onChangeDraft).toHaveBeenCalledWith({ availableModelsMode: "selected" });
  });

  it("removes a selected model from the allowlist", async () => {
    const user = userEvent.setup();
    const draft = createEmptyUpstream();
    draft.availableModelsMode = "selected";
    draft.availableModels = ["gpt-5.4"];
    const onChangeDraft = vi.fn();

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={onChangeDraft}
      />
    );

    await user.click(
      screen.getByRole("button", {
        name: m.available_models_remove({ model: "gpt-5.4" }),
      }),
    );

    expect(onChangeDraft).toHaveBeenCalledWith({ availableModels: [] });
  });

  it("selects every fetched model from an indeterminate state", async () => {
    const user = userEvent.setup();
    const draft = createEmptyUpstream();
    draft.availableModelsMode = "selected";
    draft.availableModels = ["gpt-5.4"];
    const onChangeDraft = vi.fn();
    vi.mocked(invoke).mockResolvedValue([
      "gpt-5.5",
      "claude-sonnet-4.6",
      "gpt-5.4",
    ]);

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={onChangeDraft}
      />
    );

    await user.click(
      screen.getByRole("button", { name: m.available_models_sync() }),
    );
    const selectAll = await screen.findByRole("checkbox", {
      name: m.available_models_select_all(),
    });

    expect(selectAll).toHaveAttribute("data-state", "indeterminate");
    await user.click(selectAll);

    expect(onChangeDraft).toHaveBeenCalledWith({
      availableModels: ["claude-sonnet-4.6", "gpt-5.4", "gpt-5.5"],
    });
  });

  it("clears only the models visible in the current search", async () => {
    const user = userEvent.setup();
    const draft = createEmptyUpstream();
    draft.availableModelsMode = "selected";
    draft.availableModels = ["claude-sonnet-4.6", "gpt-5.4"];
    const onChangeDraft = vi.fn();

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={onChangeDraft}
      />
    );

    await user.type(
      screen.getByPlaceholderText(m.available_models_search_placeholder()),
      "gpt",
    );
    await user.click(
      screen.getByRole("checkbox", { name: m.available_models_clear_all() }),
    );

    expect(onChangeDraft).toHaveBeenCalledWith({
      availableModels: ["claude-sonnet-4.6"],
    });
  });

  it("shows missing account banner for unbound kiro and keeps proxy editable", () => {
    const draft = createEmptyUpstream();
    draft.id = "kiro-unbound";
    draft.providers = ["kiro"];
    draft.accountId = "";

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

    expect(screen.getByText(m.upstreams_account_missing())).toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.getByLabelText(m.field_proxy_url())).toBeInTheDocument();
    expect(screen.getByLabelText(m.field_id())).toBeEnabled();
  });

  it("shows missing-account warning and keeps base_url hidden for unbound codex", () => {
    const draft = createEmptyUpstream();
    draft.id = "codex-unbound";
    draft.providers = ["codex"];
    draft.accountId = "";

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

    expect(screen.getByText(m.upstreams_account_missing())).toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_api_key())).not.toBeInTheDocument();
    // 未绑定 account 时 identity 未锁定，id 可编辑。
    expect(screen.getByLabelText(m.field_id())).toBeEnabled();
  });

  it("locks credential identity for bound xai account upstream", async () => {
    const draft = createEmptyUpstream();
    draft.id = "xai-bound";
    draft.providers = ["xai"];
    draft.accountId = "xai-user@example.com";
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "xai_list_accounts") {
        return [
          {
            account_id: "xai-user@example.com",
            email: "user@example.com",
            expires_at: "2027-01-01T00:00:00Z",
            status: "active",
            auto_refresh_enabled: true,
            proxy_url: null,
            priority: 0,
          },
        ];
      }
      if (command === "xai_fetch_quotas") {
        return [];
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["xai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={vi.fn()}
      />
    );

    expect(await screen.findByText("xai-user@example.com")).toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.getByLabelText(m.field_id())).toBeDisabled();
    expect(screen.getByRole("button", { name: /xai/i })).toBeDisabled();
  });

  it("hides base_url and api key for antigravity while keeping proxy editable", () => {
    const draft = createEmptyUpstream();
    draft.id = "antigravity-default";
    draft.providers = ["antigravity"];

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["antigravity"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={vi.fn()}
        onChangeDraft={vi.fn()}
      />
    );

    expect(screen.queryByLabelText(m.field_base_url())).not.toBeInTheDocument();
    expect(screen.queryByLabelText(m.field_api_key())).not.toBeInTheDocument();
    // proxy 属于 Upstream 路由字段，在 Advanced 中可编辑。
    expect(screen.getByLabelText(m.field_proxy_url())).toBeInTheDocument();
    expect(screen.getByLabelText(m.field_id())).toBeEnabled();
    expect(screen.getByRole("button", { name: /antigravity/i })).toBeEnabled();
  });
});
