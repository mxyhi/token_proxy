/**
 * Phase D：账户迁入 Upstream UI 的关键断言。
 * - account-backed 可删不可复制
 * - 删除文案声明级联凭据
 * - 登录失败走 toast/error 路径（不调用已删除命令）
 * - 普通创建不暴露 Kiro/Codex/xAI
 * - 成功导入触发 onConfigReload
 * - 账户 panel quota refresh 调用正确 command
 */
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { UpstreamsCard } from "@/features/config/cards/upstreams-card";
import { UpstreamEditorFields } from "@/features/config/cards/upstreams/editor-dialog-form";
import { createEmptyUpstream } from "@/features/config/form";
import type { UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

const invokeMock = vi.hoisted(() => vi.fn());
const openFileDialogMock = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: openFileDialogMock,
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

const DEFAULT_STRATEGY = {
  order: "fill_first" as const,
  dispatchType: "serial" as const,
  hedgeDelayMs: "2000",
  maxParallel: "2",
};

function buildAccountUpstream(): UpstreamForm {
  const upstream = createEmptyUpstream();
  upstream.id = "kiro-acc-1";
  upstream.providers = ["kiro"];
  upstream.accountId = "kiro-primary.json";
  upstream.enabled = true;
  upstream.baseUrl = "";
  return upstream;
}

function buildApiKeyUpstream(): UpstreamForm {
  const upstream = createEmptyUpstream();
  upstream.id = "openai-1";
  upstream.providers = ["openai"];
  upstream.apiKeys = "sk-test";
  return upstream;
}

function renderUpstreamsCard(
  props: Partial<ComponentProps<typeof UpstreamsCard>> = {},
) {
  return render(
    <UpstreamsCard
      upstreams={props.upstreams ?? []}
      appProxyUrl={props.appProxyUrl ?? ""}
      strategy={props.strategy ?? DEFAULT_STRATEGY}
      showApiKeys={props.showApiKeys ?? false}
      providerOptions={props.providerOptions ?? ["openai"]}
      onToggleApiKeys={props.onToggleApiKeys ?? (() => undefined)}
      onStrategyChange={props.onStrategyChange ?? (() => undefined)}
      onAdd={props.onAdd ?? (() => undefined)}
      onRemove={props.onRemove ?? (() => undefined)}
      onChange={props.onChange ?? (() => undefined)}
      onConfigReload={props.onConfigReload ?? (() => undefined)}
    />,
  );
}

afterEach(() => {
  cleanup();
  invokeMock.mockReset();
  openFileDialogMock.mockReset();
});

describe("upstreams account UI (Phase D)", () => {
  it("disables copy and enables delete for account-backed rows", () => {
    const account = buildAccountUpstream();
    const regular = buildApiKeyUpstream();

    renderUpstreamsCard({
      upstreams: [account, regular],
      providerOptions: ["openai", "kiro"],
    });

    const label1 = m.upstreams_upstream_n({ number: "1" });
    const label2 = m.upstreams_upstream_n({ number: "2" });
    const accountCopy = screen.getByRole("button", {
      name: m.upstreams_row_copy({ rowLabel: label1 }),
    });
    const accountDelete = screen.getByRole("button", {
      name: m.upstreams_row_delete({ rowLabel: label1 }),
    });
    const regularCopy = screen.getByRole("button", {
      name: m.upstreams_row_copy({ rowLabel: label2 }),
    });

    expect(accountCopy).toBeDisabled();
    expect(accountDelete).toBeEnabled();
    expect(regularCopy).toBeEnabled();
  });

  it("shows cascade credential copy when deleting account-backed upstream", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    const label1 = m.upstreams_upstream_n({ number: "1" });

    renderUpstreamsCard({
      upstreams: [buildAccountUpstream()],
      providerOptions: ["kiro"],
      onRemove,
    });

    await user.click(
      screen.getByRole("button", { name: m.upstreams_row_delete({ rowLabel: label1 }) }),
    );

    expect(screen.getByText(m.upstreams_delete_account_title())).toBeInTheDocument();
    expect(
      screen.getByText(m.upstreams_delete_account_description({ rowLabel: label1 })),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: m.common_delete() }));
    expect(onRemove).toHaveBeenCalledWith(0);
  });

  it("excludes Kiro/Codex/xAI from ordinary create provider options", async () => {
    const user = userEvent.setup();

    renderUpstreamsCard({
      providerOptions: ["openai", "kiro", "codex", "xai", "antigravity"],
    });

    await user.click(screen.getByRole("button", { name: m.upstreams_add() }));
    // Upstream 编辑器使用 AlertDialog，role 为 alertdialog。
    expect(screen.getByRole("alertdialog")).toBeInTheDocument();

    // 打开 provider multi-select 下拉，断言账户 provider 不在可选项中。
    await user.click(screen.getByRole("button", { name: /openai/i }));
    expect(screen.getByRole("menuitemcheckbox", { name: "openai" })).toBeInTheDocument();
    expect(
      screen.getByRole("menuitemcheckbox", { name: "antigravity" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("menuitemcheckbox", { name: "kiro" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitemcheckbox", { name: "codex" })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitemcheckbox", { name: "xai" })).not.toBeInTheDocument();
  });

  it("opens add-account dialog and surfaces kiro login errors without deleted commands", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation(async (command) => {
      if (command === "kiro_start_login") {
        throw new Error("device auth unavailable");
      }
      throw new Error(`unexpected command: ${command}`);
    });

    renderUpstreamsCard();

    await user.click(screen.getByRole("button", { name: m.upstreams_add_account() }));
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: m.kiro_login_method_aws() }));

    await waitFor(() => {
      expect(screen.getByText("device auth unavailable")).toBeInTheDocument();
    });

    const commands = invokeMock.mock.calls.map(([command]) => String(command));
    expect(commands).toContain("kiro_start_login");
    expect(commands).not.toContain("kiro_logout");
    expect(commands).not.toContain("kiro_set_priority");
    expect(commands).not.toContain("providers_delete_accounts");
  });

  it("reloads config after successful kiro folder import", async () => {
    const user = userEvent.setup();
    const onConfigReload = vi.fn();
    openFileDialogMock.mockResolvedValue("/tmp/kiro-ide");
    invokeMock.mockImplementation(async (command) => {
      if (command === "kiro_import_ide") {
        return [
          {
            account_id: "kiro-imported.json",
            provider: "kiro",
            auth_method: "google",
            email: "alice@example.com",
            expires_at: "2027-01-01T00:00:00Z",
            status: "active",
          },
        ];
      }
      throw new Error(`unexpected command: ${command}`);
    });

    renderUpstreamsCard({ onConfigReload });

    await user.click(screen.getByRole("button", { name: m.upstreams_add_account() }));
    await user.click(screen.getByRole("button", { name: m.kiro_login_method_import() }));

    await waitFor(() => {
      expect(onConfigReload).toHaveBeenCalledTimes(1);
    });
    expect(openFileDialog).toHaveBeenCalled();
    expect(invokeMock).toHaveBeenCalledWith("kiro_import_ide", {
      directory: "/tmp/kiro-ide",
    });
  });

  it("calls kiro_refresh_quota_now from account credential panel", async () => {
    const user = userEvent.setup();
    const draft = buildAccountUpstream();
    invokeMock.mockImplementation(async (command) => {
      if (command === "kiro_list_accounts") {
        return [
          {
            account_id: "kiro-primary.json",
            provider: "kiro",
            auth_method: "google",
            email: "alice@example.com",
            expires_at: "2027-01-01T00:00:00Z",
            status: "active",
          },
        ];
      }
      if (command === "kiro_fetch_quotas") {
        return [
          {
            account_id: "kiro-primary.json",
            quotas: [
              {
                name: "credits",
                percentage: 40,
                used: 40,
                limit: 100,
                reset_at: null,
              },
            ],
          },
        ];
      }
      if (command === "kiro_refresh_quota_now") {
        return undefined;
      }
      throw new Error(`unexpected command: ${command}`);
    });

    render(
      <UpstreamEditorFields
        draft={draft}
        providerOptions={["openai"]}
        appProxyUrl=""
        showApiKeys={false}
        onToggleApiKeys={() => undefined}
        onChangeDraft={() => undefined}
      />,
    );

    expect(await screen.findByText("kiro-primary.json")).toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: m.providers_account_refresh_quota() }),
    );

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("kiro_refresh_quota_now", {
        accountId: "kiro-primary.json",
      });
    });
  });
});
