import { useCallback, useEffect, useMemo, useState } from "react";

import { Plus, RefreshCw, Search } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
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
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useCodexLogin } from "@/features/codex/use-codex-login";
import { type CodexLoginState } from "@/features/codex/use-codex-login";
import { formatDateLabel } from "@/features/providers/date";
import type { ProviderAccountPageItem } from "@/features/providers/types";
import { deleteProviderAccounts } from "@/features/providers/api";
import { useProviderAccountsPage } from "@/features/providers/use-provider-accounts-page";
import {
  ProvidersAccountsTableSection,
  type ProviderAccountQuotaDetailItem,
  type ProviderAccountTableRow,
} from "@/features/providers/providers-accounts-table";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroLogin } from "@/features/kiro/use-kiro-login";
import { type KiroLoginMethod } from "@/features/kiro/types";
import { type KiroLoginState } from "@/features/kiro/use-kiro-login";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

const PROVIDER_FILTER_ALL = "all";
const STATUS_FILTER_ALL = "all";
const PLACEHOLDER = "—";
const NUMBER_FORMATTER = new Intl.NumberFormat(undefined, {
  maximumFractionDigits: 2,
});

type ProviderFilterValue = typeof PROVIDER_FILTER_ALL | "kiro" | "codex";
type StatusFilterValue =
  | typeof STATUS_FILTER_ALL
  | "active"
  | "disabled"
  | "expired"
  | "cooling_down";

type AccountBase = {
  account_id: string;
  email?: string | null;
  status: "active" | "disabled" | "expired" | "cooling_down";
};

type AddDialogProvider = "kiro" | "codex";

type ProvidersToolbarProps = {
  search: string;
  providerFilter: ProviderFilterValue;
  statusFilter: StatusFilterValue;
  onSearchChange: (value: string) => void;
  onProviderFilterChange: (value: ProviderFilterValue) => void;
  onStatusFilterChange: (value: StatusFilterValue) => void;
  onRefresh: () => void;
  addDialogOpen: boolean;
  onAddDialogOpenChange: (open: boolean) => void;
  onKiroLogin: (method: KiroLoginMethod) => Promise<void>;
  onImportKiroIde: () => Promise<void>;
  onImportKiroKam: () => Promise<void>;
  onCodexLogin: () => Promise<void>;
  onImportCodexFile: () => Promise<void>;
  onImportCodexDirectory: () => Promise<void>;
  refreshing: boolean;
  kiroActionBusy: boolean;
  codexActionBusy: boolean;
  kiroStatusText: string;
  kiroVerificationUrl: string;
  kiroUserCode: string;
  codexStatusText: string;
  codexLoginUrl: string;
};

type ProvidersSectionsProps = {
  rows: ProviderAccountTableRow[];
  loading: boolean;
  error: string;
  page: number;
  totalPages: number;
  totalItems: number;
  onPrevPage: () => void;
  onNextPage: () => void;
  onRefresh: (row: ProviderAccountTableRow) => Promise<void>;
  onRefreshQuota: (row: ProviderAccountTableRow) => Promise<void>;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onBatchDelete: (rows: ProviderAccountTableRow[]) => Promise<void>;
  onSaveProxyUrl: (row: ProviderAccountTableRow, proxyUrl: string) => Promise<void>;
  onToggleStatus: (row: ProviderAccountTableRow, status: "active" | "disabled") => Promise<void>;
  onToggleAutoRefresh: (row: ProviderAccountTableRow, enabled: boolean) => Promise<void>;
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
          <SelectItem value="disabled">{m.common_disabled()}</SelectItem>
          <SelectItem value="expired">{m.kiro_account_status_expired()}</SelectItem>
          <SelectItem value="cooling_down">{m.providers_account_status_cooling_down()}</SelectItem>
        </SelectContent>
      </Select>
    </div>
  );
}

function getAddLabel() {
  const label = m.providers_add_account();
  if (label.endsWith("账户")) {
    return label.slice(0, -2);
  }
  return label.replace(/\s*account$/i, "").trim();
}

