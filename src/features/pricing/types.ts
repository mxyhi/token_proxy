export type PricingSource = {
  url: string;
  commit: string;
  sha256: string;
  commitTime: string;
};

export type ModelPricingProfile = {
  inputNanoUsdPerToken: number | null;
  outputNanoUsdPerToken: number | null;
  cacheReadNanoUsdPerToken: number | null;
  cacheWriteNanoUsdPerToken: number | null;
  cacheWrite5mNanoUsdPerToken: number | null;
  cacheWrite1hNanoUsdPerToken: number | null;
  cacheWrite30mNanoUsdPerToken?: number | null;
  cacheWrite12hNanoUsdPerToken?: number | null;
  imageInputNanoUsdPerToken: number | null;
  imageOutputNanoUsdPerToken: number | null;
};

export type LongContextPricing = {
  threshold: number;
  inputMultiplierScaled: number;
  outputMultiplierScaled: number;
};

export type ModelPricingModel = {
  modelId: string;
  aliases: string[];
  priceMultiplierScaled: number;
  standard: ModelPricingProfile;
  serviceTierProfiles: Record<string, ModelPricingProfile>;
  longContext: LongContextPricing | null;
};

export type ModelPricingSettings = {
  version: string;
  source: PricingSource | null;
  models: ModelPricingModel[];
};

export type ModelPricingSettingsInput = {
  models: ModelPricingModel[];
};

export type ModelPricingSettingsSnapshot = {
  settings: ModelPricingSettings;
  defaultSettings: ModelPricingSettings;
};
