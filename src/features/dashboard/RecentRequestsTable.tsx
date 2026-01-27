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
  formatDashboardTimestamp,
  formatInteger,
} from "@/features/dashboard/format";
import type { DashboardRequestItem } from "@/features/dashboard/types";
import { useI18n } from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";

const TABLE_HEIGHT_PX = 360;
const ROW_HEIGHT_PX = 44;
const OVERSCAN = 6;

// 让 time/tokens 列更紧凑、model 列更宽（同时保持其它列的响应式伸缩）。
const GRID_COLS = "grid-cols-[154px_1fr_1fr_1.6fr_67.5px_80px_81px]";
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
      return (
        <CellTooltip content={timestamp}>
          <span className="block truncate text-xs text-muted-foreground">{timestamp}</span>
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
      const full = `${row.original.upstreamId} · ${row.original.provider}`;
      return (
        <CellTooltip content={full}>
          <span className="block truncate text-xs text-muted-foreground">
            {row.original.upstreamId}
            <span className="text-muted-foreground/70"> · {row.original.provider}</span>
          </span>
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
        row.original.totalTokens === null ? CELL_PLACEHOLDER : formatInteger(row.original.totalTokens);
      const cachedText = row.original.cachedTokens ? formatInteger(row.original.cachedTokens) : null;
      const tooltipText = cachedText ? `${totalText}\n${cachedText}` : totalText;

      return (
        <CellTooltip content={tooltipText} disabled={totalText === CELL_PLACEHOLDER && !cachedText}>
          <div className="flex min-w-0 flex-col items-end gap-0.5 font-medium text-foreground">
            <span className="block w-full truncate text-right">{totalText}</span>
            {cachedText ? (
              <span className="block w-full truncate text-xs font-normal text-muted-foreground text-right">
                {cachedText}
              </span>
            ) : null}
          </div>
        </CellTooltip>
      );
    },
  };
}

function latencyColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "latency",
    header: m.dashboard_table_latency_ms(),
    cell: ({ row }) => {
      const latencyText = formatInteger(row.original.latencyMs);
      return (
        <CellTooltip content={latencyText}>
          <span className="block w-full truncate text-xs text-muted-foreground text-right">
            {latencyText}
          </span>
        </CellTooltip>
      );
    },
  };
}

function buildColumns(formatter: Intl.DateTimeFormat) {
  return [
    timeColumn(formatter),
    pathColumn(),
    providerColumn(),
    modelColumn(),
    statusColumn(),
    tokensColumn(),
    latencyColumn(),
  ];
}

function headerCellClass(columnId: string) {
  if (columnId === "tokens" || columnId === "latency") {
    return "text-right";
  }
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
  if (columnId === "tokens" || columnId === "latency") {
    return "min-w-0 px-3 py-2 text-right";
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
    <div className={cn("grid bg-muted/50 text-xs text-muted-foreground", GRID_COLS)}>
      {table.getHeaderGroups().map((group) =>
        group.headers.map((header) => (
          <div key={header.id} className={cn("px-3 py-2 font-medium", headerCellClass(header.column.id))}>
            {header.isPlaceholder ? null : flexRender(header.column.columnDef.header, header.getContext())}
          </div>
        )),
      )}
    </div>
  );
}

function useRecentRowVirtualizer(rows: Row<DashboardRequestItem>[], scrollKey: string) {
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
  }, [scrollKey]);

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
        className={cn(
          "absolute inset-x-0 grid items-center border-t border-border/60 bg-background/70 text-sm hover:bg-accent/30",
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

function RecentRequestsBody({
  rows,
  scrollKey,
  onSelectItem,
}: {
  rows: Row<DashboardRequestItem>[];
  scrollKey: string;
  onSelectItem?: (item: DashboardRequestItem) => void;
}) {
  const { scrollRef, rowVirtualizer, virtualRows } = useRecentRowVirtualizer(rows, scrollKey);

  return (
    <div
      ref={scrollRef}
      className="overflow-y-auto overflow-x-hidden"
      style={{ height: TABLE_HEIGHT_PX }}
    >
      <div className="relative" style={{ height: rowVirtualizer.getTotalSize() }}>
        <RecentRequestsRows
          rows={rows}
          virtualRows={virtualRows}
          onSelectItem={onSelectItem}
        />
      </div>
    </div>
  );
}

export function RecentRequestsTable({ items, scrollKey, onSelectItem }: RecentRequestsTableProps) {
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
      <div data-slot="recent-requests-table" className="overflow-hidden rounded-lg border border-border/60">
        <RecentRequestsHeader table={table} />
        <RecentRequestsBody
          rows={table.getRowModel().rows}
          scrollKey={scrollKey}
          onSelectItem={onSelectItem}
        />
      </div>
    </TooltipProvider>
  );
}
