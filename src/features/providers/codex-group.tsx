import { useMemo, useState } from "react";

import { AlertCircle, ChevronDown, RefreshCw } from "lucide-react";
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
import { m } from "@/paraglide/messages.js";
import type { CodexAccountSummary, CodexQuotaItem } from "@/features/codex/types";

const NUMBER_FORMATTER = new Intl.NumberFormat();

type CodexLoginSectionProps = {
  loading: boolean;
  onLogin: () => void;
  statusText: string;
  loginUrl: string;
};

function CodexLoginSection({ loading, onLogin, statusText, loginUrl }: CodexLoginSectionProps) {
  return (
    <div data-slot="codex-login-section" className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" size="sm" onClick={onLogin} disabled={loading}>
          {m.codex_login_button()}
        </Button>
      </div>
      {statusText ? (
        <p className="text-xs text-muted-foreground">{statusText}</p>
      ) : null}
      {loginUrl ? (
        <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs">
          <p className="font-medium text-foreground">{m.codex_login_url_title()}</p>
          <p className="mt-2 break-all text-muted-foreground">{loginUrl}</p>
          <p className="mt-2 text-muted-foreground">{m.codex_login_open_hint()}</p>
        </div>
      ) : null}
    </div>
  );
}

type CodexLoginDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  loading: boolean;
  onLogin: () => void;
  statusText: string;
  loginUrl: string;
};

