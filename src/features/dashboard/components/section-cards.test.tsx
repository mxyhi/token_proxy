import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { SectionCards } from "@/features/dashboard/components/section-cards";
import type { DashboardSummary } from "@/features/dashboard/types";
import { I18nProvider } from "@/lib/i18n";
import { m } from "@/paraglide/messages.js";

const summary: DashboardSummary = {
  totalRequests: 10,
  successRequests: 8,
  errorRequests: 2,
  costNanoUsd: 1_210_000_000,
  totalTokens: 210_000,
  inputTokens: 200_000,
  outputTokens: 10_000,
  cachedTokens: 20_000,
  avgLatencyMs: 120,
  medianLatencyMs: 80,
};

function renderCards() {
  return render(
    <I18nProvider>
      <SectionCards summary={summary} />
    </I18nProvider>
  );
}

describe("dashboard/SectionCards", () => {
  afterEach(() => {
    cleanup();
  });

  it("merges error count into the request card", () => {
    renderCards();

    expect(screen.getByText(m.dashboard_stat_requests())).toBeInTheDocument();
    expect(screen.queryByText(m.dashboard_stat_errors())).not.toBeInTheDocument();
    expect(
      screen.getByText(m.dashboard_requests_footer({
        success: "8",
        errors: "2",
      }))
    ).toBeInTheDocument();
    expect(screen.getByText("80%")).toBeInTheDocument();
    expect(screen.queryByText("20%")).not.toBeInTheDocument();
  });

  it("renders logged cost as the second stat card", () => {
    renderCards();

    const labels = screen
      .getAllByText((_, element) => element?.getAttribute("data-slot") === "card-description")
      .map((node) => node.textContent);

    expect(labels).toEqual([
      m.dashboard_stat_requests(),
      m.dashboard_stat_cost(),
      m.dashboard_stat_total_tokens(),
      m.dashboard_stat_latency_ms(),
    ]);
    expect(screen.getByText("1.21")).toBeInTheDocument();
    expect(screen.queryByText("$1.21")).not.toBeInTheDocument();
    expect(screen.queryByText("Logged")).not.toBeInTheDocument();
  });
});