function ProvidersToolbar({
  search,
  providerFilter,
  statusFilter,
  onSearchChange,
  onProviderFilterChange,
  onStatusFilterChange,
  onRefresh,
  addDialogOpen,
  onAddDialogOpenChange,
  onKiroLogin,
  onImportKiroIde,
  onImportKiroKam,
  onCodexLogin,
  onImportCodexFile,
  onImportCodexDirectory,
  refreshing,
  kiroActionBusy,
  codexActionBusy,
  kiroStatusText,
  kiroVerificationUrl,
  kiroUserCode,
  codexStatusText,
  codexLoginUrl,
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
        onClick={() => onAddDialogOpenChange(true)}
        data-slot="providers-toolbar-add"
        aria-label={getAddLabel()}
      >
        <Plus className="size-4" aria-hidden="true" />
        <span className="sr-only">{getAddLabel()}</span>
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
      <ProvidersAddAccountDialog
        open={addDialogOpen}
        onOpenChange={onAddDialogOpenChange}
        onKiroLogin={onKiroLogin}
        onImportKiroIde={onImportKiroIde}
        onImportKiroKam={onImportKiroKam}
        onCodexLogin={onCodexLogin}
        onImportCodexFile={onImportCodexFile}
        onImportCodexDirectory={onImportCodexDirectory}
        kiroActionBusy={kiroActionBusy}
        codexActionBusy={codexActionBusy}
        kiroStatusText={kiroStatusText}
        kiroVerificationUrl={kiroVerificationUrl}
        kiroUserCode={kiroUserCode}
        codexStatusText={codexStatusText}
        codexLoginUrl={codexLoginUrl}
      />
    </div>
  );
}

function KiroLoginHint({
  verificationUrl,
  userCode,
}: {
  verificationUrl: string;
  userCode: string;
}) {
  if (!verificationUrl || !userCode) {
    return null;
  }
  return (
    <div className="rounded-md border border-border/60 bg-background/70 p-3 text-xs">
      <p className="font-medium text-foreground">{m.kiro_device_code_title()}</p>
      <p className="mt-2 break-all text-muted-foreground">{verificationUrl}</p>
      <p className="mt-1 font-mono text-sm text-foreground">{userCode}</p>
      <p className="mt-2 text-muted-foreground">{m.kiro_login_open_hint()}</p>
    </div>
  );
}

function CodexLoginHint({ loginUrl }: { loginUrl: string }) {
  if (!loginUrl) {
    return null;
  }
  return (
    <div className="rounded-md border border-border/60 bg-background/70 p-3 text-xs">
      <p className="font-medium text-foreground">{m.codex_login_url_title()}</p>
      <p className="mt-2 break-all text-muted-foreground">{loginUrl}</p>
      <p className="mt-2 text-muted-foreground">{m.codex_login_open_hint()}</p>
    </div>
  );
}

