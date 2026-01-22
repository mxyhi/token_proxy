import { useCallback, useEffect, useRef, useState } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { pollAntigravityLogin, startAntigravityLogin } from "@/features/antigravity/api";
import type { AntigravityLoginStartResponse } from "@/features/antigravity/types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

export type AntigravityLoginState = {
  status: "idle" | "waiting" | "polling" | "success" | "error";
  start?: AntigravityLoginStartResponse;
  error?: string;
};

type LoginPollingHandlers = {
  onSuccess: (accountId?: string) => Promise<void>;
  onError: (message: string) => void;
  onPending: () => void;
  onException: (error: unknown) => void;
};

type UseAntigravityLoginOptions = {
  onRefresh: () => Promise<void> | void;
  onSelect?: (accountId: string) => void;
};

function startLoginPolling(
  state: string,
  intervalSeconds: number,
  handlers: LoginPollingHandlers
) {
  return window.setInterval(async () => {
    try {
      const result = await pollAntigravityLogin(state);
      if (result.status === "success") {
        await handlers.onSuccess(result.account?.account_id ?? undefined);
        return;
      }
      if (result.status === "error") {
        handlers.onError(result.error ?? m.antigravity_login_failed());
        return;
      }
      handlers.onPending();
    } catch (error) {
      handlers.onException(error);
    }
  }, intervalSeconds * 1000);
}

export function useAntigravityLogin({ onRefresh, onSelect }: UseAntigravityLoginOptions) {
  const [login, setLogin] = useState<AntigravityLoginState>({ status: "idle" });
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
          toast.success(m.antigravity_login_success());
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
    [clearPoller, onRefresh, onSelect]
  );

  const beginLogin = useCallback(async () => {
    setLogin({ status: "waiting" });
    try {
      const start = await startAntigravityLogin();
      setLogin({ status: "waiting", start });
      if (start.login_url) {
        void openUrl(start.login_url);
      }
      const intervalSeconds = start.interval_seconds || 2;
      startPolling(start.state, intervalSeconds);
    } catch (err) {
      setLogin({ status: "error", error: parseError(err) });
    }
  }, [startPolling]);

  useEffect(() => () => clearPoller(), [clearPoller]);

  return { login, beginLogin, clearPoller };
}
