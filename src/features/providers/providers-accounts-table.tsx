import { useCallback, useEffect, useMemo, useState } from "react";

import { AlertCircle, Eye } from "lucide-react";
import type { CheckedState } from "@radix-ui/react-checkbox";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { AccountDeleteAction, AccountsBatchDeleteAction } from "@/features/providers/account-delete-dialog";
import { m } from "@/paraglide/messages.js";

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

export type ProviderAccountQuotaDetailItem = {
  name: string;
  summary: string;
  secondary: string;
};

export type ProviderAccountTableRow = {
  id: string;
  provider: "kiro" | "codex";
  providerLabel: string;
  displayName: string;
  accountId: string;
  statusLabel: string;
  statusVariant: BadgeVariant;
  expiresAtLabel: string;
  planType: string;
  quotaSummary: string;
  sourceOrMethodLabel: string;
  detailDescription: string;
  detailFields: Array<{
    label: string;
    value: string;
  }>;
  quotaError: string;
  quotaItems: ProviderAccountQuotaDetailItem[];
  canRefresh: boolean;
  logoutLabel: string;
  autoRefreshEnabled: boolean | null;
};

type ProviderAccountDialogProps = {
  open: boolean;
  row: ProviderAccountTableRow | null;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  onRefresh: (row: ProviderAccountTableRow) => Promise<void>;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onToggleAutoRefresh: (row: ProviderAccountTableRow, enabled: boolean) => Promise<void>;
};

function AccountFieldGrid({ fields }: { fields: ProviderAccountTableRow["detailFields"] }) {
  return (
    <div className="grid gap-3 sm:grid-cols-2">
      {fields.map((field) => (
        <div
          key={field.label}
          className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2"
        >
          <p className="text-xs text-muted-foreground">{field.label}</p>
          <p className="mt-1 text-sm font-medium text-foreground break-all">{field.value}</p>
        </div>
      ))}
    </div>
  );
}

function QuotaDetailSection({
  quotaError,
  quotaItems,
}: {
  quotaError: string;
  quotaItems: ProviderAccountQuotaDetailItem[];
}) {
  if (quotaError) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="size-4" aria-hidden="true" />
        <div>
          <AlertTitle>{m.providers_quota_failed_title()}</AlertTitle>
          <AlertDescription>{quotaError}</AlertDescription>
        </div>
      </Alert>
    );
  }

  if (!quotaItems.length) {
    return <p className="text-sm text-muted-foreground">{m.providers_quota_empty()}</p>;
  }

  return (
    <div className="space-y-2">
      {quotaItems.map((item) => (
        <div
          key={item.name}
          className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2"
        >
          <div className="flex flex-wrap items-start justify-between gap-2">
            <p className="text-sm font-medium text-foreground">{item.name}</p>
            <p className="text-xs text-muted-foreground">{item.summary}</p>
          </div>
          {item.secondary ? (
            <p className="mt-1 text-xs text-muted-foreground">{item.secondary}</p>
          ) : null}
        </div>
      ))}
    </div>
  );
}

function ProviderAccountDialog({
  open,
  row,
  busy,
  onOpenChange,
  onRefresh,
  onLogout,
  onToggleAutoRefresh,
}: ProviderAccountDialogProps) {
  const [refreshConfirmOpen, setRefreshConfirmOpen] = useState(false);

  const handleRefresh = () => {
    if (!row) {
      return;
    }
    void onRefresh(row).finally(() => setRefreshConfirmOpen(false));
  };

  const handleLogout = () => {
    if (!row) {
      return;
    }
    void onLogout(row).finally(() => onOpenChange(false));
  };

  const handleToggleAutoRefresh = (enabled: boolean) => {
    if (!row || row.provider !== "codex") {
      return;
    }
    void onToggleAutoRefresh(row, enabled);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-slot="provider-account-dialog">
        <DialogHeader>
          <DialogTitle>{m.providers_account_dialog_title()}</DialogTitle>
          {row ? (
            <DialogDescription>{row.detailDescription}</DialogDescription>
          ) : null}
        </DialogHeader>
        <DialogBody className="space-y-4">
          {row ? (
            <>
              <AccountFieldGrid fields={row.detailFields} />
              <div className="space-y-2">
                <p className="text-sm font-semibold text-foreground">
                  {m.providers_table_quota()}
                </p>
                <QuotaDetailSection quotaError={row.quotaError} quotaItems={row.quotaItems} />
              </div>
              <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border/60 pt-4">
                {row.provider === "codex" && row.autoRefreshEnabled !== null ? (
                  <div className="mr-auto flex items-center gap-2">
                    <Switch
                      checked={row.autoRefreshEnabled}
                      onCheckedChange={handleToggleAutoRefresh}
                      disabled={busy}
                      aria-label="Codex 自动置换 Token"
                    />
                    <p className="text-xs text-muted-foreground">Codex 自动置换 Token</p>
                  </div>
                ) : null}
                {row.canRefresh ? (
                  <>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setRefreshConfirmOpen(true)}
                      disabled={busy}
                    >
                      {m.common_refresh()}
                    </Button>
                    <AlertDialog open={refreshConfirmOpen} onOpenChange={setRefreshConfirmOpen}>
                      <AlertDialogContent data-slot="codex-refresh-confirm-dialog">
                        <AlertDialogHeader>
                          <AlertDialogTitle>确认刷新 Token？</AlertDialogTitle>
                          <AlertDialogDescription>
                            将尝试刷新当前 Codex 账户的访问令牌。
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
                          <AlertDialogAction onClick={handleRefresh}>
                            {m.common_refresh()}
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  </>
                ) : null}
                <AccountDeleteAction
                  accountLabel={row.displayName}
                  buttonLabel={row.logoutLabel}
                  disabled={busy}
                  onConfirm={handleLogout}
                />
              </div>
            </>
          ) : null}
        </DialogBody>
      </DialogContent>
    </Dialog>
  );
}

