import { useMemo, useState } from "react";

import { AlertCircle, ChevronDown, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { AccountDeleteAction } from "@/features/providers/account-delete-dialog";
import { formatDateLabel } from "@/features/providers/date";
import { useAutoCloseLoginDialog } from "@/features/providers/use-auto-close-login-dialog";
import type { LoginStatus } from "@/features/providers/use-auto-close-login-dialog";
import type { KiroAccountSummary, KiroLoginMethod, KiroQuotaItem } from "@/features/kiro/types";
import { m } from "@/paraglide/messages.js";

const LOGIN_METHODS: ReadonlyArray<{ method: KiroLoginMethod; label: () => string }> = [
  { method: "aws", label: () => m.kiro_login_method_aws() },
  { method: "aws_authcode", label: () => m.kiro_login_method_aws_authcode() },
  { method: "google", label: () => m.kiro_login_method_google() },
] as const;

const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

type ProviderStatusSummary = {
  active: number;
  expired: number;
};

type KiroQuotaView = {
  planType: string | null;
  quotas: KiroQuotaItem[];
  error: string | null;
};

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
  return formatDateLabel(value);
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

function buildStatusSummary(accounts: KiroAccountSummary[]) {
  const summary = accounts.reduce<ProviderStatusSummary>(
    (acc, account) => {
      if (account.status === "expired") {
        acc.expired += 1;
      } else {
        acc.active += 1;
      }
      return acc;
    },
    { active: 0, expired: 0 }
  );

  return m.providers_status_summary({
    active: summary.active,
    expired: summary.expired,
  });
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
  onImportKam,
  statusText,
  deviceLink,
  deviceCode,
}: {
  loading: boolean;
  onLogin: (method: KiroLoginMethod) => void;
  onImport: () => Promise<void>;
  onImportKam: () => Promise<void>;
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
        <Button type="button" variant="outline" size="sm" onClick={onImportKam} disabled={loading}>
          {m.kiro_login_method_import_kam()}
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

type KiroLoginDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  loading: boolean;
  onLogin: (method: KiroLoginMethod) => void;
  onImport: () => Promise<void>;
  onImportKam: () => Promise<void>;
  statusText: string;
  deviceLink: string;
  deviceCode: string;
};

function KiroLoginDialog({
  open,
  onOpenChange,
  loading,
  onLogin,
  onImport,
  onImportKam,
  statusText,
  deviceLink,
  deviceCode,
}: KiroLoginDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{m.providers_add_account()}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <KiroLoginSection
            loading={loading}
            onLogin={onLogin}
            onImport={onImport}
            onImportKam={onImportKam}
            statusText={statusText}
            deviceLink={deviceLink}
            deviceCode={deviceCode}
          />
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

function KiroQuotaSection({ quota, loading }: { quota: KiroQuotaView | null; loading: boolean }) {
  if (quota?.error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="size-4" aria-hidden="true" />
        <div>
          <AlertTitle>{m.providers_quota_failed_title()}</AlertTitle>
          <AlertDescription>{quota.error}</AlertDescription>
        </div>
      </Alert>
    );
  }

  if (loading && !quota) {
    return <p className="text-xs text-muted-foreground">{m.providers_quota_loading()}</p>;
  }

  if (!quota || !quota.quotas.length) {
    return <p className="text-xs text-muted-foreground">{m.providers_quota_empty()}</p>;
  }

  return (
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
              <p className="text-sm font-semibold text-foreground">{Math.round(item.percentage)}%</p>
              <p className="text-xs text-muted-foreground">{formatQuotaReset(item)}</p>
            </div>
          </div>
          <QuotaBar percentage={item.percentage} />
        </div>
      ))}
    </div>
  );
}

