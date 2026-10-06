import { Ban, Check, Copy, HelpCircle, Pencil, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { toStatusLabel } from "@/features/config/cards/upstreams/constants";
import {
  createDashboardMinuteFormatter,
  formatCompact,
  formatDashboardTimestamp,
  formatInteger,
  formatNanoUsdCost,
} from "@/features/dashboard/format";
import { useI18n } from "@/lib/i18n";
import type { LocalApiKey, LocalApiKeyUsage } from "../types";
import { m } from "@/paraglide/messages.js";

type Props = {
  keys: readonly LocalApiKey[];
  upstreamIds: ReadonlySet<string>;
  usage: ReadonlyMap<string, LocalApiKeyUsage>;
  showKeys: boolean;
  onCopy: (key: LocalApiKey) => void;
  onEdit: (key: LocalApiKey) => void;
  onToggleEnabled: (key: LocalApiKey) => void;
  onDelete: (key: LocalApiKey) => void;
};

const HEADER_CELL_CLASS = "px-3 py-2 text-left text-xs font-medium text-muted-foreground";
const ACTIONS_CELL_CLASS =
  "sticky right-0 w-[8rem] border-l border-border/40 bg-background/95 px-3 py-2";

// 隐藏时保留首尾少量字符，便于区分多个 Key 又不暴露完整值。
function maskKey(value: string) {
  return value.length > 12 ? `${value.slice(0, 3)}••••${value.slice(-4)}` : "••••••••";
}

function ScopeCell({
  scope,
  upstreamIds,
}: {
  scope: LocalApiKey["scope"];
  upstreamIds: ReadonlySet<string>;
}) {
  if (scope.type === "auto") {
    return <Badge variant="outline">Auto</Badge>;
  }
  const expired = scope.upstream_ids.some((id) => !upstreamIds.has(id));
  return (
    <div className="flex min-w-0 items-center gap-1.5" title={scope.upstream_ids.join(", ")}>
      <Badge variant="outline">
        {m.api_keys_selected_count({ count: scope.upstream_ids.length })}
      </Badge>
      {expired ? <Badge variant="destructive">{m.api_keys_expired()}</Badge> : null}
    </div>
  );
}

function UsageCell({ usage }: { usage: LocalApiKeyUsage | undefined }) {
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

export function ApiKeysTable({
  keys,
  upstreamIds,
  usage,
  showKeys,
  onCopy,
  onEdit,
  onToggleEnabled,
  onDelete,
}: Props) {
  const { locale } = useI18n();
  const minuteFormatter = createDashboardMinuteFormatter(locale);
  return (
    <div className="overflow-x-auto rounded-md border border-border/60 bg-background/60">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr className="border-b border-border/60 bg-background/40">
            <th className={`${HEADER_CELL_CLASS} w-[10rem]`}>{m.api_keys_name()}</th>
            <th className={`${HEADER_CELL_CLASS} min-w-[11rem]`}>{m.api_keys_key()}</th>
            <th className={`${HEADER_CELL_CLASS} w-[11rem]`}>{m.api_keys_scope()}</th>
            <th className={`${HEADER_CELL_CLASS} w-[9rem]`} title={m.api_keys_usage_hint()}>
              <span className="inline-flex items-center gap-1">
                {m.api_keys_usage()}
                <HelpCircle className="size-3" aria-hidden="true" />
              </span>
            </th>
            <th className={`${HEADER_CELL_CLASS} w-[8rem]`}>{m.api_keys_last_used()}</th>
            <th className={`${HEADER_CELL_CLASS} w-[5rem]`}>{m.field_status()}</th>
            <th className={`${ACTIONS_CELL_CLASS} z-20 text-right text-xs font-medium text-muted-foreground`}>
              {m.common_actions()}
            </th>
          </tr>
        </thead>
        <tbody>
          {keys.map((key) => (
            <tr key={key.id} className="group border-b border-border/40 last:border-b-0">
              <td className="max-w-[10rem] px-3 py-2">
                <div className="flex h-8 items-center">
                  <span className="truncate font-medium" title={key.name}>
                    {key.name}
                  </span>
                </div>
              </td>
              <td className="px-3 py-2">
                <div className="flex h-8 min-w-0 items-center gap-1">
                  <code className="truncate font-mono text-xs text-muted-foreground">
                    {showKeys ? key.key : maskKey(key.key)}
                  </code>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className="text-muted-foreground"
                    onClick={() => onCopy(key)}
                    aria-label={m.upstreams_row_copy({ rowLabel: key.name })}
                  >
                    <Copy className="size-3.5" aria-hidden="true" />
                  </Button>
                </div>
              </td>
              <td className="px-3 py-2">
                <div className="flex h-8 items-center">
                  <ScopeCell scope={key.scope} upstreamIds={upstreamIds} />
                </div>
              </td>
              <td className="max-w-[9rem] px-3 py-2">
                <div className="flex min-h-8 items-center">
                  <UsageCell usage={usage.get(key.id)} />
                </div>
              </td>
              <td className="px-3 py-2">
                <div className="flex h-8 items-center">
                  <span className="truncate text-xs text-muted-foreground">
                    {usage.has(key.id)
                      ? formatDashboardTimestamp(usage.get(key.id)?.lastUsedMs ?? 0, minuteFormatter)
                      : m.api_keys_never_used()}
                  </span>
                </div>
              </td>
              <td className="px-3 py-2">
                <div className="flex h-8 items-center">
                  <Badge variant={key.enabled ? "default" : "secondary"}>
                    {toStatusLabel(key.enabled)}
                  </Badge>
                </div>
              </td>
              <td className={`${ACTIONS_CELL_CLASS} z-10 backdrop-blur-xs group-hover:bg-muted/50`}>
                <div className="flex justify-end gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => onEdit(key)}
                    aria-label={m.upstreams_row_edit({ rowLabel: key.name })}
                  >
                    <Pencil className="size-4" aria-hidden="true" />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => onToggleEnabled(key)}
                    aria-label={
                      key.enabled
                        ? m.upstreams_row_disable({ rowLabel: key.name })
                        : m.upstreams_row_enable({ rowLabel: key.name })
                    }
                  >
                    {key.enabled ? (
                      <Ban className="size-4 text-muted-foreground" aria-hidden="true" />
                    ) : (
                      <Check className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                    )}
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    onClick={() => onDelete(key)}
                    aria-label={m.upstreams_row_delete({ rowLabel: key.name })}
                  >
                    <Trash2 className="size-4" aria-hidden="true" />
                  </Button>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
