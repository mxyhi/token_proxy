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
    onSelectItem,
  }: {
    items: Array<{ id: number; upstreamId: string; provider: string; accountId?: string | null }>;
    onSelectItem?: (item: { id: number; upstreamId: string; provider: string; accountId?: string | null }) => void;
  }) => (
    <div data-testid="logs-items">
      {items.map((item) => (
        <button key={item.id} type="button" onClick={() => onSelectItem?.(item)}>
          {[item.upstreamId, item.provider, item.accountId].filter(Boolean).join(" · ")}
        </button>
      ))}
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
  readRequestLogDetailMock,
  readAccountStateLogsMock,
} = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
  readRequestDetailCaptureMock: vi.fn(),
  setRequestDetailCaptureMock: vi.fn(),
  readRequestLogDetailMock: vi.fn(),
  readAccountStateLogsMock: vi.fn(),
}));

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
}));

vi.mock("@/features/logs/api", () => ({
  readRequestDetailCapture: readRequestDetailCaptureMock,
  setRequestDetailCapture: setRequestDetailCaptureMock,
  readRequestLogDetail: readRequestLogDetailMock,
  readAccountStateLogs: readAccountStateLogsMock,
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
    readRequestLogDetailMock.mockReset();
    readAccountStateLogsMock.mockReset();

    readRequestDetailCaptureMock.mockResolvedValue({
      enabled: false,
      expiresAtMs: null,
    });
    setRequestDetailCaptureMock.mockResolvedValue({
      enabled: false,
      expiresAtMs: null,
    });
    readRequestLogDetailMock.mockResolvedValue({
      id: 1,
      tsMs: 100,
      path: "/v1/chat/completions",
      provider: "codex",
      upstreamId: "alpha",
      accountId: "codex-a.json",
      model: "gpt-5",
      mappedModel: null,
      stream: false,
      status: 200,
      inputTokens: 10,
      outputTokens: 20,
      totalTokens: 30,
      cachedTokens: 5,
      latencyMs: 30,
      upstreamRequestId: "req-1",
      usageJson: null,
      requestHeaders: null,
      requestBody: null,
      responseError: null,
    });
    readAccountStateLogsMock.mockResolvedValue([
      {
        id: 11,
        tsMs: 140,
        provider: "codex",
        accountId: "codex-a.json",
        eventKind: "cooldown_cleared",
        triggerKind: "success",
        status: "active",
        reasonDetail: null,
        cooldownUntilMs: null,
      },
      {
        id: 10,
        tsMs: 100,
        provider: "codex",
        accountId: "codex-a.json",
        eventKind: "cooldown_started",
        triggerKind: "http_status",
        status: "cooling_down",
        reasonDetail: "429 retry-after=30",
        cooldownUntilMs: 130,
      },
    ]);

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
                accountId: null,
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
                accountId: null,
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
                accountId: null,
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
      expect(screen.getByTestId("logs-items")).toHaveTextContent("alpha · openai");
      expect(screen.getByTestId("logs-items")).toHaveTextContent("beta · anthropic");
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

  it("shows account id in the provider field inside request detail", async () => {
    const user = userEvent.setup();

    renderPanel();

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "alpha · openai" })).toBeInTheDocument();
    });

    await user.click(screen.getByRole("button", { name: "alpha · openai" }));

    await waitFor(() => {
      expect(readRequestLogDetailMock).toHaveBeenCalledWith(1);
    });

    const providerValues = await screen.findAllByText("alpha · codex · codex-a.json");
    expect(providerValues.length).toBeGreaterThan(0);
  });

  it("shows recent account state logs", async () => {
    renderPanel();

    expect((await screen.findAllByText(m.logs_state_events_title())).length).toBeGreaterThan(0);
    expect((await screen.findAllByText("codex · codex-a.json")).length).toBeGreaterThan(0);
    expect((await screen.findAllByText(m.logs_state_event_kind_cooldown_started())).length).toBeGreaterThan(0);
    expect((await screen.findAllByText("429 retry-after=30")).length).toBeGreaterThan(0);
  });
});
