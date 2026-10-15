import { formatCompact, formatInteger, formatNanoUsdCost } from "@/features/dashboard/format";
import type { UsageTotals } from "../types";
import { m } from "@/paraglide/messages.js";

/** 累计用量单元格：主行 token 缩写，副行成本与请求数；API Key 页与渠道页共用。 */
export function UsageCell({ usage }: { usage: UsageTotals | undefined }) {
  if (!usage) {
    return <span className="text-xs text-muted-foreground">—</span>;
  }
  const cost = `$${formatNanoUsdCost(usage.costNanoUsd)}`;
  const requests = m.api_keys_usage_requests({ count: formatInteger(usage.requests) });
  return (
    <div
      className="flex min-w-0 flex-col gap-0.5"
      title={`${formatInteger(usage.totalTokens)} tokens · ${cost} · ${requests}`}
    >
      <span className="truncate font-medium tabular-nums">{formatCompact(usage.totalTokens)}</span>
      <span className="truncate text-[11px] tabular-nums text-muted-foreground">
        {cost} · {requests}
      </span>
    </div>
  );
}
