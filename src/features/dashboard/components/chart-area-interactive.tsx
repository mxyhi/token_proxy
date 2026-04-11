"use client"

import * as React from "react"
import { CartesianGrid, Line, LineChart, XAxis } from "recharts"

import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart"
import {
  createDashboardMinuteFormatter,
  formatDashboardTimestamp,
  formatInteger,
} from "@/features/dashboard/format"
import type { DashboardRange, DashboardSeriesPoint } from "@/features/dashboard/types"
import { useI18n } from "@/lib/i18n"
import { m } from "@/paraglide/messages.js"

type ChartPoint = {
  tsMs: number
  inputTokens: number
  outputTokens: number
  cachedTokens: number
  totalTokens: number
}

const chartConfig = {
  inputTokens: {
    label: m.dashboard_chart_input_tokens(),
    color: "var(--chart-1)",
  },
  outputTokens: {
    label: m.dashboard_chart_output_tokens(),
    color: "var(--chart-2)",
  },
  cachedTokens: {
    label: m.dashboard_chart_cached_tokens(),
    color: "var(--chart-3)",
  },
  totalTokens: {
    label: m.dashboard_chart_total_tokens(),
    color: "var(--chart-4)",
  },
} satisfies ChartConfig

type ChartAreaInteractiveProps = {
  series: DashboardSeriesPoint[]
  range: DashboardRange
}

type ChartBodyProps = {
  data: ChartPoint[]
  timeFormatter: Intl.DateTimeFormat
}

const FALLBACK_RANGE_DAYS = 7
const DAY_MS = 24 * 60 * 60 * 1000

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null
}

function resolveTimestampMs(value: unknown) {
  if (typeof value === "number" && Number.isFinite(value)) {
    return value
  }
  if (typeof value === "string") {
    const numeric = Number(value)
    if (Number.isFinite(numeric)) {
      return numeric
    }
    const parsed = Date.parse(value)
    if (!Number.isNaN(parsed)) {
      return parsed
    }
  }
  return null
}

function resolveTooltipTimestamp(payload: unknown) {
  if (!Array.isArray(payload) || payload.length === 0) {
    return null
  }
  const first = payload[0]
  if (!isRecord(first)) {
    return null
  }
  const inner = first.payload
  if (!isRecord(inner)) {
    return null
  }
  // Recharts tooltip label 可能缺失或被格式化，优先使用原始 payload 的时间戳。
  return resolveTimestampMs(inner.tsMs)
}

function formatTick(value: unknown, formatter: Intl.DateTimeFormat) {
  const tsMs = resolveTimestampMs(value)
  if (tsMs === null) {
    return "—"
  }
  return formatDashboardTimestamp(tsMs, formatter)
}

function resolveRangeBounds(range: DashboardRange) {
  const now = Date.now()
  // range=all 时用最近 7 天生成 0 线的时间范围
  const resolvedEnd = range.toTsMs ?? now
  const resolvedStart =
    range.fromTsMs ?? resolvedEnd - FALLBACK_RANGE_DAYS * DAY_MS
  const start = Math.min(resolvedStart, resolvedEnd)
  const end = Math.max(resolvedStart, resolvedEnd)
  if (end <= start) {
    return { start, end: start + 60 * 1000 }
  }
  return { start, end }
}

function buildZeroSeries(range: DashboardRange) {
  const { start, end } = resolveRangeBounds(range)
  return [
    { tsMs: start, inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0 },
    { tsMs: end, inputTokens: 0, outputTokens: 0, cachedTokens: 0, totalTokens: 0 },
  ]
}

function ChartHeader() {
  return (
    <CardHeader>
      <CardTitle>{m.dashboard_chart_title_usage_trend()}</CardTitle>
    </CardHeader>
  )
}

function ChartCanvas({ data, timeFormatter }: ChartBodyProps) {
  return (
    <ChartContainer config={chartConfig} className="aspect-auto h-[250px] w-full">
      <LineChart data={data}>
        <CartesianGrid vertical={false} />
        <XAxis
          dataKey="tsMs"
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          minTickGap={24}
          tickFormatter={(value) => formatTick(value, timeFormatter)}
        />
        <ChartTooltip
          cursor={false}
          content={(props) => (
            <ChartTooltipContent
              {...props}
              labelFormatter={(value, payload) =>
                formatTick(resolveTooltipTimestamp(payload) ?? value, timeFormatter)
              }
              formatter={(value, name) => {
                const label = chartConfig[name as keyof typeof chartConfig]?.label ?? name
                return (
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="text-muted-foreground">{label}</span>
                    <span className="ml-auto font-medium">{formatInteger(Number(value))}</span>
                  </div>
                )
              }}
              indicator="dot"
            />
          )}
        />
        <ChartLegend content={<ChartLegendContent />} />
        <Line
          dataKey="inputTokens"
          type="monotone"
          stroke="var(--color-inputTokens)"
          strokeWidth={2}
          dot={false}
        />
        <Line
          dataKey="outputTokens"
          type="monotone"
          stroke="var(--color-outputTokens)"
          strokeWidth={2}
          dot={false}
        />
        <Line
          dataKey="cachedTokens"
          type="monotone"
          stroke="var(--color-cachedTokens)"
          strokeDasharray="5 5"
          strokeWidth={2}
          dot={false}
        />
        <Line
          dataKey="totalTokens"
          type="monotone"
          stroke="var(--color-totalTokens)"
          strokeWidth={2}
          dot={false}
        />
      </LineChart>
    </ChartContainer>
  )
}

function ChartBody({ data, timeFormatter }: ChartBodyProps) {
  return (
    <CardContent className="px-2 pt-4 sm:px-6 sm:pt-6">
      <ChartCanvas data={data} timeFormatter={timeFormatter} />
    </CardContent>
  )
}

export function ChartAreaInteractive({ series, range }: ChartAreaInteractiveProps) {
  const { locale } = useI18n()
  const timeFormatter = React.useMemo(
    () => createDashboardMinuteFormatter(locale),
    [locale]
  )
  const chartData = React.useMemo(
    () => {
      if (!series.length) {
        return buildZeroSeries(range)
      }
      return series.map((item) => ({
        tsMs: item.tsMs,
        inputTokens: item.inputTokens,
        outputTokens: item.outputTokens,
        cachedTokens: item.cachedTokens,
        totalTokens: item.totalTokens,
      }))
    },
    [range, series]
  )

  return (
    <Card className="@container/card">
      <ChartHeader />
      <ChartBody data={chartData} timeFormatter={timeFormatter} />
    </Card>
  )
}
