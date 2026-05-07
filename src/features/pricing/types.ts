export type ModelPricingTier = {
  inputNanoUsdPerToken: number;
  cachedInputNanoUsdPerToken: number;
  outputNanoUsdPerToken: number;
};

export type ModelPricingModel = {
  modelId: string;
  aliases: string[];
  short: ModelPricingTier;
  long: ModelPricingTier | null;
  longContextInputTokenThreshold: number | null;
};

export type ModelPricingSettings = {
  version: string;
  models: ModelPricingModel[];
};

export type ModelPricingSettingsInput = {
  models: ModelPricingModel[];
};

export type ModelPricingSettingsSnapshot = {
  settings: ModelPricingSettings;
  defaultSettings: ModelPricingSettings;
};
