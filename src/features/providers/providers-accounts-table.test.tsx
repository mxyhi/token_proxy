import { useState } from "react";

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ProvidersAccountsTableSection,
  type ProviderAccountTableRow,
} from "@/features/providers/providers-accounts-table";
import { m } from "@/paraglide/messages.js";

afterEach(() => {
  cleanup();
});

function buildRow(index: number): ProviderAccountTableRow {
  return {
    id: `row-${index}`,
    provider: index % 2 === 0 ? "kiro" : "codex",
    providerLabel: index % 2 === 0 ? "Kiro" : "Codex",
    displayName: `user-${index}@example.com`,
    accountId: `account-${index}.json`,
    status: "active",
    statusLabel: "Active",
    statusVariant: "secondary",
    expiresAtLabel: "2026-04-01",
    planType: "Pro",
    quotaSummary: "Requests · 1 / 100",
    sourceOrMethodLabel: index % 2 === 0 ? "Google" : "—",
    detailDescription: `detail-${index}`,
    detailFields: [],
    proxyUrlValue: index % 2 === 0 ? "http://127.0.0.1:7890" : "",
    quotaError: "",
    quotaItems: [],
    canRefresh: index % 2 === 1,
    logoutLabel: "Logout",
    autoRefreshEnabled: index % 2 === 1 ? true : null,
  };
}

