import Big from "big.js";

import type {
  LongContextPricing,
  ModelPricingModel,
  ModelPricingProfile,
  ModelPricingSettings,
  ModelPricingSettingsInput,
} from "@/features/pricing/types";
import { m } from "@/paraglide/messages.js";

const NANO_USD_PER_USD_PER_MILLION_TOKEN_BIG = new Big(1_000);
const PRICE_DECIMAL_SCALE = 3;
const PRICE_MULTIPLIER_SCALE = 1_000_000_000_000;
const PRICE_MULTIPLIER_SCALE_BIG = new Big(PRICE_MULTIPLIER_SCALE);
const PRICE_MULTIPLIER_DECIMAL_SCALE = 12;

let rowCounter = 0;

export type ModelPricingProfileForm = {
  input: string;
  output: string;
  cacheRead: string;
  cacheWrite: string;
  cacheWrite5m: string;
  cacheWrite1h: string;
  imageInput: string;
  imageOutput: string;
};

export type LongContextPricingForm = {
  enabled: boolean;
  threshold: string;
  inputMultiplier: string;
  outputMultiplier: string;
};

export type ModelPricingFormRow = {
  id: string;
  modelId: string;
  aliasesText: string;
  priceMultiplier: string;
  standard: ModelPricingProfileForm;
  serviceTierProfiles: Record<string, ModelPricingProfileForm>;
  longContext: LongContextPricingForm;
};

export type PricingFormResult =
  | { ok: true; input: ModelPricingSettingsInput }
  | { ok: false; message: string };

function createRowId() {
  rowCounter += 1;
  return `pricing-row-${Date.now()}-${rowCounter}`;
}

function emptyProfile(): ModelPricingProfileForm {
  return {
    input: "",
    output: "",
    cacheRead: "",
    cacheWrite: "",
    cacheWrite5m: "",
    cacheWrite1h: "",
    imageInput: "",
    imageOutput: "",
  };
}

export function createEmptyPricingRow(): ModelPricingFormRow {
  return {
    id: createRowId(),
    modelId: "",
    aliasesText: "",
    priceMultiplier: "1",
    standard: emptyProfile(),
    serviceTierProfiles: {},
    longContext: {
      enabled: false,
      threshold: "272000",
      inputMultiplier: "2",
      outputMultiplier: "1.5",
    },
  };
}

export function createEmptyProfileForm() {
  return emptyProfile();
}

export function toPricingRows(settings: ModelPricingSettings): ModelPricingFormRow[] {
  return settings.models.map((model) => ({
    id: createRowId(),
    modelId: model.modelId,
    aliasesText: model.aliases.join(", "),
    priceMultiplier: formatScaled(model.priceMultiplierScaled),
    standard: profileToForm(model.standard),
    serviceTierProfiles: Object.fromEntries(
      Object.entries(model.serviceTierProfiles).map(([tier, profile]) => [
        tier,
        profileToForm(profile),
      ]),
    ),
    longContext: {
      enabled: model.longContext !== null,
      threshold: String(model.longContext?.threshold ?? 272000),
      inputMultiplier: formatScaled(
        model.longContext?.inputMultiplierScaled ?? PRICE_MULTIPLIER_SCALE * 2,
      ),
      outputMultiplier: formatScaled(
        model.longContext?.outputMultiplierScaled ??
          (PRICE_MULTIPLIER_SCALE * 3) / 2,
      ),
    },
  }));
}

export function toPricingSettingsInput(
  rows: readonly ModelPricingFormRow[],
): PricingFormResult {
  if (rows.length === 0) {
    return { ok: false, message: m.model_pricing_error_at_least_one() };
  }

  const lookupKeys = new Set<string>();
  const models: ModelPricingModel[] = [];
  for (const row of rows) {
    const modelId = row.modelId.trim();
    if (!modelId) {
      return { ok: false, message: m.model_pricing_error_model_required() };
    }
    const rowLookupKeys = new Set(modelLookupKeys(modelId));
    const aliases: string[] = [];
    for (const alias of parseAliases(row.aliasesText)) {
      const aliasLookupKeys = modelLookupKeys(alias);
      if (aliasLookupKeys.every((lookupKey) => rowLookupKeys.has(lookupKey))) {
        continue;
      }
      aliasLookupKeys.forEach((lookupKey) => rowLookupKeys.add(lookupKey));
      aliases.push(alias);
    }
    for (const lookupKey of rowLookupKeys) {
      if (lookupKeys.has(lookupKey)) {
        return {
          ok: false,
          message: m.model_pricing_error_duplicate_alias({ alias: lookupKey }),
        };
      }
      lookupKeys.add(lookupKey);
    }
    const priceMultiplierScaled = parseScaled(row.priceMultiplier);
    if (priceMultiplierScaled === null) {
      return { ok: false, message: m.model_pricing_error_multiplier() };
    }
    const standard = parseProfile(row.standard);
    if (!standard.ok) {
      return standard;
    }
    const serviceTierProfiles: Record<string, ModelPricingProfile> = {};
    for (const [rawTier, profileForm] of Object.entries(row.serviceTierProfiles)) {
      const tier = rawTier.trim().toLowerCase();
      if (!tier || serviceTierProfiles[tier]) {
        return { ok: false, message: m.model_pricing_error_service_tier() };
      }
      const profile = parseProfile(profileForm);
      if (!profile.ok) {
        return profile;
      }
      serviceTierProfiles[tier] = profile.profile;
    }
    const longContext = parseLongContext(row.longContext);
    if (!longContext.ok) {
      return longContext;
    }
    models.push({
      modelId,
      aliases,
      priceMultiplierScaled,
      standard: standard.profile,
      serviceTierProfiles,
      longContext: longContext.value,
    });
  }
  return { ok: true, input: { models } };
}

