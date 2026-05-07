import { useEffect, useRef, type ReactElement } from "react";

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

import { Badge } from "@/components/ui/badge";
import { TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import {
  createDashboardTimeFormatter,
  formatCompact,
  formatDashboardClockTime,
  formatDashboardProviderLabel,
  formatDashboardTimestamp,
  formatInteger,
  formatNanoUsdCost,
} from "@/features/dashboard/format";
import type { DashboardRequestItem } from "@/features/dashboard/types";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";

const ROW_HEIGHT_PX = 44;
const HEADER_HEIGHT_PX = 34;
const OVERSCAN = 6;

// 固定列宽避免虚拟列表行在状态、费用、延迟文本变化时抖动。
const GRID_COLS = "grid-cols-[85px_140px_132px_104px_64px_82px_60px_104px]";
const TABLE_MIN_WIDTH_PX = 771;
const CELL_PLACEHOLDER = "—";
const TOOLTIP_CONTENT_CLASS = "max-w-[560px] whitespace-pre-wrap break-words";
type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

function statusToVariant(status: number): BadgeVariant {
  if (status >= 200 && status < 300) {
    return "default";
  }
  if (status >= 400) {
    return "destructive";
  }
  if (status >= 300) {
    return "secondary";
  }
  return "outline";
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

function timeColumn(formatter: Intl.DateTimeFormat): ColumnDef<DashboardRequestItem> {
  return {
    id: "time",
    header: m.dashboard_table_time(),
    cell: ({ row }) => {
      const timestamp = formatDashboardTimestamp(row.original.tsMs, formatter);
      const clockTime = formatDashboardClockTime(row.original.tsMs);
      return (
        <CellTooltip content={timestamp}>
          <span className="block truncate text-xs text-muted-foreground">{clockTime}</span>
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
      const tooltipText = mapped ? `${primary}\n${mapped}` : primary;

      return (
        <CellTooltip content={tooltipText} disabled={primary === CELL_PLACEHOLDER && !mapped}>
          <div className="flex min-w-0 flex-col items-start gap-0.5">
            <span className="block w-full truncate font-medium text-foreground">{primary}</span>
            {mapped ? (
              <span className="block w-full truncate text-xs font-normal text-muted-foreground">
                {mapped}
              </span>
            ) : null}
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
    cell: ({ row }) => <Badge variant={statusToVariant(row.original.status)}>{row.original.status}</Badge>,
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

function buildColumns(formatter: Intl.DateTimeFormat) {
  return [
    timeColumn(formatter),
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
  if (columnId === "time") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "path") {
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
};

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

function useRecentRowVirtualizer(rows: Row<DashboardRequestItem>[], scrollKey: string) {
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

function RecentRequestsRows({
  rows,
  virtualRows,
  onSelectItem,
}: {
  rows: Row<DashboardRequestItem>[];
  virtualRows: VirtualItem[];
  onSelectItem?: (item: DashboardRequestItem) => void;
}) {
  return virtualRows.map((virtualRow) => {
    const row = rows[virtualRow.index];
    if (!row) {
      return null;
    }
    const isInteractive = Boolean(onSelectItem);

    return (
      <div
        key={row.id}
        data-slot="recent-requests-table-row"
        className={cn(
          "absolute inset-x-0 grid justify-start items-center border-t border-border/60 bg-background/70 text-sm hover:bg-accent/30",
          GRID_COLS,
          isInteractive && "cursor-pointer"
        )}
        style={{
          transform: `translateY(${virtualRow.start}px)`,
          height: `${virtualRow.size}px`,
        }}
        role={isInteractive ? "button" : undefined}
        tabIndex={isInteractive ? 0 : undefined}
        onClick={isInteractive ? () => onSelectItem?.(row.original) : undefined}
        onKeyDown={(event) => {
          if (!isInteractive) {
            return;
          }
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            onSelectItem?.(row.original);
          }
        }}
      >
        {row.getVisibleCells().map((cell) => (
          <div key={cell.id} className={rowCellClass(cell.column.id)}>
            {flexRender(cell.column.columnDef.cell, cell.getContext())}
          </div>
        ))}
      </div>
    );
  });
}

function RecentRequestsScrollArea({
  table,
  rows,
  scrollKey,
  onSelectItem,
}: {
  table: Table<DashboardRequestItem>;
  rows: Row<DashboardRequestItem>[];
  scrollKey: string;
  onSelectItem?: (item: DashboardRequestItem) => void;
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
            onSelectItem={onSelectItem}
          />
        </div>
      </div>
    </div>
  );
}

export function RecentRequestsTable({ items, scrollKey, onSelectItem }: RecentRequestsTableProps) {
  "use no memo";

  const { locale } = useI18n();
  const formatter = createDashboardTimeFormatter(locale);
  const columns = buildColumns(formatter);

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
          rows={table.getRowModel().rows}
          scrollKey={scrollKey}
          onSelectItem={onSelectItem}
        />
      </div>
    </TooltipProvider>
  );
}
