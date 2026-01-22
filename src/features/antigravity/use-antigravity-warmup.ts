import { useCallback, useEffect, useState } from "react";

import {
  listAntigravityWarmupSchedules,
  runAntigravityWarmup,
  setAntigravityWarmupSchedule,
  toggleAntigravityWarmupSchedule,
} from "@/features/antigravity/api";
import type { AntigravityWarmupScheduleSummary } from "@/features/antigravity/types";
import { parseError } from "@/lib/error";

export function useAntigravityWarmup() {
  const [schedules, setSchedules] = useState<AntigravityWarmupScheduleSummary[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [running, setRunning] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await listAntigravityWarmupSchedules();
      setSchedules(next);
      setError("");
    } catch (err) {
      setError(parseError(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const runWarmup = useCallback(async (accountId: string, model: string, stream: boolean) => {
    setRunning(true);
    try {
      await runAntigravityWarmup(accountId, model, stream);
      setError("");
    } catch (err) {
      const message = parseError(err);
      setError(message);
      throw err;
    } finally {
      setRunning(false);
    }
  }, []);

  const setSchedule = useCallback(
    async (accountId: string, model: string, intervalMinutes: number, enabled: boolean) => {
      setLoading(true);
      try {
        const next = await setAntigravityWarmupSchedule(
          accountId,
          model,
          intervalMinutes,
          enabled
        );
        setSchedules((prev) => {
          const other = prev.filter(
            (item) => !(item.account_id === accountId && item.model === model)
          );
          return [...other, next];
        });
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

  const toggleSchedule = useCallback(
    async (accountId: string, model: string, enabled: boolean) => {
      setLoading(true);
      try {
        await toggleAntigravityWarmupSchedule(accountId, model, enabled);
        setSchedules((prev) =>
          prev.map((item) =>
            item.account_id === accountId && item.model === model
              ? { ...item, enabled }
              : item
          )
        );
        setError("");
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

  return { schedules, loading, error, running, refresh, runWarmup, setSchedule, toggleSchedule };
}
