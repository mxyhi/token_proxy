import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useCodexAccounts } from "@/features/codex/use-codex-accounts";

const apiMocks = vi.hoisted(() => ({
  listCodexAccounts: vi.fn(),
  importCodexFile: vi.fn(),
  setCodexAutoRefresh: vi.fn(),
  refreshCodexAccount: vi.fn(),
  logoutCodexAccount: vi.fn(),
}));

vi.mock("@/features/codex/api", () => ({
  listCodexAccounts: apiMocks.listCodexAccounts,
  importCodexFile: apiMocks.importCodexFile,
  setCodexAutoRefresh: apiMocks.setCodexAutoRefresh,
  refreshCodexAccount: apiMocks.refreshCodexAccount,
  logoutCodexAccount: apiMocks.logoutCodexAccount,
}));

describe("codex/use-codex-accounts", () => {
  afterEach(() => {
    vi.clearAllMocks();
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
});