function profileToForm(profile: ModelPricingProfile): ModelPricingProfileForm {
  return {
    input: formatPrice(profile.inputNanoUsdPerToken),
    output: formatPrice(profile.outputNanoUsdPerToken),
    cacheRead: formatPrice(profile.cacheReadNanoUsdPerToken),
    cacheWrite: formatPrice(profile.cacheWriteNanoUsdPerToken),
    cacheWrite5m: formatPrice(profile.cacheWrite5mNanoUsdPerToken),
    cacheWrite1h: formatPrice(profile.cacheWrite1hNanoUsdPerToken),
    imageInput: formatPrice(profile.imageInputNanoUsdPerToken),
    imageOutput: formatPrice(profile.imageOutputNanoUsdPerToken),
  };
}

function parseProfile(
  form: ModelPricingProfileForm,
): { ok: true; profile: ModelPricingProfile } | { ok: false; message: string } {
  const values = {
    inputNanoUsdPerToken: parseOptionalPrice(form.input),
    outputNanoUsdPerToken: parseOptionalPrice(form.output),
    cacheReadNanoUsdPerToken: parseOptionalPrice(form.cacheRead),
    cacheWriteNanoUsdPerToken: parseOptionalPrice(form.cacheWrite),
    cacheWrite5mNanoUsdPerToken: parseOptionalPrice(form.cacheWrite5m),
    cacheWrite1hNanoUsdPerToken: parseOptionalPrice(form.cacheWrite1h),
    imageInputNanoUsdPerToken: parseOptionalPrice(form.imageInput),
    imageOutputNanoUsdPerToken: parseOptionalPrice(form.imageOutput),
  };
  if (Object.values(values).some((value) => value === undefined)) {
    return {
      ok: false,
      message: m.model_pricing_error_price_number({
        field: m.model_pricing_advanced(),
      }),
    };
  }
  return { ok: true, profile: values as ModelPricingProfile };
}

function parseLongContext(
  form: LongContextPricingForm,
): { ok: true; value: LongContextPricing | null } | { ok: false; message: string } {
  if (!form.enabled) {
    return { ok: true, value: null };
  }
  const threshold = parsePositiveInteger(form.threshold);
  const inputMultiplierScaled = parseScaled(form.inputMultiplier);
  const outputMultiplierScaled = parseScaled(form.outputMultiplier);
  if (threshold === null || inputMultiplierScaled === null || outputMultiplierScaled === null) {
    return { ok: false, message: m.model_pricing_error_threshold() };
  }
  return {
    ok: true,
    value: { threshold, inputMultiplierScaled, outputMultiplierScaled },
  };
}

function formatPrice(value: number | null) {
  return value === null
    ? ""
    : new Big(value)
        .div(NANO_USD_PER_USD_PER_MILLION_TOKEN_BIG)
        .toFixed(PRICE_DECIMAL_SCALE);
}

function formatScaled(value: number) {
  const formatted = new Big(value)
    .div(PRICE_MULTIPLIER_SCALE_BIG)
    .toFixed(PRICE_MULTIPLIER_DECIMAL_SCALE);
  return formatted.includes(".")
    ? formatted.replace(/0+$/, "").replace(/\.$/, "")
    : formatted;
}

function parseOptionalPrice(value: string): number | null | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  try {
    const parsed = new Big(trimmed);
    if (parsed.lt(0)) {
      return undefined;
    }
    const scaled = parsed
      .times(NANO_USD_PER_USD_PER_MILLION_TOKEN_BIG)
      .round(0, Big.roundHalfUp);
    return scaled.gt(Number.MAX_SAFE_INTEGER) ? undefined : scaled.toNumber();
  } catch {
    return undefined;
  }
}

function parseScaled(value: string) {
  try {
    const parsed = new Big(value.trim());
    if (parsed.lte(0)) {
      return null;
    }
    const scaled = parsed.times(PRICE_MULTIPLIER_SCALE_BIG).round(0, Big.roundHalfUp);
    return scaled.gt(Number.MAX_SAFE_INTEGER) ? null : scaled.toNumber();
  } catch {
    return null;
  }
}

function parsePositiveInteger(value: string) {
  if (!/^[1-9]\d*$/.test(value.trim())) {
    return null;
  }
  const parsed = Number.parseInt(value.trim(), 10);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function parseAliases(value: string) {
  const aliases: string[] = [];
  const seen = new Set<string>();
  for (const item of value.split(/[,\n]/)) {
    const alias = item.trim();
    if (!alias) {
      continue;
    }
    const normalized = normalizeAlias(alias);
    if (seen.has(normalized)) {
      continue;
    }
    seen.add(normalized);
    aliases.push(alias);
  }
  return aliases;
}

function normalizeAlias(value: string) {
  return value.trim().toLowerCase().replace(/\s+/g, "-");
}

function modelLookupKeys(value: string) {
  const normalized = normalizeAlias(value);
  const keys = [canonicalModelLookupKey(normalized)];
  const slashIndex = normalized.indexOf("/");
  if (slashIndex >= 0) {
    keys.push(canonicalModelLookupKey(normalized.slice(slashIndex + 1)));
  }
  return [...new Set(keys.filter(Boolean))];
}

function canonicalModelLookupKey(value: string) {
  if (value === "claude-opus-4.7") {
    return "claude-opus-4-7";
  }
  if (value === "claude-opus-4.8") {
    return "claude-opus-4-8";
  }
  // 官方 ID 是连字符 claude-opus-5-5；点号写法对齐 opus-4.8。
  if (value === "claude-opus-5.5") {
    return "claude-opus-5-5";
  }
  return value;
}
