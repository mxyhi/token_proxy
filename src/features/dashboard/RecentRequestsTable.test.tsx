import { render, screen } from "@testing-library/react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { RecentRequestsTable } from "@/features/dashboard/RecentRequestsTable";
import { I18nProvider } from "@/lib/i18n";

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
});
