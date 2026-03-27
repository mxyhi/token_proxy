import { useCallback, useMemo, useState } from "react";

import { RefreshCw, Search } from "lucide-react";
import { homeDir, join } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useAntigravityAccounts } from "@/features/antigravity/use-antigravity-accounts";
import { useAntigravityIde } from "@/features/antigravity/use-antigravity-ide";
import { useAntigravityLogin } from "@/features/antigravity/use-antigravity-login";
import { useAntigravityQuotas } from "@/features/antigravity/use-antigravity-quotas";
import { useAntigravityWarmup } from "@/features/antigravity/use-antigravity-warmup";
import type { AntigravityIdeStatus } from "@/features/antigravity/types";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useCodexLogin } from "@/features/codex/use-codex-login";
import { useCodexQuotas } from "@/features/codex/use-codex-quotas";
import { AntigravityProviderGroup } from "@/features/providers/antigravity-group";
import { CodexProviderGroup } from "@/features/providers/codex-group";
import { formatDateLabel } from "@/features/providers/date";
import { KiroProviderGroup } from "@/features/providers/kiro-group";
import {
  ProvidersAccountsTableSection,
  type ProviderAccountQuotaDetailItem,
  type ProviderAccountTableRow,
} from "@/features/providers/providers-accounts-table";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroLogin } from "@/features/kiro/use-kiro-login";
import { useKiroQuotas } from "@/features/kiro/use-kiro-quotas";
import { m } from "@/paraglide/messages.js";

const PROVIDER_FILTER_ALL = "all";
const STATUS_FILTER_ALL = "all";

type ProviderFilterValue = typeof PROVIDER_FILTER_ALL | "kiro" | "codex" | "antigravity";

type StatusFilterValue = typeof STATUS_FILTER_ALL | "active" | "expired";

type LoginStatusLabels = {
  failed: () => string;
  success: () => string;
  polling: () => string;
  waiting: () => string;
};

type AccountBase = {
  account_id: string;
  email?: string | null;
  status: "active" | "expired";
};

type QuotaEntry = ReturnType<typeof useKiroQuotas>["quotas"][number];

type CodexQuotaEntry = ReturnType<typeof useCodexQuotas>["quotas"][number];

type AntigravityQuotaEntry = ReturnType<typeof useAntigravityQuotas>["quotas"][number];

type KiroQuotaView = {
  planType: string | null;
  quotas: QuotaEntry["quotas"];
  error: string | null;
};

type CodexQuotaView = {
  planType: string | null;
  quotas: CodexQuotaEntry["quotas"];
  error: string | null;
};

type AntigravityQuotaView = {
  planType: string | null;
  quotas: AntigravityQuotaEntry["quotas"];
  error: string | null;
};

type KiroGroupProps = Parameters<typeof KiroProviderGroup>[0];

type CodexGroupProps = Parameters<typeof CodexProviderGroup>[0];

type AntigravityGroupProps = Parameters<typeof AntigravityProviderGroup>[0];

type ProvidersToolbarProps = {
  search: string;
  providerFilter: ProviderFilterValue;
  statusFilter: StatusFilterValue;
  onSearchChange: (value: string) => void;
  onProviderFilterChange: (value: ProviderFilterValue) => void;
  onStatusFilterChange: (value: StatusFilterValue) => void;
  onRefresh: () => void;
  refreshing: boolean;
};

type ProvidersSectionsProps = {
  visibleCount: number;
  rows: ProviderAccountTableRow[];
  loading: boolean;
  error: string;
  showKiro: boolean;
  showCodex: boolean;
  showAntigravity: boolean;
  kiroGroupProps: KiroGroupProps;
  codexGroupProps: CodexGroupProps;
  antigravityGroupProps: AntigravityGroupProps;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onSwitchIde: (row: ProviderAccountTableRow) => Promise<void>;
};

async function resolveKiroIdeDir() {
  try {
    const home = await homeDir();
    return await join(home, ".aws", "sso", "cache");
  } catch {
    return "";
  }
}

async function resolveAntigravityIdeDbPath() {
  try {
    const home = await homeDir();
    return await join(
      home,
      "Library",
      "Application Support",
      "Antigravity",
      "User",
      "globalStorage",
      "state.vscdb"
    );
  } catch {
    return "";
  }
}

function ProvidersSearchInput({
  search,
  onSearchChange,
}: {
  search: string;
  onSearchChange: (value: string) => void;
}) {
  return (
    <div data-slot="providers-search" className="relative flex min-w-[220px] flex-1 items-center">
      <Search className="pointer-events-none absolute left-3 size-4 text-muted-foreground" />
      <Input
        value={search}
        onChange={(event) => onSearchChange(event.target.value)}
        placeholder={m.providers_toolbar_search_placeholder()}
        className="h-9 pl-9"
        aria-label={m.providers_toolbar_search_placeholder()}
      />
    </div>
  );
}

