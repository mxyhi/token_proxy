import { useEffect, useRef, type ReactElement, type ReactNode } from "react";

import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { useVirtualizer, type VirtualItem } from "@tanstack/react-virtual";
import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
  type Row,
  type Table,
} from "@tanstack/react-table";

import { useConfigLocalApiKeys } from "@/features/config/ConfigScreen";
import { TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import {
  createDashboardTimeFormatter,
  formatCompact,
  formatDashboardClientIp,
  formatDashboardClockTime,
  formatDashboardProviderLabel,
  formatDashboardTimestamp,
  formatInteger,
  formatNanoUsdCost,
} from "@/features/dashboard/format";
import type { DashboardRequestAttempt, DashboardRequestItem } from "@/features/dashboard/types";
import {
  AttemptTraceCell,
  attemptNumber,
  formatAttemptChain,
  summarizeAttemptError,
} from "@/features/logs/AttemptTrace";
import { HttpStatusBadge } from "@/features/logs/HttpStatusBadge";
import { UpstreamResponseModelLine, describeUpstreamResponseModel } from "@/features/logs/UpstreamResponseModelLine";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";

const ROW_HEIGHT_PX = 44;
const HEADER_HEIGHT_PX = 34;
const OVERSCAN = 6;

// 固定列宽避免虚拟列表行在状态、费用、延迟文本变化时抖动。
// 首列是 attempt 状态点；模型列要放下「↳ 上游响应: <model>」和不一致标记，比单行模型名更宽。
const GRID_COLS = "grid-cols-[28px_85px_79px_96px_124px_99px_236px_64px_82px_60px_104px]";
const TABLE_MIN_WIDTH_PX = 1057;
const CELL_PLACEHOLDER = "—";
const TOOLTIP_CONTENT_CLASS = "max-w-[560px] whitespace-pre-wrap break-words";
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

function traceColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "trace",
    header: "",
    cell: ({ row }) => (
      <AttemptTraceCell
        status={row.original.status}
        variant="final"
        lineBelow={Boolean(row.original.attempts?.length)}
      />
    ),
  };
}

function timeColumn(formatter: Intl.DateTimeFormat): ColumnDef<DashboardRequestItem> {
  return {
    id: "time",
    header: m.dashboard_table_time(),
    cell: ({ row }) => {
      const timestamp = formatDashboardTimestamp(row.original.tsMs, formatter);
      const clockTime = formatDashboardClockTime(row.original.tsMs);
      const attemptCount = row.original.attempts?.length ? row.original.attempts.length + 1 : 0;
      // 有中间 attempt 时按发送先后列出渠道链，便于一眼看出从哪个渠道切走。
      const tooltip = attemptCount ? `${timestamp}\n${formatAttemptChain(row.original)}` : timestamp;
      return (
        <CellTooltip content={tooltip}>
          <div className="flex min-w-0 flex-col items-start gap-0.5">
            <span className="block w-full truncate text-xs text-muted-foreground">{clockTime}</span>
            {attemptCount ? (
              <span className="block w-full truncate text-[11px] text-muted-foreground">
                {m.logs_attempt_count({ count: String(attemptCount) })}
              </span>
            ) : null}
          </div>
        </CellTooltip>
      );
    },
  };
}

// Key 名称按当前配置解析：未启用鉴权的请求为空，已删除的 Key 只剩 ID。
function apiKeyColumn(keyNames: ReadonlyMap<string, string>): ColumnDef<DashboardRequestItem> {
  return {
    id: "apiKey",
    header: m.api_keys_key(),
    cell: ({ row }) => {
      const keyId = row.original.localApiKeyId?.trim();
      if (!keyId) {
        return <span className="block truncate text-xs text-muted-foreground">{CELL_PLACEHOLDER}</span>;
      }
      const name = keyNames.get(keyId);
      return (
        <CellTooltip content={name ?? keyId}>
          <span
            className={cn(
              "block truncate text-xs",
              name ? "font-medium text-foreground" : "text-muted-foreground",
            )}
          >
            {name ?? m.api_keys_deleted()}
          </span>
        </CellTooltip>
      );
    },
  };
}

function pathColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "path",
    header: m.dashboard_table_path(),
    cell: ({ row }) => (
      <CellTooltip content={row.original.path}>
        <span className="block truncate font-medium text-foreground">{row.original.path}</span>
      </CellTooltip>
    ),
  };
}

function ipColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "ip",
    header: m.dashboard_table_ip(),
    cell: ({ row }) => {
      const clientIp = formatDashboardClientIp(row.original.clientIp);
      return (
        <CellTooltip content={clientIp}>
          <span className="block truncate text-xs font-medium text-foreground">{clientIp}</span>
        </CellTooltip>
      );
    },
  };
}

function providerColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "provider",
    header: m.dashboard_table_provider(),
    cell: ({ row }) => {
      const full = formatDashboardProviderLabel(
        row.original.upstreamId,
        row.original.provider,
        row.original.accountId,
      );
      return (
        <CellTooltip content={full}>
          <span className="block truncate text-xs text-muted-foreground">{full}</span>
        </CellTooltip>
      );
    },
  };
}

function modelColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "model",
    header: m.dashboard_table_model(),
    cell: ({ row }) => {
      const primary = row.original.model?.trim() ? row.original.model : CELL_PLACEHOLDER;
      const mapped = row.original.mappedModel?.trim() ? row.original.mappedModel : null;
      // 行高只够两行。上游响应和映射同时存在时，表格展示上游响应，映射留在 tooltip。
      const responseLine = describeUpstreamResponseModel(
        row.original.model,
        row.original.mappedModel,
        row.original.upstreamResponseModel,
      );
      const tooltipText = [primary, mapped, responseLine].filter((line) => line).join("\n");

      return (
        <CellTooltip content={tooltipText} disabled={primary === CELL_PLACEHOLDER && !mapped && !responseLine}>
          <div className="flex min-w-0 flex-col items-start gap-0.5">
            <span className="block w-full truncate font-medium text-foreground">{primary}</span>
            {mapped && !responseLine ? (
              <span className="block w-full truncate text-xs font-normal text-muted-foreground">
                {mapped}
              </span>
            ) : null}
            <UpstreamResponseModelLine
              model={row.original.model}
              mappedModel={row.original.mappedModel}
              upstreamResponseModel={row.original.upstreamResponseModel}
            />
          </div>
        </CellTooltip>
      );
    },
  };
}

function statusColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "status",
    header: m.dashboard_table_status(),
    cell: ({ row }) => <HttpStatusBadge status={row.original.status} />,
  };
}

function tokensColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "tokens",
    header: m.dashboard_table_tokens(),
    cell: ({ row }) => {
      const totalText =
        row.original.totalTokens === null ? CELL_PLACEHOLDER : formatCompact(row.original.totalTokens);
      const outputText =
        row.original.outputTokens === null ? CELL_PLACEHOLDER : formatCompact(row.original.outputTokens);
      const cachedText =
        row.original.cachedTokens ? formatCompact(row.original.cachedTokens) : null;
      const tooltipParts = [
        `${m.dashboard_chart_total_tokens()} ${totalText}`,
        `${m.dashboard_chart_output_tokens()} ${outputText}`,
        cachedText ? `${m.dashboard_chart_cached_tokens()} ${cachedText}` : null,
      ].filter((part): part is string => Boolean(part));
      const tooltipText = tooltipParts.join(" · ");
      const secondaryParts = [outputText, cachedText].filter((part): part is string => Boolean(part));
      const secondaryText = secondaryParts.length > 0 ? secondaryParts.join(" · ") : CELL_PLACEHOLDER;

      return (
        <CellTooltip
          content={tooltipText}
          disabled={
            totalText === CELL_PLACEHOLDER &&
            outputText === CELL_PLACEHOLDER &&
            !cachedText
          }
        >
          <div className="flex min-w-0 flex-col items-start gap-0.5 font-medium text-foreground">
            <span className="block w-full truncate text-left">{totalText}</span>
            <span className="block w-full truncate text-[11px] font-normal text-muted-foreground text-left">
              {secondaryText}
            </span>
          </div>
        </CellTooltip>
      );
    },
  };
}

function formatPricingContextTier(tier: string | null | undefined) {
  if (tier === "long") {
    return m.logs_detail_pricing_context_long();
  }
  if (tier === "short") {
    return m.logs_detail_pricing_context_short();
  }
  return CELL_PLACEHOLDER;
}

function costColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "cost",
    header: m.dashboard_table_cost(),
    cell: ({ row }) => {
      const item = row.original;
      const costText = formatNanoUsdCost(item.costNanoUsd);
      const tooltip = [
        `${m.dashboard_table_cost()}: ${costText}`,
        `${m.logs_detail_pricing_model()}: ${item.pricingModel?.trim() || CELL_PLACEHOLDER}`,
        `${m.logs_detail_pricing_context_tier()}: ${formatPricingContextTier(item.pricingContextTier)}`,
        `${m.logs_detail_pricing_version()}: ${item.pricingVersion?.trim() || CELL_PLACEHOLDER}`,
      ].join("\n");
      return (
        <CellTooltip content={tooltip} disabled={item.costNanoUsd == null}>
          <span className="block w-full truncate text-xs text-muted-foreground text-left">
            {costText}
          </span>
        </CellTooltip>
      );
    },
  };
}

function latencyColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "latency",
    header: m.logs_timing_upstream_response_headers_ms(),
    cell: ({ row }) => {
      const item = row.original;
      const latencyText = formatInteger(item.latencyMs);
      const responseHeadersLatencyText = formatOptionalLatency(item.upstreamResponseHeadersMs);
      const firstBodyChunkLatencyText = formatOptionalLatency(
        item.upstreamFirstBodyChunkMs ?? item.upstreamFirstByteMs,
      );
      const tooltip = [
        `${m.dashboard_table_latency_ms()}: ${latencyText}`,
        `${m.logs_timing_upstream_response_headers_ms()}: ${responseHeadersLatencyText}`,
        `${m.logs_timing_upstream_first_body_chunk_ms()}: ${firstBodyChunkLatencyText}`,
        `${m.logs_timing_first_client_flush_ms()}: ${formatOptionalLatency(item.firstClientFlushMs)}`,
        `${m.logs_timing_first_output_ms()}: ${formatOptionalLatency(item.firstOutputMs)}`,
      ].join("\n");
      return (
        <CellTooltip content={tooltip}>
          <span className="block w-full truncate text-xs text-muted-foreground text-left">
            {responseHeadersLatencyText}
          </span>
        </CellTooltip>
      );
    },
  };
}

function formatOptionalLatency(value: number | null | undefined) {
  return value == null ? CELL_PLACEHOLDER : formatInteger(value);
}

function buildColumns(formatter: Intl.DateTimeFormat, keyNames: ReadonlyMap<string, string>) {
  return [
    traceColumn(),
    timeColumn(formatter),
    ipColumn(),
    apiKeyColumn(keyNames),
    pathColumn(),
    providerColumn(),
    modelColumn(),
    statusColumn(),
    tokensColumn(),
    costColumn(),
    latencyColumn(),
  ];
}

function headerCellClass() {
  return "text-left";
}

function rowCellClass(columnId: string) {
  if (columnId === "trace") {
    return "h-full";
  }
  if (columnId === "time") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "ip" || columnId === "apiKey" || columnId === "path") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "provider") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "model") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "status") {
    return "px-3 py-2";
  }
  if (columnId === "tokens" || columnId === "cost" || columnId === "latency") {
    return "min-w-0 px-3 py-2 text-left";
  }
  return "px-3 py-2";
}

type RecentRequestsTableProps = {
  items: DashboardRequestItem[];
  scrollKey: string;
  onSelectItem?: (item: DashboardRequestItem) => void;
  onSelectAttempt?: (attempt: DashboardRequestAttempt) => void;
};

// 虚拟列表的一行：最终请求，或挂在它下面的中间 attempt（按序号倒序，与列表时间倒序一致）。
type DisplayRow =
  | { kind: "request"; key: string; row: Row<DashboardRequestItem> }
  | { kind: "attempt"; key: string; attempt: DashboardRequestAttempt; isLast: boolean };

function buildDisplayRows(rows: Row<DashboardRequestItem>[]): DisplayRow[] {
  return rows.flatMap((row): DisplayRow[] => {
    const attempts = row.original.attempts ?? [];
    return [
      { kind: "request", key: `request-${row.id}`, row },
      ...attempts.map((attempt, index): DisplayRow => ({
        kind: "attempt",
        key: `attempt-${attempt.id}`,
        attempt,
        isLast: index === attempts.length - 1,
      })),
    ];
  });
}

function RecentRequestsHeader({ table }: { table: Table<DashboardRequestItem> }) {
  return (
    <div
      data-slot="recent-requests-table-header"
      className={cn(
        "sticky top-0 z-10 grid items-center justify-start bg-muted/50 text-xs text-muted-foreground",
        GRID_COLS,
      )}
      style={{ height: HEADER_HEIGHT_PX }}
    >
      {table.getHeaderGroups().map((group) =>
        group.headers.map((header) => (
          <div key={header.id} className={cn("px-3 py-2 font-medium", headerCellClass())}>
            {header.isPlaceholder ? null : flexRender(header.column.columnDef.header, header.getContext())}
          </div>
        )),
      )}
    </div>
  );
}

