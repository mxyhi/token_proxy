import { useEffect, useMemo, useState } from "react";

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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { AccountDeleteAction } from "@/features/providers/account-delete-dialog";
import { formatDateLabel } from "@/features/providers/date";
import { useAutoCloseLoginDialog } from "@/features/providers/use-auto-close-login-dialog";
import type { LoginStatus } from "@/features/providers/use-auto-close-login-dialog";
import type {
  AntigravityAccountSummary,
  AntigravityIdeStatus,
  AntigravityQuotaItem,
  AntigravityWarmupScheduleSummary,
} from "@/features/antigravity/types";
import { m } from "@/paraglide/messages.js";

const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

type AntigravityQuotaView = {
  planType: string | null;
  quotas: AntigravityQuotaItem[];
  error: string | null;
};

type AntigravityLoginSectionProps = {
  loading: boolean;
  onLogin: () => void;
  onImport: () => Promise<void>;
  statusText: string;
  loginUrl: string;
};

function AntigravityLoginSection({
  loading,
  onLogin,
  onImport,
  statusText,
  loginUrl,
}: AntigravityLoginSectionProps) {
  return (
    <div data-slot="antigravity-login-section" className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="secondary" size="sm" onClick={onLogin} disabled={loading}>
          {m.antigravity_login_button()}
        </Button>
        <Button type="button" variant="outline" size="sm" onClick={onImport} disabled={loading}>
          {m.antigravity_import_ide_button()}
        </Button>
      </div>
      {statusText ? (
        <p className="text-xs text-muted-foreground">{statusText}</p>
      ) : null}
      {loginUrl ? (
        <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs">
          <p className="font-medium text-foreground">{m.antigravity_login_url_title()}</p>
          <p className="mt-2 break-all text-muted-foreground">{loginUrl}</p>
          <p className="mt-2 text-muted-foreground">{m.antigravity_login_open_hint()}</p>
        </div>
      ) : null}
    </div>
  );
}

type AntigravityLoginDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  loading: boolean;
  onLogin: () => void;
  onImport: () => Promise<void>;
  statusText: string;
  loginUrl: string;
};

function AntigravityLoginDialog({
  open,
  onOpenChange,
  loading,
  onLogin,
  onImport,
  statusText,
  loginUrl,
}: AntigravityLoginDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="antigravity-login-dialog">
        <DialogHeader>
          <DialogTitle>{m.providers_add_account()}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <AntigravityLoginSection
            loading={loading}
            onLogin={onLogin}
            onImport={onImport}
            statusText={statusText}
            loginUrl={loginUrl}
          />
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

function formatAccountLabel(account: AntigravityAccountSummary) {
  const email = account.email?.trim();
  return email || account.account_id;
}

function formatAccountStatus(account: AntigravityAccountSummary) {
  return account.status === "expired"
    ? m.antigravity_account_status_expired()
    : m.antigravity_account_status_active();
}

function formatAccountSource(account: AntigravityAccountSummary) {
  if (account.source === "ide") {
    return m.antigravity_account_source_ide();
  }
  if (account.source === "oauth") {
    return m.antigravity_account_source_oauth();
  }
  return account.source ?? "";
}

function formatDate(value: string | null | undefined) {
  return formatDateLabel(value ?? null);
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

function formatQuotaPercentage(value: number) {
  if (!Number.isFinite(value)) {
    return "0%";
  }
  return `${NUMBER_FORMATTER.format(value)}%`;
}

function buildStatusSummary(accounts: AntigravityAccountSummary[]) {
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

function AntigravityQuotaSection({
  quota,
  loading,
}: {
  quota: AntigravityQuotaView | null;
  loading: boolean;
}) {
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
              <p className="text-xs text-muted-foreground">{formatQuotaReset(item.reset_at)}</p>
            </div>
            <div className="text-right">
              <p className="text-sm font-semibold text-foreground">
                {formatQuotaPercentage(item.percentage)}
              </p>
            </div>
          </div>
          <QuotaBar percentage={item.percentage} />
        </div>
      ))}
    </div>
  );
}

