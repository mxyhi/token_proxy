import { useCallback, useEffect, useState } from "react";

import { listCodexAccounts, logoutCodexAccount } from "@/features/codex/api";
import type { CodexAccountSummary } from "@/features/codex/types";
import { parseError } from "@/lib/error";

export function useCodexAccounts() {
  const [accounts, setAccounts] = useState<CodexAccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listCodexAccounts();
      setAccounts(next);
      setError("");
    } catch (err) {
      setError(parseError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const logout = useCallback(async (accountId: string) => {
    await logoutCodexAccount(accountId);
    await refresh();
  }, [refresh]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { accounts, loading, error, refresh, logout };
}
