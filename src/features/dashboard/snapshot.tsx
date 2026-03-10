import { useCallback, useEffect, useRef, useState } from "react"
import { HelpCircle, RefreshCcw } from "lucide-react"

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
import { Switch } from "@/components/ui/switch"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import { readDashboardSnapshot } from "@/features/dashboard/api"
import {
  DASHBOARD_RANGE_OPTIONS,
  type DashboardTimeRange,
  resolveDashboardRange,
  toDashboardTimeRange,
} from "@/features/dashboard/range"
import type {
  DashboardRange,
  DashboardSnapshot,
  DashboardUpstreamOption,
} from "@/features/dashboard/types"
import { parseError } from "@/lib/error"
import { cn } from "@/lib/utils"
import { m } from "@/paraglide/messages.js"

export const RECENT_PAGE_SIZE = 50
const ALL_UPSTREAMS_VALUE = "__all_upstreams__"

type DashboardStatus = "idle" | "loading" | "error"

function hasUpstreamOption(
  upstreams: DashboardUpstreamOption[],
  upstreamId: string
) {
  return upstreams.some((item) => item.upstreamId === upstreamId)
}

function usePagination(totalRequests: number) {
  const [page, setPage] = useState(1)
  const totalPages = Math.max(1, Math.ceil(totalRequests / RECENT_PAGE_SIZE))
  const currentPage = Math.min(page, totalPages)

  const resetPage = useCallback(() => {
    setPage(1)
  }, [])

  const onPrevPage = useCallback(() => {
    setPage((current) => Math.max(1, current - 1))
  }, [])

  const onNextPage = useCallback(() => {
    setPage((current) => current + 1)
  }, [])

  return {
    page: currentPage,
    totalPages,
    totalRequests,
    resetPage,
    onPrevPage,
    onNextPage,
  }
}

