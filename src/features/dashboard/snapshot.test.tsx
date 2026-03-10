import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { useDashboardSnapshot } from "@/features/dashboard/snapshot"
import type { DashboardSnapshot } from "@/features/dashboard/types"

const { readDashboardSnapshotMock } = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
}))

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
}))

function createSnapshot(
  overrides: Partial<DashboardSnapshot> = {}
): DashboardSnapshot {
  return {
    summary: {
      totalRequests: 2,
      successRequests: 1,
      errorRequests: 1,
      totalTokens: 37,
      inputTokens: 13,
      outputTokens: 24,
      cachedTokens: 5,
      avgLatencyMs: 60,
      medianLatencyMs: 55,
    },
    providers: [
      { provider: "openai", requests: 1, totalTokens: 30, cachedTokens: 5 },
      { provider: "anthropic", requests: 1, totalTokens: 7, cachedTokens: 0 },
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
        cachedTokens: 0,
      },
    ],
    series: [],
    recent: [],
    truncated: false,
    ...overrides,
  }
}

function HookHarness() {
  const { snapshot, selectedUpstreamId, onUpstreamChange } =
    useDashboardSnapshot()

  return (
    <div>
      <div data-testid="selected-upstream">
        {selectedUpstreamId ?? "all"}
      </div>
      <div data-testid="upstream-options">
        {snapshot?.upstreams
          .map((item) => `${item.upstreamId}:${item.provider}`)
          .join(",") ?? ""}
      </div>
      <button type="button" onClick={() => onUpstreamChange("alpha")}>
        filter-alpha
      </button>
    </div>
  )
}

describe("dashboard/useDashboardSnapshot", () => {
  beforeEach(() => {
    readDashboardSnapshotMock.mockReset()
  })

  afterEach(() => {
    vi.clearAllMocks()
  })

  it("loads all upstreams by default and refetches with the selected upstream", async () => {
    readDashboardSnapshotMock
      .mockResolvedValueOnce(createSnapshot())
      .mockResolvedValueOnce(
        createSnapshot({
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
            { provider: "openai", requests: 1, totalTokens: 30, cachedTokens: 5 },
          ],
          recent: [
            {
              id: 1,
              tsMs: 100,
              path: "/v1/chat/completions",
              provider: "openai",
              upstreamId: "alpha",
              model: "gpt-4.1",
              mappedModel: null,
              stream: false,
              status: 200,
              totalTokens: 30,
              cachedTokens: 5,
              latencyMs: 30,
              upstreamRequestId: "req-alpha",
            },
          ],
        })
      )

    render(<HookHarness />)

    await waitFor(() => {
      expect(readDashboardSnapshotMock).toHaveBeenNthCalledWith(1, {
        range: {
          fromTsMs: expect.any(Number),
          toTsMs: expect.any(Number),
        },
        offset: 0,
        upstreamId: null,
      })
    })

    expect(screen.getByTestId("selected-upstream")).toHaveTextContent("all")
    expect(screen.getByTestId("upstream-options")).toHaveTextContent(
      "alpha:openai,beta:anthropic"
    )

    fireEvent.click(screen.getByRole("button", { name: "filter-alpha" }))

    await waitFor(() => {
      expect(readDashboardSnapshotMock).toHaveBeenNthCalledWith(2, {
        range: {
          fromTsMs: expect.any(Number),
          toTsMs: expect.any(Number),
        },
        offset: 0,
        upstreamId: "alpha",
      })
    })

    expect(screen.getByTestId("selected-upstream")).toHaveTextContent("alpha")
  })
})
