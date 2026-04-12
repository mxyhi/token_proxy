import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

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

const { readDashboardSnapshotMock } = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
}));

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
}));

function renderPanel() {
  return render(
    <I18nProvider>
      <DashboardPanel />
    </I18nProvider>
  );
}

describe("dashboard/DashboardPanel", () => {
  beforeEach(() => {
    readDashboardSnapshotMock.mockReset();
    readDashboardSnapshotMock.mockImplementation(
      async ({ upstreamId, accountId, publicOnly }: DashboardSnapshotQuery) => {
        if (upstreamId === "alpha" && accountId === "codex-a.json") {
          return {
            summary: {
              totalRequests: 1,
              successRequests: 1,
              errorRequests: 0,
              totalTokens: 30,
              inputTokens: 10,
              outputTokens: 20,
              cachedTokens: 5,
              avgLatencyMs: 30,
              medianLatencyMs: 30,
            },
            providers: [
              {
                provider: "openai",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
            ],
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
            accounts: [
              {
                upstreamId: "alpha",
                accountId: "codex-a.json",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
              {
                upstreamId: "alpha",
                accountId: null,
                requests: 1,
                totalTokens: 5,
                cachedTokens: 1,
              },
              {
                upstreamId: "beta",
                accountId: null,
                requests: 1,
                totalTokens: 7,
                cachedTokens: 1,
              },
            ],
            series: [
              {
                tsMs: 100,
                totalRequests: 1,
                errorRequests: 0,
                inputTokens: 10,
                outputTokens: 20,
                cachedTokens: 5,
                totalTokens: 30,
              },
            ],
            recent: [
              {
                id: 1,
                tsMs: 100,
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
            truncated: false,
          };
        }

        if (upstreamId === "alpha" && publicOnly) {
          return {
            summary: {
              totalRequests: 1,
              successRequests: 1,
              errorRequests: 0,
              totalTokens: 30,
              inputTokens: 10,
              outputTokens: 20,
              cachedTokens: 5,
              avgLatencyMs: 30,
              medianLatencyMs: 30,
            },
            providers: [
              {
                provider: "openai",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
            ],
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
            accounts: [
              {
                upstreamId: "alpha",
                accountId: "codex-a.json",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
              {
                upstreamId: "alpha",
                accountId: null,
                requests: 1,
                totalTokens: 5,
                cachedTokens: 1,
              },
              {
                upstreamId: "beta",
                accountId: null,
                requests: 1,
                totalTokens: 7,
                cachedTokens: 1,
              },
            ],
            series: [
              {
                tsMs: 100,
                totalRequests: 1,
                errorRequests: 0,
                inputTokens: 10,
                outputTokens: 20,
                cachedTokens: 5,
                totalTokens: 30,
              },
            ],
            recent: [
              {
                id: 1,
                tsMs: 100,
                path: "/v1/chat/completions",
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
            ],
            truncated: false,
          };
        }

        if (upstreamId === "alpha") {
          return {
            summary: {
              totalRequests: 2,
              successRequests: 2,
              errorRequests: 0,
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
            accounts: [
              {
                upstreamId: "alpha",
                accountId: "codex-a.json",
                requests: 1,
                totalTokens: 30,
                cachedTokens: 5,
              },
              {
                upstreamId: "alpha",
                accountId: null,
                requests: 1,
                totalTokens: 5,
                cachedTokens: 1,
              },
              {
                upstreamId: "beta",
                accountId: null,
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
            truncated: false,
          };
        }

        return {
          summary: {
            totalRequests: 3,
            successRequests: 2,
            errorRequests: 1,
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
          accounts: [
            {
              upstreamId: "alpha",
              accountId: "codex-a.json",
              requests: 1,
              totalTokens: 30,
              cachedTokens: 5,
            },
            {
              upstreamId: "alpha",
              accountId: null,
              requests: 1,
              totalTokens: 5,
              cachedTokens: 1,
            },
            {
              upstreamId: "beta",
              accountId: null,
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
          truncated: false,
        };
      }
    );
  });

  it("defaults to all upstream data and refetches when an upstream and account are selected", async () => {
    const user = userEvent.setup();

    renderPanel();

    await waitFor(() => {
      expect(screen.getByTestId("dashboard-summary-total")).toHaveTextContent(
        "3"
      );
    });
    expect(screen.getByTestId("dashboard-chart-total")).toHaveTextContent("42");
    expect(readDashboardSnapshotMock).toHaveBeenCalledWith(
      {
        range: { fromTsMs: expect.any(Number), toTsMs: expect.any(Number) },
        offset: 0,
        upstreamId: null,
        accountId: null,
        publicOnly: false,
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
        accountId: null,
        publicOnly: false,
      }
    );

    await user.click(
      screen.getByRole("combobox", { name: m.dashboard_account_label() })
    );
    await user.click(
      await screen.findByRole("option", { name: "codex-a.json" })
    );

    await waitFor(() => {
      expect(screen.getByTestId("dashboard-summary-total")).toHaveTextContent(
        "1"
      );
    });
    expect(screen.getByTestId("dashboard-chart-total")).toHaveTextContent("30");
    expect(readDashboardSnapshotMock).toHaveBeenLastCalledWith(
      {
        range: { fromTsMs: expect.any(Number), toTsMs: expect.any(Number) },
        offset: 0,
        upstreamId: "alpha",
        accountId: "codex-a.json",
        publicOnly: false,
      }
    );
  });
});
