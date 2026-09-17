import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DashboardPanel } from "@/features/dashboard/DashboardPanel";
import type { DashboardSnapshotQuery } from "@/features/dashboard/types";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

vi.mock("@/features/dashboard/components/section-cards", () => ({
  SectionCards: ({
    summary,
  }: {
    summary: { totalRequests: number } | null;
  }) => (
    <div data-testid="dashboard-summary-total">
      {String(summary?.totalRequests ?? 0)}
    </div>
  ),
}));

vi.mock("@/features/dashboard/components/chart-area-interactive", () => ({
  ChartAreaInteractive: ({
    series,
  }: {
    series: Array<{ totalTokens: number }>;
  }) => (
    <div data-testid="dashboard-chart-total">
      {String(series.reduce((sum, item) => sum + item.totalTokens, 0))}
    </div>
  ),
}));

vi.mock("@/features/dashboard/components/chart-usage-ranking", () => ({
  ChartModelUsage: ({
    models,
  }: {
    models: Array<{ model: string; totalTokens: number }>;
  }) => (
    <div data-testid="dashboard-model-usage">
      {models.map((item) => item.model).join(",") || "empty"}
    </div>
  ),
}));

const { readDashboardSnapshotMock, refreshDashboardModelDiscoveryMock } = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
  refreshDashboardModelDiscoveryMock: vi.fn(),
}));

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
  refreshDashboardModelDiscovery: refreshDashboardModelDiscoveryMock,
}));

function renderPanel() {
  return render(
    <I18nProvider>
      <DashboardPanel />
    </I18nProvider>
  );
}

