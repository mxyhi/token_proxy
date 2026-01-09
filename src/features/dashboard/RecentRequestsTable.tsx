import { useEffect, useRef } from "react";

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
import type { DashboardRequestItem } from "@/features/dashboard/types";
import { formatInteger } from "@/features/dashboard/format";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";

const TABLE_HEIGHT_PX = 360;
const ROW_HEIGHT_PX = 44;
const OVERSCAN = 6;

const GRID_COLS = "grid-cols-[170px_1fr_1fr_90px_140px_90px]";

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

function timeColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "time",
    header: m.dashboard_table_time(),
    cell: ({ row }) => (
      <span className="whitespace-nowrap text-xs text-muted-foreground">
        {new Date(row.original.tsMs).toLocaleString()}
      </span>
    ),
  };
}

function pathColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "path",
    header: m.dashboard_table_path(),
    cell: ({ row }) => <span className="block truncate font-medium text-foreground">{row.original.path}</span>,
  };
}

function providerColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "provider",
    header: m.dashboard_table_provider(),
    cell: ({ row }) => (
      <span className="block truncate text-xs text-muted-foreground">
        {row.original.upstreamId}
        <span className="text-muted-foreground/70"> · {row.original.provider}</span>
      </span>
    ),
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
    cell: ({ row }) => (
      <div className="flex flex-col items-end gap-0.5 font-medium text-foreground">
        <span>{row.original.totalTokens === null ? "—" : formatInteger(row.original.totalTokens)}</span>
        {row.original.cachedTokens ? (
          <span className="text-xs font-normal text-muted-foreground">
            {m.dashboard_cached({ count: formatInteger(row.original.cachedTokens) })}
          </span>
        ) : null}
      </div>
    ),
  };
}

function latencyColumn(): ColumnDef<DashboardRequestItem> {
  return {
    id: "latency",
    header: m.dashboard_table_latency_ms(),
    cell: ({ row }) => (
      <span className="text-xs text-muted-foreground">{formatInteger(row.original.latencyMs)}</span>
    ),
  };
}

function buildColumns() {
  return [timeColumn(), pathColumn(), providerColumn(), statusColumn(), tokensColumn(), latencyColumn()];
}

function headerCellClass(columnId: string) {
  if (columnId === "tokens" || columnId === "latency") {
    return "text-right";
  }
  return "text-left";
}

function rowCellClass(columnId: string) {
  if (columnId === "time") {
    return "px-3 py-2";
  }
  if (columnId === "path") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "provider") {
    return "min-w-0 px-3 py-2";
  }
  if (columnId === "status") {
    return "px-3 py-2";
  }
  if (columnId === "tokens" || columnId === "latency") {
    return "px-3 py-2 text-right";
  }
  return "px-3 py-2";
}

type RecentRequestsTableProps = {
  items: DashboardRequestItem[];
  scrollKey: string;
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
}: {
  rows: Row<DashboardRequestItem>[];
  virtualRows: VirtualItem[];
}) {
  return virtualRows.map((virtualRow) => {
    const row = rows[virtualRow.index];
    if (!row) {
      return null;
    }

    return (
      <div
        key={row.id}
        className={cn(
          "absolute inset-x-0 grid items-center border-t border-border/60 bg-background/70 text-sm hover:bg-accent/30",
          GRID_COLS,
        )}
        style={{
          transform: `translateY(${virtualRow.start}px)`,
          height: `${virtualRow.size}px`,
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

function RecentRequestsBody({ rows, scrollKey }: { rows: Row<DashboardRequestItem>[]; scrollKey: string }) {
  const { scrollRef, rowVirtualizer, virtualRows } = useRecentRowVirtualizer(rows, scrollKey);

  return (
    <div
      ref={scrollRef}
      className="overflow-y-auto overflow-x-hidden"
      style={{ height: TABLE_HEIGHT_PX }}
    >
      <div className="relative" style={{ height: rowVirtualizer.getTotalSize() }}>
        <RecentRequestsRows rows={rows} virtualRows={virtualRows} />
      </div>
    </div>
  );
}

export function RecentRequestsTable({ items, scrollKey }: RecentRequestsTableProps) {
  const columns = buildColumns();

  const table = useReactTable({
    data: items,
    columns,
    getCoreRowModel: getCoreRowModel(),
    getRowId: (row) => String(row.id),
  });

  return (
    <div data-slot="recent-requests-table" className="overflow-hidden rounded-lg border border-border/60">
      <RecentRequestsHeader table={table} />
      <RecentRequestsBody rows={table.getRowModel().rows} scrollKey={scrollKey} />
    </div>
  );
}
