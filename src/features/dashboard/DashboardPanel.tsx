import { AlertCircle } from "lucide-react"

import { ChartAreaInteractive } from "@/features/dashboard/components/chart-area-interactive"
import { SectionCards } from "@/features/dashboard/components/section-cards"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  DashboardFilters,
  useDashboardSnapshot,
} from "@/features/dashboard/snapshot"
import { m } from "@/paraglide/messages.js"

export function DashboardPanel() {
  const {
    snapshot,
    status,
    statusMessage,
    activeRange,
    rangePreset,
    selectedUpstreamId,
    selectedAccountId,
    selectedPublicOnly,
    upstreamOptions,
    accountOptions,
    refresh,
    onRangeChange,
    onUpstreamChange,
    onAccountChange,
  } = useDashboardSnapshot()

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
        upstreamId={selectedUpstreamId}
        upstreamOptions={upstreamOptions}
        accountId={selectedAccountId}
        publicOnly={selectedPublicOnly}
        accountOptions={accountOptions}
        loading={isLoading}
        onRangeChange={onRangeChange}
        onUpstreamChange={onUpstreamChange}
        onAccountChange={onAccountChange}
        onRefresh={refresh}
      />

      <SectionCards summary={snapshot?.summary ?? null} />

      <div className="px-4 lg:px-6">
        <ChartAreaInteractive
          series={snapshot?.series ?? []}
          range={activeRange}
        />
      </div>
    </div>
  )
}
