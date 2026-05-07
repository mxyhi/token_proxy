import type {
  ModelPricingModel,
  ModelPricingSettings,
  ModelPricingSettingsInput,
  ModelPricingTier,
} from "@/features/pricing/types";
import { m } from "@/paraglide/messages.js";

const NANO_USD_PER_USD = 1_000_000_000;
const TOKENS_PER_MILLION = 1_000_000;
const NANO_USD_PER_USD_PER_MILLION_TOKEN =
  NANO_USD_PER_USD / TOKENS_PER_MILLION;

let rowCounter = 0;

export type ModelPricingFormRow = {
  id: string;
  modelId: string;
  aliasesText: string;
  shortInputUsdPerMillion: string;
  shortCachedUsdPerMillion: string;
  shortOutputUsdPerMillion: string;
  longEnabled: boolean;
  longInputUsdPerMillion: string;
  longCachedUsdPerMillion: string;
  longOutputUsdPerMillion: string;
  longContextInputTokenThreshold: string;
};

export type PricingFormResult =
  | { ok: true; input: ModelPricingSettingsInput }
  | { ok: false; message: string };

function createRowId() {
  rowCounter += 1;
  return `pricing-row-${Date.now()}-${rowCounter}`;
}

export function createEmptyPricingRow(): ModelPricingFormRow {
  return {
    id: createRowId(),
    modelId: "",
    aliasesText: "",
    shortInputUsdPerMillion: "0.00",
    shortCachedUsdPerMillion: "0.00",
    shortOutputUsdPerMillion: "0.00",
    longEnabled: false,
    longInputUsdPerMillion: "0.00",
    longCachedUsdPerMillion: "0.00",
    longOutputUsdPerMillion: "0.00",
    longContextInputTokenThreshold: "272000",
  };
}

export function toPricingRows(settings: ModelPricingSettings): ModelPricingFormRow[] {
  return settings.models.map((model) => ({
    id: createRowId(),
    modelId: model.modelId,
    aliasesText: model.aliases.join(", "),
    shortInputUsdPerMillion: formatUsdPerMillion(model.short.inputNanoUsdPerToken),
    shortCachedUsdPerMillion: formatUsdPerMillion(model.short.cachedInputNanoUsdPerToken),
    shortOutputUsdPerMillion: formatUsdPerMillion(model.short.outputNanoUsdPerToken),
    longEnabled: model.long !== null,
    longInputUsdPerMillion: formatUsdPerMillion(model.long?.inputNanoUsdPerToken ?? 0),
    longCachedUsdPerMillion: formatUsdPerMillion(
      model.long?.cachedInputNanoUsdPerToken ?? 0,
    ),
    longOutputUsdPerMillion: formatUsdPerMillion(model.long?.outputNanoUsdPerToken ?? 0),
    longContextInputTokenThreshold: String(
      model.longContextInputTokenThreshold ?? 272000,
    ),
  }));
}

export function toPricingSettingsInput(
  rows: readonly ModelPricingFormRow[],
): PricingFormResult {
  if (rows.length === 0) {
    return {
      ok: false,
      message: m.model_pricing_error_at_least_one(),
    };
  }

  const aliases = new Set<string>();
  const models: ModelPricingModel[] = [];
  for (const row of rows) {
    const modelId = row.modelId.trim();
    if (!modelId) {
      return { ok: false, message: m.model_pricing_error_model_required() };
    }
    const normalizedModelId = normalizeAlias(modelId);
    const rowAliases = parseAliases(row.aliasesText).filter(
      (alias) => normalizeAlias(alias) !== normalizedModelId,
    );
    const canonicalAliases = [modelId, ...rowAliases];
    for (const alias of canonicalAliases) {
      const normalized = normalizeAlias(alias);
      if (aliases.has(normalized)) {
        return {
          ok: false,
          message: m.model_pricing_error_duplicate_alias({ alias }),
        };
      }
      aliases.add(normalized);
    }

    const short = parseTier({
      input: row.shortInputUsdPerMillion,
      cached: row.shortCachedUsdPerMillion,
      output: row.shortOutputUsdPerMillion,
      inputLabel: m.model_pricing_column_short_input(),
      cachedLabel: m.model_pricing_column_short_cached(),
      outputLabel: m.model_pricing_column_short_output(),
    });
    if (!short.ok) {
      return short;
    }

    const long = row.longEnabled
      ? parseTier({
          input: row.longInputUsdPerMillion,
          cached: row.longCachedUsdPerMillion,
          output: row.longOutputUsdPerMillion,
          inputLabel: m.model_pricing_column_long_input(),
          cachedLabel: m.model_pricing_column_long_cached(),
          outputLabel: m.model_pricing_column_long_output(),
        })
      : null;
    if (long && !long.ok) {
      return long;
    }

    const threshold = row.longEnabled
      ? parsePositiveInteger(row.longContextInputTokenThreshold)
      : null;
    if (row.longEnabled && threshold === null) {
      return { ok: false, message: m.model_pricing_error_threshold() };
    }

    models.push({
      modelId,
      aliases: rowAliases,
      short: short.tier,
      long: long?.tier ?? null,
      longContextInputTokenThreshold: threshold,
    });
  }

  return { ok: true, input: { models } };
}

function parseTier(args: {
  input: string;
  cached: string;
  output: string;
  inputLabel: string;
  cachedLabel: string;
  outputLabel: string;
}): { ok: true; tier: ModelPricingTier } | { ok: false; message: string } {
  const input = parseUsdPerMillion(args.input);
  if (input === null) {
    return {
      ok: false,
      message: m.model_pricing_error_price_number({ field: args.inputLabel }),
    };
  }
  const cached = parseUsdPerMillion(args.cached);
  if (cached === null) {
    return {
      ok: false,
      message: m.model_pricing_error_price_number({ field: args.cachedLabel }),
    };
  }
  const output = parseUsdPerMillion(args.output);
  if (output === null) {
    return {
      ok: false,
      message: m.model_pricing_error_price_number({ field: args.outputLabel }),
    };
  }

  return {
    ok: true,
    tier: {
      inputNanoUsdPerToken: input,
      cachedInputNanoUsdPerToken: cached,
      outputNanoUsdPerToken: output,
    },
  };
}

function formatUsdPerMillion(value: number) {
  return (value / NANO_USD_PER_USD_PER_MILLION_TOKEN).toFixed(2);
}

function parseUsdPerMillion(value: string) {
  const parsed = Number.parseFloat(value.trim());
  if (!Number.isFinite(parsed) || parsed < 0) {
    return null;
  }
  return Math.round(parsed * NANO_USD_PER_USD_PER_MILLION_TOKEN);
}

function parsePositiveInteger(value: string) {
  if (!/^[1-9]\d*$/.test(value.trim())) {
    return null;
  }
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function parseAliases(value: string) {
  const seen = new Set<string>();
  const aliases: string[] = [];
  for (const item of value.split(/[,\n]/)) {
    const trimmed = item.trim();
    if (!trimmed) {
      continue;
    }
    const normalized = normalizeAlias(trimmed);
    if (seen.has(normalized)) {
      continue;
    }
    seen.add(normalized);
    aliases.push(trimmed);
  }
  return aliases;
}

function normalizeAlias(value: string) {
  return value.trim().toLowerCase().replace(/\s+/g, "-");
}
