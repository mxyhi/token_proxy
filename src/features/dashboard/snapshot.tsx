import { useCallback, useEffect, useRef, useState } from "react"
import { RefreshCcw } from "lucide-react"

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
import type { DashboardRange, DashboardSnapshot } from "@/features/dashboard/types"
import { parseError } from "@/lib/error"
import { cn } from "@/lib/utils"
import { m } from "@/paraglide/messages.js"

export const RECENT_PAGE_SIZE = 50

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

export function useDashboardSnapshot() {
  const [rangePreset, setRangePreset] = useState<DashboardTimeRange>("today")
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null)
  const [activeRange, setActiveRange] = useState<DashboardRange>(() =>
    resolveDashboardRange("today")
  )
  const [status, setStatus] = useState<DashboardStatus>("idle")
  const [statusMessage, setStatusMessage] = useState("")
  const totalRequests = snapshot?.summary.totalRequests ?? 0
  const { page, totalPages, resetPage, onPrevPage, onNextPage } =
    usePagination(totalRequests)
  const requestSeq = useRef(0)

  const loadSnapshot = useCallback(async () => {
    // Ignore out-of-order responses; only the latest request updates state.
    const requestId = requestSeq.current + 1
    requestSeq.current = requestId
    setStatus("loading")
    setStatusMessage("")
    try {
      const range = resolveDashboardRange(rangePreset)
      const offset = (page - 1) * RECENT_PAGE_SIZE
      const data = await readDashboardSnapshot(range, offset)
      if (requestSeq.current !== requestId) {
        return
      }
      setSnapshot(data)
      setActiveRange(range)
      setStatus("idle")
    } catch (error) {
      if (requestSeq.current !== requestId) {
        return
      }
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
    activeRange,
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

export function DashboardFilters({
  range,
  loading,
  onRangeChange,
  onRefresh,
}: DashboardFiltersProps) {
  return (
    <div
      data-slot="dashboard-filters"
      className="sticky top-0 z-20 px-4 lg:px-6"
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
