import { useCallback, useEffect, useState } from "react";

import {
  fetchAntigravityIdeStatus,
  importAntigravityIde,
  switchAntigravityIdeAccount,
} from "@/features/antigravity/api";
import type { AntigravityIdeStatus } from "@/features/antigravity/types";
import { parseError } from "@/lib/error";

type UseAntigravityIdeOptions = {
  onRefreshAccounts?: () => Promise<void> | void;
};

export function useAntigravityIde({ onRefreshAccounts }: UseAntigravityIdeOptions = {}) {
  const [status, setStatus] = useState<AntigravityIdeStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await fetchAntigravityIdeStatus();
      setStatus(next);
      setError("");
    } catch (err) {
      setError(parseError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const importIde = useCallback(
    async (ideDbPath?: string) => {
      setLoading(true);
      try {
        const imported = await importAntigravityIde(ideDbPath);
        await Promise.resolve(onRefreshAccounts?.());
        await refresh();
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
    [onRefreshAccounts, refresh]
  );

  const switchAccount = useCallback(
    async (accountId: string, ideDbPath?: string) => {
      setLoading(true);
      try {
        const next = await switchAntigravityIdeAccount(accountId, ideDbPath);
        setStatus(next);
        setError("");
        return next;
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

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { status, loading, error, refresh, importIde, switchAccount };
}