function AntigravityAccountRow({
  account,
  quota,
  loading,
  quotaLoading,
  canSwitchIde,
  onSwitchIde,
  onLogout,
}: {
  account: AntigravityAccountSummary;
  quota: AntigravityQuotaView | null;
  loading: boolean;
  quotaLoading: boolean;
  canSwitchIde: boolean;
  onSwitchIde: (accountId: string) => Promise<AntigravityIdeStatus>;
  onLogout: (accountId: string) => Promise<void>;
}) {
  const accountLabel = formatAccountLabel(account);
  const statusLabel = formatAccountStatus(account);
  const sourceLabel = formatAccountSource(account);
  const expiresAt = formatDate(account.expires_at ?? null);
  const statusVariant = account.status === "expired" ? "destructive" : "secondary";

  const handleSwitch = () => {
    void onSwitchIde(account.account_id).catch(() => undefined);
  };
  const handleLogout = () => {
    void onLogout(account.account_id).catch(() => undefined);
  };

  return (
    <div
      data-slot="antigravity-account-row"
      className="space-y-3 rounded-lg border border-border/60 bg-background/60 p-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{accountLabel}</p>
            <Badge variant={statusVariant}>{statusLabel}</Badge>
            {quota?.planType ? <Badge variant="outline">{quota.planType}</Badge> : null}
            {sourceLabel ? <Badge variant="secondary">{sourceLabel}</Badge> : null}
          </div>
          <p className="text-xs text-muted-foreground">
            {m.providers_account_id({ id: account.account_id })}
            {expiresAt ? ` · ${m.providers_account_expires({ date: expiresAt })}` : ""}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={handleSwitch}
            disabled={!canSwitchIde || loading}
          >
            {m.antigravity_switch_ide_button()}
          </Button>
          <AccountDeleteAction
            accountLabel={accountLabel}
            buttonLabel={m.antigravity_account_logout()}
            disabled={loading}
            onConfirm={handleLogout}
          />
        </div>
      </div>
      <AntigravityQuotaSection quota={quota} loading={quotaLoading} />
    </div>
  );
}

type AntigravityIdePanelProps = {
  status: AntigravityIdeStatus | null;
  loading: boolean;
  error: string;
  onRefresh: () => void;
};

function AntigravityIdePanel({ status, loading, error, onRefresh }: AntigravityIdePanelProps) {
  return (
    <div data-slot="antigravity-ide-panel" className="rounded-lg border border-border/60 bg-muted/20 p-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <p className="text-sm font-medium text-foreground">{m.antigravity_ide_status_title()}</p>
          <p className="text-xs text-muted-foreground">{m.antigravity_ide_status_desc()}</p>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          onClick={onRefresh}
          disabled={loading}
        >
          <RefreshCw
            className={["size-4", loading ? "animate-spin" : ""].filter(Boolean).join(" ")}
            aria-hidden="true"
          />
          <span className="sr-only">{m.common_refresh()}</span>
        </Button>
      </div>
      {error ? (
        <p className="mt-2 text-xs text-destructive">{error}</p>
      ) : (
        <div className="mt-3 grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
          <div className="flex items-center justify-between gap-2">
            <span>{m.antigravity_ide_db_label()}</span>
            <Badge variant={status?.database_available ? "secondary" : "outline"}>
              {status?.database_available
                ? m.antigravity_ide_db_available()
                : m.antigravity_ide_db_missing()}
            </Badge>
          </div>
          <div className="flex items-center justify-between gap-2">
            <span>{m.antigravity_ide_running_label()}</span>
            <Badge variant={status?.ide_running ? "secondary" : "outline"}>
              {status?.ide_running ? m.antigravity_ide_running() : m.antigravity_ide_not_running()}
            </Badge>
          </div>
          <div className="sm:col-span-2">
            <span>{m.antigravity_ide_active_label()}</span>
            <span className="ml-2 text-foreground">
              {status?.active_email?.trim()
                ? status.active_email
                : m.antigravity_ide_active_empty()}
            </span>
          </div>
        </div>
      )}
    </div>
  );
}

type WarmupPanelProps = {
  accounts: AntigravityAccountSummary[];
  quotaMap: Map<string, AntigravityQuotaView>;
  schedules: AntigravityWarmupScheduleSummary[];
  loading: boolean;
  quotasLoading: boolean;
  running: boolean;
  error: string;
  onRunWarmup: (accountId: string, model: string, stream: boolean) => Promise<void>;
  onRefreshQuotas: () => Promise<void>;
  onSetSchedule: (
    accountId: string,
    model: string,
    intervalMinutes: number,
    enabled: boolean
  ) => Promise<AntigravityWarmupScheduleSummary>;
  onToggleSchedule: (accountId: string, model: string, enabled: boolean) => Promise<void>;
};

