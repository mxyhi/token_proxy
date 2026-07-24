import { useCallback, useEffect, useState } from "react";

import {
  importKiroIdeTokens,
  importKiroKamTokens,
  listKiroAccounts,
  refreshKiroQuotaCache,
  refreshKiroQuotaNow,
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
      return next;
    } catch (err) {
      setError(parseError(err));
      return [];
    } finally {
      setLoading(false);
    }
  }, []);

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
    importIde,
    importKam,
    refreshQuotaCache,
    refreshQuotaNow,
  };
}
