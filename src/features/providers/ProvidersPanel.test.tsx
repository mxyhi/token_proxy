import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ProvidersPanel } from "@/features/providers/ProvidersPanel";
import { m } from "@/paraglide/messages.js";

const providerMocks = vi.hoisted(() => {
  const refreshKiroAccounts = vi.fn(async () => undefined);
  const refreshCodexAccounts = vi.fn(async () => undefined);
  const refreshAntigravityAccounts = vi.fn(async () => undefined);
  const refreshKiroQuotas = vi.fn(async () => undefined);
  const refreshCodexQuotas = vi.fn(async () => undefined);
  const refreshAntigravityQuotas = vi.fn(async () => undefined);
  const refreshAntigravityIde = vi.fn(async () => undefined);
  const refreshAntigravityWarmup = vi.fn(async () => undefined);
  const logoutKiro = vi.fn(async () => undefined);
  const logoutCodex = vi.fn(async () => undefined);
  const logoutAntigravity = vi.fn(async () => undefined);
  const beginKiroLogin = vi.fn();
  const beginCodexLogin = vi.fn();
  const beginAntigravityLogin = vi.fn();
  const importKiroIde = vi.fn(async () => undefined);
  const importKiroKam = vi.fn(async () => undefined);
  const importAntigravityIde = vi.fn(async () => undefined);
  const switchAntigravityIdeAccount = vi.fn(async () => ({
    database_available: true,
    ide_running: true,
    active_email: "antigravity@example.com",
  }));
  const runWarmup = vi.fn(async () => undefined);
  const setWarmupSchedule = vi.fn(async () => ({
    account_id: "ag-1",
    model: "sonnet",
    interval_minutes: 60,
    enabled: true,
  }));
  const toggleWarmupSchedule = vi.fn(async () => undefined);

  return {
    refreshKiroAccounts,
    refreshCodexAccounts,
    refreshAntigravityAccounts,
    refreshKiroQuotas,
    refreshCodexQuotas,
    refreshAntigravityQuotas,
    refreshAntigravityIde,
    refreshAntigravityWarmup,
    logoutKiro,
    logoutCodex,
    logoutAntigravity,
    beginKiroLogin,
    beginCodexLogin,
    beginAntigravityLogin,
    importKiroIde,
    importKiroKam,
    importAntigravityIde,
    switchAntigravityIdeAccount,
    runWarmup,
    setWarmupSchedule,
    toggleWarmupSchedule,
  };
});

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: vi.fn(async () => "/Users/test"),
  join: vi.fn(async (...parts: string[]) => parts.join("/")),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
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
      },
    ],
    loading: false,
    error: "",
    refresh: providerMocks.refreshKiroAccounts,
    logout: providerMocks.logoutKiro,
    importIde: providerMocks.importKiroIde,
    importKam: providerMocks.importKiroKam,
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
      },
    ],
    loading: false,
    error: "",
    refresh: providerMocks.refreshCodexAccounts,
    logout: providerMocks.logoutCodex,
  }),
}));

vi.mock("@/features/antigravity/use-antigravity-accounts", () => ({
  useAntigravityAccounts: () => ({
    accounts: [
      {
        account_id: "ag-1",
        email: "antigravity@example.com",
        expires_at: "2026-06-01T00:00:00Z",
        status: "active",
        source: "ide",
      },
    ],
    loading: false,
    error: "",
    refresh: providerMocks.refreshAntigravityAccounts,
    logout: providerMocks.logoutAntigravity,
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
    loading: false,
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
    loading: false,
    error: "",
    refresh: providerMocks.refreshCodexQuotas,
  }),
}));

vi.mock("@/features/antigravity/use-antigravity-quotas", () => ({
  useAntigravityQuotas: () => ({
    quotas: [
      {
        account_id: "ag-1",
        plan_type: "Team",
        error: null,
        quotas: [
          {
            name: "sonnet",
            percentage: 75,
            reset_at: "2026-04-20T00:00:00Z",
          },
        ],
      },
    ],
    loading: false,
    error: "",
    refresh: providerMocks.refreshAntigravityQuotas,
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

vi.mock("@/features/antigravity/use-antigravity-login", () => ({
  useAntigravityLogin: () => ({
    login: { status: "idle" },
    beginLogin: providerMocks.beginAntigravityLogin,
  }),
}));

vi.mock("@/features/antigravity/use-antigravity-ide", () => ({
  useAntigravityIde: () => ({
    status: {
      database_available: true,
      ide_running: true,
      active_email: "antigravity@example.com",
    },
    loading: false,
    error: "",
    refresh: providerMocks.refreshAntigravityIde,
    importIde: providerMocks.importAntigravityIde,
    switchAccount: providerMocks.switchAntigravityIdeAccount,
  }),
}));

vi.mock("@/features/antigravity/use-antigravity-warmup", () => ({
  useAntigravityWarmup: () => ({
    schedules: [],
    loading: false,
    error: "",
    running: false,
    refresh: providerMocks.refreshAntigravityWarmup,
    runWarmup: providerMocks.runWarmup,
    setSchedule: providerMocks.setWarmupSchedule,
    toggleSchedule: providerMocks.toggleWarmupSchedule,
  }),
}));

function getAccountRow(label: string) {
  const accountCell = within(getAccountsTable()).getByText(label);
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

afterEach(() => {
  cleanup();
});

describe("providers/ProvidersPanel", () => {
  it("renders accounts in a unified table", () => {
    render(<ProvidersPanel />);

    expect(screen.getByRole("columnheader", { name: m.providers_table_provider() })).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: m.providers_table_account() })).toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("alice@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("antigravity@example.com")).toBeInTheDocument();
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
    expect(within(getAccountsTable()).queryByText("antigravity@example.com")).not.toBeInTheDocument();
  });

  it("filters rows by provider and status", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(within(getToolbar()).getByLabelText(m.providers_filter_provider_label()));
    await user.click(screen.getByRole("option", { name: m.providers_codex_title() }));

    expect(within(getAccountsTable()).queryByText("alice@example.com")).not.toBeInTheDocument();
    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).queryByText("antigravity@example.com")).not.toBeInTheDocument();

    await user.click(within(getToolbar()).getByLabelText(m.providers_filter_status_label()));
    await user.click(screen.getByRole("option", { name: m.codex_account_status_expired() }));

    expect(within(getAccountsTable()).getByText("bob@example.com")).toBeInTheDocument();
    expect(within(getAccountsTable()).queryByText("alice@example.com")).not.toBeInTheDocument();
  });

  it("opens account dialog from edit action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(within(getAccountRow("alice@example.com")).getByRole("button", { name: m.common_edit() }));

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText(m.providers_account_dialog_title())).toBeInTheDocument();
    expect(screen.getAllByText("alice@example.com").length).toBeGreaterThan(0);
    expect(screen.getAllByText("kiro-1").length).toBeGreaterThan(0);
  });

  it("refreshes all provider data from toolbar action", async () => {
    const user = userEvent.setup();
    render(<ProvidersPanel />);

    await user.click(within(getToolbar()).getByRole("button", { name: m.common_refresh() }));

    expect(providerMocks.refreshKiroAccounts).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshKiroQuotas).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshCodexAccounts).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshCodexQuotas).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshAntigravityAccounts).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshAntigravityQuotas).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshAntigravityIde).toHaveBeenCalledTimes(1);
    expect(providerMocks.refreshAntigravityWarmup).toHaveBeenCalledTimes(1);
  });
});
