import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { useDashboardSnapshot } from "@/features/dashboard/snapshot"
import type { DashboardSnapshot } from "@/features/dashboard/types"

const { readDashboardSnapshotMock, refreshDashboardModelDiscoveryMock } = vi.hoisted(() => ({
  readDashboardSnapshotMock: vi.fn(),
  refreshDashboardModelDiscoveryMock: vi.fn(),
}))

vi.mock("@/features/dashboard/api", () => ({
  readDashboardSnapshot: readDashboardSnapshotMock,
  refreshDashboardModelDiscovery: refreshDashboardModelDiscoveryMock,
}))

function createSnapshot(
  overrides: Partial<DashboardSnapshot> = {}
): DashboardSnapshot {
  return {
    summary: {
      totalRequests: 2,
      successRequests: 1,
      errorRequests: 1,
      costNanoUsd: 0,
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
        cachedTokens: 0,
      },
    ],
    series: [],
    recent: [],
    modelProbes: [],
    truncated: false,
    ...overrides,
  }
}

function HookHarness() {
  const {
    snapshot,
    selectedUpstreamId,
    onUpstreamChange,
    refresh,
  } =
    useDashboardSnapshot({ refreshModelDiscoveryOnRefresh: true })

  return (
    <div>
      <div data-testid="selected-upstream">
        {selectedUpstreamId ?? "all"}
      </div>
      <div data-testid="upstream-options">
        {snapshot?.upstreams
          .map((item) => item.upstreamId)
          .join(",") ?? ""}
      </div>
      <button type="button" onClick={() => onUpstreamChange("alpha")}>
        filter-alpha
      </button>
      <button type="button" onClick={refresh}>
        refresh-dashboard
      </button>
    </div>
  )
}

describe("dashboard/useDashboardSnapshot", () => {
  beforeEach(() => {
    readDashboardSnapshotMock.mockReset()
    refreshDashboardModelDiscoveryMock.mockReset()
  })

  afterEach(() => {
    cleanup()
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
            costNanoUsd: 0,
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
              clientIp: null,
              path: "/v1/chat/completions",
              provider: "openai",
              upstreamId: "alpha",
              model: "gpt-4.1",
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
        model: null,
      })
    })

    expect(screen.getByTestId("selected-upstream")).toHaveTextContent("all")
    expect(screen.getByTestId("upstream-options")).toHaveTextContent("alpha,beta")

    fireEvent.click(screen.getByRole("button", { name: "filter-alpha" }))

    await waitFor(() => {
      expect(readDashboardSnapshotMock).toHaveBeenNthCalledWith(2, {
        range: {
          fromTsMs: expect.any(Number),
          toTsMs: expect.any(Number),
        },
        offset: 0,
        upstreamId: "alpha",
        model: null,
      })
    })

    expect(screen.getByTestId("selected-upstream")).toHaveTextContent("alpha")
  })

  it("runs model discovery only from dashboard refresh", async () => {
    readDashboardSnapshotMock
      .mockResolvedValueOnce(createSnapshot())
      .mockResolvedValueOnce(createSnapshot())
    refreshDashboardModelDiscoveryMock.mockResolvedValueOnce(undefined)

    render(<HookHarness />)

    await waitFor(() => {
      expect(readDashboardSnapshotMock).toHaveBeenCalledTimes(1)
    })
    expect(refreshDashboardModelDiscoveryMock).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "refresh-dashboard" }))

    await waitFor(() => {
      expect(refreshDashboardModelDiscoveryMock).toHaveBeenCalledTimes(1)
      expect(readDashboardSnapshotMock).toHaveBeenCalledTimes(2)
    })
  })
})
