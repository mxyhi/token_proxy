import { useCallback, useEffect, useRef, useState } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";
import { pollKiroLogin, startKiroLogin } from "@/features/kiro/api";
import type { KiroLoginMethod, KiroLoginStartResponse } from "@/features/kiro/types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

export type KiroLoginState = {
  status: "idle" | "waiting" | "polling" | "success" | "error";
  start?: KiroLoginStartResponse;
  error?: string;
};

type LoginPollingHandlers = {
  onSuccess: (accountId?: string) => Promise<void>;
  onError: (message: string) => void;
  onPending: () => void;
  onException: (error: unknown) => void;
};

type UseKiroLoginOptions = {
  onRefresh: () => Promise<void> | void;
  onSelect?: (accountId: string) => void;
};

function startLoginPolling(
  state: string,
  intervalSeconds: number,
  handlers: LoginPollingHandlers,
) {
  return window.setInterval(async () => {
    try {
      const result = await pollKiroLogin(state);
      if (result.status === "success") {
        await handlers.onSuccess(result.account?.account_id);
        return;
      }
      if (result.status === "error") {
        handlers.onError(result.error ?? m.kiro_login_failed());
        return;
      }
      handlers.onPending();
    } catch (error) {
      handlers.onException(error);
    }
  }, intervalSeconds * 1000);
}

export function useKiroLogin({ onRefresh, onSelect }: UseKiroLoginOptions) {
  const [login, setLogin] = useState<KiroLoginState>({ status: "idle" });
  const pollTimer = useRef<number | null>(null);

  const clearPoller = useCallback(() => {
    if (pollTimer.current !== null) {
      window.clearInterval(pollTimer.current);
      pollTimer.current = null;
    }
  }, []);

  const startPolling = useCallback(
    (state: string, intervalSeconds: number) => {
      clearPoller();
      pollTimer.current = startLoginPolling(state, intervalSeconds, {
        onSuccess: async (accountId) => {
          clearPoller();
          setLogin({ status: "success" });
          await Promise.resolve(onRefresh());
          if (accountId && onSelect) {
            onSelect(accountId);
          }
        },
        onError: (message) => {
          clearPoller();
          setLogin({ status: "error", error: message });
        },
        onPending: () => {
          setLogin((prev) => ({ ...prev, status: "polling", error: "" }));
        },
        onException: (error) => {
          clearPoller();
          setLogin({ status: "error", error: parseError(error) });
        },
      });
    },
    [clearPoller, onRefresh, onSelect],
  );

  const beginLogin = useCallback(
    async (method: KiroLoginMethod) => {
      setLogin({ status: "waiting" });
      try {
        const start = await startKiroLogin(method);
        setLogin({ status: "waiting", start });
        if (start.login_url) {
          void openUrl(start.login_url);
        }
        const intervalSeconds = start.interval_seconds ?? 3;
        startPolling(start.state, intervalSeconds);
      } catch (err) {
        setLogin({ status: "error", error: parseError(err) });
      }
    },
    [startPolling],
  );

  useEffect(() => () => clearPoller(), [clearPoller]);

  return { login, beginLogin, clearPoller };
}