type ProvidersAccountsTableSectionProps = {
  rows: ProviderAccountTableRow[];
  loading: boolean;
  error: string;
  page: number;
  totalPages: number;
  totalItems: number;
  onPrevPage: () => void;
  onNextPage: () => void;
  onRefresh: (row: ProviderAccountTableRow) => Promise<void>;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onBatchDelete: (rows: ProviderAccountTableRow[]) => Promise<void>;
  onToggleAutoRefresh: (row: ProviderAccountTableRow, enabled: boolean) => Promise<void>;
};

function ProviderAccountRowActions({
  row,
  onOpenDetails,
}: {
  row: ProviderAccountTableRow;
  onOpenDetails: (row: ProviderAccountTableRow) => void;
}) {
  return (
    <TableCell className="sticky right-0 z-10 w-[5rem] border-l border-border/40 bg-background/95 text-right backdrop-blur-xs group-hover:bg-muted/50">
      <div className="flex justify-end gap-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              aria-label={m.providers_account_dialog_title()}
              data-slot="provider-account-row-details"
              onClick={() => onOpenDetails(row)}
            >
              <Eye className="size-4" aria-hidden="true" />
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top">{m.providers_account_dialog_title()}</TooltipContent>
        </Tooltip>
      </div>
    </TableCell>
  );
}