function CodexLoginDialog({
  open,
  onOpenChange,
  loading,
  onLogin,
  statusText,
  loginUrl,
}: CodexLoginDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="codex-login-dialog">
        <DialogHeader>
          <DialogTitle>{m.providers_add_account()}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <CodexLoginSection
            loading={loading}
            onLogin={onLogin}
            statusText={statusText}
            loginUrl={loginUrl}
          />
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

function formatAccountLabel(account: CodexAccountSummary) {
  const email = account.email?.trim();
  return email || account.account_id;
}

function formatAccountStatus(account: CodexAccountSummary) {
  return account.status === "expired"
    ? m.codex_account_status_expired()
    : m.codex_account_status_active();
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

function formatQuotaLabel(item: CodexQuotaItem) {
  if (item.name === "codex-session") {
    return m.codex_quota_session();
  }
  if (item.name === "codex-weekly") {
    return m.codex_quota_weekly();
  }
  return item.name;
}

function formatQuotaReset(resetAt: string | null) {
  if (!resetAt) {
    return "";
  }
  const dateLabel = formatDate(resetAt);
  if (!dateLabel) {
    return resetAt;
  }
  return m.providers_quota_resets({ date: dateLabel });
}

function buildStatusSummary(accounts: CodexAccountSummary[]) {
  const summary = accounts.reduce(
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

type CodexQuotaView = {
  planType: string | null;
  quotas: CodexQuotaItem[];
  error: string | null;
};

function CodexQuotaSection({ quota, loading }: { quota: CodexQuotaView | null; loading: boolean }) {
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
      {quota.quotas.map((item) => {
        const showUsage = item.used !== null || item.limit !== null;
        return (
          <div key={item.name} className="space-y-2">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div>
                <p className="text-sm font-medium text-foreground">{formatQuotaLabel(item)}</p>
                {showUsage ? (
                  <p className="text-xs text-muted-foreground">
                    {m.providers_quota_usage({
                      used: formatQuotaValue(item.used),
                      limit: formatQuotaValue(item.limit),
                    })}
                  </p>
                ) : null}
              </div>
              <div className="text-right">
                <p className="text-sm font-semibold text-foreground">
                  {Math.round(item.percentage)}%
                </p>
                <p className="text-xs text-muted-foreground">
                  {formatQuotaReset(item.reset_at)}
                </p>
              </div>
            </div>
            <QuotaBar percentage={item.percentage} />
          </div>
        );
      })}
    </div>
  );
}

function CodexAccountRow({
  account,
  quota,
  loading,
  quotaLoading,
  onLogout,
}: {
  account: CodexAccountSummary;
  quota: CodexQuotaView | null;
  loading: boolean;
  quotaLoading: boolean;
  onLogout: (accountId: string) => Promise<void>;
}) {
  const accountLabel = formatAccountLabel(account);
  const statusLabel = formatAccountStatus(account);
  const expiresAt = formatDate(account.expires_at ?? null);
  const statusVariant = account.status === "expired" ? "destructive" : "secondary";
  const handleLogout = () => {
    void onLogout(account.account_id).catch(() => undefined);
  };
  return (
    <div
      data-slot="codex-account-row"
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
          buttonLabel={m.codex_account_logout()}
          disabled={loading}
          onConfirm={handleLogout}
        />
      </div>
      <CodexQuotaSection quota={quota} loading={quotaLoading} />
    </div>
  );
}

type CodexProviderHeaderProps = {
  accountsCount: number;
  statusSummary: string;
  loading: boolean;
  onRefresh: () => void;
  onAddAccount: () => void;
};

function CodexProviderHeader({
  accountsCount,
  statusSummary,
  loading,
  onRefresh,
  onAddAccount,
}: CodexProviderHeaderProps) {
  return (
    <summary data-slot="codex-provider-header" className="flex cursor-pointer list-none items-center justify-between gap-4 rounded-lg px-4 py-3">
      <div className="flex items-center gap-3">
        <ChevronDown
          className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
          aria-hidden="true"
        />
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{m.providers_codex_title()}</p>
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

type CodexProviderBodyProps = {
  filteredAccounts: CodexAccountSummary[];
  quotaMap: Map<string, CodexQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  emptyMessage: string;
  onLogout: (accountId: string) => Promise<void>;
  showAccounts: boolean;
};

function CodexProviderBody({
  filteredAccounts,
  quotaMap,
  accountsLoading,
  quotasLoading,
  accountsError,
  quotasError,
  emptyMessage,
  onLogout,
  showAccounts,
}: CodexProviderBodyProps) {
  return (
    <div data-slot="codex-provider-body" className="border-t border-border/60 px-4 py-4">
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
      {showAccounts
        ? accountsLoading
          ? null
          : filteredAccounts.length ? (
            <div className="mt-4 space-y-3">
              {filteredAccounts.map((account) => (
                <CodexAccountRow
                  key={account.account_id}
                  account={account}
                  quota={quotaMap.get(account.account_id) ?? null}
                  loading={accountsLoading}
                  quotaLoading={quotasLoading}
                  onLogout={onLogout}
                />
              ))}
            </div>
          ) : (
            <p className="mt-3 text-sm text-muted-foreground">{emptyMessage}</p>
          )
        : null}
    </div>
  );
}

type CodexProviderDetailsProps = {
  open: boolean;
  onToggle: (open: boolean) => void;
  headerProps: CodexProviderHeaderProps;
  bodyProps: CodexProviderBodyProps;
};

function CodexProviderDetails({ open, onToggle, headerProps, bodyProps }: CodexProviderDetailsProps) {
  return (
    <details
      data-slot="codex-provider-details"
      className="group"
      open={open}
      onToggle={(event) => {
        onToggle(event.currentTarget.open);
      }}
    >
      <CodexProviderHeader {...headerProps} />
      <CodexProviderBody {...bodyProps} />
    </details>
  );
}

export type CodexProviderGroupProps = {
  accounts: CodexAccountSummary[];
  filteredAccounts: CodexAccountSummary[];
  quotaMap: Map<string, CodexQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  onRefresh: () => void;
  onLogout: (accountId: string) => Promise<void>;
  onLogin: () => void;
  statusText: string;
  loginUrl: string;
  loginBusy: boolean;
  loginStatus: LoginStatus;
  showAccounts?: boolean;
};

export function CodexProviderGroup({
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
  statusText,
  loginUrl,
  loginBusy,
  loginStatus,
  showAccounts = true,
}: CodexProviderGroupProps) {
  const [loginOpen, setLoginOpen] = useState(false);
  const [isOpen, setIsOpen] = useState(accounts.length > 0);
  const [hasToggled, setHasToggled] = useState(false);
  const statusSummary = useMemo(() => buildStatusSummary(accounts), [accounts]);
  const open = hasToggled ? isOpen : accounts.length > 0;

  // Auto-close the dialog shortly after a successful login.
  useAutoCloseLoginDialog({ open: loginOpen, status: loginStatus, setOpen: setLoginOpen });

  const emptyMessage = accounts.length
    ? m.providers_accounts_empty_filtered()
    : m.providers_accounts_empty();

  const bodyProps: CodexProviderBodyProps = {
    filteredAccounts,
    quotaMap,
    accountsLoading,
    quotasLoading,
    accountsError,
    quotasError,
    emptyMessage,
    onLogout,
    showAccounts,
  };
  const headerProps: CodexProviderHeaderProps = {
    accountsCount: accounts.length,
    statusSummary,
    loading: accountsLoading || quotasLoading,
    onRefresh,
    onAddAccount: () => setLoginOpen(true),
  };
  const handleToggle = (nextOpen: boolean) => {
    setHasToggled(true);
    setIsOpen(nextOpen);
  };

  return (
    <div data-slot="provider-group" className="rounded-lg border border-border/60 bg-muted/20">
      <CodexLoginDialog
        open={loginOpen}
        onOpenChange={setLoginOpen}
        loading={loginBusy || accountsLoading || quotasLoading}
        onLogin={onLogin}
        statusText={statusText}
        loginUrl={loginUrl}
      />
      <CodexProviderDetails open={open} onToggle={handleToggle} headerProps={headerProps} bodyProps={bodyProps} />
    </div>
  );
}
