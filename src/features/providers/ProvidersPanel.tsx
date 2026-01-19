import { useCallback, useMemo, useState } from "react";

import { AlertCircle, ChevronDown, RefreshCw, Search } from "lucide-react";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { KiroAccountSummary, KiroLoginMethod, KiroQuotaItem } from "@/features/kiro/types";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroLogin } from "@/features/kiro/use-kiro-login";
import { useKiroQuotas } from "@/features/kiro/use-kiro-quotas";
import { m } from "@/paraglide/messages.js";

const LOGIN_METHODS: ReadonlyArray<{ method: KiroLoginMethod; label: () => string }> = [
  { method: "aws", label: () => m.kiro_login_method_aws() },
  { method: "aws_authcode", label: () => m.kiro_login_method_aws_authcode() },
  { method: "google", label: () => m.kiro_login_method_google() },
] as const;

const PROVIDER_FILTER_ALL = "all";
const STATUS_FILTER_ALL = "all";

type ProviderFilterValue = typeof PROVIDER_FILTER_ALL | "kiro";

type StatusFilterValue = typeof STATUS_FILTER_ALL | "active" | "expired";

type ProviderStatusSummary = {
  active: number;
  expired: number;
};

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

function matchesSearch(account: KiroAccountSummary, keyword: string) {
  if (!keyword) {
    return true;
  }
  const haystack = `${account.email ?? ""} ${account.account_id}`.toLowerCase();
  return haystack.includes(keyword);
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
  onLogin: (method: KiroLoginMethod) => void;
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

function ProvidersToolbar({
  search,
  providerFilter,
  statusFilter,
  onSearchChange,
  onProviderFilterChange,
  onStatusFilterChange,
  onRefresh,
  refreshing,
}: {
  search: string;
  providerFilter: ProviderFilterValue;
  statusFilter: StatusFilterValue;
  onSearchChange: (value: string) => void;
  onProviderFilterChange: (value: ProviderFilterValue) => void;
  onStatusFilterChange: (value: StatusFilterValue) => void;
  onRefresh: () => void;
  refreshing: boolean;
}) {
  return (
    <div
      data-slot="providers-toolbar"
      className="flex flex-wrap items-center gap-2 rounded-lg border border-border/60 bg-background/70 px-3 py-2"
    >
      <div className="relative flex min-w-[220px] flex-1 items-center">
        <Search className="pointer-events-none absolute left-3 size-4 text-muted-foreground" />
        <Input
          value={search}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder={m.providers_toolbar_search_placeholder()}
          className="h-9 pl-9"
          aria-label={m.providers_toolbar_search_placeholder()}
        />
      </div>
      <Select
        value={providerFilter}
        onValueChange={(value) => onProviderFilterChange(value as ProviderFilterValue)}
      >
        <SelectTrigger size="sm">
          <SelectValue placeholder={m.providers_filter_provider_label()} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={PROVIDER_FILTER_ALL}>
            {m.providers_filter_all_providers()}
          </SelectItem>
          <SelectItem value="kiro">{m.providers_kiro_title()}</SelectItem>
        </SelectContent>
      </Select>
      <Select
        value={statusFilter}
        onValueChange={(value) => onStatusFilterChange(value as StatusFilterValue)}
      >
        <SelectTrigger size="sm">
          <SelectValue placeholder={m.providers_filter_status_label()} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={STATUS_FILTER_ALL}>{m.providers_filter_all_statuses()}</SelectItem>
          <SelectItem value="active">{m.kiro_account_status_active()}</SelectItem>
          <SelectItem value="expired">{m.kiro_account_status_expired()}</SelectItem>
        </SelectContent>
      </Select>
      <Button type="button" variant="outline" size="icon" onClick={onRefresh} disabled={refreshing}>
        <RefreshCw
          className={["size-4", refreshing ? "animate-spin" : ""].filter(Boolean).join(" ")}
          aria-hidden="true"
        />
        <span className="sr-only">{m.common_refresh()}</span>
      </Button>
    </div>
  );
}

function ProvidersSectionHeader({ title, count }: { title: string; count: number }) {
  return (
    <div data-slot="providers-section-header" className="flex flex-wrap items-start gap-2">
      <div className="flex-1">
        <div className="flex items-center gap-2">
          <p className="text-sm font-semibold text-foreground">{title}</p>
          <Badge variant="secondary">{count}</Badge>
        </div>
      </div>
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
  quota: { planType: string | null; quotas: KiroQuotaItem[]; error: string | null } | null;
  loading: boolean;
  quotaLoading: boolean;
  onLogout: (accountId: string) => Promise<void>;
}) {
  const statusLabel = formatAccountStatus(account);
  const expiresAt = formatDate(account.expires_at);
  const statusVariant = account.status === "expired" ? "destructive" : "secondary";

  return (
    <div
      data-slot="provider-account-row"
      className="space-y-3 rounded-lg border border-border/60 bg-background/60 p-4"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm font-medium text-foreground">{formatAccountLabel(account)}</p>
            <Badge variant={statusVariant}>{statusLabel}</Badge>
            {quota?.planType ? <Badge variant="outline">{quota.planType}</Badge> : null}
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
      {quotaLoading && !quota ? (
        <p className="text-xs text-muted-foreground">{m.providers_quota_loading()}</p>
      ) : quota && quota.quotas.length ? (
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

function KiroProviderGroup({
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
  statusText,
  deviceLink,
  deviceCode,
  loginBusy,
}: {
  accounts: KiroAccountSummary[];
  filteredAccounts: KiroAccountSummary[];
  quotaMap: Map<string, { planType: string | null; quotas: KiroQuotaItem[]; error: string | null }>;
  accountsLoading: boolean;
  quotasLoading: boolean;
  accountsError: string;
  quotasError: string;
  onRefresh: () => void;
  onLogout: (accountId: string) => Promise<void>;
  onLogin: (method: KiroLoginMethod) => void;
  onImport: () => void;
  statusText: string;
  deviceLink: string;
  deviceCode: string;
  loginBusy: boolean;
}) {
  const [loginOpen, setLoginOpen] = useState(false);
  const [isOpen, setIsOpen] = useState(accounts.length > 0);
  const [hasToggled, setHasToggled] = useState(false);
  const statusSummary = useMemo(() => buildStatusSummary(accounts), [accounts]);
  // Auto-open when accounts are present until the user toggles manually.
  const open = hasToggled ? isOpen : accounts.length > 0;

  const emptyMessage = accounts.length
    ? m.providers_accounts_empty_filtered()
    : m.providers_accounts_empty();

  return (
    <div data-slot="provider-group" className="rounded-lg border border-border/60 bg-muted/20">
      <Dialog open={loginOpen} onOpenChange={setLoginOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{m.providers_add_account()}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <KiroLoginSection
              loading={loginBusy || accountsLoading}
              onLogin={onLogin}
              onImport={onImport}
              statusText={statusText}
              deviceLink={deviceLink}
              deviceCode={deviceCode}
            />
          </DialogBody>
        </DialogContent>
      </Dialog>
      <details
        className="group"
        open={open}
        onToggle={(event) => {
          setHasToggled(true);
          setIsOpen(event.currentTarget.open);
        }}
      >
        <summary className="flex cursor-pointer list-none items-center justify-between gap-4 rounded-lg px-4 py-3">
          <div className="flex items-center gap-3">
            <ChevronDown
              className="size-4 text-muted-foreground transition-transform group-open:rotate-180"
              aria-hidden="true"
            />
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <p className="text-sm font-medium text-foreground">{m.providers_kiro_title()}</p>
                <Badge variant="outline">{accounts.length}</Badge>
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
              disabled={accountsLoading || quotasLoading}
            >
              <RefreshCw
                className={[
                  "size-4",
                  accountsLoading || quotasLoading ? "animate-spin" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
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
                setLoginOpen(true);
              }}
            >
              {m.providers_add_account()}
            </Button>
          </div>
        </summary>
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
      </details>
    </div>
  );
}

export function ProvidersPanel() {
  const [search, setSearch] = useState("");
  const [providerFilter, setProviderFilter] = useState<ProviderFilterValue>(PROVIDER_FILTER_ALL);
  const [statusFilter, setStatusFilter] = useState<StatusFilterValue>(STATUS_FILTER_ALL);

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

  const searchKeyword = search.trim().toLowerCase();
  const filteredAccounts = useMemo(() => {
    return accounts.filter((account) => {
      if (statusFilter !== STATUS_FILTER_ALL && account.status !== statusFilter) {
        return false;
      }
      return matchesSearch(account, searchKeyword);
    });
  }, [accounts, searchKeyword, statusFilter]);

  const showKiro = providerFilter === PROVIDER_FILTER_ALL || providerFilter === "kiro";
  const visibleAccounts = showKiro ? filteredAccounts : [];

  const refreshBusy = accountsLoading || quotasLoading;

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ProvidersToolbar
        search={search}
        providerFilter={providerFilter}
        statusFilter={statusFilter}
        onSearchChange={setSearch}
        onProviderFilterChange={setProviderFilter}
        onStatusFilterChange={setStatusFilter}
        onRefresh={refreshAll}
        refreshing={refreshBusy}
      />

      <section className="space-y-3">
        <ProvidersSectionHeader title={m.providers_section_accounts_title()} count={visibleAccounts.length} />
        {showKiro ? (
          <KiroProviderGroup
            accounts={accounts}
            filteredAccounts={filteredAccounts}
            quotaMap={quotaMap}
            accountsLoading={accountsLoading}
            quotasLoading={quotasLoading}
            accountsError={accountsError}
            quotasError={quotasError}
            onRefresh={refreshAll}
            onLogout={logoutAccount}
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
            loginBusy={loginBusy}
          />
        ) : (
          <p className="text-sm text-muted-foreground">{m.providers_accounts_empty_filtered()}</p>
        )}
      </section>

    </div>
  );
}
