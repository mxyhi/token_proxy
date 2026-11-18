import { useCallback, useEffect, useState } from "react";
import {
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  SlidersHorizontal,
  Trash2,
} from "lucide-react";
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
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  readModelPricingSettings,
  resetModelPricingSettings,
  saveModelPricingSettings,
} from "@/features/pricing/api";
import {
  createEmptyPricingRow,
  createEmptyProfileForm,
  toPricingRows,
  toPricingSettingsInput,
  type ModelPricingFormRow,
  type ModelPricingProfileForm,
} from "@/features/pricing/form";
import type { ModelPricingSettings } from "@/features/pricing/types";
import { AppShell } from "@/layouts/app-shell";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type LoadStatus = "loading" | "idle" | "saving";

const STICKY_HEAD_CLASS = "sticky top-0 z-20 bg-background/95 backdrop-blur-xs";
const STICKY_ACTION_CLASS =
  "sticky right-0 z-10 w-[6rem] border-l border-border/40 bg-background/95 text-right backdrop-blur-xs group-hover:bg-muted/50";

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

type ProfileEditorProps = {
  profile: ModelPricingProfileForm;
  disabled: boolean;
  onChange: (profile: ModelPricingProfileForm) => void;
};

function ProfileEditor({ profile, disabled, onChange }: ProfileEditorProps) {
  const fields: Array<{
    key: keyof ModelPricingProfileForm;
    label: string;
  }> = [
    { key: "input", label: m.model_pricing_column_standard_input() },
    { key: "cacheRead", label: m.model_pricing_column_cache_read() },
    { key: "cacheWrite", label: m.model_pricing_column_cache_write() },
    { key: "output", label: m.model_pricing_column_standard_output() },
    { key: "cacheWrite5m", label: m.model_pricing_cache_write_5m() },
    { key: "cacheWrite1h", label: m.model_pricing_cache_write_1h() },
    { key: "cacheWrite30m", label: "缓存写入 30 分钟" },
    { key: "cacheWrite12h", label: "缓存写入 12 小时" },
    { key: "imageInput", label: m.model_pricing_image_input() },
    { key: "imageOutput", label: m.model_pricing_image_output() },
  ];
  return (
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
      {fields.map((field) => (
        <div key={field.key} className="grid gap-1.5">
          <Label>{field.label}</Label>
          <PriceInput
            value={profile[field.key]}
            ariaLabel={field.label}
            disabled={disabled}
            onChange={(value) => onChange({ ...profile, [field.key]: value })}
          />
        </div>
      ))}
    </div>
  );
}

type AdvancedPricingDialogProps = {
  row: ModelPricingFormRow;
  disabled: boolean;
  onChange: (patch: Partial<ModelPricingFormRow>) => void;
};