function ProviderFilterSelect({
  value,
  onChange,
}: {
  value: ProviderFilterValue;
  onChange: (value: ProviderFilterValue) => void;
}) {
  return (
    <div data-slot="providers-filter-provider">
      <Select value={value} onValueChange={(nextValue) => onChange(nextValue as ProviderFilterValue)}>
        <SelectTrigger size="sm" aria-label={m.providers_filter_provider_label()}>
          <SelectValue placeholder={m.providers_filter_provider_label()} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={PROVIDER_FILTER_ALL}>{m.providers_filter_all_providers()}</SelectItem>
          <SelectItem value="kiro">{m.providers_kiro_title()}</SelectItem>
          <SelectItem value="codex">{m.providers_codex_title()}</SelectItem>
          <SelectItem value="antigravity">{m.providers_antigravity_title()}</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

function StatusFilterSelect({
  value,
  onChange,
}: {
  value: StatusFilterValue;
  onChange: (value: StatusFilterValue) => void;
}) {
  return (
    <div data-slot="providers-filter-status">
      <Select value={value} onValueChange={(nextValue) => onChange(nextValue as StatusFilterValue)}>
        <SelectTrigger size="sm" aria-label={m.providers_filter_status_label()}>
          <SelectValue placeholder={m.providers_filter_status_label()} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={STATUS_FILTER_ALL}>{m.providers_filter_all_statuses()}</SelectItem>
          <SelectItem value="active">{m.kiro_account_status_active()}</SelectItem>
          <SelectItem value="expired">{m.kiro_account_status_expired()}</SelectItem>
        </SelectContent>
      </Select>
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
}: ProvidersToolbarProps) {
  return (
    <div
      data-slot="providers-toolbar"
      className="flex flex-wrap items-center gap-2 rounded-lg border border-border/60 bg-background/70 px-3 py-2"
    >
      <ProvidersSearchInput search={search} onSearchChange={onSearchChange} />
      <ProviderFilterSelect value={providerFilter} onChange={onProviderFilterChange} />
      <StatusFilterSelect value={statusFilter} onChange={onStatusFilterChange} />
      <Button
        type="button"
        variant="outline"
        size="icon"
        onClick={onRefresh}
        disabled={refreshing}
        data-slot="providers-toolbar-refresh"
        aria-label={m.common_refresh()}
      >
        <RefreshCw
          className={["size-4", refreshing ? "animate-spin" : ""].filter(Boolean).join(" ")}
          aria-hidden="true"
        />
      </Button>
    </div>
  );
}

function matchesAccount(keyword: string, accountId: string, email?: string | null) {
  if (!keyword) {
    return true;
  }
  const haystack = `${email ?? ""} ${accountId}`.toLowerCase();
  return haystack.includes(keyword);
}

function resolveLoginStatusText(
  status: string,
  error: string | null | undefined,
  labels: LoginStatusLabels
) {
  if (status === "error") {
    return error ?? labels.failed();
  }
  if (status === "success") {
    return labels.success();
  }
  if (status === "polling") {
    return labels.polling();
  }
  if (status === "waiting") {
    return labels.waiting();
  }
  return "";
}

function useProviderFilters() {
  const [search, setSearch] = useState("");
  const [providerFilter, setProviderFilter] = useState<ProviderFilterValue>(PROVIDER_FILTER_ALL);
  const [statusFilter, setStatusFilter] = useState<StatusFilterValue>(STATUS_FILTER_ALL);
  const searchKeyword = search.trim().toLowerCase();

  return {
    search,
    providerFilter,
    statusFilter,
    searchKeyword,
    setSearch,
    setProviderFilter,
    setStatusFilter,
  };
}

function useFilteredAccounts<T extends AccountBase>(
  accounts: T[],
  searchKeyword: string,
  statusFilter: StatusFilterValue
) {
  return useMemo(() => {
    return accounts.filter((account) => {
      if (statusFilter !== STATUS_FILTER_ALL && account.status !== statusFilter) {
        return false;
      }
      return matchesAccount(searchKeyword, account.account_id, account.email ?? null);
    });
  }, [accounts, searchKeyword, statusFilter]);
}

function buildQuotaMap(quotas: QuotaEntry[]): Map<string, KiroQuotaView> {
  return new Map(
    quotas.map((item) => [
      item.account_id,
      {
        planType: item.plan_type ?? null,
        quotas: item.quotas,
        error: item.error ?? null,
      },
    ])
  );
}

function buildCodexQuotaMap(quotas: CodexQuotaEntry[]): Map<string, CodexQuotaView> {
  return new Map(
    quotas.map((item) => [
      item.account_id,
      {
        planType: item.plan_type ?? null,
        quotas: item.quotas,
        error: item.error ?? null,
      },
    ])
  );
}

function buildAntigravityQuotaMap(quotas: AntigravityQuotaEntry[]): Map<string, AntigravityQuotaView> {
  return new Map(
    quotas.map((item) => [
      item.account_id,
      {
        planType: item.plan_type ?? null,
        quotas: item.quotas,
        error: item.error ?? null,
      },
    ])
  );
}

function useKiroLoginState(onRefresh: () => Promise<void>) {
  const { login, beginLogin } = useKiroLogin({ onRefresh });
  const statusText = useMemo(
    () =>
      resolveLoginStatusText(login.status, login.error, {
        failed: m.kiro_login_failed,
        success: m.kiro_login_success,
        polling: m.kiro_login_polling,
        waiting: m.kiro_login_waiting,
      }),
    [login]
  );
  const loginBusy = login.status === "polling" || login.status === "waiting";

  return {
    beginLogin,
    statusText,
    loginBusy,
    loginStatus: login.status,
    deviceLink: login.start?.verification_uri_complete ?? "",
    deviceCode: login.start?.user_code ?? "",
  };
}

function useCodexLoginState(onRefresh: () => Promise<void>) {
  const { login, beginLogin } = useCodexLogin({ onRefresh });
  const statusText = useMemo(
    () =>
      resolveLoginStatusText(login.status, login.error, {
        failed: m.codex_login_failed,
        success: m.codex_login_success,
        polling: m.codex_login_polling,
        waiting: m.codex_login_waiting,
      }),
    [login]
  );
  const loginBusy = login.status === "polling" || login.status === "waiting";

  return {
    beginLogin,
    statusText,
    loginBusy,
    loginStatus: login.status,
    loginUrl: login.start?.login_url ?? "",
  };
}

function useAntigravityLoginState(onRefresh: () => Promise<void>) {
  const { login, beginLogin } = useAntigravityLogin({ onRefresh });
  const statusText = useMemo(
    () =>
      resolveLoginStatusText(login.status, login.error, {
        failed: m.antigravity_login_failed,
        success: m.antigravity_login_success,
        polling: m.antigravity_login_polling,
        waiting: m.antigravity_login_waiting,
      }),
    [login]
  );
  const loginBusy = login.status === "polling" || login.status === "waiting";

  return {
    beginLogin,
    statusText,
    loginBusy,
    loginStatus: login.status,
    loginUrl: login.start?.login_url ?? "",
  };
}

function buildToolbarProps(
  filters: ReturnType<typeof useProviderFilters>,
  onRefresh: () => void,
  refreshing: boolean
) {
  return {
    search: filters.search,
    providerFilter: filters.providerFilter,
    statusFilter: filters.statusFilter,
    onSearchChange: filters.setSearch,
    onProviderFilterChange: filters.setProviderFilter,
    onStatusFilterChange: filters.setStatusFilter,
    onRefresh,
    refreshing,
  };
}

function buildKiroGroupProps({
  accountsState,
  quotasState,
  filteredAccounts,
  quotaMap,
  loginState,
  onRefresh,
  onImport,
  onImportKam,
}: {
  accountsState: ReturnType<typeof useKiroAccounts>;
  quotasState: ReturnType<typeof useKiroQuotas>;
  filteredAccounts: KiroGroupProps["filteredAccounts"];
  quotaMap: KiroGroupProps["quotaMap"];
  loginState: ReturnType<typeof useKiroLoginState>;
  onRefresh: () => void;
  onImport: () => Promise<void>;
  onImportKam: () => Promise<void>;
}) {
  return {
    accounts: accountsState.accounts,
    filteredAccounts,
    quotaMap,
    accountsLoading: accountsState.loading,
    quotasLoading: quotasState.loading,
    accountsError: accountsState.error,
    quotasError: quotasState.error,
    onRefresh,
    onLogout: accountsState.logout,
    onLogin: loginState.beginLogin,
    onImport,
    onImportKam,
    statusText: loginState.statusText,
    deviceLink: loginState.deviceLink,
    deviceCode: loginState.deviceCode,
    loginBusy: loginState.loginBusy,
    loginStatus: loginState.loginStatus,
  };
}

function buildCodexGroupProps({
  accountsState,
  quotasState,
  filteredAccounts,
  quotaMap,
  loginState,
  onRefresh,
}: {
  accountsState: ReturnType<typeof useCodexAccounts>;
  quotasState: ReturnType<typeof useCodexQuotas>;
  filteredAccounts: CodexGroupProps["filteredAccounts"];
  quotaMap: CodexGroupProps["quotaMap"];
  loginState: ReturnType<typeof useCodexLoginState>;
  onRefresh: () => void;
}) {
  return {
    accounts: accountsState.accounts,
    filteredAccounts,
    quotaMap,
    accountsLoading: accountsState.loading,
    quotasLoading: quotasState.loading,
    accountsError: accountsState.error,
    quotasError: quotasState.error,
    onRefresh,
    onLogout: accountsState.logout,
    onLogin: loginState.beginLogin,
    statusText: loginState.statusText,
    loginUrl: loginState.loginUrl,
    loginBusy: loginState.loginBusy,
    loginStatus: loginState.loginStatus,
  };
}

function buildAntigravityGroupProps({
  accountsState,
  quotasState,
  ideState,
  warmupState,
  filteredAccounts,
  quotaMap,
  loginState,
  onRefresh,
  onImport,
  onSwitchIdeAccount,
}: {
  accountsState: ReturnType<typeof useAntigravityAccounts>;
  quotasState: ReturnType<typeof useAntigravityQuotas>;
  ideState: ReturnType<typeof useAntigravityIde>;
  warmupState: ReturnType<typeof useAntigravityWarmup>;
  filteredAccounts: AntigravityGroupProps["filteredAccounts"];
  quotaMap: AntigravityGroupProps["quotaMap"];
  loginState: ReturnType<typeof useAntigravityLoginState>;
  onRefresh: () => void;
  onImport: () => Promise<void>;
  onSwitchIdeAccount: (accountId: string) => Promise<AntigravityIdeStatus>;
}) {
  return {
    accounts: accountsState.accounts,
    filteredAccounts,
    quotaMap,
    accountsLoading: accountsState.loading,
    quotasLoading: quotasState.loading,
    accountsError: accountsState.error,
    quotasError: quotasState.error,
    ideStatus: ideState.status,
    ideLoading: ideState.loading,
    ideError: ideState.error,
    onRefreshIde: ideState.refresh,
    warmupProps: {
      accounts: accountsState.accounts,
      quotaMap,
      schedules: warmupState.schedules,
      loading: warmupState.loading,
      quotasLoading: quotasState.loading,
      running: warmupState.running,
      error: warmupState.error,
      onRunWarmup: warmupState.runWarmup,
      onRefreshQuotas: quotasState.refresh,
      onSetSchedule: warmupState.setSchedule,
      onToggleSchedule: warmupState.toggleSchedule,
    },
    onRefresh,
    onLogout: accountsState.logout,
    onLogin: loginState.beginLogin,
    onImport,
    statusText: loginState.statusText,
    loginUrl: loginState.loginUrl,
    loginBusy: loginState.loginBusy,
    loginStatus: loginState.loginStatus,
    onSwitchIdeAccount,
  };
}

const PLACEHOLDER = "—";
const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

function formatNumber(value: number | null) {
  if (value === null || Number.isNaN(value)) {
    return PLACEHOLDER;
  }
  return NUMBER_FORMATTER.format(value);
}

function formatPercentage(value: number) {
  if (!Number.isFinite(value)) {
    return "0%";
  }
  return `${NUMBER_FORMATTER.format(value)}%`;
}

function formatDateValue(value: string | null | undefined) {
  if (!value) {
    return PLACEHOLDER;
  }
  const label = formatDateLabel(value);
  return label || value;
}

function formatDisplayName(account: AccountBase) {
  const email = account.email?.trim();
  return email || account.account_id;
}

function formatStatusVariant(status: AccountBase["status"]) {
  return status === "expired" ? "destructive" : "secondary";
}

function formatKiroStatus(status: AccountBase["status"]) {
  return status === "expired"
    ? m.kiro_account_status_expired()
    : m.kiro_account_status_active();
}

function formatCodexStatus(status: AccountBase["status"]) {
  return status === "expired"
    ? m.codex_account_status_expired()
    : m.codex_account_status_active();
}

function formatAntigravityStatus(status: AccountBase["status"]) {
  return status === "expired"
    ? m.antigravity_account_status_expired()
    : m.antigravity_account_status_active();
}

function formatKiroAuthMethod(method: string | null | undefined) {
  if (method === "aws") {
    return m.kiro_login_method_aws();
  }
  if (method === "aws_authcode") {
    return m.kiro_login_method_aws_authcode();
  }
  if (method === "google") {
    return m.kiro_login_method_google();
  }
  return method?.trim() || PLACEHOLDER;
}

function formatAntigravitySource(source: string | null | undefined) {
  if (source === "ide") {
    return m.antigravity_account_source_ide();
  }
  if (source === "oauth") {
    return m.antigravity_account_source_oauth();
  }
  return source?.trim() || PLACEHOLDER;
}

function summarizeQuota(summary: string, count: number) {
  if (!summary) {
    return PLACEHOLDER;
  }
  if (count > 1) {
    return m.providers_table_quota_items({ count });
  }
  return summary;
}

function joinSummaryParts(parts: Array<string>) {
  return parts.filter(Boolean).join(" · ");
}

function buildKiroQuotaDetails(quota: KiroQuotaView | null) {
  if (quota?.error) {
    return {
      planType: quota.planType ?? PLACEHOLDER,
      quotaSummary: m.providers_quota_failed_title(),
      quotaError: quota.error,
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  if (!quota || quota.quotas.length === 0) {
    return {
      planType: quota?.planType ?? PLACEHOLDER,
      quotaSummary: PLACEHOLDER,
      quotaError: "",
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  const quotaItems = quota.quotas.map((item) => {
    const resetLabel = item.reset_at
      ? item.is_trial
        ? m.providers_quota_expires({ date: formatDateValue(item.reset_at) })
        : m.providers_quota_resets({ date: formatDateValue(item.reset_at) })
      : "";
    return {
      name: item.name,
      summary: m.providers_quota_usage({
        used: formatNumber(item.used),
        limit: formatNumber(item.limit),
      }),
      secondary: joinSummaryParts([formatPercentage(item.percentage), resetLabel]),
    };
  });
  return {
    planType: quota.planType ?? PLACEHOLDER,
    quotaSummary: summarizeQuota(
      `${quotaItems[0]?.name} · ${quotaItems[0]?.summary ?? PLACEHOLDER}`,
      quotaItems.length
    ),
    quotaError: "",
    quotaItems,
  };
}

function buildCodexQuotaDetails(quota: CodexQuotaView | null) {
  if (quota?.error) {
    return {
      planType: quota.planType ?? PLACEHOLDER,
      quotaSummary: m.providers_quota_failed_title(),
      quotaError: quota.error,
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  if (!quota || quota.quotas.length === 0) {
    return {
      planType: quota?.planType ?? PLACEHOLDER,
      quotaSummary: PLACEHOLDER,
      quotaError: "",
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  const quotaItems = quota.quotas.map((item) => {
    const usageLabel =
      item.used !== null || item.limit !== null
        ? m.providers_quota_usage({
            used: formatNumber(item.used),
            limit: formatNumber(item.limit),
          })
        : formatPercentage(item.percentage);
    const resetLabel = item.reset_at
      ? m.providers_quota_resets({ date: formatDateValue(item.reset_at) })
      : "";
    const quotaName =
      item.name === "codex-session"
        ? m.codex_quota_session()
        : item.name === "codex-weekly"
          ? m.codex_quota_weekly()
          : item.name;
    return {
      name: quotaName,
      summary: usageLabel,
      secondary: joinSummaryParts([formatPercentage(item.percentage), resetLabel]),
    };
  });
  return {
    planType: quota.planType ?? PLACEHOLDER,
    quotaSummary: summarizeQuota(
      `${quotaItems[0]?.name} · ${quotaItems[0]?.summary ?? PLACEHOLDER}`,
      quotaItems.length
    ),
    quotaError: "",
    quotaItems,
  };
}

function buildAntigravityQuotaDetails(quota: AntigravityQuotaView | null) {
  if (quota?.error) {
    return {
      planType: quota.planType ?? PLACEHOLDER,
      quotaSummary: m.providers_quota_failed_title(),
      quotaError: quota.error,
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  if (!quota || quota.quotas.length === 0) {
    return {
      planType: quota?.planType ?? PLACEHOLDER,
      quotaSummary: PLACEHOLDER,
      quotaError: "",
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  const quotaItems = quota.quotas.map((item) => ({
    name: item.name,
    summary: formatPercentage(item.percentage),
    secondary: item.reset_at ? m.providers_quota_resets({ date: formatDateValue(item.reset_at) }) : "",
  }));
  return {
    planType: quota.planType ?? PLACEHOLDER,
    quotaSummary: summarizeQuota(
      `${quotaItems[0]?.name} · ${quotaItems[0]?.summary ?? PLACEHOLDER}`,
      quotaItems.length
    ),
    quotaError: "",
    quotaItems,
  };
}

function buildKiroRows(
  accounts: KiroGroupProps["filteredAccounts"],
  quotaMap: KiroGroupProps["quotaMap"]
): ProviderAccountTableRow[] {
  return accounts.map((account) => {
    const quota = buildKiroQuotaDetails(quotaMap.get(account.account_id) ?? null);
    return {
      id: `kiro:${account.account_id}`,
      provider: "kiro",
      providerLabel: m.providers_kiro_title(),
      displayName: formatDisplayName(account),
      accountId: account.account_id,
      statusLabel: formatKiroStatus(account.status),
      statusVariant: formatStatusVariant(account.status),
      expiresAtLabel: formatDateValue(account.expires_at),
      planType: quota.planType,
      quotaSummary: quota.quotaSummary,
      sourceOrMethodLabel: formatKiroAuthMethod(account.auth_method),
      detailDescription: `${m.providers_kiro_title()} · ${account.account_id}`,
      detailFields: [
        { label: m.providers_table_provider(), value: m.providers_kiro_title() },
        { label: m.providers_table_account(), value: formatDisplayName(account) },
        { label: m.providers_table_account_id(), value: account.account_id },
        { label: m.providers_table_status(), value: formatKiroStatus(account.status) },
        { label: m.providers_table_expires(), value: formatDateValue(account.expires_at) },
        { label: m.providers_table_plan(), value: quota.planType },
        { label: m.providers_table_source(), value: formatKiroAuthMethod(account.auth_method) },
      ],
      quotaError: quota.quotaError,
      quotaItems: quota.quotaItems,
      logoutLabel: m.kiro_account_logout(),
      canSwitchIde: false,
    };
  });
}

function buildCodexRows(
  accounts: CodexGroupProps["filteredAccounts"],
  quotaMap: CodexGroupProps["quotaMap"]
): ProviderAccountTableRow[] {
  return accounts.map((account) => {
    const quota = buildCodexQuotaDetails(quotaMap.get(account.account_id) ?? null);
    return {
      id: `codex:${account.account_id}`,
      provider: "codex",
      providerLabel: m.providers_codex_title(),
      displayName: formatDisplayName(account),
      accountId: account.account_id,
      statusLabel: formatCodexStatus(account.status),
      statusVariant: formatStatusVariant(account.status),
      expiresAtLabel: formatDateValue(account.expires_at ?? null),
      planType: quota.planType,
      quotaSummary: quota.quotaSummary,
      sourceOrMethodLabel: PLACEHOLDER,
      detailDescription: `${m.providers_codex_title()} · ${account.account_id}`,
      detailFields: [
        { label: m.providers_table_provider(), value: m.providers_codex_title() },
        { label: m.providers_table_account(), value: formatDisplayName(account) },
        { label: m.providers_table_account_id(), value: account.account_id },
        { label: m.providers_table_status(), value: formatCodexStatus(account.status) },
        { label: m.providers_table_expires(), value: formatDateValue(account.expires_at ?? null) },
        { label: m.providers_table_plan(), value: quota.planType },
      ],
      quotaError: quota.quotaError,
      quotaItems: quota.quotaItems,
      logoutLabel: m.codex_account_logout(),
      canSwitchIde: false,
    };
  });
}

function buildAntigravityRows(
  accounts: AntigravityGroupProps["filteredAccounts"],
  quotaMap: AntigravityGroupProps["quotaMap"],
  canSwitchIde: boolean
): ProviderAccountTableRow[] {
  return accounts.map((account) => {
    const quota = buildAntigravityQuotaDetails(quotaMap.get(account.account_id) ?? null);
    return {
      id: `antigravity:${account.account_id}`,
      provider: "antigravity",
      providerLabel: m.providers_antigravity_title(),
      displayName: formatDisplayName(account),
      accountId: account.account_id,
      statusLabel: formatAntigravityStatus(account.status),
      statusVariant: formatStatusVariant(account.status),
      expiresAtLabel: formatDateValue(account.expires_at ?? null),
      planType: quota.planType,
      quotaSummary: quota.quotaSummary,
      sourceOrMethodLabel: formatAntigravitySource(account.source ?? null),
      detailDescription: `${m.providers_antigravity_title()} · ${account.account_id}`,
      detailFields: [
        { label: m.providers_table_provider(), value: m.providers_antigravity_title() },
        { label: m.providers_table_account(), value: formatDisplayName(account) },
        { label: m.providers_table_account_id(), value: account.account_id },
        { label: m.providers_table_status(), value: formatAntigravityStatus(account.status) },
        { label: m.providers_table_expires(), value: formatDateValue(account.expires_at ?? null) },
        { label: m.providers_table_plan(), value: quota.planType },
        { label: m.providers_table_source(), value: formatAntigravitySource(account.source ?? null) },
      ],
      quotaError: quota.quotaError,
      quotaItems: quota.quotaItems,
      logoutLabel: m.antigravity_account_logout(),
      canSwitchIde,
    };
  });
}

function collectErrorMessages(parts: string[]) {
  return parts.filter(Boolean).join(" · ");
}

function ProvidersSections({
  visibleCount,
  rows,
  loading,
  error,
  showKiro,
  showCodex,
  showAntigravity,
  kiroGroupProps,
  codexGroupProps,
  antigravityGroupProps,
  onLogout,
  onSwitchIde,
}: ProvidersSectionsProps) {
  return (
    <>
      <ProvidersAccountsTableSection
        count={visibleCount}
        rows={rows}
        loading={loading}
        error={error}
        onLogout={onLogout}
        onSwitchIde={onSwitchIde}
      />
      <section className="space-y-3">
        {showKiro ? <KiroProviderGroup {...kiroGroupProps} showAccounts={false} /> : null}
        {showCodex ? <CodexProviderGroup {...codexGroupProps} showAccounts={false} /> : null}
        {showAntigravity ? (
          <AntigravityProviderGroup {...antigravityGroupProps} showAccounts={false} />
        ) : null}
      </section>
    </>
  );
}

function useProvidersPanelState() {
  const filters = useProviderFilters();
  const kiroAccounts = useKiroAccounts();
  const codexAccounts = useCodexAccounts();
  const antigravityAccounts = useAntigravityAccounts();
  const kiroQuotas = useKiroQuotas();
  const codexQuotas = useCodexQuotas();
  const antigravityQuotas = useAntigravityQuotas();
  const antigravityIde = useAntigravityIde({ onRefreshAccounts: antigravityAccounts.refresh });
  const antigravityWarmup = useAntigravityWarmup();
  const refreshAntigravity = useCallback(async () => {
    await antigravityAccounts.refresh();
    await antigravityQuotas.refresh();
    await antigravityIde.refresh();
    await antigravityWarmup.refresh();
  }, [antigravityAccounts, antigravityQuotas, antigravityIde, antigravityWarmup]);
  const refreshAll = useCallback(async () => {
    await kiroAccounts.refresh();
    await kiroQuotas.refresh();
    await codexAccounts.refresh();
    await codexQuotas.refresh();
    await antigravityAccounts.refresh();
    await antigravityQuotas.refresh();
    await antigravityIde.refresh();
    await antigravityWarmup.refresh();
  }, [
    kiroAccounts,
    kiroQuotas,
    codexAccounts,
    codexQuotas,
    antigravityAccounts,
    antigravityQuotas,
    antigravityIde,
    antigravityWarmup,
  ]);
  const kiroLogin = useKiroLoginState(refreshAll);
  const codexLogin = useCodexLoginState(refreshAll);
  const antigravityLogin = useAntigravityLoginState(refreshAll);
  const quotaMap = useMemo(() => buildQuotaMap(kiroQuotas.quotas), [kiroQuotas.quotas]);
  const codexQuotaMap = useMemo(() => buildCodexQuotaMap(codexQuotas.quotas), [codexQuotas.quotas]);
  const antigravityQuotaMap = useMemo(
    () => buildAntigravityQuotaMap(antigravityQuotas.quotas),
    [antigravityQuotas.quotas]
  );
  const filteredAccounts = useFilteredAccounts(kiroAccounts.accounts, filters.searchKeyword, filters.statusFilter);
  const filteredCodexAccounts = useFilteredAccounts(
    codexAccounts.accounts,
    filters.searchKeyword,
    filters.statusFilter
  );
  const filteredAntigravityAccounts = useFilteredAccounts(
    antigravityAccounts.accounts,
    filters.searchKeyword,
    filters.statusFilter
  );

  const showKiro = filters.providerFilter === PROVIDER_FILTER_ALL || filters.providerFilter === "kiro";
  const showCodex = filters.providerFilter === PROVIDER_FILTER_ALL || filters.providerFilter === "codex";
  const showAntigravity = filters.providerFilter === PROVIDER_FILTER_ALL || filters.providerFilter === "antigravity";
  const visibleCount =
    (showKiro ? filteredAccounts.length : 0) +
    (showCodex ? filteredCodexAccounts.length : 0) +
    (showAntigravity ? filteredAntigravityAccounts.length : 0);
  const refreshBusy =
    kiroAccounts.loading ||
    kiroQuotas.loading ||
    codexAccounts.loading ||
    codexQuotas.loading ||
    antigravityAccounts.loading ||
    antigravityQuotas.loading ||
    antigravityIde.loading ||
    antigravityWarmup.loading;

  const handleImport = useCallback(async () => {
    const defaultPath = await resolveKiroIdeDir();
    const selection = await open(
      defaultPath ? { directory: true, defaultPath } : { directory: true }
    );
    const directory = Array.isArray(selection) ? selection[0] : selection;
    if (!directory) {
      throw new Error("Import cancelled.");
    }
    await kiroAccounts.importIde(directory);
    await kiroQuotas.refresh();
  }, [kiroAccounts, kiroQuotas]);

  const handleImportKam = useCallback(async () => {
    const selection = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    const path = Array.isArray(selection) ? selection[0] : selection;
    if (!path) {
      throw new Error("Import cancelled.");
    }
    await kiroAccounts.importKam(path);
    await kiroQuotas.refresh();
  }, [kiroAccounts, kiroQuotas]);

  const handleAntigravityImport = useCallback(async () => {
    const defaultPath = await resolveAntigravityIdeDbPath();
    const selection = await open(
      defaultPath
        ? {
            multiple: false,
            directory: false,
            defaultPath,
            filters: [{ name: "SQLite", extensions: ["vscdb"] }],
          }
        : { multiple: false, directory: false }
    );
    const path = Array.isArray(selection) ? selection[0] : selection;
    if (!path) {
      throw new Error("Import cancelled.");
    }
    await antigravityIde.importIde(path);
    await antigravityQuotas.refresh();
  }, [antigravityIde, antigravityQuotas]);

  const toolbarProps = buildToolbarProps(filters, refreshAll, refreshBusy);
  const kiroGroupProps = buildKiroGroupProps({
    accountsState: kiroAccounts,
    quotasState: kiroQuotas,
    filteredAccounts,
    quotaMap,
    loginState: kiroLogin,
    onRefresh: refreshAll,
    onImport: handleImport,
    onImportKam: handleImportKam,
  });
  const codexGroupProps = buildCodexGroupProps({
    accountsState: codexAccounts,
    filteredAccounts: filteredCodexAccounts,
    quotasState: codexQuotas,
    quotaMap: codexQuotaMap,
    loginState: codexLogin,
    onRefresh: refreshAll,
  });
  const antigravityGroupProps = buildAntigravityGroupProps({
    accountsState: antigravityAccounts,
    quotasState: antigravityQuotas,
    ideState: antigravityIde,
    warmupState: antigravityWarmup,
    filteredAccounts: filteredAntigravityAccounts,
    quotaMap: antigravityQuotaMap,
    loginState: antigravityLogin,
    onRefresh: refreshAntigravity,
    onImport: handleAntigravityImport,
    onSwitchIdeAccount: antigravityIde.switchAccount,
  });
  const rows = useMemo(() => {
    return [
      ...(showKiro ? buildKiroRows(filteredAccounts, quotaMap) : []),
      ...(showCodex ? buildCodexRows(filteredCodexAccounts, codexQuotaMap) : []),
      ...(
        showAntigravity
          ? buildAntigravityRows(
              filteredAntigravityAccounts,
              antigravityQuotaMap,
              antigravityIde.status?.database_available ?? false
            )
          : []
      ),
    ];
  }, [
    showKiro,
    showCodex,
    showAntigravity,
    filteredAccounts,
    filteredCodexAccounts,
    filteredAntigravityAccounts,
    quotaMap,
    codexQuotaMap,
    antigravityQuotaMap,
    antigravityIde.status,
  ]);
  const tableError = collectErrorMessages([
    showKiro ? kiroAccounts.error : "",
    showKiro ? kiroQuotas.error : "",
    showCodex ? codexAccounts.error : "",
    showCodex ? codexQuotas.error : "",
    showAntigravity ? antigravityAccounts.error : "",
    showAntigravity ? antigravityQuotas.error : "",
  ]);
  const handleRowLogout = useCallback(
    async (row: ProviderAccountTableRow) => {
      if (row.provider === "kiro") {
        await kiroAccounts.logout(row.accountId);
        await kiroQuotas.refresh();
        return;
      }
      if (row.provider === "codex") {
        await codexAccounts.logout(row.accountId);
        await codexQuotas.refresh();
        return;
      }
      await antigravityAccounts.logout(row.accountId);
      await antigravityQuotas.refresh();
      await antigravityIde.refresh();
    },
    [
      kiroAccounts,
      kiroQuotas,
      codexAccounts,
      codexQuotas,
      antigravityAccounts,
      antigravityQuotas,
      antigravityIde,
    ]
  );
  const handleRowSwitchIde = useCallback(
    async (row: ProviderAccountTableRow) => {
      if (row.provider !== "antigravity") {
        return;
      }
      await antigravityIde.switchAccount(row.accountId);
    },
    [antigravityIde]
  );
  const sectionProps: ProvidersSectionsProps = {
    visibleCount,
    rows,
    loading: refreshBusy,
    error: tableError,
    showKiro,
    showCodex,
    showAntigravity,
    kiroGroupProps,
    codexGroupProps,
    antigravityGroupProps,
    onLogout: handleRowLogout,
    onSwitchIde: handleRowSwitchIde,
  };

  return { toolbarProps, sectionProps };
}

export function ProvidersPanel() {
  const { toolbarProps, sectionProps } = useProvidersPanelState();

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ProvidersToolbar {...toolbarProps} />
      <ProvidersSections {...sectionProps} />
    </div>
  );
}