function ProvidersAddAccountDialog({
  open,
  onOpenChange,
  onKiroLogin,
  onImportKiroIde,
  onImportKiroKam,
  onCodexLogin,
  onImportCodexFile,
  onImportCodexDirectory,
  kiroActionBusy,
  codexActionBusy,
  kiroStatusText,
  kiroVerificationUrl,
  kiroUserCode,
  codexStatusText,
  codexLoginUrl,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onKiroLogin: (method: KiroLoginMethod) => Promise<void>;
  onImportKiroIde: () => Promise<void>;
  onImportKiroKam: () => Promise<void>;
  onCodexLogin: () => Promise<void>;
  onImportCodexFile: () => Promise<void>;
  onImportCodexDirectory: () => Promise<void>;
  kiroActionBusy: boolean;
  codexActionBusy: boolean;
  kiroStatusText: string;
  kiroVerificationUrl: string;
  kiroUserCode: string;
  codexStatusText: string;
  codexLoginUrl: string;
}) {
  const [activeProvider, setActiveProvider] = useState<AddDialogProvider>("kiro");

  useEffect(() => {
    if (open) {
      setActiveProvider("kiro");
    }
  }, [open]);

  const addLabel = getAddLabel();

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="providers-add-account-dialog">
        <DialogHeader>
          <DialogTitle>{addLabel}</DialogTitle>
          <DialogDescription>{m.config_section_providers_desc()}</DialogDescription>
        </DialogHeader>
        <DialogBody className="space-y-4">
          <div
            data-slot="providers-add-provider-switch"
            className="inline-flex rounded-lg border border-border/60 bg-muted/30 p-1"
          >
            <Button
              type="button"
              size="sm"
              variant={activeProvider === "kiro" ? "default" : "ghost"}
              onClick={() => setActiveProvider("kiro")}
              data-slot="providers-add-provider-kiro"
            >
              {m.providers_kiro_title()}
            </Button>
            <Button
              type="button"
              size="sm"
              variant={activeProvider === "codex" ? "default" : "ghost"}
              onClick={() => setActiveProvider("codex")}
              data-slot="providers-add-provider-codex"
            >
              {m.providers_codex_title()}
            </Button>
          </div>
          {activeProvider === "kiro" ? (
            <div data-slot="providers-add-panel-kiro" className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3">
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void onKiroLogin("aws");
                  }}
                  disabled={kiroActionBusy}
                  data-slot="providers-add-kiro-login-aws"
                >
                  {m.kiro_login_method_aws()}
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void onKiroLogin("aws_authcode");
                  }}
                  disabled={kiroActionBusy}
                  data-slot="providers-add-kiro-login-aws-authcode"
                >
                  {m.kiro_login_method_aws_authcode()}
                </Button>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void onKiroLogin("google");
                  }}
                  disabled={kiroActionBusy}
                  data-slot="providers-add-kiro-login-google"
                >
                  {m.kiro_login_method_google()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void onImportKiroIde();
                  }}
                  disabled={kiroActionBusy}
                  data-slot="providers-add-kiro-import-ide"
                >
                  {m.kiro_login_method_import()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void onImportKiroKam();
                  }}
                  disabled={kiroActionBusy}
                  data-slot="providers-add-kiro-import-kam"
                >
                  {m.kiro_login_method_import_kam()}
                </Button>
              </div>
              {kiroStatusText ? (
                <p className="text-xs text-muted-foreground">{kiroStatusText}</p>
              ) : null}
              <KiroLoginHint verificationUrl={kiroVerificationUrl} userCode={kiroUserCode} />
            </div>
          ) : (
            <div data-slot="providers-add-panel-codex" className="space-y-2 rounded-md border border-border/60 bg-muted/20 p-3">
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={() => {
                    void onCodexLogin();
                  }}
                  disabled={codexActionBusy}
                  data-slot="providers-add-codex-login"
                >
                  {m.codex_login_button()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void onImportCodexFile();
                  }}
                  disabled={codexActionBusy}
                  data-slot="providers-add-codex-import-file"
                >
                  {m.codex_import_file_button()}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void onImportCodexDirectory();
                  }}
                  disabled={codexActionBusy}
                  data-slot="providers-add-codex-import-directory"
                >
                  {m.codex_import_directory_button()}
                </Button>
              </div>
              {codexStatusText ? (
                <p className="text-xs text-muted-foreground">{codexStatusText}</p>
              ) : null}
              <CodexLoginHint loginUrl={codexLoginUrl} />
            </div>
          )}
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
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

