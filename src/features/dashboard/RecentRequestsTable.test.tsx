import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { RecentRequestsTable } from "@/features/dashboard/RecentRequestsTable";
import {
  createDashboardTimeFormatter,
  formatDashboardTimestamp,
} from "@/features/dashboard/format";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";
import { setLocale } from "@/paraglide/runtime.js";

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        start: index * 44,
        size: 44,
        key: String(index),
      })),
    getTotalSize: () => count * 44,
    scrollToOffset: () => undefined,
  }),
}));

describe("dashboard/RecentRequestsTable", () => {
  beforeAll(() => {
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      configurable: true,
      value: () => undefined,
    });
  });

  afterEach(() => {
    cleanup();
    setLocale("en", { reload: false });
  });

  it("shows account id in provider column when request is bound to an account", () => {
    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/responses",
              provider: "codex",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 30,
              outputTokens: 20,
              cachedTokens: 5,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 30,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getByText(/alpha/)).toBeInTheDocument();
    expect(screen.getByText(/codex/)).toBeInTheDocument();
    expect(screen.getByText(/codex-a\.json/)).toBeInTheDocument();
  });

  it("shows only clock time in the time column and keeps the full timestamp in tooltip", async () => {
    const user = userEvent.setup();
    const tsMs = new Date(2026, 4, 2, 15, 28, 43).getTime();
    const fullTimestamp = formatDashboardTimestamp(tsMs, createDashboardTimeFormatter("en"));

    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs,
              path: "/responses",
              provider: "codex",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 30,
              outputTokens: 20,
              cachedTokens: 5,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 30,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getByText("15:28:43")).toBeInTheDocument();
    expect(screen.queryByText(fullTimestamp)).toBeNull();

    await user.hover(screen.getByText("15:28:43"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(fullTimestamp);
  });

  it("keeps status, tokens, and latency columns left-aligned", () => {
    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/responses",
              provider: "codex",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 31,
              outputTokens: 20,
              cachedTokens: 5,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 30,
              upstreamResponseHeadersMs: 12,
              upstreamFirstBodyChunkMs: 18,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getAllByText("Status")[0]?.closest("div")).toHaveClass("text-left");
    expect(screen.getAllByText("Tokens")[0]?.closest("div")).toHaveClass("text-left");
    expect(
      screen.getAllByText((content) => content.includes("(ms)"))[0]?.closest("div")
    ).toHaveClass("text-left");

    expect(screen.getByText("12")).toHaveClass("text-left");

    const table = screen.getByTestId("recent-requests-table");
    const headerGrid = table.firstElementChild;
    expect(headerGrid?.className).not.toContain("1fr");
  });

  it("allows horizontal overflow and lets the body height come from the viewport", () => {
    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/v1/chat/completions/with/a/very/long/path",
              provider: "openai-response",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5.5-with-long-alias",
              mappedModel: "openai/gpt-5.5",
              stream: true,
              status: 200,
              totalTokens: 31,
              outputTokens: 20,
              cachedTokens: 5,
              costNanoUsd: 1_210_000_000,
              pricingVersion: "2026-05-02.openai-openrouter-v1",
              pricingModel: "gpt-5.5",
              pricingContextTier: "short",
              latencyMs: 30,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    const table = screen.getByTestId("recent-requests-table");
    expect(table).toHaveClass("flex", "min-h-0", "flex-1", "overflow-hidden");

    const scrollArea = table.querySelector(
      '[data-slot="recent-requests-table-scroll-area"]',
    );
    expect(scrollArea).toHaveClass("min-h-0", "flex-1", "overflow-auto");

    const widthTrack = table.querySelector(
      '[data-slot="recent-requests-table-width-track"]',
    ) as HTMLElement | null;
    expect(widthTrack?.style.minWidth).toBe("771px");
    expect(widthTrack?.parentElement).toBe(scrollArea);

    const header = table.querySelector('[data-slot="recent-requests-table-header"]');
    expect(header).toHaveClass("sticky", "top-0", "z-10");
    expect(header?.className).toContain("85px_140px");

    const rowsLayer = table.querySelector(
      '[data-slot="recent-requests-table-rows-layer"]',
    ) as HTMLElement | null;
    expect(rowsLayer?.previousElementSibling).toBe(header);
    expect(rowsLayer?.style.height).toBe("44px");
    expect(widthTrack?.style.height).toBe("78px");

    const firstRow = table.querySelector(
      '[data-slot="recent-requests-table-row"]',
    ) as HTMLElement | null;
    expect(firstRow?.style.transform).toBe("translateY(0px)");

    expect(table.querySelector('[data-slot="recent-requests-table-body"]')).toBeNull();
  });

  it("shows upstream response-header latency as the default latency value", async () => {
    const user = userEvent.setup();

    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/responses",
              provider: "codex",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5",
              mappedModel: null,
              stream: true,
              status: 200,
              totalTokens: 31,
              outputTokens: 20,
              cachedTokens: 5,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 30,
              upstreamFirstByteMs: 12,
              upstreamResponseHeadersMs: 8,
              upstreamFirstBodyChunkMs: 12,
              firstClientFlushMs: 18,
              firstOutputMs: 24,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getByText("Upstream response headers (ms)")).toBeInTheDocument();
    expect(screen.getByText("8")).toBeInTheDocument();
    expect(screen.queryByText("30")).toBeNull();

    await user.hover(screen.getByText("8"));
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip).toHaveTextContent(`${m.dashboard_table_latency_ms()}: 30`);
    expect(tooltip).toHaveTextContent(`${m.logs_timing_upstream_response_headers_ms()}: 8`);
    expect(tooltip).toHaveTextContent(`${m.logs_timing_upstream_first_body_chunk_ms()}: 12`);
  });

  it("shows output tokens directly in the tokens column", async () => {
    const user = userEvent.setup();

    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/responses",
              provider: "codex",
              upstreamId: "alpha",
              accountId: "codex-a.json",
              model: "gpt-5",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 45518,
              outputTokens: 1550,
              cachedTokens: 43392,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 30,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getByText("45.5K")).toBeInTheDocument();
    expect(screen.getByText("1.6K · 43.4K")).toBeInTheDocument();
    expect(screen.queryByText((content) => content.includes(m.dashboard_chart_output_tokens()))).toBeNull();
    await user.hover(screen.getByText("45.5K"));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("45.5K");
    expect(await screen.findByRole("tooltip")).toHaveTextContent("1.6K");
    expect(await screen.findByRole("tooltip")).toHaveTextContent("43.4K");
  });

  it("shows logged request cost with pricing metadata", async () => {
    const user = userEvent.setup();

    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/responses",
              provider: "openai-response",
              upstreamId: "alpha",
              accountId: null,
              model: "alias",
              mappedModel: "gpt-5.4",
              stream: false,
              status: 200,
              totalTokens: 1_010_000,
              outputTokens: 10_000,
              cachedTokens: 200_000,
              costNanoUsd: 4_325_000_000,
              pricingVersion: "2026-05-02.openai-openrouter-v1",
              pricingModel: "gpt-5.4",
              pricingContextTier: "long",
              latencyMs: 30,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    expect(screen.getByText(m.dashboard_table_cost())).toBeInTheDocument();
    expect(screen.getByText("4.33")).toBeInTheDocument();
    expect(screen.queryByText("$4.33")).not.toBeInTheDocument();
    expect(screen.queryByText("4.325")).not.toBeInTheDocument();

    await user.hover(screen.getByText("4.33"));
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip).toHaveTextContent(`${m.logs_detail_pricing_model()}: gpt-5.4`);
    expect(tooltip).toHaveTextContent(
      `${m.logs_detail_pricing_context_tier()}: ${m.logs_detail_pricing_context_long()}`,
    );
    expect(tooltip).toHaveTextContent(
      `${m.logs_detail_pricing_version()}: 2026-05-02.openai-openrouter-v1`,
    );
  });

  it("shows local proxy label for proxy local auth failures", async () => {
    setLocale("zh", { reload: false });
    const user = userEvent.setup();

    render(
      <I18nProvider>
        <RecentRequestsTable
          scrollKey="test"
          items={[
            {
              id: 1,
              tsMs: 100,
              path: "/v1/responses",
              provider: "proxy",
              upstreamId: "local",
              accountId: null,
              model: null,
              mappedModel: null,
              stream: false,
              status: 401,
              totalTokens: null,
              outputTokens: null,
              cachedTokens: null,
              costNanoUsd: null,
              pricingVersion: null,
              pricingModel: null,
              pricingContextTier: null,
              latencyMs: 0,
              upstreamRequestId: null,
            },
          ]}
        />
      </I18nProvider>,
    );

    const localProxyLabel = "本地代理";
    expect(screen.getByText(localProxyLabel)).toBeInTheDocument();
    expect(screen.queryByText("local · proxy")).toBeNull();

    await user.hover(screen.getByText(localProxyLabel));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(localProxyLabel);
  });
});