describe("dashboard/DashboardPanel", () => {
  afterEach(() => {
    cleanup();
  });

  beforeEach(() => {
    readDashboardSnapshotMock.mockReset();
    refreshDashboardModelDiscoveryMock.mockReset();
    refreshDashboardModelDiscoveryMock.mockResolvedValue(undefined);
    readDashboardSnapshotMock.mockImplementation(
      async ({ upstreamId }: DashboardSnapshotQuery) => {
        if (upstreamId === "alpha") {
          return {
            summary: {
              totalRequests: 2,
              successRequests: 2,
              errorRequests: 0,
              costNanoUsd: 0,
              totalTokens: 35,
              inputTokens: 12,
              outputTokens: 23,
              cachedTokens: 6,
              avgLatencyMs: 35,
              medianLatencyMs: 35,
            },
            providers: [
              {
                provider: "openai",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
              {
                provider: "openai-response",
                requests: 1,
                totalTokens: 5,
                cachedTokens: 1,
              },
            ],
            models: [],
            modelOptions: [],
            upstreams: [
              {
                upstreamId: "alpha",
                requests: 2,
                totalTokens: 35,
                cachedTokens: 6,
              },
              {
                upstreamId: "beta",
                requests: 1,
                totalTokens: 7,
                cachedTokens: 1,
              },
            ],
            series: [
              {
                tsMs: 100,
                totalRequests: 2,
                errorRequests: 0,
                inputTokens: 12,
                outputTokens: 23,
                cachedTokens: 6,
                totalTokens: 35,
              },
            ],
            recent: [
              {
                id: 2,
                tsMs: 110,
                clientIp: null,
                path: "/v1/responses",
                provider: "openai-response",
                upstreamId: "alpha",
                accountId: null,
                model: "gpt-5",
                mappedModel: null,
                stream: false,
                status: 200,
                totalTokens: 5,
                cachedTokens: 1,
                latencyMs: 40,
                upstreamRequestId: null,
              },
              {
                id: 1,
                tsMs: 100,
                clientIp: null,
                path: "/v1/chat/completions",
                provider: "openai",
                upstreamId: "alpha",
                accountId: "codex-a.json",
                model: "gpt-5",
                mappedModel: null,
                stream: false,
                status: 200,
                totalTokens: 30,
                cachedTokens: 5,
                latencyMs: 30,
                upstreamRequestId: null,
              },
            ],
            modelProbes: [],
            truncated: false,
          };
        }

        return {
          summary: {
            totalRequests: 3,
            successRequests: 2,
            errorRequests: 1,
            costNanoUsd: 0,
            totalTokens: 42,
            inputTokens: 15,
            outputTokens: 27,
            cachedTokens: 7,
            avgLatencyMs: 53,
            medianLatencyMs: 40,
          },
          providers: [
            {
              provider: "openai",
              requests: 1,
              totalTokens: 30,
              cachedTokens: 5,
            },
            {
              provider: "anthropic",
              requests: 1,
              totalTokens: 7,
              cachedTokens: 1,
            },
            {
              provider: "openai-response",
              requests: 1,
              totalTokens: 5,
              cachedTokens: 1,
            },
          ],
          models: [
            {
              model: "gpt-5.4",
              requests: 2,
              totalTokens: 400,
              inputTokens: 300,
              outputTokens: 100,
              costNanoUsd: 3000,
              cachedTokens: 0,
            },
            {
              model: "claude-4",
              requests: 1,
              totalTokens: 15,
              inputTokens: 10,
              outputTokens: 5,
              costNanoUsd: 100,
              cachedTokens: 0,
            },
          ],
          modelOptions: ["gpt-5.4", "claude-4"],
          upstreams: [
            {
              upstreamId: "alpha",
              requests: 2,
              totalTokens: 35,
              cachedTokens: 6,
            },
            {
              upstreamId: "beta",
              requests: 1,
              totalTokens: 7,
              cachedTokens: 1,
            },
          ],
          series: [
            {
              tsMs: 100,
              totalRequests: 3,
              errorRequests: 1,
              inputTokens: 15,
              outputTokens: 27,
              cachedTokens: 7,
              totalTokens: 42,
            },
          ],
          recent: [],
          modelProbes: [
            {
              upstreamId: "alpha",
              provider: "openai-response",
              accountId: null,
              status: "ok",
              checkedAtTsMs: 1000,
              error: null,
              models: ["gpt-5.5", "o4-mini", "gpt-5"],
            },
            {
              upstreamId: "beta",
              provider: "gemini",
              accountId: null,
              status: "failed",
              checkedAtTsMs: 2000,
              error: "quota scope denied",
              models: ["gemini-3.0-pro-preview"],
            },
          ],
          truncated: false,
        };
      }
    );
  });

  it("defaults to all channel data and refetches when a channel is selected", async () => {
    const user = userEvent.setup();

    renderPanel();

    await waitFor(() => {
      expect(screen.getByTestId("dashboard-summary-total")).toHaveTextContent(
        "3"
      );
    });
    const chart = await screen.findByTestId("dashboard-chart-total");
    const modelUsage = await screen.findByTestId("dashboard-model-usage");
    const upstreamModelsTitle = await screen.findByText(m.dashboard_upstream_models_title());
    expect(chart).toHaveTextContent("42");
    expect(modelUsage).toHaveTextContent("gpt-5.4,claude-4");
    expect(upstreamModelsTitle).toBeInTheDocument();
    expect(
      chart.compareDocumentPosition(modelUsage) & Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
    expect(
      modelUsage.compareDocumentPosition(upstreamModelsTitle) &
        Node.DOCUMENT_POSITION_FOLLOWING
    ).toBeTruthy();
    expect(screen.getByText("gpt-5.5")).toBeInTheDocument();
    expect(screen.getByText("gemini-3.0-pro-preview")).toBeInTheDocument();
    expect(screen.getByText(/quota scope denied/)).toBeInTheDocument();
    expect(readDashboardSnapshotMock).toHaveBeenCalledWith(
      {
        range: { fromTsMs: expect.any(Number), toTsMs: expect.any(Number) },
        offset: 0,
        upstreamId: null,
        model: null,
      }
    );

    await user.click(
      screen.getByRole("combobox", { name: m.dashboard_upstream_label() })
    );
    await user.click(
      await screen.findByRole("option", { name: "alpha" })
    );

    await waitFor(() => {
      expect(screen.getByTestId("dashboard-summary-total")).toHaveTextContent(
        "2"
      );
    });
    expect(screen.getByTestId("dashboard-chart-total")).toHaveTextContent("35");
    expect(readDashboardSnapshotMock).toHaveBeenLastCalledWith(
      {
        range: { fromTsMs: expect.any(Number), toTsMs: expect.any(Number) },
        offset: 0,
        upstreamId: "alpha",
        model: null,
      }
    );

    // 筛选栏只保留时间、渠道和模型，账户身份仍可展示在日志/探测结果中。
    expect(screen.getAllByRole("combobox")).toHaveLength(3);
    expect(screen.getByRole("combobox", { name: m.dashboard_model_label() })).toBeInTheDocument();
  });

  it("refreshes upstream model discovery before reloading the dashboard", async () => {
    const user = userEvent.setup();

    renderPanel();

    await waitFor(() => {
      expect(screen.getByTestId("dashboard-summary-total")).toHaveTextContent(
        "3"
      );
    });
    expect(refreshDashboardModelDiscoveryMock).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: m.common_refresh() }));

    await waitFor(() => {
      expect(refreshDashboardModelDiscoveryMock).toHaveBeenCalledTimes(1);
      expect(readDashboardSnapshotMock).toHaveBeenCalledTimes(2);
    });
    expect(
      refreshDashboardModelDiscoveryMock.mock.invocationCallOrder[0]
    ).toBeLessThan(readDashboardSnapshotMock.mock.invocationCallOrder[1]);
  });
});
