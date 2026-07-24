import { useCallback, useEffect, useState } from "react";

import {
  importXaiFile,
  importXaiRefreshTokens,
  importXaiText,
  listXaiAccounts,
  refreshXaiAccount,
  refreshXaiQuotaCache,
  refreshXaiQuotaNow,
  setXaiAutoRefresh,
} from "@/features/xai/api";
import type { XaiAccountSummary } from "@/features/xai/types";
import { parseError } from "@/lib/error";

type UseXaiAccountsOptions = {
  autoLoad?: boolean;
};

export function useXaiAccounts(options?: UseXaiAccountsOptions) {
  const autoLoad = options?.autoLoad ?? true;
  const [accounts, setAccounts] = useState<XaiAccountSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listXaiAccounts();
      setAccounts(next);
      setError("");
      return next;
    } catch (cause) {
      setError(parseError(cause));
      return [];
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshAccount = useCallback(async (accountId: string) => {
    setLoading(true);
    try {
      await refreshXaiAccount(accountId);
      const next = await listXaiAccounts();
      setAccounts(next);
      setError("");
    } catch (cause) {
      setError(parseError(cause));
      throw cause;
    } finally {
      setLoading(false);
    }
  }, []);

  const updateAccount = useCallback((updated: XaiAccountSummary) => {
    setAccounts((current) =>
      current.map((account) =>
        account.account_id === updated.account_id ? { ...account, ...updated } : account,
      ),
    );
    setError("");
    return updated;
  }, []);

  const setAutoRefresh = useCallback(
    async (accountId: string, enabled: boolean) => {
      try {
        return updateAccount(await setXaiAutoRefresh(accountId, enabled));
      } catch (cause) {
        setError(parseError(cause));
        throw cause;
      }
    },
    [updateAccount],
  );

  const importFile = useCallback(async (path: string) => {
    try {
      const imported = await importXaiFile(path);
      setError("");
      return imported;
    } catch (cause) {
      setError(parseError(cause));
      throw cause;
    }
  }, []);

  const importText = useCallback(async (contents: string) => {
    try {
      const imported = await importXaiText(contents);
      setError("");
      return imported;
    } catch (cause) {
      setError(parseError(cause));
      throw cause;
    }
  }, []);

  const importRefreshTokens = useCallback(async (contents: string) => {
    try {
      const imported = await importXaiRefreshTokens(contents);
      setError("");
      return imported;
    } catch (cause) {
      setError(parseError(cause));
      throw cause;
    }
  }, []);

  const refreshQuotaCache = useCallback(async (accountIds?: string[]) => {
    await refreshXaiQuotaCache(accountIds);
  }, []);

  const refreshQuotaNow = useCallback(async (accountId: string) => {
    await refreshXaiQuotaNow(accountId);
  }, []);

  useEffect(() => {
    if (autoLoad) {
      void refresh();
    }
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
