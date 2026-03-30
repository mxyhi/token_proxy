import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { open } from "@tauri-apps/plugin-dialog";

import { ProvidersPanel } from "@/features/providers/ProvidersPanel";
import { m } from "@/paraglide/messages.js";

const providerMocks = vi.hoisted(() => {
  let kiroAccountsLoading = false;
  let kiroQuotasLoading = false;
  let codexAccountsLoading = false;
  let codexQuotasLoading = false;
  const refreshKiroAccounts = vi.fn(async () => undefined);
  const refreshCodexAccounts = vi.fn(async () => undefined);
  const refreshCodexAccount = vi.fn(async () => undefined);
  const setCodexAutoRefresh = vi.fn(async () => ({
    account_id: "codex-1",
    email: "bob@example.com",
    expires_at: "2026-04-01T00:00:00Z",
    status: "expired" as const,
    auto_refresh_enabled: true,
  }));
  const refreshKiroQuotas = vi.fn(async () => undefined);
  const refreshCodexQuotas = vi.fn(async () => undefined);
  const logoutKiro = vi.fn(async () => undefined);
  const logoutCodex = vi.fn(async () => undefined);
  const setKiroProxyUrl = vi.fn(async () => ({
    account_id: "kiro-1",
    provider: "kiro" as const,
    auth_method: "google",
    email: "alice@example.com",
    expires_at: "2026-05-01T00:00:00Z",
    status: "active" as const,
    proxy_url: "http://127.0.0.1:7890",
  }));
  const setCodexProxyUrl = vi.fn(async () => ({
    account_id: "codex-1",
    email: "bob@example.com",
    expires_at: "2026-04-01T00:00:00Z",
    status: "expired" as const,
    auto_refresh_enabled: true,
    proxy_url: "socks5://127.0.0.1:1080",
  }));
  const beginKiroLogin = vi.fn();
  const beginCodexLogin = vi.fn();
  const importKiroIde = vi.fn(async () => undefined);
  const importKiroKam = vi.fn(async () => undefined);
  const importCodexFile = vi.fn(async () => undefined);
  const deleteProviderAccounts = vi.fn(async () => undefined);
  const listProviderAccountsPage = vi.fn(
    async ({
      page = 1,
      pageSize = 10,
      providerKind,
      status,
      search,
    }: {
      page?: number;
      pageSize?: number;
      providerKind?: "kiro" | "codex";
      status?: "active" | "expired";
      search?: string;
    }) => {
      const rows = [
        {
          provider_kind: "kiro" as const,
          account_id: "kiro-1",
          email: "alice@example.com",
          expires_at: "2026-05-01T00:00:00Z",
          status: "active" as const,
          auth_method: "google",
          provider_name: "kiro",
          proxy_url: "http://127.0.0.1:7890",
        },
        {
          provider_kind: "codex" as const,
          account_id: "codex-1",
          email: "bob@example.com",
          expires_at: "2026-04-01T00:00:00Z",
          status: "expired" as const,
          auth_method: null,
          provider_name: null,
          auto_refresh_enabled: true,
          proxy_url: "",
        },
      ];
      const keyword = search?.trim().toLowerCase() ?? "";
      const filtered = rows.filter((row) => {
        if (providerKind && row.provider_kind !== providerKind) {
          return false;
        }
        if (status && row.status !== status) {
          return false;
        }
        if (!keyword) {
          return true;
        }
        return `${row.email ?? ""} ${row.account_id}`.toLowerCase().includes(keyword);
      });
      const offset = (page - 1) * pageSize;
      return {
        items: filtered.slice(offset, offset + pageSize),
        total: filtered.length,
        page,
        page_size: pageSize,
      };
    }
  );
  const toastError = vi.fn();
  const toastSuccess = vi.fn();

  return {
    get kiroAccountsLoading() {
      return kiroAccountsLoading;
    },
    set kiroAccountsLoading(value: boolean) {
      kiroAccountsLoading = value;
    },
    get kiroQuotasLoading() {
      return kiroQuotasLoading;
    },
    set kiroQuotasLoading(value: boolean) {
      kiroQuotasLoading = value;
    },
    get codexAccountsLoading() {
      return codexAccountsLoading;
    },
    set codexAccountsLoading(value: boolean) {
      codexAccountsLoading = value;
    },
    get codexQuotasLoading() {
      return codexQuotasLoading;
    },
    set codexQuotasLoading(value: boolean) {
      codexQuotasLoading = value;
    },
    refreshKiroAccounts,
    refreshCodexAccounts,
    refreshCodexAccount,
    setCodexAutoRefresh,
    refreshKiroQuotas,
    refreshCodexQuotas,
    logoutKiro,
    logoutCodex,
    setKiroProxyUrl,
    setCodexProxyUrl,
    beginKiroLogin,
    beginCodexLogin,
    importKiroIde,
    importKiroKam,
    importCodexFile,
    deleteProviderAccounts,
    listProviderAccountsPage,
    toastError,
    toastSuccess,
  };
});

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: vi.fn(async () => "/Users/test"),
  join: vi.fn(async (...parts: string[]) => parts.join("/")),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}));

