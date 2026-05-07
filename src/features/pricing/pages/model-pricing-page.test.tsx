import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ModelPricingPage } from "@/features/pricing/pages/model-pricing-page";
import type { ModelPricingSettingsSnapshot } from "@/features/pricing/types";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

const {
  readModelPricingSettingsMock,
  saveModelPricingSettingsMock,
  resetModelPricingSettingsMock,
  toastSuccessMock,
  toastErrorMock,
} = vi.hoisted(() => ({
  readModelPricingSettingsMock: vi.fn(),
  saveModelPricingSettingsMock: vi.fn(),
  resetModelPricingSettingsMock: vi.fn(),
  toastSuccessMock: vi.fn(),
  toastErrorMock: vi.fn(),
}));

vi.mock("@/layouts/app-shell", () => ({
  AppShell: ({ children }: { children: ReactNode }) => <div>{children}</div>,
}));

vi.mock("@/features/pricing/api", () => ({
  readModelPricingSettings: readModelPricingSettingsMock,
  saveModelPricingSettings: saveModelPricingSettingsMock,
  resetModelPricingSettings: resetModelPricingSettingsMock,
}));

vi.mock("sonner", () => ({
  toast: {
    success: toastSuccessMock,
    error: toastErrorMock,
  },
}));

const snapshot: ModelPricingSettingsSnapshot = {
  settings: {
    version: "2026-05-02.openai-openrouter-v1",
    models: [
      {
        modelId: "gpt-5.5",
        aliases: ["gpt-5.5", "openai/gpt-5.5"],
        short: {
          inputNanoUsdPerToken: 5_000,
          cachedInputNanoUsdPerToken: 500,
          outputNanoUsdPerToken: 30_000,
        },
        long: {
          inputNanoUsdPerToken: 10_000,
          cachedInputNanoUsdPerToken: 1_000,
          outputNanoUsdPerToken: 45_000,
        },
        longContextInputTokenThreshold: 272_000,
      },
    ],
  },
  defaultSettings: {
    version: "2026-05-02.openai-openrouter-v1",
    models: [],
  },
};

function renderPage() {
  return render(
    <I18nProvider>
      <ModelPricingPage />
    </I18nProvider>,
  );
}

describe("pricing/ModelPricingPage", () => {
  beforeEach(() => {
    readModelPricingSettingsMock.mockResolvedValue(snapshot);
    saveModelPricingSettingsMock.mockResolvedValue({
      ...snapshot,
      settings: { ...snapshot.settings, version: "custom.1234" },
    });
    resetModelPricingSettingsMock.mockResolvedValue(snapshot);
  });

  afterEach(() => {
    cleanup();
    readModelPricingSettingsMock.mockReset();
    saveModelPricingSettingsMock.mockReset();
    resetModelPricingSettingsMock.mockReset();
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
  });

  it("loads current model pricing into an editable table", async () => {
    const { container } = renderPage();

    expect(await screen.findByDisplayValue("gpt-5.5")).toBeInTheDocument();
    expect(screen.getByDisplayValue("5.00")).toBeInTheDocument();
    expect(screen.getByDisplayValue("0.50")).toBeInTheDocument();
    expect(screen.getByDisplayValue("30.00")).toBeInTheDocument();
    expect(screen.getByDisplayValue("272000")).toBeInTheDocument();
    expect(
      screen.getByText(m.model_pricing_version({ version: snapshot.settings.version })),
    ).toBeInTheDocument();
    expect(
      container.querySelector('[data-slot="model-pricing-table-viewport"]'),
    ).toHaveClass("overflow-auto");
    expect(container.querySelector('[data-slot="model-pricing-page"]')).toHaveClass(
      "flex-1",
      "min-h-0",
    );
    expect(screen.getByText(m.model_pricing_column_actions())).toHaveClass(
      "sticky",
      "right-0",
    );
  });

  it("saves edited model pricing", async () => {
    renderPage();

    const modelInput = await screen.findByDisplayValue("gpt-5.5");
    fireEvent.change(modelInput, { target: { value: "gpt-5.5-custom" } });
    fireEvent.click(screen.getByRole("button", { name: m.model_pricing_save() }));

    await waitFor(() => {
      expect(saveModelPricingSettingsMock).toHaveBeenCalledWith({
        models: [
          expect.objectContaining({
            modelId: "gpt-5.5-custom",
            short: expect.objectContaining({
              inputNanoUsdPerToken: 5_000,
            }),
          }),
        ],
      });
    });
    await waitFor(() => {
      expect(toastSuccessMock).toHaveBeenCalledWith(m.model_pricing_saved());
    });
    expect(screen.queryByText(m.model_pricing_saved())).not.toBeInTheDocument();
  });

  it("confirms before resetting pricing through the backend command", async () => {
    renderPage();

    await screen.findByDisplayValue("gpt-5.5");
    fireEvent.click(screen.getByRole("button", { name: m.model_pricing_reset() }));

    expect(resetModelPricingSettingsMock).not.toHaveBeenCalled();
    expect(screen.getByText(m.model_pricing_reset_confirm_title())).toBeInTheDocument();
    expect(
      screen.getByText(m.model_pricing_reset_confirm_description()),
    ).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole("button", { name: m.model_pricing_reset_confirm_action() }),
    );

    await waitFor(() => {
      expect(resetModelPricingSettingsMock).toHaveBeenCalled();
    });
    await waitFor(() => {
      expect(toastSuccessMock).toHaveBeenCalledWith(m.model_pricing_reset_done());
    });
    expect(screen.queryByText(m.model_pricing_reset_done())).not.toBeInTheDocument();
  });
});
