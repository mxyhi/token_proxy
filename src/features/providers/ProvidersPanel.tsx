import { useCallback, useMemo } from "react";

import { AlertCircle, RefreshCw } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import type { KiroAccountSummary, KiroQuotaItem } from "@/features/kiro/types";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroLogin } from "@/features/kiro/use-kiro-login";
import { useKiroQuotas } from "@/features/kiro/use-kiro-quotas";
import { m } from "@/paraglide/messages.js";

const LOGIN_METHODS = [
  { method: "aws", label: () => m.kiro_login_method_aws() },
  { method: "aws_authcode", label: () => m.kiro_login_method_aws_authcode() },
  { method: "google", label: () => m.kiro_login_method_google() },
] as const;

const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

function formatAccountLabel(account: KiroAccountSummary) {
  const email = account.email?.trim();
  if (email) {
    return email;
  }
  return account.account_id;
}

function formatAccountStatus(account: KiroAccountSummary) {
  return account.status === "expired"
    ? m.kiro_account_status_expired()
    : m.kiro_account_status_active();
}

function formatDate(value: string | null) {
  if (!value) {
    return "";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleDateString();
}

function formatQuotaValue(value: number | null) {
  if (value === null || Number.isNaN(value)) {
    return "—";
  }
  return NUMBER_FORMATTER.format(value);
}

function formatQuotaReset(quota: KiroQuotaItem) {
  if (!quota.reset_at) {
    return "";
  }
  const dateLabel = formatDate(quota.reset_at);
  if (!dateLabel) {
    return quota.reset_at;
  }
  return quota.is_trial
    ? m.providers_quota_expires({ date: dateLabel })
    : m.providers_quota_resets({ date: dateLabel });
}

function QuotaBar({ percentage }: { percentage: number }) {
  const clamped = Math.max(0, Math.min(100, percentage));
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
      <div
        className="h-full rounded-full bg-primary transition-[width]"
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

function KiroLoginSection({
  loading,
  onLogin,
  onImport,
  statusText,
  deviceLink,
  deviceCode,
}: {
  loading: boolean;
  onLogin: (method: "aws" | "aws_authcode" | "google") => void;
  onImport: () => void;
  statusText: string;
  deviceLink: string;
  deviceCode: string;
}) {
  return (
    <div data-slot="kiro-login-section" className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        {LOGIN_METHODS.map((item) => (
          <Button
            key={item.method}
            type="button"
            variant="secondary"
            size="sm"
            onClick={() => onLogin(item.method)}
            disabled={loading}
          >
            {item.label()}
          </Button>
        ))}
        <Button type="button" variant="outline" size="sm" onClick={onImport} disabled={loading}>
          {m.kiro_login_method_import()}
        </Button>
      </div>
      {statusText ? (
        <p className="text-xs text-muted-foreground">{statusText}</p>
      ) : null}
      {deviceLink && deviceCode ? (
        <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs">
          <p className="font-medium text-foreground">{m.kiro_device_code_title()}</p>
          <p className="mt-2 break-all text-muted-foreground">{deviceLink}</p>
          <p className="mt-1 font-mono text-sm text-foreground">{deviceCode}</p>
          <p className="mt-2 text-muted-foreground">{m.kiro_login_open_hint()}</p>
        </div>
      ) : null}
    </div>
  );
}

function KiroAccountCard({
  account,
  quota,
  onLogout,
  loading,
}: {
  account: KiroAccountSummary;
  quota: { planType: string | null; quotas: KiroQuotaItem[]; error: string | null } | null;
  onLogout: (accountId: string) => Promise<void>;
  loading: boolean;
}) {
  const statusLabel = formatAccountStatus(account);
  const expiresAt = formatDate(account.expires_at);
  const statusVariant = account.status === "expired" ? "destructive" : "secondary";
  return (
    <div
      data-slot="provider-account-card"
      className="space-y-3 rounded-lg border border-border/60 bg-background/60 p-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{formatAccountLabel(account)}</p>
            <Badge variant={statusVariant}>{statusLabel}</Badge>
            {quota?.planType ? (
              <Badge variant="outline">{quota.planType}</Badge>
            ) : null}
          </div>
          <p className="text-xs text-muted-foreground">
            {m.providers_account_id({ id: account.account_id })}
            {expiresAt ? ` · ${m.providers_account_expires({ date: expiresAt })}` : ""}
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => {
            void onLogout(account.account_id).catch(() => undefined);
          }}
          disabled={loading}
        >
          {m.kiro_account_logout()}
        </Button>
      </div>
      {quota?.error ? (
        <Alert variant="destructive">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.providers_quota_failed_title()}</AlertTitle>
            <AlertDescription>{quota.error}</AlertDescription>
          </div>
        </Alert>
      ) : null}
      {quota && quota.quotas.length ? (
        <div className="space-y-3">
          {quota.quotas.map((item) => (
            <div key={item.name} className="space-y-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <p className="text-sm font-medium text-foreground">{item.name}</p>
                  <p className="text-xs text-muted-foreground">
                    {m.providers_quota_usage({
                      used: formatQuotaValue(item.used),
                      limit: formatQuotaValue(item.limit),
                    })}
                  </p>
                </div>
                <div className="text-right">
                  <p className="text-sm font-semibold text-foreground">
                    {Math.round(item.percentage)}%
                  </p>
                  <p className="text-xs text-muted-foreground">{formatQuotaReset(item)}</p>
                </div>
              </div>
              <QuotaBar percentage={item.percentage} />
            </div>
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">{m.providers_quota_empty()}</p>
      )}
    </div>
  );
}

function KiroProviderCard() {
  const {
    accounts,
    loading: accountsLoading,
    error: accountsError,
    refresh: refreshAccounts,
    logout: logoutAccount,
    importIde,
  } = useKiroAccounts();
  const {
    quotas,
    loading: quotasLoading,
    error: quotasError,
    refresh: refreshQuotas,
  } = useKiroQuotas();

  const refreshAll = useCallback(async () => {
    await refreshAccounts();
    await refreshQuotas();
  }, [refreshAccounts, refreshQuotas]);

  const { login, beginLogin } = useKiroLogin({
    onRefresh: refreshAll,
  });

  const quotaMap = useMemo(() => {
    const map = new Map(
      quotas.map((item) => [
        item.account_id,
        {
          planType: item.plan_type ?? null,
          quotas: item.quotas,
          error: item.error ?? null,
        },
      ])
    );
    return map;
  }, [quotas]);

  const statusText = useMemo(() => {
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
  }, [login]);

  const loginBusy = login.status === "polling" || login.status === "waiting";
  const deviceLink = login.start?.verification_uri_complete ?? "";
  const deviceCode = login.start?.user_code ?? "";

  return (
    <Card data-slot="provider-kiro-card">
      <CardHeader>
        <CardTitle>{m.providers_kiro_title()}</CardTitle>
        <CardDescription>{m.providers_kiro_desc()}</CardDescription>
        <CardAction>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={refreshAll}
            disabled={accountsLoading || quotasLoading}
          >
            <RefreshCw
              className={[
                "mr-2 size-4",
                accountsLoading || quotasLoading ? "animate-spin" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              aria-hidden="true"
            />
            {m.common_refresh()}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="space-y-4">
        {accountsError || quotasError ? (
          <Alert variant="destructive">
            <AlertCircle className="size-4" aria-hidden="true" />
            <div>
              <AlertTitle>{m.providers_load_failed()}</AlertTitle>
              <AlertDescription>{accountsError || quotasError}</AlertDescription>
            </div>
          </Alert>
        ) : null}
        <KiroLoginSection
          loading={loginBusy || accountsLoading}
          onLogin={beginLogin}
          onImport={async () => {
            try {
              await importIde();
              await refreshQuotas();
            } catch {
            }
          }}
          statusText={statusText}
          deviceLink={deviceLink}
          deviceCode={deviceCode}
        />
        <Separator />
        {accountsLoading ? (
          <p className="text-sm text-muted-foreground">{m.providers_accounts_loading()}</p>
        ) : accounts.length ? (
          <div className="space-y-3">
            {accounts.map((account) => (
              <KiroAccountCard
                key={account.account_id}
                account={account}
                quota={quotaMap.get(account.account_id) ?? null}
                loading={accountsLoading || quotasLoading}
                onLogout={logoutAccount}
              />
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">{m.providers_accounts_empty()}</p>
        )}
      </CardContent>
    </Card>
  );
}

export function ProvidersPanel() {
  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <KiroProviderCard />
    </div>
  );
}
