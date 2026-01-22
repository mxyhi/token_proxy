import { useCallback, useMemo, useState } from "react";

import { RefreshCw, Search } from "lucide-react";
import { homeDir, join } from "@tauri-apps/api/path";
import { open } from "@tauri-apps/plugin-dialog";
import { Badge } from "@/components/ui/badge";
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
import { KiroProviderGroup } from "@/features/providers/kiro-group";
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

type ProvidersAccountsSectionProps = {
  visibleCount: number;
  showKiro: boolean;
  showCodex: boolean;
  showAntigravity: boolean;
  kiroGroupProps: KiroGroupProps;
  codexGroupProps: CodexGroupProps;
  antigravityGroupProps: AntigravityGroupProps;
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
        <SelectTrigger size="sm">
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
        <SelectTrigger size="sm">
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

function buildQuotaMap(quotas: QuotaEntry[]) {
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

function buildCodexQuotaMap(quotas: CodexQuotaEntry[]) {
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

function buildAntigravityQuotaMap(quotas: AntigravityQuotaEntry[]) {
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

function ProvidersAccountsSection({
  visibleCount,
  showKiro,
  showCodex,
  showAntigravity,
  kiroGroupProps,
  codexGroupProps,
  antigravityGroupProps,
}: ProvidersAccountsSectionProps) {
  return (
    <section className="space-y-3">
      <ProvidersSectionHeader title={m.providers_section_accounts_title()} count={visibleCount} />
      {showAntigravity ? <AntigravityProviderGroup {...antigravityGroupProps} /> : null}
      {showKiro ? <KiroProviderGroup {...kiroGroupProps} /> : null}
      {showCodex ? <CodexProviderGroup {...codexGroupProps} /> : null}
      {!showKiro && !showCodex && !showAntigravity ? (
        <p className="text-sm text-muted-foreground">{m.providers_accounts_empty_filtered()}</p>
      ) : null}
    </section>
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
  const sectionProps: ProvidersAccountsSectionProps = {
    visibleCount,
    showKiro,
    showCodex,
    showAntigravity,
    kiroGroupProps,
    codexGroupProps,
    antigravityGroupProps,
  };

  return { toolbarProps, sectionProps };
}

export function ProvidersPanel() {
  const { toolbarProps, sectionProps } = useProvidersPanelState();

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ProvidersToolbar {...toolbarProps} />
      <ProvidersAccountsSection {...sectionProps} />
    </div>
  );
}
