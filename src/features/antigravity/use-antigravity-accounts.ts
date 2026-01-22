import { useCallback, useEffect, useState } from "react";

import { listAntigravityAccounts, logoutAntigravityAccount } from "@/features/antigravity/api";
import type { AntigravityAccountSummary } from "@/features/antigravity/types";
import { parseError } from "@/lib/error";

export function useAntigravityAccounts() {
  const [accounts, setAccounts] = useState<AntigravityAccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listAntigravityAccounts();
      setAccounts(next);
      setError("");
    } catch (err) {
      setError(parseError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const logout = useCallback(
    async (accountId: string) => {
      await logoutAntigravityAccount(accountId);
      await refresh();
    },
    [refresh]
  );

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { accounts, loading, error, refresh, logout };
}
