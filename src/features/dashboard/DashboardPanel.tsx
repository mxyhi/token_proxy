import { useCallback, useEffect, useState } from "react"
import { AlertCircle, RefreshCcw } from "lucide-react"

import { ChartAreaInteractive } from "@/components/chart-area-interactive"
import { DataTable } from "@/components/data-table"
import { SectionCards } from "@/components/section-cards"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { readDashboardSnapshot } from "@/features/dashboard/api"
import {
  DASHBOARD_RANGE_OPTIONS,
  type DashboardTimeRange,
  resolveDashboardRange,
  toDashboardTimeRange,
} from "@/features/dashboard/range"
import type { DashboardSnapshot } from "@/features/dashboard/types"
import { parseError } from "@/lib/error"
import { cn } from "@/lib/utils"
import { m } from "@/paraglide/messages.js"

const RECENT_PAGE_SIZE = 50

type DashboardStatus = "idle" | "loading" | "error"

function usePagination(totalRequests: number) {
  const [page, setPage] = useState(1)
  const totalPages = Math.max(1, Math.ceil(totalRequests / RECENT_PAGE_SIZE))

  useEffect(() => {
    if (page > totalPages) {
      setPage(totalPages)
    }
  }, [page, totalPages])

  const resetPage = useCallback(() => {
    setPage(1)
  }, [])

  const onPrevPage = useCallback(() => {
    setPage((current) => Math.max(1, current - 1))
  }, [])

  const onNextPage = useCallback(() => {
    setPage((current) => Math.min(totalPages, current + 1))
  }, [totalPages])

  return { page, totalPages, totalRequests, resetPage, onPrevPage, onNextPage }
}

function useDashboardSnapshot() {
  const [rangePreset, setRangePreset] = useState<DashboardTimeRange>("30d")
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null)
  const [status, setStatus] = useState<DashboardStatus>("idle")
  const [statusMessage, setStatusMessage] = useState("")
  const totalRequests = snapshot?.summary.totalRequests ?? 0
  const { page, totalPages, resetPage, onPrevPage, onNextPage } =
    usePagination(totalRequests)

  const loadSnapshot = useCallback(async () => {
    setStatus("loading")
    setStatusMessage("")
    try {
      const range = resolveDashboardRange(rangePreset)
      const offset = (page - 1) * RECENT_PAGE_SIZE
      const data = await readDashboardSnapshot(range, offset)
      setSnapshot(data)
      setStatus("idle")
    } catch (error) {
      setStatus("error")
      setStatusMessage(parseError(error))
    }
  }, [page, rangePreset])

  useEffect(() => {
    void loadSnapshot()
  }, [loadSnapshot])

  const handleRangeChange = useCallback((next: DashboardTimeRange) => {
    setRangePreset(next)
    resetPage()
  }, [resetPage])

  return {
    snapshot,
    status,
    statusMessage,
    rangePreset,
    pagination: { page, totalPages, totalRequests },
    refresh: loadSnapshot,
    onRangeChange: handleRangeChange,
    onPrevPage,
    onNextPage,
  }
}

type DashboardFiltersProps = {
  range: DashboardTimeRange
  loading: boolean
  onRangeChange: (range: DashboardTimeRange) => void
  onRefresh: () => void
}

function DashboardFilters({
  range,
  loading,
  onRangeChange,
  onRefresh,
}: DashboardFiltersProps) {
  return (
    <div
      data-slot="dashboard-filters"
      className="sticky top-[var(--header-height)] z-20 px-4 lg:px-6"
    >
      <Card className="gap-0 border-border/60 bg-background/70 py-0">
        <CardContent className="flex flex-wrap items-center justify-between gap-3 py-3">
          <div className="flex flex-wrap items-center gap-2">
            <Label htmlFor="dashboard-range" className="text-xs text-muted-foreground">
              {m.dashboard_range_label()}
            </Label>
            <Select
              value={range}
              onValueChange={(value) => {
                const next = toDashboardTimeRange(value)
                if (next) {
                  onRangeChange(next)
                }
              }}
            >
              <SelectTrigger id="dashboard-range" className="h-9 w-[160px]">
                <SelectValue placeholder={m.dashboard_range_placeholder()} />
              </SelectTrigger>
              <SelectContent>
                {DASHBOARD_RANGE_OPTIONS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label()}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <Button type="button" variant="outline" onClick={onRefresh} disabled={loading}>
            <RefreshCcw className={cn("mr-2 size-4", loading && "animate-spin")} />
            {m.common_refresh()}
          </Button>
        </CardContent>
      </Card>
    </div>
  )
}

export function DashboardPanel() {
  const {
    snapshot,
    status,
    statusMessage,
    rangePreset,
    pagination,
    refresh,
    onRangeChange,
    onPrevPage,
    onNextPage,
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
        loading={isLoading}
        onRangeChange={onRangeChange}
        onRefresh={refresh}
      />

      <SectionCards summary={snapshot?.summary ?? null} />

      <div className="px-4 lg:px-6">
        <ChartAreaInteractive
          series={snapshot?.series ?? []}
          range={rangePreset}
        />
      </div>

      <DataTable
        items={snapshot?.recent ?? []}
        page={pagination.page}
        totalPages={pagination.totalPages}
        totalRequests={pagination.totalRequests}
        pageSize={RECENT_PAGE_SIZE}
        loading={isLoading}
        scrollKey={rangePreset + "-" + pagination.page}
        onPrevPage={onPrevPage}
        onNextPage={onNextPage}
      />
    </div>
  )
}
