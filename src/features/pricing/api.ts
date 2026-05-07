import { invoke } from "@tauri-apps/api/core";

import type {
  ModelPricingSettingsInput,
  ModelPricingSettingsSnapshot,
} from "@/features/pricing/types";

export async function readModelPricingSettings() {
  return await invoke<ModelPricingSettingsSnapshot>("read_model_pricing_settings");
}

export async function saveModelPricingSettings(settings: ModelPricingSettingsInput) {
  return await invoke<ModelPricingSettingsSnapshot>("save_model_pricing_settings", {
    settings,
  });
}

export async function resetModelPricingSettings() {
  return await invoke<ModelPricingSettingsSnapshot>("reset_model_pricing_settings");
}
