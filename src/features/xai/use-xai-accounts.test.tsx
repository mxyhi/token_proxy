import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useXaiAccounts } from "@/features/xai/use-xai-accounts";
import { useXaiQuotas } from "@/features/xai/use-xai-quotas";
import type { XaiAccountSummary, XaiQuotaSummary } from "@/features/xai/types";

const apiMocks = vi.hoisted(() => ({
  listXaiAccounts: vi.fn<() => Promise<XaiAccountSummary[]>>(),
  importXaiFile: vi.fn<(path: string) => Promise<XaiAccountSummary[]>>(),
  importXaiText: vi.fn<(contents: string) => Promise<XaiAccountSummary[]>>(),
  importXaiRefreshTokens: vi.fn<(contents: string) => Promise<XaiAccountSummary[]>>(),
  refreshXaiAccount: vi.fn<(accountId: string) => Promise<void>>(),
  refreshXaiQuotaCache: vi.fn<(accountIds?: string[]) => Promise<string[]>>(),
  refreshXaiQuotaNow: vi.fn<(accountId: string) => Promise<void>>(),
  fetchXaiQuotas: vi.fn<() => Promise<XaiQuotaSummary[]>>(),
  setXaiAutoRefresh: vi.fn(),
}));

vi.mock("@/features/xai/api", () => apiMocks);

const ACCOUNT: XaiAccountSummary = {
  account_id: "xai-user@example.com",
  email: "user@example.com",
  expires_at: "2027-01-01T00:00:00Z",
  status: "active",
  auto_refresh_enabled: true,
  proxy_url: null,
  priority: 0,
};

describe("xai account hooks", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("loads only the local account list on mount", async () => {
    apiMocks.listXaiAccounts.mockResolvedValueOnce([ACCOUNT]);

    const { result } = renderHook(() => useXaiAccounts());

    await waitFor(() => {
      expect(result.current.accounts).toEqual([ACCOUNT]);
    });
    expect(apiMocks.refreshXaiQuotaCache).not.toHaveBeenCalled();
    expect(apiMocks.refreshXaiQuotaNow).not.toHaveBeenCalled();
    expect(apiMocks.fetchXaiQuotas).not.toHaveBeenCalled();
  });

  it("autoLoad=false keeps account and quota APIs idle", async () => {
    renderHook(() => useXaiAccounts({ autoLoad: false }));

    await waitFor(() => {
      expect(apiMocks.listXaiAccounts).not.toHaveBeenCalled();
    });
    expect(apiMocks.refreshXaiQuotaCache).not.toHaveBeenCalled();
    expect(apiMocks.refreshXaiQuotaNow).not.toHaveBeenCalled();
  });

  it("stores and rethrows refresh errors without probing quota", async () => {
    apiMocks.refreshXaiAccount.mockRejectedValueOnce(new Error("refresh failed"));
    const { result } = renderHook(() => useXaiAccounts({ autoLoad: false }));

    await act(async () => {
      await expect(result.current.refreshAccount(ACCOUNT.account_id)).rejects.toThrow(
        "refresh failed",
      );
    });

    expect(result.current.error).toBe("refresh failed");
    expect(apiMocks.refreshXaiQuotaCache).not.toHaveBeenCalled();
    expect(apiMocks.refreshXaiQuotaNow).not.toHaveBeenCalled();
  });

  it("imports every supported credential form without automatic quota requests", async () => {
    apiMocks.importXaiFile.mockResolvedValueOnce([ACCOUNT]);
    apiMocks.importXaiText.mockResolvedValueOnce([ACCOUNT]);
    apiMocks.importXaiRefreshTokens.mockResolvedValueOnce([ACCOUNT]);
    const { result } = renderHook(() => useXaiAccounts({ autoLoad: false }));

    await act(async () => {
      await result.current.importFile("/tmp/xai-account.json");
      await result.current.importText('{"type":"xai","auth_kind":"oauth"}');
      await result.current.importRefreshTokens("refresh-token");
    });

    expect(apiMocks.importXaiFile).toHaveBeenCalledWith("/tmp/xai-account.json");
    expect(apiMocks.importXaiText).toHaveBeenCalledWith(
      '{"type":"xai","auth_kind":"oauth"}',
    );
    expect(apiMocks.importXaiRefreshTokens).toHaveBeenCalledWith("refresh-token");
    expect(apiMocks.listXaiAccounts).not.toHaveBeenCalled();
    expect(apiMocks.refreshXaiQuotaCache).not.toHaveBeenCalled();
    expect(apiMocks.refreshXaiQuotaNow).not.toHaveBeenCalled();
  });

  it("fetches quota only after an explicit refresh action", async () => {
    apiMocks.fetchXaiQuotas.mockResolvedValueOnce([]);
    const { result } = renderHook(() => useXaiQuotas());

    expect(apiMocks.fetchXaiQuotas).not.toHaveBeenCalled();
    await act(async () => {
      await result.current.refresh();
    });

    expect(apiMocks.fetchXaiQuotas).toHaveBeenCalledOnce();
  });
});
