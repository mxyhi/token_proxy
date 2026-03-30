import type { ReactElement } from "react";

import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { Ban, Check, Columns3, Copy, Eye, EyeOff, Pencil, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import {
  getUpstreamLabel,
  toMaskedApiKey,
  toMaskedProxyUrl,
  toStatusLabel,
} from "@/features/config/cards/upstreams/constants";
import type { UpstreamColumnDefinition, UpstreamColumnId } from "@/features/config/cards/upstreams/types";
import {
  UPSTREAM_DISPATCH_STRATEGIES,
  UPSTREAM_ORDER_STRATEGIES,
  type ConfigForm,
  type UpstreamDispatchType,
  type UpstreamForm,
  type UpstreamOrderStrategy,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsToolbarProps = {
  apiKeyVisible: boolean;
  showApiKeys: boolean;
  strategy: ConfigForm["upstreamStrategy"];
  onToggleApiKeys: () => void;
  onStrategyChange: (value: ConfigForm["upstreamStrategy"]) => void;
  onAddClick: () => void;
  onColumnsClick: () => void;
};

const UPSTREAM_ORDER_VALUES: ReadonlySet<string> = new Set(
  UPSTREAM_ORDER_STRATEGIES.map((strategy) => strategy.value)
);
const UPSTREAM_DISPATCH_VALUES: ReadonlySet<string> = new Set(
  UPSTREAM_DISPATCH_STRATEGIES.map((strategy) => strategy.value)
);
const CELL_PLACEHOLDER = "—";
const TOOLTIP_CONTENT_CLASS = "max-w-[560px] whitespace-pre-wrap break-words";

function toUpstreamOrderStrategy(value: string): UpstreamOrderStrategy | null {
  return UPSTREAM_ORDER_VALUES.has(value) ? (value as UpstreamOrderStrategy) : null;
}

function toUpstreamDispatchType(value: string): UpstreamDispatchType | null {
  return UPSTREAM_DISPATCH_VALUES.has(value) ? (value as UpstreamDispatchType) : null;
}

type CellTooltipProps = {
  content: string;
  disabled?: boolean;
  children: ReactElement;
};

function shouldDisableTooltip(content: string) {
  const trimmed = content.trim();
  return trimmed.length === 0 || trimmed === CELL_PLACEHOLDER;
}

function CellTooltip({ content, disabled, children }: CellTooltipProps) {
  if (disabled || shouldDisableTooltip(content)) {
    return children;
  }
  return (
    <TooltipPrimitive.Root>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side="top" className={TOOLTIP_CONTENT_CLASS}>
        {content}
      </TooltipContent>
    </TooltipPrimitive.Root>
  );
}

export function UpstreamsToolbar({
  apiKeyVisible,
  showApiKeys,
  strategy,
  onToggleApiKeys,
  onStrategyChange,
  onAddClick,
  onColumnsClick,
}: UpstreamsToolbarProps) {
  const updateStrategy = (patch: Partial<ConfigForm["upstreamStrategy"]>) => {
    onStrategyChange({
      ...strategy,
      ...patch,
    });
  };
  const showsHedgeDelay = strategy.dispatchType === "hedged";
  const showsMaxParallel = strategy.dispatchType !== "serial";

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" variant="outline" onClick={onAddClick}>
            {m.upstreams_add()}
          </Button>
          <Button type="button" variant="outline" onClick={onColumnsClick}>
            <Columns3 className="size-4" aria-hidden="true" />
            {m.common_columns()}
          </Button>
        </div>
        {apiKeyVisible ? (
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={onToggleApiKeys}
            aria-label={showApiKeys ? m.upstreams_hide_api_keys() : m.upstreams_show_api_keys()}
          >
            {showApiKeys ? (
              <EyeOff className="size-4" aria-hidden="true" />
            ) : (
              <Eye className="size-4" aria-hidden="true" />
            )}
          </Button>
        ) : null}
      </div>
      <div className="flex flex-wrap items-end gap-2">
        <div className="grid gap-1">
          <Label htmlFor="upstreams-order" className="text-xs text-muted-foreground">
            {m.upstream_strategy_order_label()}
          </Label>
          <Select
            value={strategy.order}
            onValueChange={(value) => {
              const nextOrder = toUpstreamOrderStrategy(value);
              if (nextOrder) {
                updateStrategy({ order: nextOrder });
              }
            }}
          >
            <SelectTrigger id="upstreams-order" className="min-w-[180px]">
              <SelectValue placeholder={m.upstream_strategy_order_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {UPSTREAM_ORDER_STRATEGIES.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1">
          <Label htmlFor="upstreams-dispatch" className="text-xs text-muted-foreground">
            {m.upstream_strategy_dispatch_label()}
          </Label>
          <Select
            value={strategy.dispatchType}
            onValueChange={(value) => {
              const nextDispatchType = toUpstreamDispatchType(value);
              if (nextDispatchType) {
                updateStrategy({ dispatchType: nextDispatchType });
              }
            }}
          >
            <SelectTrigger id="upstreams-dispatch" className="min-w-[180px]">
              <SelectValue placeholder={m.upstream_strategy_dispatch_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {UPSTREAM_DISPATCH_STRATEGIES.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {showsHedgeDelay ? (
          <div className="grid gap-1">
            <Label htmlFor="upstreams-hedge-delay" className="text-xs text-muted-foreground">
              {m.upstream_strategy_delay_ms_label()}
            </Label>
            <Input
              id="upstreams-hedge-delay"
              value={strategy.hedgeDelayMs}
              onChange={(event) => updateStrategy({ hedgeDelayMs: event.target.value })}
              placeholder="2000"
              inputMode="numeric"
              className="w-[120px]"
            />
          </div>
        ) : null}
        {showsMaxParallel ? (
          <div className="grid gap-1">
            <Label htmlFor="upstreams-max-parallel" className="text-xs text-muted-foreground">
              {m.upstream_strategy_max_parallel_label()}
            </Label>
            <Input
              id="upstreams-max-parallel"
              value={strategy.maxParallel}
              onChange={(event) => updateStrategy({ maxParallel: event.target.value })}
              placeholder="2"
              inputMode="numeric"
              className="w-[96px]"
            />
          </div>
        ) : null}
      </div>
      <p className="text-xs text-muted-foreground">{m.upstream_strategy_help()}</p>
    </div>
  );
}

type UpstreamsTableHeaderProps = {
  columns: readonly UpstreamColumnDefinition[];
};

function UpstreamsTableHeader({ columns }: UpstreamsTableHeaderProps) {
  return (
    <thead>
      <tr className="border-b border-border/60 bg-background/40">
        {columns.map((column) => (
          <th
            key={column.id}
            className={[
              "px-3 py-2 text-left text-xs font-medium text-muted-foreground",
              column.headerClassName,
            ]
              .filter(Boolean)
              .join(" ")}
          >
            {column.label()}
          </th>
        ))}
        <th className="sticky right-0 z-20 w-[9rem] border-l border-border/40 bg-background/95 px-3 py-2 text-right text-xs font-medium text-muted-foreground">
          {m.common_actions()}
        </th>
      </tr>
    </thead>
  );
}

function renderTextCell(value: string, placeholder: string) {
  const trimmed = value.trim();
  return (
    <CellTooltip content={trimmed} disabled={!trimmed}>
      <span className={trimmed ? "block w-full truncate text-foreground" : "block w-full truncate text-muted-foreground"}>
        {trimmed || placeholder}
      </span>
    </CellTooltip>
  );
}

function renderPriorityCell(value: string) {
  return value.trim() ? (
    <span className="text-foreground">{value}</span>
  ) : (
    <span className="text-muted-foreground">0</span>
  );
}

function renderApiKeyCell(upstream: UpstreamForm, showApiKeys: boolean) {
  const value = showApiKeys ? upstream.apiKeys : toMaskedApiKey(upstream.apiKeys);
  return renderTextCell(value, m.common_optional());
}

function renderProxyUrlCell(upstream: UpstreamForm, showApiKeys: boolean) {
  const rawValue = upstream.proxyUrl;
  const value = showApiKeys ? rawValue : toMaskedProxyUrl(rawValue);
  return renderTextCell(value, m.upstreams_proxy_direct());
}

function renderUpstreamCell(
  columnId: UpstreamColumnId,
  upstream: UpstreamForm,
  showApiKeys: boolean,
) {
  const providerLabel = upstream.providers
    .map((value) => value.trim())
    .filter(Boolean)
    .join(", ");
  switch (columnId) {
    case "id":
      return renderTextCell(upstream.id, "openai-default");
    case "provider":
      return renderTextCell(providerLabel, "openai");
    case "baseUrl":
      return renderTextCell(upstream.baseUrl, "https://api.openai.com");
    case "apiKeys":
      return renderApiKeyCell(upstream, showApiKeys);
    case "proxyUrl":
      return renderProxyUrlCell(upstream, showApiKeys);
    case "priority":
      return renderPriorityCell(upstream.priority);
    case "status":
      return (
        <Badge variant={upstream.enabled ? "default" : "secondary"}>
          {toStatusLabel(upstream.enabled)}
        </Badge>
      );
  }
}

type UpstreamRowActionsProps = {
  rowLabel: string;
  enabled: boolean;
  disableCopy: boolean;
  disableDelete: boolean;
  onEdit: () => void;
  onCopy: () => void;
  onToggleEnabled: () => void;
  onDelete: () => void;
};

function UpstreamRowActions({
  rowLabel,
  enabled,
  disableCopy,
  disableDelete,
  onEdit,
  onCopy,
  onToggleEnabled,
  onDelete,
}: UpstreamRowActionsProps) {
  return (
    <td className="sticky right-0 z-10 w-[9rem] border-l border-border/40 bg-background/95 px-3 py-2 align-top backdrop-blur-xs group-hover:bg-muted/50">
      <div className="flex justify-end gap-1">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onEdit}
          aria-label={m.upstreams_row_edit({ rowLabel })}
        >
          <Pencil className="size-4" aria-hidden="true" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onCopy}
          disabled={disableCopy}
          aria-label={m.upstreams_row_copy({ rowLabel })}
        >
          <Copy className="size-4" aria-hidden="true" />
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onToggleEnabled}
          aria-label={enabled ? m.upstreams_row_disable({ rowLabel }) : m.upstreams_row_enable({ rowLabel })}
        >
          {enabled ? (
            <Ban className="size-4 text-muted-foreground" aria-hidden="true" />
          ) : (
            <Check className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
          )}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onDelete}
          disabled={disableDelete}
          aria-label={m.upstreams_row_delete({ rowLabel })}
        >
          <Trash2 className="size-4" aria-hidden="true" />
        </Button>
      </div>
    </td>
  );
}

type UpstreamsTableRowProps = {
  upstream: UpstreamForm;
  upstreamIndex: number;
  displayIndex: number;
  columns: readonly UpstreamColumnDefinition[];
  showApiKeys: boolean;
  disableDelete: boolean;
  isCopyDisabled?: (upstream: UpstreamForm) => boolean;
  isDeleteDisabled?: (upstream: UpstreamForm) => boolean;
  onEdit: (index: number) => void;
  onCopy: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

function UpstreamsTableRow({
  upstream,
  upstreamIndex,
  displayIndex,
  columns,
  showApiKeys,
  disableDelete,
  isCopyDisabled,
  isDeleteDisabled,
  onEdit,
  onCopy,
  onToggleEnabled,
  onDelete,
}: UpstreamsTableRowProps) {
  const rowLabel = getUpstreamLabel(displayIndex);
  const copyDisabled = isCopyDisabled?.(upstream) === true;
  const deleteDisabled = disableDelete || isDeleteDisabled?.(upstream) === true;
  return (
    <tr className="group border-b border-border/40 last:border-b-0">
      {columns.map((column) => (
        <td
          key={column.id}
          className={["px-3 py-2 align-top", column.cellClassName].filter(Boolean).join(" ")}
        >
          <div className="flex h-8 min-w-0 items-center">
            {renderUpstreamCell(column.id, upstream, showApiKeys)}
          </div>
        </td>
      ))}
      <UpstreamRowActions
        rowLabel={rowLabel}
        enabled={upstream.enabled}
        disableCopy={copyDisabled}
        disableDelete={deleteDisabled}
        onEdit={() => onEdit(upstreamIndex)}
        onCopy={() => onCopy(upstreamIndex)}
        onToggleEnabled={() => onToggleEnabled(upstreamIndex)}
        onDelete={() => onDelete(upstreamIndex)}
      />
    </tr>
  );
}

export type UpstreamsTableProps = {
  upstreams: UpstreamForm[];
  columns: readonly UpstreamColumnDefinition[];
  showApiKeys: boolean;
  disableDelete: boolean;
  isCopyDisabled?: (upstream: UpstreamForm) => boolean;
  isDeleteDisabled?: (upstream: UpstreamForm) => boolean;
  onEdit: (index: number) => void;
  onCopy: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

type SortedUpstreamEntry = {
  upstream: UpstreamForm;
  upstreamIndex: number;
  priority: number;
};

function parsePriorityValue(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return 0;
  }
  const number = Number.parseInt(trimmed, 10);
  return Number.isFinite(number) ? number : 0;
}

function sortUpstreamsByPriority(upstreams: UpstreamForm[]) {
  // Display order follows priority descending; ties keep original list order.
  const entries = upstreams.map((upstream, upstreamIndex): SortedUpstreamEntry => ({
    upstream,
    upstreamIndex,
    priority: parsePriorityValue(upstream.priority),
  }));
  entries.sort((left, right) => {
    if (left.priority !== right.priority) {
      return right.priority - left.priority;
    }
    return left.upstreamIndex - right.upstreamIndex;
  });
  return entries;
}

export function UpstreamsTable({
  upstreams,
  columns,
  showApiKeys,
  disableDelete,
  isCopyDisabled,
  isDeleteDisabled,
  onEdit,
  onCopy,
  onToggleEnabled,
  onDelete,
}: UpstreamsTableProps) {
  const sortedUpstreams = sortUpstreamsByPriority(upstreams);
  return (
    <TooltipProvider>
      <div className="overflow-x-auto rounded-md border border-border/60 bg-background/60">
        <table className="w-full border-collapse text-sm">
          <UpstreamsTableHeader columns={columns} />
          <tbody>
            {sortedUpstreams.map((entry, displayIndex) => (
              <UpstreamsTableRow
                key={entry.upstreamIndex}
                upstream={entry.upstream}
                upstreamIndex={entry.upstreamIndex}
                displayIndex={displayIndex}
                columns={columns}
                showApiKeys={showApiKeys}
                disableDelete={disableDelete}
                isCopyDisabled={isCopyDisabled}
                isDeleteDisabled={isDeleteDisabled}
                onEdit={onEdit}
                onCopy={onCopy}
                onToggleEnabled={onToggleEnabled}
                onDelete={onDelete}
              />
            ))}
          </tbody>
        </table>
      </div>
    </TooltipProvider>
  );
}
