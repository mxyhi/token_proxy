import { useState } from "react";

import { AlertCircle } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { AccountDeleteAction } from "@/features/providers/account-delete-dialog";
import { m } from "@/paraglide/messages.js";

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

export type ProviderAccountQuotaDetailItem = {
  name: string;
  summary: string;
  secondary: string;
};

export type ProviderAccountTableRow = {
  id: string;
  provider: "kiro" | "codex" | "antigravity";
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
  logoutLabel: string;
  canSwitchIde: boolean;
};

type ProviderAccountDialogProps = {
  open: boolean;
  row: ProviderAccountTableRow | null;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onSwitchIde: (row: ProviderAccountTableRow) => Promise<void>;
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
  onLogout,
  onSwitchIde,
}: ProviderAccountDialogProps) {
  const handleLogout = () => {
    if (!row) {
      return;
    }
    void onLogout(row).finally(() => onOpenChange(false));
  };

  const handleSwitchIde = () => {
    if (!row) {
      return;
    }
    void onSwitchIde(row);
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
                {row.canSwitchIde ? (
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={handleSwitchIde}
                    disabled={busy}
                  >
                    {m.antigravity_switch_ide_button()}
                  </Button>
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
  count: number;
  rows: ProviderAccountTableRow[];
  loading: boolean;
  error: string;
  onLogout: (row: ProviderAccountTableRow) => Promise<void>;
  onSwitchIde: (row: ProviderAccountTableRow) => Promise<void>;
};

export function ProvidersAccountsTableSection({
  count,
  rows,
  loading,
  error,
  onLogout,
  onSwitchIde,
}: ProvidersAccountsTableSectionProps) {
  const [selectedRow, setSelectedRow] = useState<ProviderAccountTableRow | null>(null);

  return (
    <section className="space-y-3">
      <div data-slot="providers-section-header" className="flex flex-wrap items-start gap-2">
        <div className="flex-1">
          <div className="flex items-center gap-2">
            <p className="text-sm font-semibold text-foreground">
              {m.providers_section_accounts_title()}
            </p>
            <Badge variant="secondary">{count}</Badge>
          </div>
        </div>
      </div>
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
        <div
          data-slot="providers-accounts-table"
          className="overflow-hidden rounded-lg border border-border/60 bg-background/60"
        >
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{m.providers_table_provider()}</TableHead>
                <TableHead>{m.providers_table_account()}</TableHead>
                <TableHead>{m.providers_table_account_id()}</TableHead>
                <TableHead>{m.providers_table_status()}</TableHead>
                <TableHead>{m.providers_table_expires()}</TableHead>
                <TableHead>{m.providers_table_plan()}</TableHead>
                <TableHead>{m.providers_table_quota()}</TableHead>
                <TableHead>{m.providers_table_source()}</TableHead>
                <TableHead className="text-right">{m.providers_table_actions()}</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {rows.map((row) => (
                <TableRow key={row.id}>
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
                  <TableCell className="text-right">
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={() => setSelectedRow(row)}
                    >
                      {m.common_edit()}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </div>
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
        onLogout={onLogout}
        onSwitchIde={onSwitchIde}
      />
    </section>
  );
}