export function useDashboardSnapshot() {
  const [rangePreset, setRangePreset] = useState<DashboardTimeRange>("today")
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null)
  const [selectedUpstreamId, setSelectedUpstreamId] = useState<string | null>(null)
  const [activeRange, setActiveRange] = useState<DashboardRange>(() =>
    resolveDashboardRange("today")
  )
  const [status, setStatus] = useState<DashboardStatus>("loading")
  const [statusMessage, setStatusMessage] = useState("")
  const totalRequests = snapshot?.summary.totalRequests ?? 0
  const { page, totalPages, resetPage, onPrevPage, onNextPage } =
    usePagination(totalRequests)
  const requestSeq = useRef(0)

  const loadSnapshot = useCallback(async () => {
    // Ignore out-of-order responses; only the latest request updates state.
    const requestId = requestSeq.current + 1
    requestSeq.current = requestId
    try {
      const range = resolveDashboardRange(rangePreset)
      const offset = (page - 1) * RECENT_PAGE_SIZE
      const data = await readDashboardSnapshot({
        range,
        offset,
        upstreamId: selectedUpstreamId,
      })
      if (requestSeq.current !== requestId) {
        return
      }
      // 时间范围变化后，已选上游可能不再出现在该范围内；先回退到“全部”，
      // 让下一个 effect 重新拉取一个合法快照，避免 Select 绑定到不存在的值。
      if (
        selectedUpstreamId !== null &&
        !hasUpstreamOption(data.upstreams, selectedUpstreamId)
      ) {
        setSelectedUpstreamId(null)
        setStatus("loading")
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
  }, [page, rangePreset, selectedUpstreamId])

  useEffect(() => {
    // 提交后一拍再启动请求，避免 effect 同步路径被误判为级联 setState。
    const timerId = window.setTimeout(() => {
      void loadSnapshot()
    }, 0)
    return () => window.clearTimeout(timerId)
  }, [loadSnapshot])

  const markLoading = useCallback(() => {
    setStatus("loading")
    setStatusMessage("")
  }, [])

  const handleRangeChange = useCallback((next: DashboardTimeRange) => {
    markLoading()
    setRangePreset(next)
    resetPage()
  }, [markLoading, resetPage])

  const handleUpstreamChange = useCallback((nextUpstreamId: string | null) => {
    markLoading()
    setSelectedUpstreamId(nextUpstreamId)
    resetPage()
  }, [markLoading, resetPage])

  const handlePrevPage = useCallback(() => {
    markLoading()
    onPrevPage()
  }, [markLoading, onPrevPage])

  const handleNextPage = useCallback(() => {
    markLoading()
    onNextPage()
  }, [markLoading, onNextPage])

  const refresh = useCallback(() => {
    markLoading()
    void loadSnapshot()
  }, [loadSnapshot, markLoading])

  return {
    snapshot,
    status,
    statusMessage,
    activeRange,
    rangePreset,
    selectedUpstreamId,
    upstreamOptions: snapshot?.upstreams ?? [],
    pagination: { page, totalPages, totalRequests },
    refresh,
    onRangeChange: handleRangeChange,
    onUpstreamChange: handleUpstreamChange,
    onPrevPage: handlePrevPage,
    onNextPage: handleNextPage,
  }
}

function resolveUpstreamSelectValue(upstreamId: string | null) {
  return upstreamId ?? ALL_UPSTREAMS_VALUE
}

function toUpstreamFilterValue(value: string) {
  return value === ALL_UPSTREAMS_VALUE ? null : value
}

type DashboardFiltersProps = {
  range: DashboardTimeRange
  upstreamId: string | null
  upstreamOptions: DashboardUpstreamOption[]
  loading: boolean
  onRangeChange: (range: DashboardTimeRange) => void
  onUpstreamChange: (upstreamId: string | null) => void
  onRefresh: () => void
  /** 请求详情捕获相关，仅 LogsPanel 使用 */
  capture?: {
    enabled: boolean
    loading: boolean
    statusText?: string
    onToggle: (enabled: boolean) => void
  }
}

export function DashboardFilters({
  range,
  upstreamId,
  upstreamOptions,
  loading,
  onRangeChange,
  onUpstreamChange,
  onRefresh,
  capture,
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

            <Label htmlFor="dashboard-upstream" className="text-xs text-muted-foreground">
              {m.dashboard_upstream_label()}
            </Label>
            <Select
              value={resolveUpstreamSelectValue(upstreamId)}
              onValueChange={(value) => {
                onUpstreamChange(toUpstreamFilterValue(value))
              }}
            >
              <SelectTrigger id="dashboard-upstream" className="h-9 w-[220px]">
                <SelectValue placeholder={m.dashboard_upstream_placeholder()} />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL_UPSTREAMS_VALUE}>
                  {m.dashboard_upstream_all()}
                </SelectItem>
                {upstreamOptions.map((option) => (
                  <SelectItem key={option.upstreamId} value={option.upstreamId}>
                    {option.upstreamId} · {option.provider}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="flex items-center gap-3">
            {capture ? (
              <div className="flex items-center gap-2">
                <span
                  className={`size-2 rounded-full ${capture.enabled ? "bg-green-500" : "bg-muted-foreground/40"}`}
                  aria-hidden="true"
                />
                <Label htmlFor="logs-capture" className="text-xs text-muted-foreground">
                  {m.logs_capture_title()}
                </Label>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <HelpCircle className="size-3.5 text-muted-foreground cursor-help" />
                  </TooltipTrigger>
                  <TooltipContent side="bottom" className="max-w-xs">
                    {m.logs_capture_desc()}
                  </TooltipContent>
                </Tooltip>
                <Switch
                  id="logs-capture"
                  checked={capture.enabled}
                  disabled={capture.loading}
                  onCheckedChange={capture.onToggle}
                />
                {capture.statusText ? (
                  <span className="text-xs text-muted-foreground tabular-nums">
                    {capture.statusText}
                  </span>
                ) : null}
              </div>
            ) : null}
            <Button type="button" variant="outline" size="icon" onClick={onRefresh} disabled={loading}>
              <RefreshCcw className={cn("size-4", loading && "animate-spin")} />
              <span className="sr-only">{m.common_refresh()}</span>
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  )
}
