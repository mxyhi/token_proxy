import { useCallback, useEffect, useState } from "react";

import {
  importCodexFile,
  importCodexRefreshTokens,
  importCodexText,
  listCodexAccounts,
  refreshCodexQuotaCache,
  refreshCodexQuotaNow,
  setCodexAutoRefresh,
  refreshCodexAccount,
} from "@/features/codex/api";
import type { CodexAccountSummary } from "@/features/codex/types";
import { parseError } from "@/lib/error";

type UseCodexAccountsOptions = {
  autoLoad?: boolean;
};

export function useCodexAccounts(options?: UseCodexAccountsOptions) {
  const autoLoad = options?.autoLoad ?? true;
  const [accounts, setAccounts] = useState<CodexAccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listCodexAccounts();
      setAccounts(next);
      setError("");
      return next;
    } catch (err) {
      setError(parseError(err));
      return [];
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAccount = useCallback(async (accountId: string) => {
    setLoading(true);
    try {
      await refreshCodexAccount(accountId);
      const next = await listCodexAccounts();
      setAccounts(next);
      setError("");
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

  const importFile = useCallback(async (path: string) => {
    setLoading(true);
    try {
      const imported = await importCodexFile(path);
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

  const importText = useCallback(async (contents: string) => {
    setLoading(true);
    try {
      const imported = await importCodexText(contents);
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

  const importRefreshTokens = useCallback(
    async (contents: string, clientKind: "codex" | "mobile") => {
      setLoading(true);
      try {
        const imported = await importCodexRefreshTokens(contents, clientKind);
        setError("");
        return imported;
      } catch (err) {
        const message = parseError(err);
        setError(message);
        throw err;
      } finally {
        setLoading(false);
      }
    },
    []
  );

  const refreshQuotaCache = useCallback(async (accountIds?: string[]) => {
    await refreshCodexQuotaCache(accountIds);
  }, []);

  const refreshQuotaNow = useCallback(async (accountId: string) => {
    await refreshCodexQuotaNow(accountId);
  }, []);

  useEffect(() => {
    if (!autoLoad) {
      return;
    }
    void refresh();
  }, [autoLoad, refresh]);

  return {
    accounts,
    loading,
    error,
    refresh,
    refreshAccount,
    setAutoRefresh,
    importFile,
    importText,
    importRefreshTokens,
    refreshQuotaCache,
    refreshQuotaNow,
  };
}
