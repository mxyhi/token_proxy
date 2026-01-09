"use client"

import * as React from "react"
import { Area, AreaChart, CartesianGrid, XAxis } from "recharts"

import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart"
import { formatInteger } from "@/features/dashboard/format"
import type { DashboardTimeRange } from "@/features/dashboard/range"
import type { DashboardSeriesPoint } from "@/features/dashboard/types"
import { m } from "@/paraglide/messages.js"

type ChartPoint = {
  tsMs: number
  inputTokens: number
  outputTokens: number
}

const chartConfig = {
  inputTokens: {
    label: "Input",
    color: "var(--chart-1)",
  },
  outputTokens: {
    label: "Output",
    color: "var(--chart-2)",
  },
} satisfies ChartConfig

type ChartAreaInteractiveProps = {
  series: DashboardSeriesPoint[]
  range: DashboardTimeRange
}

type ChartBodyProps = {
  data: ChartPoint[]
  range: DashboardTimeRange
}

function formatTick(value: number, range: DashboardTimeRange) {
  const date = new Date(value)
  if (range === "today") {
    return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
  }
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" })
}

function ChartHeader() {
  return (
    <CardHeader>
      <CardTitle>{m.dashboard_stat_total_tokens()}</CardTitle>
    </CardHeader>
  )
}

function ChartDefs() {
  return (
    <defs>
      <linearGradient id="fillInput" x1="0" y1="0" x2="0" y2="1">
        <stop offset="5%" stopColor="var(--color-inputTokens)" stopOpacity={0.3} />
        <stop offset="95%" stopColor="var(--color-inputTokens)" stopOpacity={0.02} />
      </linearGradient>
      <linearGradient id="fillOutput" x1="0" y1="0" x2="0" y2="1">
        <stop offset="5%" stopColor="var(--color-outputTokens)" stopOpacity={0.28} />
        <stop offset="95%" stopColor="var(--color-outputTokens)" stopOpacity={0.02} />
      </linearGradient>
    </defs>
  )
}

function ChartCanvas({ data, range }: ChartBodyProps) {
  return (
    <ChartContainer config={chartConfig} className="aspect-auto h-[250px] w-full">
      <AreaChart data={data}>
        <ChartDefs />
        <CartesianGrid vertical={false} />
        <XAxis
          dataKey="tsMs"
          tickLine={false}
          axisLine={false}
          tickMargin={8}
          minTickGap={24}
          tickFormatter={(value) => formatTick(Number(value), range)}
        />
        <ChartTooltip
          cursor={false}
          content={
            <ChartTooltipContent
              labelFormatter={(value) => formatTick(Number(value), range)}
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
          }
        />
        <Area
          dataKey="inputTokens"
          type="monotone"
          stroke="var(--color-inputTokens)"
          fill="url(#fillInput)"
          strokeWidth={2}
          dot={false}
        />
        <Area
          dataKey="outputTokens"
          type="monotone"
          stroke="var(--color-outputTokens)"
          fill="url(#fillOutput)"
          strokeWidth={2}
          dot={false}
        />
      </AreaChart>
    </ChartContainer>
  )
}

function ChartBody({ data, range }: ChartBodyProps) {
  return (
    <CardContent className="px-2 pt-4 sm:px-6 sm:pt-6">
      {data.length ? (
        <ChartCanvas data={data} range={range} />
      ) : (
        <div className="flex h-[250px] items-center justify-center text-sm text-muted-foreground">
          {m.dashboard_no_data()}
        </div>
      )}
    </CardContent>
  )
}

export function ChartAreaInteractive({ series, range }: ChartAreaInteractiveProps) {
  const chartData = React.useMemo(
    () =>
      series.map((item) => ({
        tsMs: item.tsMs,
        inputTokens: item.inputTokens,
        outputTokens: item.outputTokens,
      })),
    [series]
  )

  return (
    <Card className="@container/card">
      <ChartHeader />
      <ChartBody data={chartData} range={range} />
    </Card>
  )
}