function useRecentRowVirtualizer(rows: DisplayRow[], scrollKey: string) {
  "use no memo";

  const scrollRef = useRef<HTMLDivElement | null>(null);

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT_PX,
    overscan: OVERSCAN,
  });

  useEffect(() => {
    rowVirtualizer.scrollToOffset(0);
    scrollRef.current?.scrollTo({ top: 0 });
  }, [rowVirtualizer, scrollKey]);

  return { scrollRef, rowVirtualizer, virtualRows: rowVirtualizer.getVirtualItems() };
}

type SelectableRowProps = {
  className: string;
  virtualRow: VirtualItem;
  onSelect?: () => void;
  children: ReactNode;
};

function SelectableRow({ className, virtualRow, onSelect, children }: SelectableRowProps) {
  const isInteractive = Boolean(onSelect);
  return (
    <div
      data-slot="recent-requests-table-row"
      className={cn(
        "absolute inset-x-0 grid justify-start items-center text-sm",
        GRID_COLS,
        className,
        isInteractive && "cursor-pointer",
      )}
      style={{
        transform: `translateY(${virtualRow.start}px)`,
        height: `${virtualRow.size}px`,
      }}
      role={isInteractive ? "button" : undefined}
      tabIndex={isInteractive ? 0 : undefined}
      onClick={onSelect}
      onKeyDown={(event) => {
        if (!onSelect) {
          return;
        }
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelect();
        }
      }}
    >
      {children}
    </div>
  );
}

function AttemptRowCells({
  attempt,
  isLast,
  formatter,
}: {
  attempt: DashboardRequestAttempt;
  isLast: boolean;
  formatter: Intl.DateTimeFormat;
}) {
  const number = attemptNumber(attempt.attemptIndex);
  const provider = formatDashboardProviderLabel(attempt.upstreamId, attempt.provider, attempt.accountId);
  const model = attempt.model?.trim() || CELL_PLACEHOLDER;
  const mapped = attempt.mappedModel?.trim() && attempt.mappedModel !== attempt.model ? attempt.mappedModel : null;
  const rawError = attempt.responseError?.trim() || CELL_PLACEHOLDER;
  const error = summarizeAttemptError(attempt.responseError) ?? CELL_PLACEHOLDER;
  const latencyTooltip = [
    `${m.dashboard_table_latency_ms()}: ${formatInteger(attempt.latencyMs)}`,
    `${m.logs_timing_upstream_response_headers_ms()}: ${formatOptionalLatency(attempt.upstreamResponseHeadersMs)}`,
  ].join("\n");

  return (
    <>
      <AttemptTraceCell status={attempt.status} variant="attempt" lineAbove lineBelow={!isLast} />
      <CellTooltip content={formatDashboardTimestamp(attempt.tsMs, formatter)}>
        <div className="flex min-w-0 flex-col items-start gap-0.5 px-3 py-2">
          <span className="block w-full truncate text-xs text-muted-foreground">
            {formatDashboardClockTime(attempt.tsMs)}
          </span>
          {number ? (
            <span className="block w-full truncate text-[11px] text-muted-foreground">
              {m.logs_attempt_number({ n: String(number) })}
            </span>
          ) : null}
        </div>
      </CellTooltip>
      {/* 中间 attempt 的 IP、Key、路径与最终请求相同，这三列合并展示错误摘要。 */}
      <CellTooltip content={rawError}>
        <span
          data-slot="recent-requests-attempt-error"
          className="col-span-3 block min-w-0 truncate px-3 py-2 text-xs text-muted-foreground"
        >
          {error}
        </span>
      </CellTooltip>
      <CellTooltip content={provider}>
        <span className="block min-w-0 truncate px-3 py-2 text-xs text-muted-foreground">{provider}</span>
      </CellTooltip>
      <div className="flex min-w-0 flex-col items-start gap-0.5 px-3 py-2">
        <span className="block w-full truncate text-xs text-muted-foreground">{model}</span>
        {mapped ? (
          <span className="block w-full truncate text-[11px] text-muted-foreground">{mapped}</span>
        ) : null}
      </div>
      <div className="px-3 py-2">
        <HttpStatusBadge status={attempt.status} />
      </div>
      <span className="px-3 py-2 text-xs text-muted-foreground">{CELL_PLACEHOLDER}</span>
      <span className="px-3 py-2 text-xs text-muted-foreground">{CELL_PLACEHOLDER}</span>
      <CellTooltip content={latencyTooltip}>
        <span className="block min-w-0 truncate px-3 py-2 text-xs text-muted-foreground">
          {formatOptionalLatency(attempt.upstreamResponseHeadersMs)}
        </span>
      </CellTooltip>
    </>
  );
}

