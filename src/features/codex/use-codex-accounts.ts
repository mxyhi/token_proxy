import { useCallback, useEffect, useState } from "react";

import {
  importCodexFile,
  listCodexAccounts,
  setCodexAutoRefresh,
  setCodexProxyUrl,
  refreshCodexAccount,
  logoutCodexAccount,
} from "@/features/codex/api";
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

  const refreshAccount = useCallback(async (accountId: string) => {
    setLoading(true);
    try {
      await refreshCodexAccount(accountId);
      const next = await listCodexAccounts();
      setAccounts(next);
      setError("");
    } catch (err) {
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const setAutoRefresh = useCallback(async (accountId: string, enabled: boolean) => {
    setLoading(true);
    try {
      const updated = await setCodexAutoRefresh(accountId, enabled);
      setAccounts((prev) =>
        prev.map((item) => (item.account_id === accountId ? { ...item, ...updated } : item))
      );
      setError("");
      return updated;
    } catch (err) {
      const message = parseError(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const setProxyUrl = useCallback(async (accountId: string, proxyUrl: string | null) => {
    setLoading(true);
    try {
      const updated = await setCodexProxyUrl(accountId, proxyUrl);
      setAccounts((prev) =>
        prev.map((item) => (item.account_id === accountId ? { ...item, ...updated } : item))
      );
      setError("");
      return updated;
    } catch (err) {
      const message = parseError(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const importFile = useCallback(async (path: string) => {
    setLoading(true);
    try {
      const imported = await importCodexFile(path);
      const next = await listCodexAccounts();
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

  return {
    accounts,
    loading,
    error,
    refresh,
    refreshAccount,
    setAutoRefresh,
    setProxyUrl,
    logout,
    importFile,
  };
}
