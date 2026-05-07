import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  readModelPricingSettings,
  resetModelPricingSettings,
  saveModelPricingSettings,
} from "@/features/pricing/api";
import type { ModelPricingSettingsInput } from "@/features/pricing/types";

describe("pricing/api", () => {
  afterEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("reads model pricing settings through Tauri", async () => {
    const invokeMock = vi.mocked(invoke);
    invokeMock.mockResolvedValue({
      settings: { version: "v", models: [] },
      defaultSettings: { version: "v", models: [] },
    });

    await readModelPricingSettings();

    expect(invokeMock).toHaveBeenCalledWith("read_model_pricing_settings");
  });

  it("saves model pricing settings through Tauri", async () => {
    const invokeMock = vi.mocked(invoke);
    const settings: ModelPricingSettingsInput = { models: [] };
    invokeMock.mockResolvedValue({
      settings: { version: "v", models: [] },
      defaultSettings: { version: "v", models: [] },
    });

    await saveModelPricingSettings(settings);

    expect(invokeMock).toHaveBeenCalledWith("save_model_pricing_settings", {
      settings,
    });
  });

  it("resets model pricing settings through Tauri", async () => {
    const invokeMock = vi.mocked(invoke);
    invokeMock.mockResolvedValue({
      settings: { version: "v", models: [] },
      defaultSettings: { version: "v", models: [] },
    });

    await resetModelPricingSettings();

    expect(invokeMock).toHaveBeenCalledWith("reset_model_pricing_settings");
  });
});