function ProviderAccountRow({
  account,
  quota,
  loading,
  onLogout,
  quotaLoading,
}: {
  account: KiroAccountSummary;
  quota: KiroQuotaView | null;
  loading: boolean;
  quotaLoading: boolean;
  onLogout: (accountId: string) => Promise<void>;
}) {
  const accountLabel = formatAccountLabel(account);
  const statusLabel = formatAccountStatus(account);
  const expiresAt = formatDate(account.expires_at);
  const statusVariant = account.status === "expired" ? "destructive" : "secondary";
  const handleLogout = () => {
    void onLogout(account.account_id).catch(() => undefined);
  };

  return (
    <div
      data-slot="provider-account-row"
      className="space-y-3 rounded-lg border border-border/60 bg-background/60 p-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{accountLabel}</p>
            <Badge variant={statusVariant}>{statusLabel}</Badge>
            {quota?.planType ? <Badge variant="outline">{quota.planType}</Badge> : null}
          </div>
          <p className="text-xs text-muted-foreground">
            {m.providers_account_id({ id: account.account_id })}
            {expiresAt ? ` · ${m.providers_account_expires({ date: expiresAt })}` : ""}
          </p>
        </div>
        <AccountDeleteAction
          accountLabel={accountLabel}
          buttonLabel={m.kiro_account_logout()}
          disabled={loading}
          onConfirm={handleLogout}
        />
      </div>
      <KiroQuotaSection quota={quota} loading={quotaLoading} />
    </div>
  );
}

export type KiroProviderGroupProps = {
  accounts: KiroAccountSummary[];
  filteredAccounts: KiroAccountSummary[];
  quotaMap: Map<string, KiroQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  onRefresh: () => void;
  onLogout: (accountId: string) => Promise<void>;
  onLogin: (method: KiroLoginMethod) => void;
  onImport: () => Promise<void>;
  onImportKam: () => Promise<void>;
  statusText: string;
  deviceLink: string;
  deviceCode: string;
  loginBusy: boolean;
  loginStatus: LoginStatus;
};

type KiroProviderHeaderProps = {
  accountsCount: number;
  statusSummary: string;
  loading: boolean;
  onRefresh: () => void;
  onAddAccount: () => void;
};

function KiroProviderHeader({
  accountsCount,
  statusSummary,
  loading,
  onRefresh,
  onAddAccount,
}: KiroProviderHeaderProps) {
  return (
    <summary className="flex cursor-pointer list-none items-center justify-between gap-4 rounded-lg px-4 py-3">
      <div className="flex items-center gap-3">
        <ChevronDown
          className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
          aria-hidden="true"
        />
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{m.providers_kiro_title()}</p>
            <Badge variant="outline">{accountsCount}</Badge>
          </div>
          <p className="text-xs text-muted-foreground">{statusSummary}</p>
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="ghost"
          size="icon"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onRefresh();
          }}
          disabled={loading}
        >
          <RefreshCw
            className={["size-4", loading ? "animate-spin" : ""].filter(Boolean).join(" ")}
            aria-hidden="true"
          />
          <span className="sr-only">{m.common_refresh()}</span>
        </Button>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={(event) => {
            event.preventDefault();
            event.stopPropagation();
            onAddAccount();
          }}
        >
          {m.providers_add_account()}
        </Button>
      </div>
    </summary>
  );
}

type KiroProviderBodyProps = {
  filteredAccounts: KiroAccountSummary[];
  quotaMap: Map<string, KiroQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  emptyMessage: string;
  onLogout: (accountId: string) => Promise<void>;
};

function KiroProviderBody({
  filteredAccounts,
  quotaMap,
  accountsLoading,
  quotasLoading,
  accountsError,
  quotasError,
  emptyMessage,
  onLogout,
}: KiroProviderBodyProps) {
  return (
    <div className="border-t border-border/60 px-4 py-4">
      {accountsLoading ? (
        <p className="text-xs text-muted-foreground">{m.providers_accounts_loading()}</p>
      ) : null}
      {accountsError || quotasError ? (
        <Alert variant="destructive" className="mt-3">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.providers_load_failed()}</AlertTitle>
            <AlertDescription>{accountsError || quotasError}</AlertDescription>
          </div>
        </Alert>
      ) : null}
      {accountsLoading ? null : filteredAccounts.length ? (
        <div className="mt-4 space-y-3">
          {filteredAccounts.map((account) => (
            <ProviderAccountRow
              key={account.account_id}
              account={account}
              quota={quotaMap.get(account.account_id) ?? null}
              loading={accountsLoading || quotasLoading}
              quotaLoading={quotasLoading}
              onLogout={onLogout}
            />
          ))}
        </div>
      ) : (
        <p className="mt-3 text-sm text-muted-foreground">{emptyMessage}</p>
      )}
    </div>
  );
}