function buildToolbarProps(
  filters: ReturnType<typeof useProviderFilters>,
  addDialogOpen: boolean,
  onAddDialogOpenChange: (open: boolean) => void,
  onKiroLogin: (method: KiroLoginMethod) => Promise<void>,
  onImportKiroIde: () => Promise<void>,
  onImportKiroKam: () => Promise<void>,
  onCodexLogin: () => Promise<void>,
  onImportCodexFile: () => Promise<void>,
  onImportCodexDirectory: () => Promise<void>,
  onRefresh: () => void,
  refreshing: boolean,
  kiroActionBusy: boolean,
  codexActionBusy: boolean,
  kiroStatusText: string,
  kiroVerificationUrl: string,
  kiroUserCode: string,
  codexStatusText: string,
  codexLoginUrl: string,
) {
  return {
    search: filters.search,
    providerFilter: filters.providerFilter,
    statusFilter: filters.statusFilter,
    onSearchChange: filters.setSearch,
    onProviderFilterChange: filters.setProviderFilter,
    onStatusFilterChange: filters.setStatusFilter,
    addDialogOpen,
    onAddDialogOpenChange,
    onKiroLogin,
    onImportKiroIde,
    onImportKiroKam,
    onCodexLogin,
    onImportCodexFile,
    onImportCodexDirectory,
    onRefresh,
    refreshing,
    kiroActionBusy,
    codexActionBusy,
    kiroStatusText,
    kiroVerificationUrl,
    kiroUserCode,
    codexStatusText,
    codexLoginUrl,
  };
}

function getKiroStatusText(login: KiroLoginState) {
  if (login.status === "waiting") {
    return m.kiro_login_waiting();
  }
  if (login.status === "polling") {
    return m.kiro_login_polling();
  }
  if (login.status === "success") {
    return m.kiro_login_success();
  }
  if (login.status === "error") {
    return login.error ?? m.kiro_login_failed();
  }
  return "";
}

function getCodexStatusText(login: CodexLoginState) {
  if (login.status === "waiting") {
    return m.codex_login_waiting();
  }
  if (login.status === "polling") {
    return m.codex_login_polling();
  }
  if (login.status === "success") {
    return m.codex_login_success();
  }
  if (login.status === "error") {
    return login.error ?? m.codex_login_failed();
  }
  return "";
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
  if (status === "expired") {
    return "destructive";
  }
  if (status === "disabled") {
    return "outline";
  }
  if (status === "cooling_down") {
    return "default";
  }
  return "secondary";
}

function formatKiroStatus(status: AccountBase["status"]) {
  if (status === "expired") {
    return m.kiro_account_status_expired();
  }
  if (status === "disabled") {
    return m.kiro_account_status_disabled();
  }
  if (status === "cooling_down") {
    return m.kiro_account_status_cooling_down();
  }
  return m.kiro_account_status_active();
}

