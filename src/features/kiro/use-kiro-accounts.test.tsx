import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";

const apiMocks = vi.hoisted(() => ({
  listKiroAccounts: vi.fn(),
  importKiroIdeTokens: vi.fn(),
  importKiroKamTokens: vi.fn(),
  refreshKiroQuotaCache: vi.fn(),
  refreshKiroQuotaNow: vi.fn(),
  logoutKiroAccount: vi.fn(),
  setKiroEnabled: vi.fn(),
  setKiroProxyUrl: vi.fn(),
}));

vi.mock("@/features/kiro/api", () => ({
  listKiroAccounts: apiMocks.listKiroAccounts,
  importKiroIdeTokens: apiMocks.importKiroIdeTokens,
  importKiroKamTokens: apiMocks.importKiroKamTokens,
  refreshKiroQuotaCache: apiMocks.refreshKiroQuotaCache,
  refreshKiroQuotaNow: apiMocks.refreshKiroQuotaNow,
  logoutKiroAccount: apiMocks.logoutKiroAccount,
  setKiroEnabled: apiMocks.setKiroEnabled,
  setKiroProxyUrl: apiMocks.setKiroProxyUrl,
}));

describe("kiro/use-kiro-accounts", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("autoLoad=false 时挂载不主动拉账户", async () => {
    renderHook(() => useKiroAccounts({ autoLoad: false }));

    await waitFor(() => {
      expect(apiMocks.listKiroAccounts).not.toHaveBeenCalled();
    });
  });

  it("importIde 不会在导入成功后额外拉账户列表", async () => {
    apiMocks.importKiroIdeTokens.mockResolvedValue([
      {
        account_id: "kiro-1",
        provider: "kiro",
        auth_method: "google",
        email: "alice@example.com",
        expires_at: "2026-05-01T00:00:00Z",
        status: "active",
      },
    ]);

    const { result } = renderHook(() => useKiroAccounts({ autoLoad: false }));

    await act(async () => {
      await expect(result.current.importIde("/tmp/kiro")).resolves.toEqual([
        {
          account_id: "kiro-1",
          provider: "kiro",
          auth_method: "google",
          email: "alice@example.com",
          expires_at: "2026-05-01T00:00:00Z",
          status: "active",
        },
      ]);
    });

    expect(apiMocks.importKiroIdeTokens).toHaveBeenCalledWith("/tmp/kiro");
    expect(apiMocks.listKiroAccounts).not.toHaveBeenCalled();
  });
});