type KiroProviderDetailsProps = {
  open: boolean;
  onToggle: (open: boolean) => void;
  accountsCount: number;
  statusSummary: string;
  loading: boolean;
  onRefresh: () => void;
  onAddAccount: () => void;
  bodyProps: KiroProviderBodyProps;
};

function KiroProviderDetails({
  open,
  onToggle,
  accountsCount,
  statusSummary,
  loading,
  onRefresh,
  onAddAccount,
  bodyProps,
}: KiroProviderDetailsProps) {
  return (
    <details
      className="group"
      open={open}
      onToggle={(event) => {
        onToggle(event.currentTarget.open);
      }}
    >
      <KiroProviderHeader
        accountsCount={accountsCount}
        statusSummary={statusSummary}
        loading={loading}
        onRefresh={onRefresh}
        onAddAccount={onAddAccount}
      />
      <KiroProviderBody {...bodyProps} />
    </details>
  );
}

export function KiroProviderGroup({
  accounts,
  filteredAccounts,
  quotaMap,
  accountsLoading,
  quotasLoading,
  accountsError,
  quotasError,
  onRefresh,
  onLogout,
  onLogin,
  onImport,
  onImportKam,
  statusText,
  deviceLink,
  deviceCode,
  loginBusy,
  loginStatus,
}: KiroProviderGroupProps) {
  const [loginOpen, setLoginOpen] = useState(false);
  const [isOpen, setIsOpen] = useState(accounts.length > 0);
  const [hasToggled, setHasToggled] = useState(false);
  const statusSummary = useMemo(() => buildStatusSummary(accounts), [accounts]);
  const open = hasToggled ? isOpen : accounts.length > 0;

  // Auto-close the dialog shortly after a successful login.
  useAutoCloseLoginDialog({ open: loginOpen, status: loginStatus, setOpen: setLoginOpen });

  const handleImport = async () => {
    try {
      await onImport();
      toast.success(m.kiro_import_success());
      // Keep the success feedback visible briefly before closing.
      window.setTimeout(() => setLoginOpen(false), 1500);
    } catch {
      // Keep dialog open on failure.
    }
  };

  const handleImportKam = async () => {
    try {
      await onImportKam();
      toast.success(m.kiro_import_kam_success());
      window.setTimeout(() => setLoginOpen(false), 1500);
    } catch {
      // Keep dialog open on failure.
    }
  };

  const emptyMessage = accounts.length ? m.providers_accounts_empty_filtered() : m.providers_accounts_empty();
  const bodyProps: KiroProviderBodyProps = { filteredAccounts, quotaMap, accountsLoading, quotasLoading, accountsError, quotasError, emptyMessage, onLogout };
  const handleToggle = (nextOpen: boolean) => { setHasToggled(true); setIsOpen(nextOpen); };

  return (
    <div data-slot="provider-group" className="rounded-lg border border-border/60 bg-muted/20">
      <KiroLoginDialog
        open={loginOpen}
        onOpenChange={setLoginOpen}
        loading={loginBusy || accountsLoading}
        onLogin={onLogin}
        onImport={handleImport}
        onImportKam={handleImportKam}
        statusText={statusText}
        deviceLink={deviceLink}
        deviceCode={deviceCode}
      />
      <KiroProviderDetails
        open={open}
        onToggle={handleToggle}
        accountsCount={accounts.length}
        statusSummary={statusSummary}
        loading={accountsLoading || quotasLoading}
        onRefresh={onRefresh}
        onAddAccount={() => setLoginOpen(true)}
        bodyProps={bodyProps}
      />
    </div>
  );
}