function RecentRequestsRows({
  rows,
  virtualRows,
  formatter,
  onSelectItem,
  onSelectAttempt,
}: {
  rows: DisplayRow[];
  virtualRows: VirtualItem[];
  formatter: Intl.DateTimeFormat;
  onSelectItem?: (item: DashboardRequestItem) => void;
  onSelectAttempt?: (attempt: DashboardRequestAttempt) => void;
}) {
  return virtualRows.map((virtualRow) => {
    const displayRow = rows[virtualRow.index];
    if (!displayRow) {
      return null;
    }

    if (displayRow.kind === "attempt") {
      const { attempt } = displayRow;
      return (
        <SelectableRow
          key={displayRow.key}
          virtualRow={virtualRow}
          // 中间 attempt 弱化展示：浅底色、无行顶分隔线，视觉上归属上方的最终请求。
          className="bg-muted/30 hover:bg-accent/30"
          onSelect={onSelectAttempt ? () => onSelectAttempt(attempt) : undefined}
        >
          <AttemptRowCells attempt={attempt} isLast={displayRow.isLast} formatter={formatter} />
        </SelectableRow>
      );
    }

    const { row } = displayRow;
    return (
      <SelectableRow
        key={displayRow.key}
        virtualRow={virtualRow}
        className="border-t border-border/60 bg-background/70 hover:bg-accent/30"
        onSelect={onSelectItem ? () => onSelectItem(row.original) : undefined}
      >
        {row.getVisibleCells().map((cell) => (
          <div key={cell.id} className={rowCellClass(cell.column.id)}>
            {flexRender(cell.column.columnDef.cell, cell.getContext())}
          </div>
        ))}
      </SelectableRow>
    );
  });
}

function RecentRequestsScrollArea({
  table,
  rows,
  scrollKey,
  formatter,
  onSelectItem,
  onSelectAttempt,
}: {
  table: Table<DashboardRequestItem>;
  rows: DisplayRow[];
  scrollKey: string;
  formatter: Intl.DateTimeFormat;
  onSelectItem?: (item: DashboardRequestItem) => void;
  onSelectAttempt?: (attempt: DashboardRequestAttempt) => void;
}) {
  const { scrollRef, rowVirtualizer, virtualRows } = useRecentRowVirtualizer(rows, scrollKey);
  const rowsHeight = rowVirtualizer.getTotalSize();

  return (
    <div
      ref={scrollRef}
      data-slot="recent-requests-table-scroll-area"
      className="min-h-0 flex-1 overflow-auto"
    >
      <div
        data-slot="recent-requests-table-width-track"
        className="relative min-h-full"
        style={{
          minWidth: TABLE_MIN_WIDTH_PX,
          height: HEADER_HEIGHT_PX + rowsHeight,
        }}
      >
        <RecentRequestsHeader table={table} />
        <div
          data-slot="recent-requests-table-rows-layer"
          className="relative"
          style={{ height: rowsHeight }}
        >
          <RecentRequestsRows
            rows={rows}
            virtualRows={virtualRows}
            formatter={formatter}
            onSelectItem={onSelectItem}
            onSelectAttempt={onSelectAttempt}
          />
        </div>
      </div>
    </div>
  );
}

export function RecentRequestsTable({
  items,
  scrollKey,
  onSelectItem,
  onSelectAttempt,
}: RecentRequestsTableProps) {
  "use no memo";

  const { locale } = useI18n();
  const localApiKeys = useConfigLocalApiKeys();
  const formatter = createDashboardTimeFormatter(locale);
  const keyNames = new Map(localApiKeys.map((key) => [key.id, key.name]));
  const columns = buildColumns(formatter, keyNames);

  const table = useReactTable({
    data: items,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => String(row.id),
  });

  return (
    <TooltipProvider>
      <div
        data-slot="recent-requests-table"
        data-testid="recent-requests-table"
        className="flex min-h-0 flex-1 overflow-hidden rounded-lg border border-border/60"
      >
        <RecentRequestsScrollArea
          table={table}
          rows={buildDisplayRows(table.getRowModel().rows)}
          scrollKey={scrollKey}
          formatter={formatter}
          onSelectItem={onSelectItem}
          onSelectAttempt={onSelectAttempt}
        />
      </div>
    </TooltipProvider>
  );
}
