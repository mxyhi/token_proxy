import { useEffect, useRef } from "react";

export type LoginStatus = "idle" | "waiting" | "polling" | "success" | "error";

type AutoCloseOptions = {
  open: boolean;
  status: LoginStatus;
  delayMs?: number;
  setOpen: (open: boolean) => void;
};

const DEFAULT_AUTO_CLOSE_DELAY_MS = 1500;

export function useAutoCloseLoginDialog({
  open,
  status,
  delayMs = DEFAULT_AUTO_CLOSE_DELAY_MS,
  setOpen,
}: AutoCloseOptions) {
  const timerRef = useRef<number | null>(null);
  const prevStatusRef = useRef<LoginStatus | null>(null);

  useEffect(() => {
    const previousStatus = prevStatusRef.current;
    prevStatusRef.current = status;

    // Only auto-close on the first success transition while the dialog is open.
    if (!open || status !== "success" || previousStatus === "success") {
      return;
    }

    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
    }

    timerRef.current = window.setTimeout(() => {
      setOpen(false);
      timerRef.current = null;
    }, delayMs);

    return () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [delayMs, open, setOpen, status]);
}
