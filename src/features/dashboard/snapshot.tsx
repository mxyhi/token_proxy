import { useCallback, useEffect, useRef, useState } from "react"
import { HelpCircle, Power, PowerOff, RefreshCcw } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Card, CardContent } from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip"
import { cn } from "@/lib/utils"
import {
  readDashboardSnapshot,
  refreshDashboardModelDiscovery,
} from "@/features/dashboard/api"
import {
  DASHBOARD_RANGE_OPTIONS,
  type DashboardTimeRange,
  datetimeLocalValueToTsMs,
  defaultCustomRange,
  normalizeCustomRange,
  resolveDashboardRange,
  toDashboardTimeRange,
  tsMsToDatetimeLocalValue,
} from "@/features/dashboard/range"
import type {
  DashboardRange,
  DashboardSnapshot,
  DashboardUpstreamOption,
} from "@/features/dashboard/types"
import { parseError } from "@/lib/error"
import { m } from "@/paraglide/messages.js"

export const RECENT_PAGE_SIZE = 50
const ALL_UPSTREAMS_VALUE = "__all_upstreams__"
const ALL_MODELS_VALUE = "__all_models__"

type DashboardStatus = "idle" | "loading" | "error"

type UseDashboardSnapshotOptions = {
  refreshModelDiscoveryOnRefresh?: boolean
}

function hasUpstreamOption(
  upstreams: DashboardUpstreamOption[],
  upstreamId: string
) {
  return upstreams.some((item) => item.upstreamId === upstreamId)
}

