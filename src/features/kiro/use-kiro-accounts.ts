import { useCallback, useEffect, useState } from "react";

import {
  importKiroIdeTokens,
  importKiroKamTokens,
  listKiroAccounts,
  logoutKiroAccount,
  refreshKiroQuotaCache,
  refreshKiroQuotaNow,
  setKiroStatus,
  setKiroProxyUrl,
} from "@/features/kiro/api";
import type { KiroAccountSummary } from "@/features/kiro/types";
import { parseError } from "@/lib/error";

type UseKiroAccountsOptions = {
  autoLoad?: boolean;
};

export function useKiroAccounts(options?: UseKiroAccountsOptions) {
  const autoLoad = options?.autoLoad ?? true;
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

  const setProxyUrl = useCallback(async (accountId: string, proxyUrl: string | null) => {
    setLoading(true);
    try {
      const updated = await setKiroProxyUrl(accountId, proxyUrl);
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

  const setStatus = useCallback(async (accountId: string, status: "active" | "disabled") => {
    setLoading(true);
    try {
      const updated = await setKiroStatus(accountId, status);
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

  const refreshQuotaCache = useCallback(async (accountIds?: string[]) => {
    await refreshKiroQuotaCache(accountIds);
  }, []);

  const refreshQuotaNow = useCallback(async (accountId: string) => {
    await refreshKiroQuotaNow(accountId);
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
    logout,
    importIde,
    importKam,
    setProxyUrl,
    setStatus,
    refreshQuotaCache,
    refreshQuotaNow,
  };
}