describe("providers/providers-accounts-table", () => {
  it("uses a compact ID header for the account id column", () => {
    const rows = [buildRow(1)];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={vi.fn(async () => undefined)}
        onLogout={vi.fn(async () => undefined)}
        onBatchDelete={vi.fn(async () => undefined)}
        onSaveProxyUrl={vi.fn(async () => undefined)}
        onRefreshQuota={vi.fn(async () => undefined)}
        onToggleStatus={vi.fn(async () => undefined)}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    expect(screen.getByRole("columnheader", { name: "ID" })).toBeInTheDocument();
    expect(
      screen.queryByRole("columnheader", { name: m.providers_table_account_id() })
    ).not.toBeInTheDocument();
  });

  it("constrains the account column width and shows tooltip for truncated account names", async () => {
    const user = userEvent.setup();
    const longDisplayName = "very-long-account-name-for-tooltip-display@example.com";
    const rows = [{ ...buildRow(1), displayName: longDisplayName }];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={vi.fn(async () => undefined)}
        onLogout={vi.fn(async () => undefined)}
        onBatchDelete={vi.fn(async () => undefined)}
        onSaveProxyUrl={vi.fn(async () => undefined)}
        onRefreshQuota={vi.fn(async () => undefined)}
        onToggleStatus={vi.fn(async () => undefined)}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    expect(screen.getByRole("columnheader", { name: m.providers_table_account() })).toHaveClass(
      "w-[10rem]"
    );

    const accountNameCell = screen.getByText(longDisplayName);
    expect(accountNameCell).toHaveClass("max-w-[10rem]", "truncate");

    await user.hover(accountNameCell);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(longDisplayName);
  });

  it("shows tooltip for truncated account id cells on hover", async () => {
    const user = userEvent.setup();
    const longAccountId = "codex-account-with-a-very-long-id-for-tooltip-display.json";
    const rows = [{ ...buildRow(1), accountId: longAccountId }];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={vi.fn(async () => undefined)}
        onLogout={vi.fn(async () => undefined)}
        onBatchDelete={vi.fn(async () => undefined)}
        onSaveProxyUrl={vi.fn(async () => undefined)}
        onRefreshQuota={vi.fn(async () => undefined)}
        onToggleStatus={vi.fn(async () => undefined)}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    const accountIdCell = screen.getByText(longAccountId);
    await user.hover(accountIdCell);
    expect(await screen.findByRole("tooltip")).toHaveTextContent(longAccountId);
  });

  it("renders pagination controls and changes rows when moving to next page", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const allRows = Array.from({ length: 11 }, (_, index) => buildRow(index + 1));

    function Harness() {
      const [page, setPage] = useState(1);
      const pageSize = 10;
      const rows = allRows.slice((page - 1) * pageSize, page * pageSize);
      const totalPages = Math.ceil(allRows.length / pageSize);

      return (
        <ProvidersAccountsTableSection
          rows={rows}
          loading={false}
          error=""
          page={page}
          totalPages={totalPages}
          totalItems={allRows.length}
          onPrevPage={() => setPage((current) => Math.max(1, current - 1))}
          onNextPage={() => setPage((current) => Math.min(totalPages, current + 1))}
          onRefresh={onRefresh}
          onLogout={onLogout}
          onBatchDelete={onBatchDelete}
          onSaveProxyUrl={onSaveProxyUrl}
          onRefreshQuota={onRefreshQuota}
          onToggleStatus={onToggleStatus}
          onToggleAutoRefresh={vi.fn(async () => undefined)}
        />
      );
    }

    render(<Harness />);

    expect(screen.getByRole("button", { name: m.dashboard_next_page() })).toBeInTheDocument();
    expect(screen.getByText("user-1@example.com")).toBeInTheDocument();
    expect(screen.queryByText("user-11@example.com")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: m.dashboard_next_page() }));

    expect(screen.queryByText("user-1@example.com")).not.toBeInTheDocument();
    expect(screen.getByText("user-11@example.com")).toBeInTheDocument();
    expect(
      screen.getByTestId("providers-pagination-indicator")
    ).toHaveTextContent(m.dashboard_page_indicator({ page: "2", totalPages: "2" }));
  });

  it("clears off-page selection after pagination changes", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const allRows = Array.from({ length: 11 }, (_, index) => buildRow(index + 1));

    function Harness() {
      const [page, setPage] = useState(1);
      const pageSize = 10;
      const rows = allRows.slice((page - 1) * pageSize, page * pageSize);
      const totalPages = Math.ceil(allRows.length / pageSize);

      return (
        <ProvidersAccountsTableSection
          rows={rows}
          loading={false}
          error=""
          page={page}
          totalPages={totalPages}
          totalItems={allRows.length}
          onPrevPage={() => setPage((current) => Math.max(1, current - 1))}
          onNextPage={() => setPage((current) => Math.min(totalPages, current + 1))}
          onRefresh={onRefresh}
          onLogout={onLogout}
          onBatchDelete={onBatchDelete}
          onSaveProxyUrl={onSaveProxyUrl}
          onRefreshQuota={onRefreshQuota}
          onToggleStatus={onToggleStatus}
          onToggleAutoRefresh={vi.fn(async () => undefined)}
        />
      );
    }

    render(<Harness />);

    await user.click(screen.getByRole("checkbox", { name: "Select user-1@example.com" }));
    expect(screen.getByText(m.providers_accounts_delete_description({ count: 1 }))).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: m.dashboard_next_page() }));

    expect(
      screen.queryByText(m.providers_accounts_delete_description({ count: 1 }))
    ).not.toBeInTheDocument();
  });

  it("passes the selected rows to batch delete after confirmation", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const rows = [buildRow(1), buildRow(2)];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onBatchDelete={onBatchDelete}
        onSaveProxyUrl={onSaveProxyUrl}
        onRefreshQuota={onRefreshQuota}
        onToggleStatus={onToggleStatus}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    await user.click(screen.getByRole("checkbox", { name: "Select user-1@example.com" }));
    await user.click(screen.getByRole("button", { name: `${m.common_delete()}(1)` }));

    const dialog = document.querySelector("[data-slot='accounts-batch-delete-dialog']");
    if (!(dialog instanceof HTMLElement)) {
      throw new Error("Missing accounts batch delete dialog");
    }

    await user.click(within(dialog).getByRole("button", { name: m.common_delete() }));

    await waitFor(() => {
      expect(onBatchDelete).toHaveBeenCalledTimes(1);
    });
    expect(onBatchDelete).toHaveBeenCalledWith([rows[0]]);
  });

  it("saves proxy url from the account detail dialog", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const rows = [{ ...buildRow(1), proxyUrlValue: "http://127.0.0.1:7890" }];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onBatchDelete={onBatchDelete}
        onSaveProxyUrl={onSaveProxyUrl}
        onRefreshQuota={onRefreshQuota}
        onToggleStatus={onToggleStatus}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    await user.click(screen.getByRole("button", { name: m.providers_account_dialog_title() }));

    const input = await screen.findByLabelText(m.field_proxy_url());
    expect(input).toHaveValue("http://127.0.0.1:7890");

    await user.clear(input);
    await user.type(input, "socks5://127.0.0.1:1080");
    await user.click(screen.getByRole("button", { name: m.common_save() }));

    await waitFor(() => {
      expect(onSaveProxyUrl).toHaveBeenCalledTimes(1);
    });
    expect(onSaveProxyUrl).toHaveBeenCalledWith(rows[0], "socks5://127.0.0.1:1080");
  });

  it("renders account detail dialog as summary and list sections instead of field cards", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const rows = [
      {
        ...buildRow(2),
        detailFields: [
          { label: "邮箱", value: "bob@example.com" },
          { label: "账户 ID", value: "account-2.json" },
        ],
        quotaItems: [
          {
            name: "Requests",
            summary: "25 / 100",
            secondary: "Reset at 2026-04-15",
          },
        ],
      },
    ];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onBatchDelete={onBatchDelete}
        onSaveProxyUrl={onSaveProxyUrl}
        onRefreshQuota={onRefreshQuota}
        onToggleStatus={onToggleStatus}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    await user.click(screen.getByRole("button", { name: m.providers_account_dialog_title() }));

    expect(document.querySelector("[data-slot='provider-account-summary-band']")).toBeTruthy();
    expect(document.querySelector("[data-slot='provider-account-detail-list']")).toBeTruthy();
    expect(document.querySelector("[data-slot='provider-account-quota-list']")).toBeTruthy();
    expect(screen.getByText("邮箱")).toBeInTheDocument();
    expect(screen.getByText("25 / 100")).toBeInTheDocument();
  });

  it("triggers manual quota refresh from the account detail dialog", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const rows = [buildRow(1)];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onBatchDelete={onBatchDelete}
        onSaveProxyUrl={onSaveProxyUrl}
        onRefreshQuota={onRefreshQuota}
        onToggleStatus={onToggleStatus}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    await user.click(screen.getByRole("button", { name: m.providers_account_dialog_title() }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Refresh Quota" }));

    expect(onRefreshQuota).toHaveBeenCalledWith(rows[0]);
  });

  it("triggers account enable toggle from the account detail dialog", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
    const onSaveProxyUrl = vi.fn(async () => undefined);
    const onRefreshQuota = vi.fn(async () => undefined);
    const onToggleStatus = vi.fn(async () => undefined);
    const rows = [{ ...buildRow(2), status: "disabled" as const, statusLabel: "Disabled" }];

    render(
      <ProvidersAccountsTableSection
        rows={rows}
        loading={false}
        error=""
        page={1}
        totalPages={1}
        totalItems={rows.length}
        onPrevPage={() => undefined}
        onNextPage={() => undefined}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onBatchDelete={onBatchDelete}
        onSaveProxyUrl={onSaveProxyUrl}
        onRefreshQuota={onRefreshQuota}
        onToggleStatus={onToggleStatus}
        onToggleAutoRefresh={vi.fn(async () => undefined)}
      />
    );

    await user.click(screen.getByRole("button", { name: m.providers_account_dialog_title() }));
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Enable" }));

    expect(onToggleStatus).toHaveBeenCalledWith(rows[0], "active");
  });
});
