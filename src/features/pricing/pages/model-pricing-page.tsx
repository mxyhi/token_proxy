import { useCallback, useEffect, useState } from "react";
import { Plus, RefreshCw, RotateCcw, Save, Trash2 } from "lucide-react";
import { toast } from "sonner";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { AppShell } from "@/layouts/app-shell";
import {
  readModelPricingSettings,
  resetModelPricingSettings,
  saveModelPricingSettings,
} from "@/features/pricing/api";
import {
  createEmptyPricingRow,
  toPricingRows,
  toPricingSettingsInput,
  type ModelPricingFormRow,
} from "@/features/pricing/form";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type LoadStatus = "loading" | "idle" | "saving";

const TABLE_MIN_WIDTH_CLASS = "min-w-[78rem]";
const STICKY_HEAD_CLASS = "sticky top-0 z-20 bg-background/95 backdrop-blur-xs";
const STICKY_ACTION_CLASS =
  "sticky right-0 z-10 w-[4.5rem] border-l border-border/40 bg-background/95 text-right backdrop-blur-xs group-hover:bg-muted/50";

type PriceInputProps = {
  value: string;
  ariaLabel: string;
  disabled: boolean;
  onChange: (value: string) => void;
};

function PriceInput({ value, ariaLabel, disabled, onChange }: PriceInputProps) {
  return (
    <Input
      value={value}
      inputMode="decimal"
      aria-label={ariaLabel}
      disabled={disabled}
      className="h-8 min-w-[84px] text-right tabular-nums"
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

type ModelPricingRowProps = {
  row: ModelPricingFormRow;
  disabled: boolean;
  onChange: (id: string, patch: Partial<ModelPricingFormRow>) => void;
  onRemove: (id: string) => void;
};

function ModelPricingRow({
  row,
  disabled,
  onChange,
  onRemove,
}: ModelPricingRowProps) {
  const modelLabel = row.modelId.trim() || m.model_pricing_column_model();
  return (
    <TableRow className="group">
      <TableCell className="min-w-[150px]">
        <Input
          value={row.modelId}
          aria-label={m.model_pricing_column_model()}
          disabled={disabled}
          className="h-8"
          onChange={(event) => onChange(row.id, { modelId: event.target.value })}
        />
      </TableCell>
      <TableCell className="min-w-[220px]">
        <Input
          value={row.aliasesText}
          aria-label={m.model_pricing_column_aliases()}
          disabled={disabled}
          placeholder={m.model_pricing_aliases_placeholder()}
          className="h-8"
          onChange={(event) => onChange(row.id, { aliasesText: event.target.value })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.shortInputUsdPerMillion}
          ariaLabel={m.model_pricing_column_short_input()}
          disabled={disabled}
          onChange={(value) => onChange(row.id, { shortInputUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.shortCachedUsdPerMillion}
          ariaLabel={m.model_pricing_column_short_cached()}
          disabled={disabled}
          onChange={(value) => onChange(row.id, { shortCachedUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.shortOutputUsdPerMillion}
          ariaLabel={m.model_pricing_column_short_output()}
          disabled={disabled}
          onChange={(value) => onChange(row.id, { shortOutputUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell>
        <Switch
          checked={row.longEnabled}
          disabled={disabled}
          aria-label={m.model_pricing_column_long_enabled()}
          onCheckedChange={(longEnabled) => onChange(row.id, { longEnabled })}
        />
      </TableCell>
      <TableCell>
        <Input
          value={row.longContextInputTokenThreshold}
          inputMode="numeric"
          aria-label={m.model_pricing_column_long_threshold()}
          disabled={disabled || !row.longEnabled}
          className="h-8 min-w-[96px] text-right tabular-nums"
          onChange={(event) =>
            onChange(row.id, {
              longContextInputTokenThreshold: event.target.value,
            })
          }
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.longInputUsdPerMillion}
          ariaLabel={m.model_pricing_column_long_input()}
          disabled={disabled || !row.longEnabled}
          onChange={(value) => onChange(row.id, { longInputUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.longCachedUsdPerMillion}
          ariaLabel={m.model_pricing_column_long_cached()}
          disabled={disabled || !row.longEnabled}
          onChange={(value) => onChange(row.id, { longCachedUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.longOutputUsdPerMillion}
          ariaLabel={m.model_pricing_column_long_output()}
          disabled={disabled || !row.longEnabled}
          onChange={(value) => onChange(row.id, { longOutputUsdPerMillion: value })}
        />
      </TableCell>
      <TableCell className={STICKY_ACTION_CLASS}>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          disabled={disabled}
          title={m.model_pricing_remove_model({ model: modelLabel })}
          onClick={() => onRemove(row.id)}
        >
          <Trash2 aria-hidden="true" />
          <span className="sr-only">
            {m.model_pricing_remove_model({ model: modelLabel })}
          </span>
        </Button>
      </TableCell>
    </TableRow>
  );
}

export function ModelPricingPage() {
  const [rows, setRows] = useState<ModelPricingFormRow[]>([]);
  const [version, setVersion] = useState("");
  const [status, setStatus] = useState<LoadStatus>("loading");
  const disabled = status === "loading" || status === "saving";
  const canSave = !disabled;

  const applyRows = useCallback((nextRows: ModelPricingFormRow[], nextVersion: string) => {
    setRows(nextRows);
    setVersion(nextVersion);
  }, []);

  const loadSettings = useCallback(async () => {
    setStatus("loading");
    try {
      const snapshot = await readModelPricingSettings();
      applyRows(toPricingRows(snapshot.settings), snapshot.settings.version);
      setStatus("idle");
    } catch (error) {
      setStatus("idle");
      toast.error(`${m.model_pricing_load_failed()}：${parseError(error)}`);
    }
  }, [applyRows]);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  const updateRow = useCallback(
    (id: string, patch: Partial<ModelPricingFormRow>) => {
      setRows((current) =>
        current.map((row) => (row.id === id ? { ...row, ...patch } : row)),
      );
    },
    [],
  );

  const removeRow = useCallback((id: string) => {
    setRows((current) => current.filter((row) => row.id !== id));
  }, []);

  const addRow = useCallback(() => {
    setRows((current) => [...current, createEmptyPricingRow()]);
  }, []);

  const saveRows = useCallback(async () => {
    const next = toPricingSettingsInput(rows);
    if (!next.ok) {
      toast.error(next.message);
      return;
    }
    setStatus("saving");
    try {
      const snapshot = await saveModelPricingSettings(next.input);
      applyRows(toPricingRows(snapshot.settings), snapshot.settings.version);
      setStatus("idle");
      toast.success(m.model_pricing_saved());
    } catch (error) {
      setStatus("idle");
      toast.error(`${m.model_pricing_save_failed()}：${parseError(error)}`);
    }
  }, [applyRows, rows]);

  const resetRows = useCallback(async () => {
    setStatus("saving");
    try {
      const snapshot = await resetModelPricingSettings();
      applyRows(toPricingRows(snapshot.settings), snapshot.settings.version);
      setStatus("idle");
      toast.success(m.model_pricing_reset_done());
    } catch (error) {
      setStatus("idle");
      toast.error(`${m.model_pricing_reset_failed()}：${parseError(error)}`);
    }
  }, [applyRows]);

  return (
    <AppShell title={m.model_pricing_title()}>
      <div
        data-slot="model-pricing-page"
        className="flex min-h-0 flex-1 flex-col px-4 lg:px-6"
      >
        <Card className="min-h-0 flex-1 gap-0 overflow-hidden py-0">
          <CardHeader className="shrink-0 gap-3 border-b border-border/60 bg-card/95 py-4 backdrop-blur-xs">
            <div className="flex min-w-0 flex-col gap-2">
              <CardTitle>{m.model_pricing_title()}</CardTitle>
              <div className="flex flex-wrap items-center gap-2">
                {version ? (
                  <Badge variant="outline">
                    {m.model_pricing_version({ version })}
                  </Badge>
                ) : null}
                <Badge variant="secondary">{m.model_pricing_price_unit()}</Badge>
              </div>
            </div>
            <CardAction className="flex flex-wrap justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                size="icon"
                disabled={disabled}
                title={m.common_refresh()}
                onClick={() => void loadSettings()}
              >
                <RefreshCw
                  className={status === "loading" ? "animate-spin" : undefined}
                  aria-hidden="true"
                />
                <span className="sr-only">{m.common_refresh()}</span>
              </Button>
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    disabled={disabled}
                    title={m.model_pricing_reset()}
                  >
                    <RotateCcw aria-hidden="true" />
                    <span className="sr-only">{m.model_pricing_reset()}</span>
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      {m.model_pricing_reset_confirm_title()}
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      {m.model_pricing_reset_confirm_description()}
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
                    <AlertDialogAction onClick={() => void resetRows()}>
                      {m.model_pricing_reset_confirm_action()}
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
              <Button
                type="button"
                variant="outline"
                onClick={addRow}
                disabled={disabled}
              >
                <Plus aria-hidden="true" />
                {m.model_pricing_add_model()}
              </Button>
              <Button
                type="button"
                onClick={() => void saveRows()}
                disabled={!canSave}
              >
                <Save aria-hidden="true" />
                {m.model_pricing_save()}
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="flex min-h-0 flex-1 flex-col gap-3 overflow-hidden p-0">
            {status === "loading" && rows.length === 0 ? (
              <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
                {m.model_pricing_loading()}
              </div>
            ) : rows.length === 0 ? (
              <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
                {m.model_pricing_empty()}
              </div>
            ) : (
              <div
                data-slot="model-pricing-table-viewport"
                className="min-h-0 flex-1 overflow-auto"
              >
              <Table className={`${TABLE_MIN_WIDTH_CLASS} border-collapse`}>
                <TableHeader className="[&_tr]:border-b-0">
                  <TableRow>
                    <TableHead className={`${STICKY_HEAD_CLASS} w-[10rem]`}>
                      {m.model_pricing_column_model()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} w-[17rem]`}>
                      {m.model_pricing_column_aliases()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_short_input()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_short_cached()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_short_output()}
                    </TableHead>
                    <TableHead className={STICKY_HEAD_CLASS}>
                      {m.model_pricing_column_long_enabled()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_long_threshold()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_long_input()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_long_cached()}
                    </TableHead>
                    <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                      {m.model_pricing_column_long_output()}
                    </TableHead>
                    <TableHead
                      className={`${STICKY_HEAD_CLASS} right-0 z-30 w-[4.5rem] border-l border-border/40 text-right`}
                    >
                      {m.model_pricing_column_actions()}
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.map((row) => (
                    <ModelPricingRow
                      key={row.id}
                      row={row}
                      disabled={disabled}
                      onChange={updateRow}
                      onRemove={removeRow}
                    />
                  ))}
                </TableBody>
              </Table>
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </AppShell>
  );
}