vi.mock("sonner", () => ({
  toast: {
    error: providerMocks.toastError,
    success: providerMocks.toastSuccess,
  },
}));

vi.mock("@/features/kiro/use-kiro-accounts", () => ({
  useKiroAccounts: () => ({
    accounts: [
      {
        account_id: "kiro-1",
        provider: "kiro",
        auth_method: "google",
        email: "alice@example.com",
        expires_at: "2026-05-01T00:00:00Z",
        status: "active",
        proxy_url: "http://127.0.0.1:7890",
      },
    ],
    loading: providerMocks.kiroAccountsLoading,
    error: "",
    refresh: providerMocks.refreshKiroAccounts,
    logout: providerMocks.logoutKiro,
    importIde: providerMocks.importKiroIde,
    importKam: providerMocks.importKiroKam,
    setProxyUrl: providerMocks.setKiroProxyUrl,
  }),
}));

vi.mock("@/features/codex/use-codex-accounts", () => ({
  useCodexAccounts: () => ({
    accounts: [
      {
        account_id: "codex-1",
        email: "bob@example.com",
        expires_at: "2026-04-01T00:00:00Z",
        status: "expired",
        proxy_url: "",
      },
    ],
    loading: providerMocks.codexAccountsLoading,
    error: "",
    refresh: providerMocks.refreshCodexAccounts,
    refreshAccount: providerMocks.refreshCodexAccount,
    setAutoRefresh: providerMocks.setCodexAutoRefresh,
    setProxyUrl: providerMocks.setCodexProxyUrl,
    logout: providerMocks.logoutCodex,
    importFile: providerMocks.importCodexFile,
  }),
}));

vi.mock("@/features/kiro/use-kiro-quotas", () => ({
  useKiroQuotas: () => ({
    quotas: [
      {
        account_id: "kiro-1",
        provider: "kiro",
        plan_type: "Pro",
        error: null,
        quotas: [
          {
            name: "Requests",
            percentage: 25,
            used: 25,
            limit: 100,
            reset_at: "2026-04-15T00:00:00Z",
            is_trial: false,
          },
        ],
      },
    ],
    loading: providerMocks.kiroQuotasLoading,
    error: "",
    refresh: providerMocks.refreshKiroQuotas,
  }),
}));

vi.mock("@/features/codex/use-codex-quotas", () => ({
  useCodexQuotas: () => ({
    quotas: [
      {
        account_id: "codex-1",
        plan_type: "Plus",
        error: null,
        quotas: [
          {
            name: "codex-weekly",
            percentage: 50,
            used: 50,
            limit: 100,
            reset_at: "2026-04-08T00:00:00Z",
          },
        ],
      },
    ],
    loading: providerMocks.codexQuotasLoading,
    error: "",
    refresh: providerMocks.refreshCodexQuotas,
  }),
}));

vi.mock("@/features/kiro/use-kiro-login", () => ({
  useKiroLogin: () => ({
    login: { status: "idle" },
    beginLogin: providerMocks.beginKiroLogin,
  }),
}));

vi.mock("@/features/codex/use-codex-login", () => ({
  useCodexLogin: () => ({
    login: { status: "idle" },
    beginLogin: providerMocks.beginCodexLogin,
  }),
}));

vi.mock("@/features/providers/api", () => ({
  listProviderAccountsPage: providerMocks.listProviderAccountsPage,
  deleteProviderAccounts: providerMocks.deleteProviderAccounts,
}));

async function findAccountRow(label: string) {
  const table = await screen.findByTestId("providers-pagination-indicator");
  const container = table.closest("section");
  if (!(container instanceof HTMLElement)) {
    throw new Error("Missing providers section");
  }
  const accountCell = await within(container).findByText(label);
  const row = accountCell.closest("tr");
  if (!(row instanceof HTMLTableRowElement)) {
    throw new Error(`Missing table row for ${label}`);
  }
  return row;
}

function getToolbar() {
  const toolbar = document.querySelector('[data-slot="providers-toolbar"]');
  if (!(toolbar instanceof HTMLElement)) {
    throw new Error("Missing providers toolbar");
  }
  return toolbar;
}

