import { lazy, Suspense } from "react"
import { AlertCircle } from "lucide-react"

import { SectionCards } from "@/features/dashboard/components/section-cards"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  DashboardFilters,
  useDashboardSnapshot,
} from "@/features/dashboard/snapshot"
import { m } from "@/paraglide/messages.js"

const ChartAreaInteractive = lazy(() =>
  import("@/features/dashboard/components/chart-area-interactive").then((module) => ({
    default: module.ChartAreaInteractive,
  }))
)
const ChartModelUsage = lazy(() =>
  import("@/features/dashboard/components/chart-usage-ranking").then((module) => ({
    default: module.ChartModelUsage,
  }))
)
const UpstreamModelProbes = lazy(() =>
  import("@/features/dashboard/components/upstream-model-probes").then((module) => ({
    default: module.UpstreamModelProbes,
  }))
)

function ChartAreaFallback() {
  return (
    <div
      aria-hidden="true"
      className="h-[334px] rounded-lg border border-border/60 bg-muted/20"
    />
  )
}

function ModelUsageFallback() {
  return (
    <div
      aria-hidden="true"
      className="h-[280px] rounded-lg border border-border/60 bg-muted/20"
    />
  )
}

export function DashboardPanel() {
  const {
    snapshot,
    status,
    statusMessage,
    activeRange,
    rangePreset,
    customRange,
    selectedUpstreamId,
    selectedModel,
    upstreamOptions,
    modelOptions,
    refresh,
    onRangeChange,
    onCustomRangeChange,
    onUpstreamChange,
    onModelChange,
  } = useDashboardSnapshot({ refreshModelDiscoveryOnRefresh: true })

  const isLoading = status === "loading"

  return (
    <div className="flex flex-col gap-4">
      {status === "error" ? (
        <Alert variant="destructive" className="mx-4 lg:mx-6">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.dashboard_load_failed()}</AlertTitle>
            <AlertDescription>{statusMessage}</AlertDescription>
          </div>
        </Alert>
      ) : null}

      <DashboardFilters
        range={rangePreset}
        customRange={customRange}
        upstreamId={selectedUpstreamId}
        upstreamOptions={upstreamOptions}
        model={selectedModel}
        modelOptions={modelOptions}
        loading={isLoading}
        onRangeChange={onRangeChange}
        onCustomRangeChange={onCustomRangeChange}
        onUpstreamChange={onUpstreamChange}
        onModelChange={onModelChange}
        onRefresh={refresh}
      />

      <SectionCards summary={snapshot?.summary ?? null} />

      <div className="px-4 lg:px-6">
        <Suspense fallback={<ChartAreaFallback />}>
          <ChartAreaInteractive
            series={snapshot?.series ?? []}
            range={activeRange}
          />
        </Suspense>
      </div>

      <div className="px-4 lg:px-6">
        <Suspense fallback={<ModelUsageFallback />}>
          <ChartModelUsage models={snapshot?.models ?? []} />
        </Suspense>
      </div>

      <Suspense fallback={null}>
        <UpstreamModelProbes probes={snapshot?.modelProbes ?? []} />
      </Suspense>
    </div>
  )
}
