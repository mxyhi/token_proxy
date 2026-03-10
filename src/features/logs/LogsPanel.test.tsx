import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { LogsPanel } from "@/features/logs/LogsPanel";
import type { DashboardSnapshotQuery } from "@/features/dashboard/types";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

vi.mock("@/components/data-table", () => ({
  DataTable: ({
    items,
  }: {
    items: Array<{ upstreamId: string }>;
  }) => (
    <div data-testid="logs-items">
      {items.map((item) => item.upstreamId).join(",")}
    </div>
  ),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn<
    (
      event: string,
      handler: (payload: { payload: { enabled: boolean; expiresAtMs: number | null } }) => void
    ) => Promise<() => void>
  >().mockResolvedValue(() => undefined),
}));

const {
  readDashboardSnapshotMock,
  readRequestDetailCaptureMock,
  setRequestDetailCaptureMock,
} = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
  readRequestDetailCaptureMock: vi.fn(),
  setRequestDetailCaptureMock: vi.fn(),
}));

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
}));

vi.mock("@/features/logs/api", () => ({
  readRequestDetailCapture: readRequestDetailCaptureMock,
  setRequestDetailCapture: setRequestDetailCaptureMock,
  readRequestLogDetail: vi.fn(),
}));

function renderPanel() {
  return render(
    <I18nProvider>
      <LogsPanel />
    </I18nProvider>
  );
}

describe("logs/LogsPanel", () => {
  beforeEach(() => {
    readDashboardSnapshotMock.mockReset();
    readRequestDetailCaptureMock.mockReset();
    setRequestDetailCaptureMock.mockReset();

    readRequestDetailCaptureMock.mockResolvedValue({
      enabled: false,
      expiresAtMs: null,
    });
    setRequestDetailCaptureMock.mockResolvedValue({
      enabled: false,
      expiresAtMs: null,
    });

    readDashboardSnapshotMock.mockImplementation(
      async ({ upstreamId }: DashboardSnapshotQuery) => {
        const base = {
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
          ],
          upstreams: [
            {
              upstreamId: "alpha",
              provider: "openai",
              requests: 1,
              totalTokens: 30,
              cachedTokens: 5,
            },
            {
              upstreamId: "beta",
              provider: "anthropic",
              requests: 1,
              totalTokens: 7,
              cachedTokens: 1,
            },
          ],
          series: [],
          truncated: false,
        };

        if (upstreamId === "alpha") {
          return {
            ...base,
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
            recent: [
              {
                id: 1,
                tsMs: 100,
                path: "/v1/chat/completions",
                provider: "openai",
                upstreamId: "alpha",
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
          };
        }

        return {
          ...base,
          summary: {
            totalRequests: 2,
            successRequests: 1,
            errorRequests: 1,
            totalTokens: 37,
            inputTokens: 13,
            outputTokens: 24,
            cachedTokens: 6,
            avgLatencyMs: 60,
            medianLatencyMs: 60,
          },
          recent: [
            {
              id: 1,
              tsMs: 100,
              path: "/v1/chat/completions",
              provider: "openai",
              upstreamId: "alpha",
              model: "gpt-5",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 30,
              cachedTokens: 5,
              latencyMs: 30,
              upstreamRequestId: null,
            },
            {
              id: 2,
              tsMs: 120,
              path: "/v1/messages",
              provider: "anthropic",
              upstreamId: "beta",
              model: "claude",
              mappedModel: null,
              stream: false,
              status: 500,
              totalTokens: 7,
              cachedTokens: 1,
              latencyMs: 90,
              upstreamRequestId: null,
            },
          ],
        };
      }
    );
  });

  it("shows all upstream logs by default and narrows the table after switching upstream", async () => {
    const user = userEvent.setup();

    renderPanel();

    await waitFor(() => {
      expect(screen.getByTestId("logs-items")).toHaveTextContent("alpha,beta");
    });

    await user.click(
      screen.getByRole("combobox", { name: m.dashboard_upstream_label() })
    );
    await user.click(
      await screen.findByRole("option", { name: "alpha · openai" })
    );

    await waitFor(() => {
      expect(screen.getByTestId("logs-items")).toHaveTextContent("alpha");
    });
    expect(readDashboardSnapshotMock).toHaveBeenLastCalledWith(
      {
        range: { fromTsMs: expect.any(Number), toTsMs: expect.any(Number) },
        offset: 0,
        upstreamId: "alpha",
      }
    );
  });
});