function getAccountsTable() {
  const table = document.querySelector('[data-slot="providers-accounts-table"]');
  if (!(table instanceof HTMLElement)) {
    throw new Error("Missing providers accounts table");
  }
  return table;
}

function getAddLabel() {
  const label = m.providers_add_account();
  if (label.endsWith("账户")) {
    return label.slice(0, -2);
  }
  return label.replace(/\s*account$/i, "").trim();
}

async function openAddAccountDialog(user: ReturnType<typeof userEvent.setup>) {
  const addButton = document.querySelector('[data-slot="providers-toolbar-add"]');
  if (!(addButton instanceof HTMLButtonElement)) {
    throw new Error("Missing providers add button");
  }
  await user.click(addButton);
}

async function switchAddProviderToCodex(user: ReturnType<typeof userEvent.setup>) {
  const switchButton = document.querySelector('[data-slot="providers-add-provider-codex"]');
  if (!(switchButton instanceof HTMLButtonElement)) {
    throw new Error("Missing providers add codex switch button");
  }
  await user.click(switchButton);
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  providerMocks.kiroAccountsLoading = false;
  providerMocks.kiroQuotasLoading = false;
  providerMocks.codexAccountsLoading = false;
  providerMocks.codexQuotasLoading = false;
});

describe("providers/ProvidersPanel", () => {
  it("renders accounts in a unified table", async () => {
    render(<ProvidersPanel />);

    expect(
      await screen.findByRole("columnheader", { name: m.providers_table_provider() })
    ).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: m.providers_table_account() })).toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("alice@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();
  });

  it("does not render extra provider group panels below the accounts table", () => {
    render(<ProvidersPanel />);

    expect(document.querySelector('[data-slot="provider-group"]')).toBeNull();
  });

  it("filters rows by search keyword", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.type(
      within(getToolbar()).getByRole("textbox", { name: m.providers_toolbar_search_placeholder() }),
      "alice"
    );

    expect(within(getAccountsTable()).getByText("alice@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).queryByText("bob@example.com")).not.toBeInTheDocument();
  });

  it("filters rows by provider and status", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(within(getToolbar()).getByLabelText(m.providers_filter_provider_label()));
    await user.click(screen.getByRole("option", { name: m.providers_codex_title() }));

    expect(within(getAccountsTable()).queryByText("alice@example.com")).not.toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();

    await user.click(within(getToolbar()).getByLabelText(m.providers_filter_status_label()));
    await user.click(screen.getByRole("option", { name: m.codex_account_status_expired() }));

    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).queryByText("alice@example.com")).not.toBeInTheDocument();
  });

  it("opens account dialog from edit action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(
      within(await findAccountRow("alice@example.com")).getByRole("button", {
        name: m.providers_account_dialog_title(),
      })
    );

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText(m.providers_account_dialog_title())).toBeInTheDocument();
    expect(screen.getAllByText("alice@example.com").length).toBeGreaterThan(0);
    expect(screen.getAllByText("kiro-1").length).toBeGreaterThan(0);
  });

  it("shows tooltip for account details action on hover", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.hover(
      within(await findAccountRow("alice@example.com")).getByRole("button", {
        name: m.providers_account_dialog_title(),
      })
    );

    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      m.providers_account_dialog_title()
    );
  });

  it("keeps the actions column pinned to the right", async () => {
    render(<ProvidersPanel />);

    const header = await screen.findByRole("columnheader", { name: m.providers_table_actions() });
    const actionButton = within(await findAccountRow("alice@example.com")).getByRole("button", {
      name: m.providers_account_dialog_title(),
    });
    const actionCell = actionButton.closest('[data-slot="table-cell"]');

    expect(header).toHaveClass("sticky", "right-0");
    expect(actionCell).not.toBeNull();
    expect(actionCell).toHaveClass("sticky", "right-0");
  });

  it("refreshes codex account token from account dialog action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(
      within(await findAccountRow("bob@example.com")).getByRole("button", {
        name: m.providers_account_dialog_title(),
      })
    );
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: m.common_refresh() }));
    const refreshConfirmDialog = document.querySelector("[data-slot='codex-refresh-confirm-dialog']");
    if (!(refreshConfirmDialog instanceof HTMLElement)) {
      throw new Error("Missing codex refresh confirm dialog");
    }
    await user.click(within(refreshConfirmDialog).getByRole("button", { name: m.common_refresh() }));

    expect(providerMocks.refreshCodexAccount).toHaveBeenCalledWith("codex-1");
    expect(providerMocks.refreshCodexQuotas).toHaveBeenCalledTimes(1);
  });

  it("shows toast when codex account refresh fails", async () => {
    const user = userEvent.setup();
    providerMocks.refreshCodexAccount.mockRejectedValueOnce(
      new Error("Codex 登录已失效，请重新登录该账户。")
    );

    render(<ProvidersPanel />);

    await user.click(
      within(await findAccountRow("bob@example.com")).getByRole("button", {
        name: m.providers_account_dialog_title(),
      })
    );
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: m.common_refresh() }));
    const refreshConfirmDialog = document.querySelector("[data-slot='codex-refresh-confirm-dialog']");
    if (!(refreshConfirmDialog instanceof HTMLElement)) {
      throw new Error("Missing codex refresh confirm dialog");
    }
    await user.click(within(refreshConfirmDialog).getByRole("button", { name: m.common_refresh() }));

    expect(providerMocks.toastError).toHaveBeenCalledWith("Codex 登录已失效，请重新登录该账户。");
  });

  it("toggles codex auto refresh in account dialog", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(
      within(await findAccountRow("bob@example.com")).getByRole("button", {
        name: m.providers_account_dialog_title(),
      })
    );
    const toggle = within(screen.getByRole("dialog")).getByRole("switch", {
      name: "Codex 自动置换 Token",
    });
    await user.click(toggle);

    expect(providerMocks.setCodexAutoRefresh).toHaveBeenCalledWith("codex-1", false);
  });

  it("refreshes all provider data from toolbar action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(within(getToolbar()).getByRole("button", { name: m.common_refresh() }));

    expect(providerMocks.refreshKiroAccounts).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshKiroQuotas).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshCodexAccounts).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshCodexQuotas).toHaveBeenCalledTimes(1);
  });

  it("opens add account dialog from toolbar add button", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await openAddAccountDialog(user);

    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeInTheDocument();
    expect(within(dialog).getByText(getAddLabel())).toBeInTheDocument();
    expect(within(dialog).getByText(m.providers_kiro_title())).toBeInTheDocument();
    expect(within(dialog).getByText(m.providers_codex_title())).toBeInTheDocument();
  });

  it("starts kiro aws builder id login from toolbar action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);
    await openAddAccountDialog(user);

    const loginButton = document.querySelector('[data-slot="providers-add-kiro-login-aws"]');
    if (!(loginButton instanceof HTMLButtonElement)) {
      throw new Error("Missing kiro aws login button");
    }

    await user.click(loginButton);

    expect(providerMocks.beginKiroLogin).toHaveBeenCalledWith("aws");
  });

  it("starts kiro google login from toolbar action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);
    await openAddAccountDialog(user);

    const loginButton = document.querySelector('[data-slot="providers-add-kiro-login-google"]');
    if (!(loginButton instanceof HTMLButtonElement)) {
      throw new Error("Missing kiro google login button");
    }

    await user.click(loginButton);

    expect(providerMocks.beginKiroLogin).toHaveBeenCalledWith("google");
  });

  it("imports kiro ide directory from toolbar action", async () => {
    const user = userEvent.setup();
    vi.mocked(open).mockResolvedValueOnce("/tmp/kiro");

    render(<ProvidersPanel />);
    await openAddAccountDialog(user);

    const importButton = document.querySelector('[data-slot="providers-add-kiro-import-ide"]');
    if (!(importButton instanceof HTMLButtonElement)) {
      throw new Error("Missing kiro import ide button");
    }

    await user.click(importButton);

    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
    });
    expect(providerMocks.importKiroIde).toHaveBeenCalledWith("/tmp/kiro");
    expect(providerMocks.refreshKiroQuotas).toHaveBeenCalledTimes(1);
  });

  it("imports kiro kam json from toolbar action", async () => {
    const user = userEvent.setup();
    vi.mocked(open).mockResolvedValueOnce("/tmp/kiro.json");

    render(<ProvidersPanel />);
    await openAddAccountDialog(user);

    const importButton = document.querySelector('[data-slot="providers-add-kiro-import-kam"]');
    if (!(importButton instanceof HTMLButtonElement)) {
      throw new Error("Missing kiro import kam button");
    }

    await user.click(importButton);

    expect(open).toHaveBeenCalledWith({
      directory: false,
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    expect(providerMocks.importKiroKam).toHaveBeenCalledWith("/tmp/kiro.json");
    expect(providerMocks.refreshKiroQuotas).toHaveBeenCalledTimes(1);
  });

  it("starts codex login from toolbar action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);
    await openAddAccountDialog(user);
    await switchAddProviderToCodex(user);

    const loginButton = document.querySelector('[data-slot="providers-add-codex-login"]');
    if (!(loginButton instanceof HTMLButtonElement)) {
      throw new Error("Missing codex login button");
    }

    await user.click(loginButton);

    expect(providerMocks.beginCodexLogin).toHaveBeenCalledTimes(1);
  });

  it("imports codex account file from toolbar action", async () => {
    const user = userEvent.setup();
    vi.mocked(open).mockResolvedValueOnce("/tmp/codex-account.json");

    render(<ProvidersPanel />);
    await openAddAccountDialog(user);
    await switchAddProviderToCodex(user);

    const importButton = document.querySelector('[data-slot="providers-add-codex-import-file"]');
    if (!(importButton instanceof HTMLButtonElement)) {
      throw new Error("Missing codex import file button");
    }

    await user.click(importButton);

    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith({
      directory: false,
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    expect(providerMocks.importCodexFile).toHaveBeenCalledWith("/tmp/codex-account.json");
    expect(providerMocks.refreshCodexQuotas).toHaveBeenCalledTimes(1);
  });

  it("imports codex account directory from toolbar action", async () => {
    const user = userEvent.setup();
    vi.mocked(open).mockResolvedValueOnce("/tmp/codex-auth");

    render(<ProvidersPanel />);
    await openAddAccountDialog(user);
    await switchAddProviderToCodex(user);

    const importButton = document.querySelector('[data-slot="providers-add-codex-import-directory"]');
    if (!(importButton instanceof HTMLButtonElement)) {
      throw new Error("Missing codex import directory button");
    }

    await user.click(importButton);

    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
    });
    expect(providerMocks.importCodexFile).toHaveBeenCalledWith("/tmp/codex-auth");
    expect(providerMocks.refreshCodexQuotas).toHaveBeenCalledTimes(1);
  });

  it("optimistically hides selected rows while batch delete is in progress", async () => {
    const user = userEvent.setup();
    let resolveDelete: ((value: undefined) => void) | undefined;
    const deletePromise = new Promise<undefined>((resolve) => {
      resolveDelete = resolve;
    });
    providerMocks.deleteProviderAccounts.mockReturnValueOnce(deletePromise);

    render(<ProvidersPanel />);
    await screen.findByText("alice@example.com");

    await user.click(within(getAccountsTable()).getByRole("checkbox", { name: "Select all" }));
    await user.click(screen.getByRole("button", { name: `${m.common_delete()}(2)` }));

    const dialog = document.querySelector("[data-slot='accounts-batch-delete-dialog']");
    if (!(dialog instanceof HTMLElement)) {
      throw new Error("Missing accounts batch delete dialog");
    }
    await user.click(within(dialog).getByRole("button", { name: m.common_delete() }));

    expect(screen.queryByText("alice@example.com")).not.toBeInTheDocument();
    expect(screen.queryByText("bob@example.com")).not.toBeInTheDocument();
    expect(screen.getByText(m.providers_accounts_loading())).toBeInTheDocument();

    resolveDelete?.(undefined);
    await waitFor(() => {
      expect(providerMocks.toastSuccess).toHaveBeenCalledWith(
        m.providers_accounts_delete_success({ count: 2 })
      );
    });
  });

  it("keeps codex import enabled while unrelated kiro data is loading", async () => {
    const user = userEvent.setup();
    providerMocks.kiroQuotasLoading = true;
    vi.mocked(open)
      .mockResolvedValueOnce("/tmp/codex-account.json")
      .mockResolvedValueOnce("/tmp/codex-auth");

    render(<ProvidersPanel />);
    await openAddAccountDialog(user);
    await switchAddProviderToCodex(user);

    const importFileButton = document.querySelector('[data-slot="providers-add-codex-import-file"]');
    if (!(importFileButton instanceof HTMLButtonElement)) {
      throw new Error("Missing codex import file button");
    }
    const importDirectoryButton = document.querySelector('[data-slot="providers-add-codex-import-directory"]');
    if (!(importDirectoryButton instanceof HTMLButtonElement)) {
      throw new Error("Missing codex import directory button");
    }

    expect(importFileButton.disabled).toBe(false);
    expect(importDirectoryButton.disabled).toBe(false);

    await user.click(importFileButton);
    await user.click(importDirectoryButton);

    expect(open).toHaveBeenNthCalledWith(1, {
      directory: false,
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    expect(open).toHaveBeenNthCalledWith(2, {
      directory: true,
      multiple: false,
    });
    expect(providerMocks.importCodexFile).toHaveBeenNthCalledWith(1, "/tmp/codex-account.json");
    expect(providerMocks.importCodexFile).toHaveBeenNthCalledWith(2, "/tmp/codex-auth");
  });
});