export function ProvidersAccountsTableSection({
  rows,
  loading,
  error,
  page,
  totalPages,
  totalItems,
  onPrevPage,
  onNextPage,
  onRefresh,
  onLogout,
  onBatchDelete,
  onToggleAutoRefresh,
}: ProvidersAccountsTableSectionProps) {
  const [selectedRow, setSelectedRow] = useState<ProviderAccountTableRow | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  const selectedRows = useMemo(
    () => rows.filter((row) => selectedIds.has(row.id)),
    [rows, selectedIds]
  );
  const visibleRowIds = useMemo(() => new Set(rows.map((row) => row.id)), [rows]);
  const selectedCount = selectedRows.length;

  useEffect(() => {
    setSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const rowId of prev) {
        if (visibleRowIds.has(rowId)) {
          next.add(rowId);
          continue;
        }
        changed = true;
      }
      return changed ? next : prev;
    });
  }, [visibleRowIds]);

  useEffect(() => {
    setSelectedRow((prev) => {
      if (!prev) {
        return prev;
      }
      const next = rows.find((row) => row.id === prev.id);
      return next ?? null;
    });
  }, [rows]);

  const toggleSelect = useCallback((rowId: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(rowId)) {
        next.delete(rowId);
      } else {
        next.add(rowId);
      }
      return next;
    });
  }, []);

  const toggleSelectAll = useCallback(
    (checked: CheckedState) => {
      if (checked === true) {
        setSelectedIds(new Set(rows.map((row) => row.id)));
      } else {
        setSelectedIds(new Set());
      }
    },
    [rows]
  );

  const isAllSelected = selectedCount === rows.length && rows.length > 0;
  const isIndeterminate = selectedCount > 0 && selectedCount < rows.length;

  const handleBatchDelete = useCallback(() => {
    if (selectedRows.length === 0) {
      return;
    }
    void onBatchDelete(selectedRows);
    setSelectedIds(new Set());
  }, [onBatchDelete, selectedRows]);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  return (
    <section className="space-y-3">
      {error ? (
        <Alert variant="destructive">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.providers_load_failed()}</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </div>
        </Alert>
      ) : null}
      {rows.length ? (
        <>
          {selectedCount > 0 ? (
            <div
              data-slot="providers-accounts-selection-bar"
              className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border/60 bg-background/70 px-3 py-2"
            >
              <p className="text-sm text-foreground">
                {m.providers_accounts_delete_description({ count: selectedCount })}
              </p>
              <div className="flex items-center gap-2">
                <AccountsBatchDeleteAction
                  count={selectedCount}
                  disabled={loading}
                  onConfirm={handleBatchDelete}
                />
                <Button type="button" size="sm" variant="ghost" onClick={clearSelection}>
                  {m.common_cancel()}
                </Button>
              </div>
            </div>
          ) : null}
          <div
            data-slot="providers-accounts-table"
            className="rounded-lg border border-border/60 bg-background/60"
          >
            <Table className="min-w-[72rem] border-collapse text-sm">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[2.5rem]">
                    <Checkbox
                      checked={isIndeterminate ? "indeterminate" : isAllSelected}
                      onCheckedChange={toggleSelectAll}
                      aria-label="Select all"
                    />
                  </TableHead>
                  <TableHead>{m.providers_table_provider()}</TableHead>
                  <TableHead>{m.providers_table_account()}</TableHead>
                  <TableHead>{m.providers_table_account_id()}</TableHead>
                  <TableHead>{m.providers_table_status()}</TableHead>
                  <TableHead>{m.providers_table_expires()}</TableHead>
                  <TableHead>{m.providers_table_plan()}</TableHead>
                  <TableHead>{m.providers_table_quota()}</TableHead>
                  <TableHead>{m.providers_table_source()}</TableHead>
                  <TableHead className="sticky right-0 z-20 w-[5rem] border-l border-border/40 bg-background/95 text-right backdrop-blur-xs">
                    {m.providers_table_actions()}
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((row) => (
                  <TableRow key={row.id} className="group">
                    <TableCell>
                      <Checkbox
                        checked={selectedIds.has(row.id)}
                        onCheckedChange={() => toggleSelect(row.id)}
                        aria-label={`Select ${row.displayName}`}
                      />
                    </TableCell>
                    <TableCell>{row.providerLabel}</TableCell>
                    <TableCell className="font-medium text-foreground">{row.displayName}</TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground">
                      {row.accountId}
                    </TableCell>
                    <TableCell>
                      <Badge variant={row.statusVariant}>{row.statusLabel}</Badge>
                    </TableCell>
                    <TableCell>{row.expiresAtLabel}</TableCell>
                    <TableCell>{row.planType}</TableCell>
                    <TableCell>{row.quotaSummary}</TableCell>
                    <TableCell>{row.sourceOrMethodLabel}</TableCell>
                    <ProviderAccountRowActions row={row} onOpenDetails={setSelectedRow} />
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
          <div
            data-slot="providers-pagination"
            className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border/60 bg-background/70 px-3 py-2"
          >
            <p
              data-testid="providers-pagination-indicator"
              className="text-xs text-muted-foreground"
            >
              {m.dashboard_page_indicator({
                page: String(page),
                totalPages: String(totalPages),
              })}
              {` · ${totalItems}`}
            </p>
            <div className="flex items-center gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={page <= 1 || loading}
                onClick={onPrevPage}
              >
                {m.dashboard_prev_page()}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={page >= totalPages || loading}
                onClick={onNextPage}
              >
                {m.dashboard_next_page()}
              </Button>
            </div>
          </div>
        </>
      ) : loading ? (
        <p className="text-sm text-muted-foreground">{m.providers_accounts_loading()}</p>
      ) : (
        <p className="text-sm text-muted-foreground">{m.providers_accounts_empty_filtered()}</p>
      )}
      <ProviderAccountDialog
        open={selectedRow !== null}
        row={selectedRow}
        busy={loading}
        onOpenChange={(open) => {
          if (!open) {
            setSelectedRow(null);
          }
        }}
        onRefresh={onRefresh}
        onLogout={onLogout}
        onToggleAutoRefresh={onToggleAutoRefresh}
      />
    </section>
  );
}
