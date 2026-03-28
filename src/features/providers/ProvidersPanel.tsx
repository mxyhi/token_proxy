import { useCallback, useMemo, useState } from "react";

import { RefreshCw, Search } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useCodexQuotas } from "@/features/codex/use-codex-quotas";
import { formatDateLabel } from "@/features/providers/date";
import {
  ProvidersAccountsTableSection,
  type ProviderAccountQuotaDetailItem,
  type ProviderAccountTableRow,
} from "@/features/providers/providers-accounts-table";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroQuotas } from "@/features/kiro/use-kiro-quotas";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

const PROVIDER_FILTER_ALL = "all";
const STATUS_FILTER_ALL = "all";
const PLACEHOLDER = "—";
const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

type ProviderFilterValue = typeof PROVIDER_FILTER_ALL | "kiro" | "codex";
type StatusFilterValue = typeof STATUS_FILTER_ALL | "active" | "expired";

type AccountBase = {
  account_id: string;
  email?: string | null;
  status: "active" | "expired";
};

type KiroAccountEntry = ReturnType<typeof useKiroAccounts>["accounts"][number];
type CodexAccountEntry = ReturnType<typeof useCodexAccounts>["accounts"][number];
type QuotaEntry = ReturnType<typeof useKiroQuotas>["quotas"][number];
type CodexQuotaEntry = ReturnType<typeof useCodexQuotas>["quotas"][number];

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

type ProvidersToolbarProps = {
  search: string;
  providerFilter: ProviderFilterValue;
  statusFilter: StatusFilterValue;
  onSearchChange: (value: string) => void;
  onProviderFilterChange: (value: ProviderFilterValue) => void;
  onStatusFilterChange: (value: StatusFilterValue) => void;
  onRefresh: () => void;
  onImportCodex: () => Promise<void>;
  refreshing: boolean;
  codexImporting: boolean;
};

type ProvidersSectionsProps = {
  visibleCount: number;
  rows: ProviderAccountTableRow[];
  loading: boolean;
  error: string;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
};

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
  onImportCodex,
  refreshing,
  codexImporting,
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
        size="sm"
        onClick={() => { void onImportCodex(); }}
        disabled={refreshing || codexImporting}
        data-slot="providers-toolbar-codex-import"
      >
        {m.codex_import_button()}
      </Button>
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

function useProviderFilters() {
  const [search, setSearch] = useState("");
  const [providerFilter, setProviderFilter] = useState<ProviderFilterValue>(PROVIDER_FILTER_ALL);
  const [statusFilter, setStatusFilter] = useState<StatusFilterValue>(STATUS_FILTER_ALL);

  return {
    search,
    providerFilter,
    statusFilter,
    searchKeyword: search.trim().toLowerCase(),
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

function buildToolbarProps(
  filters: ReturnType<typeof useProviderFilters>,
  onImportCodex: () => Promise<void>,
  onRefresh: () => void,
  refreshing: boolean,
  codexImporting: boolean,
) {
  return {
    search: filters.search,
    providerFilter: filters.providerFilter,
    statusFilter: filters.statusFilter,
    onSearchChange: filters.setSearch,
    onProviderFilterChange: filters.setProviderFilter,
    onStatusFilterChange: filters.setStatusFilter,
    onImportCodex,
    onRefresh,
    refreshing,
    codexImporting,
  };
}

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

function buildKiroRows(
  accounts: KiroAccountEntry[],
  quotaMap: Map<string, KiroQuotaView>
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
    };
  });
}

function buildCodexRows(
  accounts: CodexAccountEntry[],
  quotaMap: Map<string, CodexQuotaView>
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
  onLogout,
}: ProvidersSectionsProps) {
  return (
    <ProvidersAccountsTableSection
      count={visibleCount}
      rows={rows}
      loading={loading}
      error={error}
      onLogout={onLogout}
    />
  );
}

function useProvidersPanelState() {
  const filters = useProviderFilters();
  const kiroAccounts = useKiroAccounts();
  const codexAccounts = useCodexAccounts();
  const kiroQuotas = useKiroQuotas();
  const codexQuotas = useCodexQuotas();
  const [codexImporting, setCodexImporting] = useState(false);
  const refreshAll = useCallback(async () => {
    await kiroAccounts.refresh();
    await kiroQuotas.refresh();
    await codexAccounts.refresh();
    await codexQuotas.refresh();
  }, [kiroAccounts, kiroQuotas, codexAccounts, codexQuotas]);
  const importCodex = useCallback(async () => {
    const selection = await open({
      directory: false,
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof selection !== "string" || !selection.trim()) {
      return;
    }
    setCodexImporting(true);
    try {
      await codexAccounts.importFile(selection);
      await codexQuotas.refresh();
      toast.success(m.codex_import_success());
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setCodexImporting(false);
    }
  }, [codexAccounts, codexQuotas]);
  const quotaMap = useMemo(() => buildQuotaMap(kiroQuotas.quotas), [kiroQuotas.quotas]);
  const codexQuotaMap = useMemo(() => buildCodexQuotaMap(codexQuotas.quotas), [codexQuotas.quotas]);
  const filteredAccounts = useFilteredAccounts(kiroAccounts.accounts, filters.searchKeyword, filters.statusFilter);
  const filteredCodexAccounts = useFilteredAccounts(
    codexAccounts.accounts,
    filters.searchKeyword,
    filters.statusFilter
  );
  const showKiro = filters.providerFilter === PROVIDER_FILTER_ALL || filters.providerFilter === "kiro";
  const showCodex = filters.providerFilter === PROVIDER_FILTER_ALL || filters.providerFilter === "codex";
  const visibleCount = (showKiro ? filteredAccounts.length : 0) + (showCodex ? filteredCodexAccounts.length : 0);
  const refreshBusy =
    kiroAccounts.loading ||
    kiroQuotas.loading ||
    codexAccounts.loading ||
    codexQuotas.loading;

  const toolbarProps = buildToolbarProps(filters, importCodex, refreshAll, refreshBusy, codexImporting);
  const rows = useMemo(() => {
    return [
      ...(showKiro ? buildKiroRows(filteredAccounts, quotaMap) : []),
      ...(showCodex ? buildCodexRows(filteredCodexAccounts, codexQuotaMap) : []),
    ];
  }, [showKiro, showCodex, filteredAccounts, filteredCodexAccounts, quotaMap, codexQuotaMap]);
  const tableError = collectErrorMessages([
    showKiro ? kiroAccounts.error : "",
    showKiro ? kiroQuotas.error : "",
    showCodex ? codexAccounts.error : "",
    showCodex ? codexQuotas.error : "",
  ]);
  const handleRowLogout = useCallback(
    async (row: ProviderAccountTableRow) => {
      if (row.provider === "kiro") {
        await kiroAccounts.logout(row.accountId);
        await kiroQuotas.refresh();
        return;
      }
      await codexAccounts.logout(row.accountId);
      await codexQuotas.refresh();
    },
    [kiroAccounts, kiroQuotas, codexAccounts, codexQuotas]
  );

  return {
    toolbarProps,
    visibleCount,
    rows,
    loading: refreshBusy,
    error: tableError,
    onLogout: handleRowLogout,
  };
}

export function ProvidersPanel() {
  const state = useProvidersPanelState();

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ProvidersToolbar {...state.toolbarProps} />
      <ProvidersSections
        visibleCount={state.visibleCount}
        rows={state.rows}
        loading={state.loading}
        error={state.error}
        onLogout={state.onLogout}
      />
    </div>
  );
}