function hasModelOption(modelOptions: string[], model: string) {
  return modelOptions.includes(model)
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

export function useDashboardSnapshot({
  refreshModelDiscoveryOnRefresh = false,
}: UseDashboardSnapshotOptions = {}) {
  const [rangePreset, setRangePreset] = useState<DashboardTimeRange>("today")
  const [customRange, setCustomRange] = useState<DashboardRange>(() =>
    defaultCustomRange()
  )
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null)
  const [selectedUpstreamId, setSelectedUpstreamId] = useState<string | null>(null)
  const [selectedModel, setSelectedModel] = useState<string | null>(null)
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
      const range = resolveDashboardRange(rangePreset, customRange)
      const offset = (page - 1) * RECENT_PAGE_SIZE
      const data = await readDashboardSnapshot({
        range,
        offset,
        upstreamId: selectedUpstreamId,
        model: selectedModel,
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
      // 上游/时间变化后模型选项会收窄；已选模型不在列表时回退到全部。
      if (
        selectedModel !== null &&
        !hasModelOption(data.modelOptions, selectedModel)
      ) {
        setSelectedModel(null)
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
  }, [
    customRange,
    page,
    rangePreset,
    selectedModel,
    selectedUpstreamId,
  ])

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
    // 切入自定义时，用当前预设解析结果种子，避免空白区间。
    if (next === "custom") {
      const seed = resolveDashboardRange(rangePreset, customRange)
      if (seed.fromTsMs != null || seed.toTsMs != null) {
        setCustomRange(
          normalizeCustomRange({
            fromTsMs: seed.fromTsMs ?? defaultCustomRange().fromTsMs,
            toTsMs: seed.toTsMs ?? Date.now(),
          })
        )
      } else {
        setCustomRange(defaultCustomRange())
      }
    }
    setRangePreset(next)
    resetPage()
  }, [customRange, markLoading, rangePreset, resetPage])

  const handleCustomRangeChange = useCallback((next: DashboardRange) => {
    markLoading()
    setRangePreset("custom")
    setCustomRange(normalizeCustomRange(next))
    resetPage()
  }, [markLoading, resetPage])

  const handleUpstreamChange = useCallback((nextUpstreamId: string | null) => {
    markLoading()
    setSelectedUpstreamId(nextUpstreamId)
    // 上游切换后模型集合会变，先清模型避免短暂请求到无效组合。
    setSelectedModel(null)
    resetPage()
  }, [markLoading, resetPage])

  const handleModelChange = useCallback((nextModel: string | null) => {
    markLoading()
    setSelectedModel(nextModel)
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
    void (async () => {
      if (refreshModelDiscoveryOnRefresh) {
        try {
          await refreshDashboardModelDiscovery()
        } catch (error) {
          setStatus("error")
          setStatusMessage(parseError(error))
          return
        }
      }
      await loadSnapshot()
    })()
  }, [loadSnapshot, markLoading, refreshModelDiscoveryOnRefresh])

  return {
    snapshot,
    status,
    statusMessage,
    activeRange,
    rangePreset,
    customRange,
    selectedUpstreamId,
    selectedModel,
    upstreamOptions: snapshot?.upstreams ?? [],
    modelOptions: snapshot?.modelOptions ?? [],
    pagination: { page, totalPages, totalRequests },
    refresh,
    onRangeChange: handleRangeChange,
    onCustomRangeChange: handleCustomRangeChange,
    onUpstreamChange: handleUpstreamChange,
    onModelChange: handleModelChange,
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

function resolveModelSelectValue(model: string | null) {
  return model ?? ALL_MODELS_VALUE
}

function toModelFilterValue(value: string) {
  return value === ALL_MODELS_VALUE ? null : value
}

type DashboardFiltersProps = {
  range: DashboardTimeRange
  customRange: DashboardRange
  upstreamId: string | null
  upstreamOptions: DashboardUpstreamOption[]
  model: string | null
  modelOptions: string[]
  loading: boolean
  onRangeChange: (range: DashboardTimeRange) => void
  onCustomRangeChange: (range: DashboardRange) => void
  onUpstreamChange: (upstreamId: string | null) => void
  onModelChange: (model: string | null) => void
  onRefresh: () => void
  /** 请求详情捕获相关，仅 LogsPanel 使用 */
  capture?: {
    enabled: boolean
    loading: boolean
    statusText?: string
    onToggle: (enabled: boolean) => void
  }
}

function resolveDatetimeLocalDisplay(tsMs: number | null) {
  if (tsMs == null) {
    return ""
  }
  return tsMsToDatetimeLocalValue(tsMs)
}

export function DashboardFilters({
  range,
  customRange,
  upstreamId,
  upstreamOptions,
  model,
  modelOptions,
  loading,
  onRangeChange,
  onCustomRangeChange,
  onUpstreamChange,
  onModelChange,
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
              {/* 筛选条空间紧：触发器约原宽度 2/3，下拉仍可随最长项变宽 */}
              <SelectTrigger id="dashboard-range" className="h-9 w-[107px]">
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

            {range === "custom" ? (
              <>
                <Label
                  htmlFor="dashboard-range-from"
                  className="text-xs text-muted-foreground"
                >
                  {m.dashboard_range_from()}
                </Label>
                <Input
                  id="dashboard-range-from"
                  type="datetime-local"
                  className="h-9 w-[190px]"
                  value={resolveDatetimeLocalDisplay(customRange.fromTsMs)}
                  disabled={loading}
                  onChange={(event) => {
                    const fromTsMs = datetimeLocalValueToTsMs(event.target.value)
                    if (fromTsMs == null) {
                      return
                    }
                    onCustomRangeChange({
                      fromTsMs,
                      toTsMs: customRange.toTsMs,
                    })
                  }}
                />
                <Label
                  htmlFor="dashboard-range-to"
                  className="text-xs text-muted-foreground"
                >
                  {m.dashboard_range_to()}
                </Label>
                <Input
                  id="dashboard-range-to"
                  type="datetime-local"
                  className="h-9 w-[190px]"
                  value={resolveDatetimeLocalDisplay(customRange.toTsMs)}
                  disabled={loading}
                  onChange={(event) => {
                    const toTsMs = datetimeLocalValueToTsMs(event.target.value)
                    if (toTsMs == null) {
                      return
                    }
                    onCustomRangeChange({
                      fromTsMs: customRange.fromTsMs,
                      toTsMs,
                    })
                  }}
                />
              </>
            ) : null}

            <Label htmlFor="dashboard-upstream" className="text-xs text-muted-foreground">
              {m.dashboard_upstream_label()}
            </Label>
            <Select
              value={resolveUpstreamSelectValue(upstreamId)}
              onValueChange={(value) => {
                onUpstreamChange(toUpstreamFilterValue(value))
              }}
            >
              <SelectTrigger id="dashboard-upstream" className="h-9 w-[99px]">
                <SelectValue placeholder={m.dashboard_upstream_placeholder()} />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL_UPSTREAMS_VALUE}>
                  {m.dashboard_upstream_all()}
                </SelectItem>
                {upstreamOptions.map((option) => (
                  <SelectItem key={option.upstreamId} value={option.upstreamId}>
                    {option.upstreamId}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <Label htmlFor="dashboard-model" className="text-xs text-muted-foreground">
              {m.dashboard_model_label()}
            </Label>
            <Select
              value={resolveModelSelectValue(model)}
              onValueChange={(value) => {
                onModelChange(toModelFilterValue(value))
              }}
            >
              <SelectTrigger id="dashboard-model" className="h-9 w-[107px]">
                <SelectValue placeholder={m.dashboard_model_placeholder()} />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL_MODELS_VALUE}>
                  {m.dashboard_model_all()}
                </SelectItem>
                {modelOptions.map((option) => (
                  <SelectItem key={option} value={option}>
                    {option}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="flex items-center gap-3">
            {capture ? (
              <div className="flex items-center gap-2">
                <span
                  className={cn(
                    "size-2 rounded-full",
                    capture.enabled ? "bg-green-500" : "bg-muted-foreground/40"
                  )}
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
                    {capture.enabled ? m.logs_capture_desc() : m.logs_capture_idle_desc()}
                  </TooltipContent>
                </Tooltip>
                {capture.enabled ? (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="size-7 text-destructive"
                        onClick={() => capture.onToggle(false)}
                        disabled={capture.loading}
                      >
                        <PowerOff className="size-3.5" />
                        <span className="sr-only">{m.logs_capture_stop()}</span>
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="bottom">{m.logs_capture_stop()}</TooltipContent>
                  </Tooltip>
                ) : (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="size-7 text-green-600"
                        onClick={() => {
                          capture.onToggle(true);
                        }}
                        disabled={capture.loading}
                      >
                        <Power className="size-3.5" />
                        <span className="sr-only">{m.logs_capture_start()}</span>
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="bottom">{m.logs_capture_start()}</TooltipContent>
                  </Tooltip>
                )}
                {capture.enabled && capture.statusText ? (
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
