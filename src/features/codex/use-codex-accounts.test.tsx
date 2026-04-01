import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useCodexAccounts } from "@/features/codex/use-codex-accounts";

const apiMocks = vi.hoisted(() => ({
  listCodexAccounts: vi.fn(),
  importCodexFile: vi.fn(),
  refreshCodexQuotaCache: vi.fn(),
  refreshCodexQuotaNow: vi.fn(),
  setCodexAutoRefresh: vi.fn(),
  setCodexEnabled: vi.fn(),
  refreshCodexAccount: vi.fn(),
  logoutCodexAccount: vi.fn(),
}));

vi.mock("@/features/codex/api", () => ({
  listCodexAccounts: apiMocks.listCodexAccounts,
  importCodexFile: apiMocks.importCodexFile,
  refreshCodexQuotaCache: apiMocks.refreshCodexQuotaCache,
  refreshCodexQuotaNow: apiMocks.refreshCodexQuotaNow,
  setCodexAutoRefresh: apiMocks.setCodexAutoRefresh,
  setCodexEnabled: apiMocks.setCodexEnabled,
  refreshCodexAccount: apiMocks.refreshCodexAccount,
  logoutCodexAccount: apiMocks.logoutCodexAccount,
}));

describe("codex/use-codex-accounts", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("autoLoad=false 时挂载不主动拉账户", async () => {
    renderHook(() => useCodexAccounts({ autoLoad: false }));

    await waitFor(() => {
      expect(apiMocks.listCodexAccounts).not.toHaveBeenCalled();
    });
  });

  it("refreshAccount 失败时不写入全局 error", async () => {
    apiMocks.listCodexAccounts.mockResolvedValue([
      {
        account_id: "codex-1",
        email: "bob@example.com",
        expires_at: "2026-04-01T00:00:00Z",
        status: "active",
      },
    ]);
    apiMocks.refreshCodexAccount.mockRejectedValue(new Error("Codex 登录已失效，请重新登录该账户。"));

    const { result } = renderHook(() => useCodexAccounts());

    await waitFor(() => {
      expect(result.current.accounts).toHaveLength(1);
    });

    await act(async () => {
      await expect(result.current.refreshAccount("codex-1")).rejects.toThrow(
        "Codex 登录已失效，请重新登录该账户。"
      );
    });

    expect(result.current.error).toBe("");
  });

  it("importFile 不会在导入成功后额外拉账户列表", async () => {
    apiMocks.importCodexFile.mockResolvedValue([
      {
        account_id: "codex-1",
        email: "bob@example.com",
        expires_at: "2026-04-01T00:00:00Z",
        status: "active",
      },
    ]);

    const { result } = renderHook(() => useCodexAccounts({ autoLoad: false }));

    await act(async () => {
      await expect(result.current.importFile("/tmp/codex-account.json")).resolves.toEqual([
        {
          account_id: "codex-1",
          email: "bob@example.com",
          expires_at: "2026-04-01T00:00:00Z",
          status: "active",
        },
      ]);
    });

    expect(apiMocks.importCodexFile).toHaveBeenCalledWith("/tmp/codex-account.json");
    expect(apiMocks.listCodexAccounts).not.toHaveBeenCalled();
  });
});
