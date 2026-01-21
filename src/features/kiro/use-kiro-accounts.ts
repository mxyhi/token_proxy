import { useCallback, useEffect, useState } from "react";

import {
  importKiroIdeTokens,
  importKiroKamTokens,
  listKiroAccounts,
  logoutKiroAccount,
} from "@/features/kiro/api";
import type { KiroAccountSummary } from "@/features/kiro/types";
import { parseError } from "@/lib/error";

export function useKiroAccounts() {
  const [accounts, setAccounts] = useState<KiroAccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listKiroAccounts();
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
      await logoutKiroAccount(accountId);
      await refresh();
    },
    [refresh],
  );

  const importIde = useCallback(async (directory: string) => {
    setLoading(true);
    try {
      const imported = await importKiroIdeTokens(directory);
      const next = await listKiroAccounts();
      setAccounts(next);
      setError("");
      return imported;
    } catch (err) {
      const message = parseError(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const importKam = useCallback(async (path: string) => {
    setLoading(true);
    try {
      const imported = await importKiroKamTokens(path);
      const next = await listKiroAccounts();
      setAccounts(next);
      setError("");
      return imported;
    } catch (err) {
      const message = parseError(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { accounts, loading, error, refresh, logout, importIde, importKam };
}
