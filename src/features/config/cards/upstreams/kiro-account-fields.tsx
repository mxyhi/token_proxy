import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { pollKiroLogin, startKiroLogin } from "@/features/kiro/api";
import type {
  KiroAccountSummary,
  KiroLoginMethod,
  KiroLoginStartResponse,
} from "@/features/kiro/types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type KiroAccountFieldsProps = {
  accountId: string;
  accounts: KiroAccountSummary[];
  loading: boolean;
  error: string;
  onRefresh: () => void;
  onLogout: (accountId: string) => void;
  onSelect: (accountId: string) => void;
  onImport: () => Promise<KiroAccountSummary[]>;
};

type LoginState = {
  status: "idle" | "waiting" | "polling" | "success" | "error";
  start?: KiroLoginStartResponse;
  error?: string;
};

type KiroAccountOption = {
  value: string;
  label: string;
};

type LoginPollingHandlers = {
  onSuccess: (accountId?: string) => Promise<void>;
  onError: (message: string) => void;
  onPending: () => void;
  onException: (error: unknown) => void;
};

const LOGIN_METHODS: ReadonlyArray<{ method: KiroLoginMethod; label: () => string }> = [
  { method: "aws", label: () => m.kiro_login_method_aws() },
  { method: "aws_authcode", label: () => m.kiro_login_method_aws_authcode() },
  { method: "google", label: () => m.kiro_login_method_google() },
];

function formatAccountLabel(account: KiroAccountSummary) {
  const email = account.email?.trim();
  if (email) {
    return `${account.provider} · ${email}`;
  }
  return `${account.provider} · ${account.account_id}`;
}

function getLoginStatusText(login: LoginState) {
  if (login.status === "error") {
    return login.error ?? m.kiro_login_failed();
  }
  if (login.status === "success") {
    return m.kiro_login_success();
  }
  if (login.status === "polling") {
    return m.kiro_login_polling();
  }
  if (login.status === "waiting") {
    return m.kiro_login_waiting();
  }
  return "";
}

function shouldShowDeviceInfo(login: LoginState) {
  return Boolean(login.start?.verification_uri_complete && login.start.user_code);
}

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

function useKiroLogin(onRefresh: () => void, onSelect: (accountId: string) => void) {
  const [login, setLogin] = useState<LoginState>({ status: "idle" });
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
          await onRefresh();
          if (accountId) {
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
  return { login, beginLogin };
}

type KiroAccountSelectProps = {
  accountId: string;
  options: KiroAccountOption[];
  onSelect: (accountId: string) => void;
};

function KiroAccountSelect({ accountId, options, onSelect }: KiroAccountSelectProps) {
  return (
    <Select value={accountId.trim() ? accountId : undefined} onValueChange={onSelect}>
      <SelectTrigger>
        <SelectValue placeholder={m.kiro_account_placeholder()} />
      </SelectTrigger>
      <SelectContent>
        {options.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

type KiroLoginActionsProps = {
  accountId: string;
  loading: boolean;
  polling: boolean;
  onLogin: (method: KiroLoginMethod) => void;
  onImport: () => Promise<void>;
  onRefresh: () => void;
  onLogout: (accountId: string) => void;
};

function KiroLoginActions({
  accountId,
  loading,
  polling,
  onLogin,
  onImport,
  onRefresh,
  onLogout,
}: KiroLoginActionsProps) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      {LOGIN_METHODS.map((item) => (
        <Button
          key={item.method}
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => onLogin(item.method)}
          disabled={polling}
        >
          {item.label()}
        </Button>
      ))}
      <Button
        type="button"
        variant="secondary"
        size="sm"
        onClick={onImport}
        disabled={polling || loading}
      >
        {m.kiro_login_method_import()}
      </Button>
      <Button type="button" variant="ghost" size="sm" onClick={onRefresh} disabled={loading}>
        {m.common_refresh()}
      </Button>
      {accountId.trim() ? (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => onLogout(accountId)}
        >
          {m.kiro_account_logout()}
        </Button>
      ) : null}
    </div>
  );
}

type KiroLoginHintsProps = {
  loginInfo?: KiroLoginStartResponse;
  showDeviceInfo: boolean;
};

function KiroLoginHints({ loginInfo, showDeviceInfo }: KiroLoginHintsProps) {
  return (
    <>
      {loginInfo?.login_url ? (
        <div className="rounded-md border border-border/60 bg-muted/40 p-3 text-xs">
          <p className="font-medium text-foreground">{m.kiro_login_open_hint()}</p>
          <p className="break-all text-muted-foreground">{loginInfo.login_url}</p>
        </div>
      ) : null}
      {showDeviceInfo ? (
        <div className="rounded-md border border-border/60 bg-muted/40 p-3 text-xs">
          <p className="font-medium text-foreground">{m.kiro_device_code_title()}</p>
          <p className="text-muted-foreground">{loginInfo?.verification_uri_complete}</p>
          <p className="mt-2 font-mono text-sm text-foreground">{loginInfo?.user_code}</p>
        </div>
      ) : null}
    </>
  );
}

type KiroLoginStatusProps = {
  statusText: string;
  errorMessage: string;
};

function KiroLoginStatus({ statusText, errorMessage }: KiroLoginStatusProps) {
  return (
    <>
      {statusText ? <p className="text-xs text-muted-foreground">{statusText}</p> : null}
      {errorMessage ? <p className="text-xs text-destructive">{errorMessage}</p> : null}
    </>
  );
}

export function KiroAccountFields({
  accountId,
  accounts,
  loading,
  error,
  onRefresh,
  onLogout,
  onSelect,
  onImport,
}: KiroAccountFieldsProps) {
  const { login, beginLogin } = useKiroLogin(onRefresh, onSelect);
  const handleImport = useCallback(async () => {
    try {
      const accounts = await onImport();
      if (accountId.trim() === "" && accounts.length > 0) {
        onSelect(accounts[0].account_id);
      }
    } catch {
      // Errors are surfaced via the shared error state in useKiroAccounts.
    }
  }, [accountId, onImport, onSelect]);
  const accountOptions = useMemo(
    () =>
      accounts.map((account) => ({
        value: account.account_id,
        label: formatAccountLabel(account),
      })),
    [accounts],
  );
  const loginStatusText = getLoginStatusText(login);
  const showDeviceInfo = shouldShowDeviceInfo(login);

  return (
    <div data-slot="kiro-account-fields" className="contents">
      <Label>{m.field_kiro_account()}</Label>
      <div className="grid gap-3">
        <KiroAccountSelect
          accountId={accountId}
          options={accountOptions}
          onSelect={onSelect}
        />
        <KiroLoginActions
          accountId={accountId}
          loading={loading}
          polling={login.status === "polling"}
          onLogin={beginLogin}
          onImport={handleImport}
          onRefresh={onRefresh}
          onLogout={onLogout}
        />
        <KiroLoginHints loginInfo={login.start} showDeviceInfo={showDeviceInfo} />
        <KiroLoginStatus statusText={loginStatusText} errorMessage={error} />
      </div>
    </div>
  );
}
