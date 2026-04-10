import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { RecentRequestsTable } from "@/features/dashboard/RecentRequestsTable";
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
              totalTokens: 30,
              outputTokens: 20,
              cachedTokens: 5,
              latencyMs: 30,
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

    expect(screen.getAllByText("30")[0]).toHaveClass("text-left");

    const table = screen.getByTestId("recent-requests-table");
    const headerGrid = table.firstElementChild;
    expect(headerGrid?.className).not.toContain("1fr");
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