function WarmupPanel({
  accounts,
  quotaMap,
  schedules,
  loading,
  quotasLoading,
  running,
  error,
  onRunWarmup,
  onRefreshQuotas,
  onSetSchedule,
  onToggleSchedule,
}: WarmupPanelProps) {
  const defaultAccount = accounts[0]?.account_id ?? "";
  const [accountId, setAccountId] = useState(defaultAccount);
  useEffect(() => {
    if (!accountId && defaultAccount) {
      setAccountId(defaultAccount);
    }
  }, [accountId, defaultAccount]);
  const selectedAccountId = accountId || defaultAccount;
  const modelOptions = useMemo(() => {
    if (!selectedAccountId) return [];
    const quota = quotaMap.get(selectedAccountId);
    return quota?.quotas.map((item) => item.name) ?? [];
  }, [selectedAccountId, quotaMap]);
  const defaultModel = modelOptions[0] ?? "";
  const [model, setModel] = useState(defaultModel);
  useEffect(() => {
    if (!model && defaultModel) {
      setModel(defaultModel);
    }
  }, [model, defaultModel]);
  useEffect(() => {
    if (!model || modelOptions.length === 0) {
      return;
    }
    if (!modelOptions.includes(model)) {
      setModel(modelOptions[0] ?? "");
    }
  }, [model, modelOptions]);
  const [intervalMinutes, setIntervalMinutes] = useState("60");
  const [enabled, setEnabled] = useState(true);
  const selectedModel = model || defaultModel;
  const canSubmit = Boolean(selectedAccountId && selectedModel);
  const showModelHint = Boolean(selectedAccountId) && modelOptions.length === 0 && !quotasLoading;

  const handleAccountChange = (nextAccountId: string) => {
    setAccountId(nextAccountId);
    setModel("");
    void onRefreshQuotas();
  };

  const handleRun = async () => {
    if (!selectedAccountId || !selectedModel) {
      toast.error(m.antigravity_warmup_missing());
      return;
    }
    await onRunWarmup(selectedAccountId, selectedModel, false);
    toast.success(m.antigravity_warmup_success());
  };

  const handleSave = async () => {
    if (!selectedAccountId || !selectedModel) {
      toast.error(m.antigravity_warmup_missing());
      return;
    }
    const interval = Number.parseInt(intervalMinutes, 10);
    const safeInterval = Number.isFinite(interval) ? Math.max(1, interval) : 60;
    await onSetSchedule(selectedAccountId, selectedModel, safeInterval, enabled);
    toast.success(m.antigravity_warmup_schedule_saved());
  };

  return (
    <div data-slot="antigravity-warmup-panel" className="rounded-lg border border-border/60 bg-muted/20 p-4">
      <div className="space-y-2">
        <p className="text-sm font-medium text-foreground">{m.antigravity_warmup_title()}</p>
        <p className="text-xs text-muted-foreground">{m.antigravity_warmup_desc()}</p>
      </div>
      <div className="mt-3 grid gap-3 md:grid-cols-2">
        <div className="grid gap-2">
          <Label>{m.antigravity_warmup_account_label()}</Label>
          <Select value={selectedAccountId || undefined} onValueChange={handleAccountChange}>
            <SelectTrigger>
              <SelectValue placeholder={m.antigravity_warmup_account_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((account) => (
                <SelectItem key={account.account_id} value={account.account_id}>
                  {formatAccountLabel(account)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-2">
          <Label>{m.antigravity_warmup_model_label()}</Label>
          <Select
            value={selectedModel || undefined}
            onValueChange={setModel}
            disabled={quotasLoading || modelOptions.length === 0}
          >
            <SelectTrigger>
              <SelectValue placeholder={m.antigravity_warmup_model_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {modelOptions.map((option) => (
              <SelectItem key={option} value={option}>
                {option}
              </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {showModelHint ? (
            <p className="text-xs text-muted-foreground">{m.antigravity_warmup_model_hint()}</p>
          ) : null}
        </div>
        <div className="grid gap-2">
          <Label>{m.antigravity_warmup_interval_label()}</Label>
          <Input
            value={intervalMinutes}
            onChange={(event) => setIntervalMinutes(event.target.value)}
            placeholder="60"
            inputMode="numeric"
          />
        </div>
        <div className="flex items-center justify-between gap-2 rounded-md border border-border/60 bg-background/60 px-3">
          <Label>{m.antigravity_warmup_schedule_label()}</Label>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>
      </div>
      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => void handleRun()}
          disabled={loading || running || quotasLoading || !canSubmit}
        >
          {m.antigravity_warmup_run()}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void handleSave()}
          disabled={loading || running || quotasLoading || !canSubmit}
        >
          {m.antigravity_warmup_save_schedule()}
        </Button>
      </div>
      {error ? <p className="mt-2 text-xs text-destructive">{error}</p> : null}
      {schedules.length ? (
        <div className="mt-4 space-y-2 text-xs text-muted-foreground">
          <p className="font-medium text-foreground">{m.antigravity_warmup_schedules_title()}</p>
          {schedules.map((schedule) => (
            <div
              key={`${schedule.account_id}:${schedule.model}`}
              className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-border/60 bg-background/60 px-3 py-2"
            >
              <div className="space-y-1">
                <p className="text-sm text-foreground">
                  {schedule.model} · {schedule.account_id}
                </p>
                <p>
                  {m.antigravity_warmup_interval_label()} {schedule.interval_minutes}m
                  {schedule.next_run_at
                    ? ` · ${m.antigravity_warmup_next_run({ date: schedule.next_run_at })}`
                    : ""}
                </p>
              </div>
              <Switch
                checked={schedule.enabled}
                onCheckedChange={(next) =>
                  void onToggleSchedule(schedule.account_id, schedule.model, next)
                }
              />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

type AntigravityProviderHeaderProps = {
  accountsCount: number;
  statusSummary: string;
  loading: boolean;
  onRefresh: () => void;
  onAddAccount: () => void;
};

function AntigravityProviderHeader({
  accountsCount,
  statusSummary,
  loading,
  onRefresh,
  onAddAccount,
}: AntigravityProviderHeaderProps) {
  return (
    <summary data-slot="antigravity-provider-header" className="flex cursor-pointer list-none items-center justify-between gap-4 rounded-lg px-4 py-3">
      <div className="flex items-center gap-3">
        <ChevronDown
          className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
          aria-hidden="true"
        />
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{m.providers_antigravity_title()}</p>
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

type AntigravityProviderBodyProps = {
  filteredAccounts: AntigravityAccountSummary[];
  quotaMap: Map<string, AntigravityQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  emptyMessage: string;
  ideStatus: AntigravityIdeStatus | null;
  ideLoading: boolean;
  ideError: string;
  onRefreshIde: () => void;
  canSwitchIde: boolean;
  onSwitchIdeAccount: (accountId: string) => Promise<AntigravityIdeStatus>;
  onLogout: (accountId: string) => Promise<void>;
  warmupProps: WarmupPanelProps;
};

function AntigravityProviderBody({
  filteredAccounts,
  quotaMap,
  accountsLoading,
  quotasLoading,
  accountsError,
  quotasError,
  emptyMessage,
  ideStatus,
  ideLoading,
  ideError,
  onRefreshIde,
  canSwitchIde,
  onSwitchIdeAccount,
  onLogout,
  warmupProps,
}: AntigravityProviderBodyProps) {
  return (
    <div data-slot="antigravity-provider-body" className="border-t border-border/60 px-4 py-4 space-y-4">
      {accountsLoading ? (
        <p className="text-xs text-muted-foreground">{m.providers_accounts_loading()}</p>
      ) : null}
      {accountsError || quotasError ? (
        <Alert variant="destructive">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.providers_load_failed()}</AlertTitle>
            <AlertDescription>{accountsError || quotasError}</AlertDescription>
          </div>
        </Alert>
      ) : null}

      <AntigravityIdePanel status={ideStatus} loading={ideLoading} error={ideError} onRefresh={onRefreshIde} />

      <WarmupPanel {...warmupProps} />

      {accountsLoading ? null : filteredAccounts.length ? (
        <div className="space-y-3">
          {filteredAccounts.map((account) => (
            <AntigravityAccountRow
              key={account.account_id}
              account={account}
              quota={quotaMap.get(account.account_id) ?? null}
              loading={accountsLoading}
              quotaLoading={quotasLoading}
              canSwitchIde={canSwitchIde}
              onSwitchIde={onSwitchIdeAccount}
              onLogout={onLogout}
            />
          ))}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">{emptyMessage}</p>
      )}
    </div>
  );
}

type AntigravityProviderDetailsProps = {
  open: boolean;
  onToggle: (open: boolean) => void;
  headerProps: AntigravityProviderHeaderProps;
  bodyProps: AntigravityProviderBodyProps;
};

function AntigravityProviderDetails({
  open,
  onToggle,
  headerProps,
  bodyProps,
}: AntigravityProviderDetailsProps) {
  return (
    <details
      data-slot="antigravity-provider-details"
      className="group"
      open={open}
      onToggle={(event) => {
        onToggle(event.currentTarget.open);
      }}
    >
      <AntigravityProviderHeader {...headerProps} />
      <AntigravityProviderBody {...bodyProps} />
    </details>
  );
}

export type AntigravityProviderGroupProps = {
  accounts: AntigravityAccountSummary[];
  filteredAccounts: AntigravityAccountSummary[];
  quotaMap: Map<string, AntigravityQuotaView>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  ideStatus: AntigravityIdeStatus | null;
  ideLoading: boolean;
  ideError: string;
  onRefreshIde: () => void;
  warmupProps: WarmupPanelProps;
  onRefresh: () => void;
  onLogout: (accountId: string) => Promise<void>;
  onLogin: () => void;
  onImport: () => Promise<void>;
  statusText: string;
  loginUrl: string;
  loginBusy: boolean;
  loginStatus: LoginStatus;
  onSwitchIdeAccount: (accountId: string) => Promise<AntigravityIdeStatus>;
};

export function AntigravityProviderGroup({
  accounts,
  filteredAccounts,
  quotaMap,
  accountsLoading,
  quotasLoading,
  accountsError,
  quotasError,
  ideStatus,
  ideLoading,
  ideError,
  onRefreshIde,
  warmupProps,
  onRefresh,
  onLogout,
  onLogin,
  onImport,
  statusText,
  loginUrl,
  loginBusy,
  loginStatus,
  onSwitchIdeAccount,
}: AntigravityProviderGroupProps) {
  const [loginOpen, setLoginOpen] = useState(false);
  const [isOpen, setIsOpen] = useState(accounts.length > 0);
  const [hasToggled, setHasToggled] = useState(false);
  const statusSummary = useMemo(() => buildStatusSummary(accounts), [accounts]);
  const open = hasToggled ? isOpen : accounts.length > 0;

  useAutoCloseLoginDialog({ open: loginOpen, status: loginStatus, setOpen: setLoginOpen });

  const emptyMessage = accounts.length
    ? m.providers_accounts_empty_filtered()
    : m.providers_accounts_empty();
  const canSwitchIde = ideStatus?.database_available ?? false;

  const bodyProps: AntigravityProviderBodyProps = {
    filteredAccounts,
    quotaMap,
    accountsLoading,
    quotasLoading,
    accountsError,
    quotasError,
    emptyMessage,
    ideStatus,
    ideLoading,
    ideError,
    onRefreshIde,
    canSwitchIde,
    onSwitchIdeAccount,
    onLogout,
    warmupProps,
  };
  const headerProps: AntigravityProviderHeaderProps = {
    accountsCount: accounts.length,
    statusSummary,
    loading: accountsLoading || quotasLoading || ideLoading || warmupProps.loading,
    onRefresh,
    onAddAccount: () => setLoginOpen(true),
  };
  const handleToggle = (nextOpen: boolean) => {
    setHasToggled(true);
    setIsOpen(nextOpen);
  };

  return (
    <div data-slot="provider-group" className="rounded-lg border border-border/60 bg-muted/20">
      <AntigravityLoginDialog
        open={loginOpen}
        onOpenChange={setLoginOpen}
        loading={loginBusy || accountsLoading || quotasLoading}
        onLogin={onLogin}
        onImport={onImport}
        statusText={statusText}
        loginUrl={loginUrl}
      />
      <AntigravityProviderDetails
        open={open}
        onToggle={handleToggle}
        headerProps={headerProps}
        bodyProps={bodyProps}
      />
    </div>
  );
}