function AdvancedPricingDialog({
  row,
  disabled,
  onChange,
}: AdvancedPricingDialogProps) {
  const tierEntries = Object.entries(row.serviceTierProfiles);

  const addTier = () => {
    const preferredNames = ["priority", "flex"];
    const tier =
      preferredNames.find((name) => !row.serviceTierProfiles[name]) ??
      `tier-${tierEntries.length + 1}`;
    onChange({
      serviceTierProfiles: {
        ...row.serviceTierProfiles,
        [tier]: createEmptyProfileForm(),
      },
    });
  };

  const renameTier = (current: string, next: string) => {
    const profiles = { ...row.serviceTierProfiles };
    const profile = profiles[current];
    delete profiles[current];
    profiles[next] = profile;
    onChange({ serviceTierProfiles: profiles });
  };

  const updateTier = (tier: string, profile: ModelPricingProfileForm) => {
    onChange({
      serviceTierProfiles: { ...row.serviceTierProfiles, [tier]: profile },
    });
  };

  const removeTier = (tier: string) => {
    const profiles = { ...row.serviceTierProfiles };
    delete profiles[tier];
    onChange({ serviceTierProfiles: profiles });
  };

  return (
    <Dialog>
      <DialogTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          disabled={disabled}
          title={m.model_pricing_advanced()}
        >
          <SlidersHorizontal aria-hidden="true" />
          <span className="sr-only">{m.model_pricing_advanced()}</span>
        </Button>
      </DialogTrigger>
      <DialogContent
        className="max-h-[85vh] max-w-5xl overflow-y-auto"
        aria-describedby={undefined}
      >
        <DialogHeader>
          <DialogTitle>
            {row.modelId.trim() || m.model_pricing_advanced()}
          </DialogTitle>
        </DialogHeader>

        <section className="grid gap-3 border-t pt-4">
          <h3 className="text-sm font-medium">{m.model_pricing_advanced()}</h3>
          <ProfileEditor
            profile={row.standard}
            disabled={disabled}
            onChange={(standard) => onChange({ standard })}
          />
        </section>

        <section className="grid gap-3 border-t pt-4">
          <div className="flex items-center justify-between gap-3">
            <h3 className="text-sm font-medium">
              {m.model_pricing_column_long_enabled()}
            </h3>
            <Switch
              checked={row.longContext.enabled}
              disabled={disabled}
              aria-label={m.model_pricing_column_long_enabled()}
              onCheckedChange={(enabled) =>
                onChange({ longContext: { ...row.longContext, enabled } })
              }
            />
          </div>
          <div className="grid gap-3 sm:grid-cols-3">
            <div className="grid gap-1.5">
              <Label>{m.model_pricing_column_long_threshold()}</Label>
              <Input
                value={row.longContext.threshold}
                inputMode="numeric"
                disabled={disabled || !row.longContext.enabled}
                onChange={(event) =>
                  onChange({
                    longContext: {
                      ...row.longContext,
                      threshold: event.target.value,
                    },
                  })
                }
              />
            </div>
            <div className="grid gap-1.5">
              <Label>{m.model_pricing_long_input_multiplier()}</Label>
              <PriceInput
                value={row.longContext.inputMultiplier}
                ariaLabel={m.model_pricing_long_input_multiplier()}
                disabled={disabled || !row.longContext.enabled}
                onChange={(inputMultiplier) =>
                  onChange({
                    longContext: { ...row.longContext, inputMultiplier },
                  })
                }
              />
            </div>
            <div className="grid gap-1.5">
              <Label>{m.model_pricing_long_output_multiplier()}</Label>
              <PriceInput
                value={row.longContext.outputMultiplier}
                ariaLabel={m.model_pricing_long_output_multiplier()}
                disabled={disabled || !row.longContext.enabled}
                onChange={(outputMultiplier) =>
                  onChange({
                    longContext: { ...row.longContext, outputMultiplier },
                  })
                }
              />
            </div>
          </div>
        </section>

        <section className="grid gap-3 border-t pt-4">
          <div className="flex items-center justify-between gap-3">
            <h3 className="text-sm font-medium">
              {m.model_pricing_service_tiers()}
            </h3>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={disabled}
              onClick={addTier}
            >
              <Plus aria-hidden="true" />
              {m.model_pricing_add_service_tier()}
            </Button>
          </div>
          {tierEntries.map(([tier, profile]) => (
            <div key={tier} className="grid gap-3 border-t pt-3">
              <div className="flex items-center gap-2">
                <Input
                  value={tier}
                  disabled={disabled}
                  aria-label={m.model_pricing_service_tier()}
                  className="h-8 max-w-48"
                  onChange={(event) => renameTier(tier, event.target.value)}
                />
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  disabled={disabled}
                  title={m.model_pricing_remove_model({ model: tier })}
                  onClick={() => removeTier(tier)}
                >
                  <Trash2 aria-hidden="true" />
                </Button>
              </div>
              <ProfileEditor
                profile={profile}
                disabled={disabled}
                onChange={(nextProfile) => updateTier(tier, nextProfile)}
              />
            </div>
          ))}
        </section>
      </DialogContent>
    </Dialog>
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
  const updateStandard = (
    field: keyof Pick<
      ModelPricingProfileForm,
      "input" | "cacheRead" | "cacheWrite" | "output"
    >,
    value: string,
  ) => onChange(row.id, { standard: { ...row.standard, [field]: value } });

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
          value={row.priceMultiplier}
          ariaLabel={m.model_pricing_column_multiplier()}
          disabled={disabled}
          onChange={(priceMultiplier) => onChange(row.id, { priceMultiplier })}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.standard.input}
          ariaLabel={m.model_pricing_column_standard_input()}
          disabled={disabled}
          onChange={(value) => updateStandard("input", value)}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.standard.cacheRead}
          ariaLabel={m.model_pricing_column_cache_read()}
          disabled={disabled}
          onChange={(value) => updateStandard("cacheRead", value)}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.standard.cacheWrite}
          ariaLabel={m.model_pricing_column_cache_write()}
          disabled={disabled}
          onChange={(value) => updateStandard("cacheWrite", value)}
        />
      </TableCell>
      <TableCell>
        <PriceInput
          value={row.standard.output}
          ariaLabel={m.model_pricing_column_standard_output()}
          disabled={disabled}
          onChange={(value) => updateStandard("output", value)}
        />
      </TableCell>
      <TableCell className={STICKY_ACTION_CLASS}>
        <div className="flex justify-end gap-1">
          <AdvancedPricingDialog
            row={row}
            disabled={disabled}
            onChange={(patch) => onChange(row.id, patch)}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            disabled={disabled}
            title={m.model_pricing_remove_model({ model: row.modelId })}
            onClick={() => onRemove(row.id)}
          >
            <Trash2 aria-hidden="true" />
          </Button>
        </div>
      </TableCell>
    </TableRow>
  );
}