function formatCodexStatus(status: AccountBase["status"]) {
  if (status === "expired") {
    return m.codex_account_status_expired();
  }
  if (status === "disabled") {
    return m.codex_account_status_disabled();
  }
  if (status === "cooling_down") {
    return m.codex_account_status_cooling_down();
  }
  return m.codex_account_status_active();
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

function buildKiroQuotaDetails(quota: ProviderAccountPageItem["quota"] | null) {
  if (quota?.error) {
    return {
      planType: quota.plan_type ?? PLACEHOLDER,
      quotaSummary: m.providers_quota_failed_title(),
      quotaError: quota.error,
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  if (!quota || quota.items.length === 0) {
    return {
      planType: quota?.plan_type ?? PLACEHOLDER,
      quotaSummary: PLACEHOLDER,
      quotaError: "",
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  const quotaItems = quota.items.map((item) => {
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
    planType: quota.plan_type ?? PLACEHOLDER,
    quotaSummary: summarizeQuota(
      `${quotaItems[0]?.name} · ${quotaItems[0]?.summary ?? PLACEHOLDER}`,
      quotaItems.length
    ),
    quotaError: "",
    quotaItems,
  };
}

function buildCodexQuotaDetails(quota: ProviderAccountPageItem["quota"] | null) {
  if (quota?.error) {
    return {
      planType: quota.plan_type ?? PLACEHOLDER,
      quotaSummary: m.providers_quota_failed_title(),
      quotaError: quota.error,
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  if (!quota || quota.items.length === 0) {
    return {
      planType: quota?.plan_type ?? PLACEHOLDER,
      quotaSummary: PLACEHOLDER,
      quotaError: "",
      quotaItems: [] as ProviderAccountQuotaDetailItem[],
    };
  }
  const quotaItems = quota.items.map((item) => {
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
    planType: quota.plan_type ?? PLACEHOLDER,
    quotaSummary: summarizeQuota(
      `${quotaItems[0]?.name} · ${quotaItems[0]?.summary ?? PLACEHOLDER}`,
      quotaItems.length
    ),
    quotaError: "",
    quotaItems,
  };
}

function buildKiroRows(
  accounts: Array<ProviderAccountPageItem & { provider_kind: "kiro" }>
): ProviderAccountTableRow[] {
  return accounts.map((account) => {
    const quota = buildKiroQuotaDetails(account.quota);
    return {
      id: `kiro:${account.account_id}`,
      provider: "kiro",
      providerLabel: m.providers_kiro_title(),
      displayName: formatDisplayName(account),
      accountId: account.account_id,
      status: account.status,
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
      proxyUrlValue: account.proxy_url ?? "",
      canRefresh: false,
      logoutLabel: m.kiro_account_logout(),
      autoRefreshEnabled: null,
    };
  });
}

function buildCodexRows(
  accounts: Array<ProviderAccountPageItem & { provider_kind: "codex" }>
): ProviderAccountTableRow[] {
  return accounts.map((account) => {
    const quota = buildCodexQuotaDetails(account.quota);
    return {
      id: `codex:${account.account_id}`,
      provider: "codex",
      providerLabel: m.providers_codex_title(),
      displayName: formatDisplayName(account),
      accountId: account.account_id,
      status: account.status,
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
      proxyUrlValue: account.proxy_url ?? "",
      canRefresh: true,
      logoutLabel: m.codex_account_logout(),
      autoRefreshEnabled: account.auto_refresh_enabled ?? true,
    };
  });
}

function collectErrorMessages(parts: string[]) {
  return parts.filter(Boolean).join(" · ");
}

function ProvidersSections({
  rows,
  loading,
  error,
  page,
  totalPages,
  totalItems,
  onPrevPage,
  onNextPage,
  onRefresh,
  onRefreshQuota,
  onLogout,
  onBatchDelete,
  onSaveProxyUrl,
  onToggleStatus,
  onToggleAutoRefresh,
}: ProvidersSectionsProps) {
  return (
    <ProvidersAccountsTableSection
      rows={rows}
      loading={loading}
      error={error}
      page={page}
      totalPages={totalPages}
      totalItems={totalItems}
      onPrevPage={onPrevPage}
      onNextPage={onNextPage}
      onRefresh={onRefresh}
      onRefreshQuota={onRefreshQuota}
      onLogout={onLogout}
      onBatchDelete={onBatchDelete}
      onSaveProxyUrl={onSaveProxyUrl}
      onToggleStatus={onToggleStatus}
      onToggleAutoRefresh={onToggleAutoRefresh}
    />
  );
}

function useProvidersPanelState() {
  const filters = useProviderFilters();
  const providerAccounts = useProviderAccountsPage({
    searchKeyword: filters.searchKeyword,
    providerFilter: filters.providerFilter,
    statusFilter: filters.statusFilter,
  });
  const kiroAccounts = useKiroAccounts({ autoLoad: false });
  const codexAccounts = useCodexAccounts({ autoLoad: false });
  const refreshKiroData = useCallback(async (accountId?: string) => {
    await kiroAccounts.refreshQuotaCache(accountId ? [accountId] : undefined);
    await providerAccounts.refresh();
    await kiroAccounts.refresh();
  }, [kiroAccounts, providerAccounts]);
  const refreshCodexData = useCallback(async (accountId?: string) => {
    await codexAccounts.refreshQuotaCache(accountId ? [accountId] : undefined);
    await providerAccounts.refresh();
    await codexAccounts.refresh();
  }, [codexAccounts, providerAccounts]);
  const syncImportedKiroAccounts = useCallback(async (accountIds: string[]) => {
    try {
      await kiroAccounts.refreshQuotaCache(accountIds);
      await Promise.all([providerAccounts.refresh(), kiroAccounts.refresh()]);
    } catch (error) {
      toast.error(parseError(error));
    }
  }, [kiroAccounts, providerAccounts]);
  const syncImportedCodexAccounts = useCallback(async (accountIds: string[]) => {
    try {
      await codexAccounts.refreshQuotaCache(accountIds);
      await Promise.all([providerAccounts.refresh(), codexAccounts.refresh()]);
    } catch (error) {
      toast.error(parseError(error));
    }
  }, [codexAccounts, providerAccounts]);
  const kiroLogin = useKiroLogin({ onRefresh: refreshKiroData });
  const codexLogin = useCodexLogin({ onRefresh: refreshCodexData });
  const [addDialogOpen, setAddDialogOpen] = useState(false);
  const [kiroImporting, setKiroImporting] = useState(false);
  const [codexImporting, setCodexImporting] = useState(false);
  const [batchDeleting, setBatchDeleting] = useState(false);
  const [optimisticDeletedIds, setOptimisticDeletedIds] = useState<Set<string>>(new Set());
  const refreshAll = useCallback(async () => {
    await Promise.all([kiroAccounts.refresh(), codexAccounts.refresh()]);
    await providerAccounts.refresh();
  }, [kiroAccounts, codexAccounts, providerAccounts]);
  const loginKiro = useCallback(async (method: KiroLoginMethod) => {
    await kiroLogin.beginLogin(method);
  }, [kiroLogin]);
  const importKiroIde = useCallback(async () => {
    const selection = await open({
      directory: true,
      multiple: false,
    });
    if (typeof selection !== "string" || !selection.trim()) {
      return;
    }
    setKiroImporting(true);
    try {
      const imported = await kiroAccounts.importIde(selection);
      toast.success(m.kiro_import_success());
      void syncImportedKiroAccounts(imported.map((item) => item.account_id));
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setKiroImporting(false);
    }
  }, [kiroAccounts, syncImportedKiroAccounts]);
  const importKiroKam = useCallback(async () => {
    const selection = await open({
      directory: false,
      multiple: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof selection !== "string" || !selection.trim()) {
      return;
    }
    setKiroImporting(true);
    try {
      const imported = await kiroAccounts.importKam(selection);
      toast.success(m.kiro_import_kam_success());
      void syncImportedKiroAccounts(imported.map((item) => item.account_id));
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setKiroImporting(false);
    }
  }, [kiroAccounts, syncImportedKiroAccounts]);
  const loginCodex = useCallback(async () => {
    await codexLogin.beginLogin();
  }, [codexLogin]);
  const importCodexFile = useCallback(async () => {
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
      const imported = await codexAccounts.importFile(selection);
      toast.success(m.codex_import_success());
      void syncImportedCodexAccounts(imported.map((item) => item.account_id));
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setCodexImporting(false);
    }
  }, [codexAccounts, syncImportedCodexAccounts]);
  const importCodexDirectory = useCallback(async () => {
    const selection = await open({
      directory: true,
      multiple: false,
    });
    if (typeof selection !== "string" || !selection.trim()) {
      return;
    }
    setCodexImporting(true);
    try {
      const imported = await codexAccounts.importFile(selection);
      toast.success(m.codex_import_success());
      void syncImportedCodexAccounts(imported.map((item) => item.account_id));
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setCodexImporting(false);
    }
  }, [codexAccounts, syncImportedCodexAccounts]);
  const refreshBusy = kiroAccounts.loading || codexAccounts.loading || providerAccounts.loading;
  const kiroActionBusy =
    kiroImporting ||
    kiroLogin.login.status === "waiting" ||
    kiroLogin.login.status === "polling";
  const codexActionBusy =
    codexImporting ||
    codexLogin.login.status === "waiting" ||
    codexLogin.login.status === "polling";
  const kiroVerificationUrl =
    kiroLogin.login.start?.verification_uri_complete ??
    kiroLogin.login.start?.verification_uri ??
    "";
  const kiroUserCode = kiroLogin.login.start?.user_code ?? "";
  const codexLoginUrl = codexLogin.login.start?.login_url ?? "";

  const toolbarProps = buildToolbarProps(
    filters,
    addDialogOpen,
    setAddDialogOpen,
    loginKiro,
    importKiroIde,
    importKiroKam,
    loginCodex,
    importCodexFile,
    importCodexDirectory,
    refreshAll,
    refreshBusy,
    kiroActionBusy,
    codexActionBusy,
    getKiroStatusText(kiroLogin.login),
    kiroVerificationUrl,
    kiroUserCode,
    getCodexStatusText(codexLogin.login),
    codexLoginUrl,
  );
  const rows = useMemo(() => {
    const kiroPageItems = providerAccounts.items.filter(
      (item): item is ProviderAccountPageItem & { provider_kind: "kiro" } =>
        item.provider_kind === "kiro"
    );
    const codexPageItems = providerAccounts.items.filter(
      (item): item is ProviderAccountPageItem & { provider_kind: "codex" } =>
        item.provider_kind === "codex"
    );
    return [
      ...buildKiroRows(kiroPageItems),
      ...buildCodexRows(codexPageItems),
    ];
  }, [providerAccounts.items]);
  const visibleRows = useMemo(
    () => rows.filter((row) => !optimisticDeletedIds.has(row.id)),
    [rows, optimisticDeletedIds]
  );
  const tableBusy = providerAccounts.loading || batchDeleting;
  const tableError = collectErrorMessages([
    providerAccounts.error,
    filters.providerFilter !== "codex" ? kiroAccounts.error : "",
    filters.providerFilter !== "kiro" ? codexAccounts.error : "",
  ]);
  const handleRowLogout = useCallback(
    async (row: ProviderAccountTableRow) => {
      if (row.provider === "kiro") {
        await kiroAccounts.logout(row.accountId);
        await providerAccounts.refresh();
        await kiroAccounts.refresh();
        return;
      }
      await codexAccounts.logout(row.accountId);
      await providerAccounts.refresh();
      await codexAccounts.refresh();
    },
    [kiroAccounts, codexAccounts, providerAccounts]
  );
  const handleRowRefresh = useCallback(
    async (row: ProviderAccountTableRow) => {
      if (row.provider !== "codex") {
        return;
      }
      try {
        await codexAccounts.refreshAccount(row.accountId);
        await codexAccounts.refreshQuotaCache([row.accountId]);
        await providerAccounts.refresh();
        await codexAccounts.refresh();
      } catch (error) {
        toast.error(parseError(error));
      }
    },
    [codexAccounts, providerAccounts]
  );
  const handleCodexAutoRefreshToggle = useCallback(
    async (row: ProviderAccountTableRow, enabled: boolean) => {
      if (row.provider !== "codex") {
        return;
      }
      try {
        await codexAccounts.setAutoRefresh(row.accountId, enabled);
        await providerAccounts.refresh();
      } catch (error) {
        toast.error(parseError(error));
      }
    },
    [codexAccounts, providerAccounts]
  );
  const handleRowRefreshQuota = useCallback(
    async (row: ProviderAccountTableRow) => {
      try {
        if (row.provider === "kiro") {
          await kiroAccounts.refreshQuotaNow(row.accountId);
          await Promise.all([providerAccounts.refresh(), kiroAccounts.refresh()]);
          return;
        }
        await codexAccounts.refreshQuotaNow(row.accountId);
        await Promise.all([providerAccounts.refresh(), codexAccounts.refresh()]);
      } catch (error) {
        toast.error(parseError(error));
      }
    },
    [kiroAccounts, codexAccounts, providerAccounts]
  );
  const handleAccountStatusToggle = useCallback(
    async (row: ProviderAccountTableRow, status: "active" | "disabled") => {
      try {
        if (row.provider === "kiro") {
          await kiroAccounts.setStatus(row.accountId, status);
          await Promise.all([providerAccounts.refresh(), kiroAccounts.refresh()]);
          return;
        }
        await codexAccounts.setStatus(row.accountId, status);
        await Promise.all([providerAccounts.refresh(), codexAccounts.refresh()]);
      } catch (error) {
        toast.error(parseError(error));
      }
    },
    [kiroAccounts, codexAccounts, providerAccounts]
  );
  const handleSaveProxyUrl = useCallback(
    async (row: ProviderAccountTableRow, proxyUrl: string) => {
      try {
        if (row.provider === "kiro") {
          await kiroAccounts.setProxyUrl(row.accountId, proxyUrl || null);
          await kiroAccounts.refresh();
        } else {
          await codexAccounts.setProxyUrl(row.accountId, proxyUrl || null);
          await codexAccounts.refresh();
        }
        await providerAccounts.refresh();
      } catch (error) {
        toast.error(parseError(error));
      }
    },
    [kiroAccounts, codexAccounts, providerAccounts]
  );

  const handleBatchDelete = useCallback(
    async (rowsToDelete: ProviderAccountTableRow[]) => {
      if (rowsToDelete.length === 0) {
        return;
      }
      // 删除确认后先做乐观隐藏，避免用户在慢 I/O 场景下看到“点击后没反应”。
      setBatchDeleting(true);
      setOptimisticDeletedIds(new Set(rowsToDelete.map((row) => row.id)));
      const accountIds = rowsToDelete.map((row) => row.accountId);
      try {
        await deleteProviderAccounts(accountIds);
        await providerAccounts.refresh();
        void Promise.all([kiroAccounts.refresh(), codexAccounts.refresh()]).catch(() => undefined);
        toast.success(m.providers_accounts_delete_success({ count: accountIds.length }));
      } catch (error) {
        toast.error(parseError(error));
      } finally {
        setBatchDeleting(false);
        setOptimisticDeletedIds(new Set());
      }
    },
    [kiroAccounts, codexAccounts, providerAccounts]
  );

  return {
    toolbarProps,
    rows: visibleRows,
    loading: tableBusy,
    error: tableError,
    page: providerAccounts.page,
    totalPages: providerAccounts.totalPages,
    totalItems: providerAccounts.total,
    onPrevPage: providerAccounts.onPrevPage,
    onNextPage: providerAccounts.onNextPage,
    onRefresh: handleRowRefresh,
    onRefreshQuota: handleRowRefreshQuota,
    onLogout: handleRowLogout,
    onBatchDelete: handleBatchDelete,
    onSaveProxyUrl: handleSaveProxyUrl,
    onToggleStatus: handleAccountStatusToggle,
    onToggleAutoRefresh: handleCodexAutoRefreshToggle,
  };
}

export function ProvidersPanel() {
  const state = useProvidersPanelState();

  return (
    <div className="flex flex-col gap-4 px-4 lg:px-6">
      <ProvidersToolbar {...state.toolbarProps} />
      <ProvidersSections
        rows={state.rows}
        loading={state.loading}
        error={state.error}
        page={state.page}
        totalPages={state.totalPages}
        totalItems={state.totalItems}
        onPrevPage={state.onPrevPage}
        onNextPage={state.onNextPage}
        onRefresh={state.onRefresh}
        onRefreshQuota={state.onRefreshQuota}
        onLogout={state.onLogout}
        onBatchDelete={state.onBatchDelete}
        onSaveProxyUrl={state.onSaveProxyUrl}
        onToggleStatus={state.onToggleStatus}
        onToggleAutoRefresh={state.onToggleAutoRefresh}
      />
    </div>
  );
}
