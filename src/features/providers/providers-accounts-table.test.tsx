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
    statusLabel: "Active",
    statusVariant: "secondary",
    expiresAtLabel: "2026-04-01",
    planType: "Pro",
    quotaSummary: "Requests · 1 / 100",
    sourceOrMethodLabel: index % 2 === 0 ? "Google" : "—",
    detailDescription: `detail-${index}`,
    detailFields: [],
    quotaError: "",
    quotaItems: [],
    canRefresh: index % 2 === 1,
    logoutLabel: "Logout",
    autoRefreshEnabled: index % 2 === 1 ? true : null,
  };
}

describe("providers/providers-accounts-table", () => {
  it("renders pagination controls and changes rows when moving to next page", async () => {
    const user = userEvent.setup();
    const onRefresh = vi.fn(async () => undefined);
    const onLogout = vi.fn(async () => undefined);
    const onBatchDelete = vi.fn(async () => undefined);
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
});
