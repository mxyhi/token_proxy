import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardAction,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { formatCompact, formatInteger } from "@/features/dashboard/format";
import type { DashboardSummary } from "@/features/dashboard/types";
import { m } from "@/paraglide/messages.js";

type SectionCardsProps = {
  summary: DashboardSummary | null;
};

const PERCENT_FORMAT = new Intl.NumberFormat(undefined, {
  style: "percent",
  maximumFractionDigits: 1,
});

export function SectionCards({ summary }: SectionCardsProps) {
  const totalRequests = summary?.totalRequests ?? 0;
  const successRequests = summary?.successRequests ?? 0;
  const errorRequests = summary?.errorRequests ?? 0;
  const totalTokens = summary?.totalTokens ?? 0;
  const inputTokens = summary?.inputTokens ?? 0;
  const outputTokens = summary?.outputTokens ?? 0;
  const cachedTokens = summary?.cachedTokens ?? 0;
  const avgLatencyMs = summary?.avgLatencyMs ?? 0;
  const medianLatencyMs = summary?.medianLatencyMs ?? 0;

  const successRate = totalRequests > 0 ? successRequests / totalRequests : 0;
  const errorRate = totalRequests > 0 ? errorRequests / totalRequests : 0;

  // 缓存信息已在 Badge 中显示，footer 只展示输入/输出
  const tokensHint = m.dashboard_tokens_hint_no_cache({
    input: formatCompact(inputTokens),
    output: formatCompact(outputTokens),
  });

  return (
    <div className="*:data-[slot=card]:from-primary/5 *:data-[slot=card]:to-card dark:*:data-[slot=card]:bg-card grid grid-cols-1 gap-4 px-4 *:data-[slot=card]:bg-gradient-to-t *:data-[slot=card]:shadow-xs lg:px-6 @xl/main:grid-cols-2 @5xl/main:grid-cols-4">
      <Card className="@container/card">
        <CardHeader>
          <CardDescription>{m.dashboard_stat_requests()}</CardDescription>
          <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
            {formatCompact(totalRequests)}
          </CardTitle>
          <CardAction>
            <Badge variant="outline">
              {PERCENT_FORMAT.format(successRate)}
            </Badge>
          </CardAction>
        </CardHeader>
        <CardFooter className="flex-col items-start gap-1.5 text-sm">
          <div className="line-clamp-1 font-medium">
            {m.dashboard_hint_success_rate({
              rate: PERCENT_FORMAT.format(successRate),
            })}
          </div>
        </CardFooter>
      </Card>

      <Card className="@container/card">
        <CardHeader>
          <CardDescription>{m.dashboard_stat_errors()}</CardDescription>
          <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
            {formatCompact(errorRequests)}
          </CardTitle>
          <CardAction>
            <Badge variant="outline">{PERCENT_FORMAT.format(errorRate)}</Badge>
          </CardAction>
        </CardHeader>
        <CardFooter className="flex-col items-start gap-1.5 text-sm">
          <div className="line-clamp-1 font-medium">
            {m.dashboard_hint_error_rate({
              rate: PERCENT_FORMAT.format(errorRate),
            })}
          </div>
        </CardFooter>
      </Card>

      <Card className="@container/card">
        <CardHeader>
          <CardDescription>{m.dashboard_stat_total_tokens()}</CardDescription>
          <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
            {formatCompact(totalTokens)}
          </CardTitle>
          {cachedTokens ? (
            <CardAction>
              <Badge variant="outline">
                {m.dashboard_cached({ count: formatCompact(cachedTokens) })}
              </Badge>
            </CardAction>
          ) : null}
        </CardHeader>
        <CardFooter className="flex-col items-start gap-1.5 text-sm">
          <div className="line-clamp-1 font-medium">{tokensHint}</div>
        </CardFooter>
      </Card>

      <Card className="@container/card">
        <CardHeader>
          <CardDescription>{m.dashboard_stat_latency_ms()}</CardDescription>
          <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
            {formatInteger(avgLatencyMs)}
          </CardTitle>
        </CardHeader>
        <CardFooter className="flex-col items-start gap-1.5 text-sm">
          <div className="line-clamp-1 font-medium">
            {m.dashboard_latency_hint({
              median: formatInteger(medianLatencyMs),
            })}
          </div>
        </CardFooter>
      </Card>
    </div>
  );
}