export function ModelPricingPage() {
  const [rows, setRows] = useState<ModelPricingFormRow[]>([]);
  const [version, setVersion] = useState("");
  const [sourceCommit, setSourceCommit] = useState("");
  const [status, setStatus] = useState<LoadStatus>("loading");
  const disabled = status !== "idle";

  const applySnapshot = useCallback((settings: ModelPricingSettings) => {
      setRows(toPricingRows(settings));
      setVersion(settings.version);
      setSourceCommit(settings.source?.commit.slice(0, 8) ?? "");
  }, []);

  const loadSettings = useCallback(async () => {
    setStatus("loading");
    try {
      const snapshot = await readModelPricingSettings();
      applySnapshot(snapshot.settings);
    } catch (error) {
      toast.error(`${m.model_pricing_load_failed()}：${parseError(error)}`);
    } finally {
      setStatus("idle");
    }
  }, [applySnapshot]);

  useEffect(() => {
    const timerId = window.setTimeout(() => void loadSettings(), 0);
    return () => window.clearTimeout(timerId);
  }, [loadSettings]);

  const updateRow = useCallback(
    (id: string, patch: Partial<ModelPricingFormRow>) => {
      setRows((current) =>
        current.map((row) => (row.id === id ? { ...row, ...patch } : row)),
      );
    },
    [],
  );

  const saveRows = useCallback(async () => {
    const next = toPricingSettingsInput(rows);
    if (!next.ok) {
      toast.error(next.message);
      return;
    }
    setStatus("saving");
    try {
      const snapshot = await saveModelPricingSettings(next.input);
      applySnapshot(snapshot.settings);
      toast.success(m.model_pricing_saved());
    } catch (error) {
      toast.error(`${m.model_pricing_save_failed()}：${parseError(error)}`);
    } finally {
      setStatus("idle");
    }
  }, [applySnapshot, rows]);

  const resetRows = useCallback(async () => {
    setStatus("saving");
    try {
      const snapshot = await resetModelPricingSettings();
      applySnapshot(snapshot.settings);
      toast.success(m.model_pricing_reset_done());
    } catch (error) {
      toast.error(`${m.model_pricing_reset_failed()}：${parseError(error)}`);
    } finally {
      setStatus("idle");
    }
  }, [applySnapshot]);

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
                {sourceCommit ? (
                  <Badge variant="secondary">
                    {m.model_pricing_source_commit({ commit: sourceCommit })}
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
                disabled={disabled}
                onClick={() => setRows((current) => [...current, createEmptyPricingRow()])}
              >
                <Plus aria-hidden="true" />
                {m.model_pricing_add_model()}
              </Button>
              <Button type="button" disabled={disabled} onClick={() => void saveRows()}>
                <Save aria-hidden="true" />
                {m.model_pricing_save()}
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="flex min-h-0 flex-1 flex-col overflow-hidden p-0">
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
                <Table className="min-w-[72rem] border-collapse">
                  <TableHeader className="[&_tr]:border-b-0">
                    <TableRow>
                      <TableHead className={`${STICKY_HEAD_CLASS} w-[10rem]`}>
                        {m.model_pricing_column_model()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} w-[17rem]`}>
                        {m.model_pricing_column_aliases()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                        {m.model_pricing_column_multiplier()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                        {m.model_pricing_column_standard_input()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                        {m.model_pricing_column_cache_read()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                        {m.model_pricing_column_cache_write()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} text-right`}>
                        {m.model_pricing_column_standard_output()}
                      </TableHead>
                      <TableHead className={`${STICKY_HEAD_CLASS} right-0 text-right`}>
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
                        onRemove={(id) =>
                          setRows((current) => current.filter((item) => item.id !== id))
                        }
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
