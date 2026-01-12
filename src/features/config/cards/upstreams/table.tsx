import { Ban, Check, Columns3, Eye, EyeOff, Pencil, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  getUpstreamLabel,
  toMaskedApiKey,
  toStatusLabel,
} from "@/features/config/cards/upstreams/constants";
import type { UpstreamColumnDefinition, UpstreamColumnId } from "@/features/config/cards/upstreams/types";
import type { UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsToolbarProps = {
  apiKeyVisible: boolean;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onAddClick: () => void;
  onColumnsClick: () => void;
};

export function UpstreamsToolbar({
  apiKeyVisible,
  showApiKeys,
  onToggleApiKeys,
  onAddClick,
  onColumnsClick,
}: UpstreamsToolbarProps) {
  return (
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
          {showApiKeys ? <EyeOff className="size-4" aria-hidden="true" /> : <Eye className="size-4" aria-hidden="true" />}
        </Button>
      ) : null}
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
        <th className="w-[9rem] px-3 py-2 text-right text-xs font-medium text-muted-foreground">
          {m.common_actions()}
        </th>
      </tr>
    </thead>
  );
}

function renderUpstreamCell(columnId: UpstreamColumnId, upstream: UpstreamForm, showApiKeys: boolean) {
  switch (columnId) {
    case "id":
      return upstream.id.trim() ? (
        <span className="truncate text-foreground">{upstream.id}</span>
      ) : (
        <span className="truncate text-muted-foreground">openai-default</span>
      );
    case "provider":
      return upstream.provider.trim() ? (
        <span className="truncate text-foreground">{upstream.provider}</span>
      ) : (
        <span className="truncate text-muted-foreground">openai</span>
      );
    case "baseUrl":
      return upstream.baseUrl.trim() ? (
        <span className="truncate text-foreground">{upstream.baseUrl}</span>
      ) : (
        <span className="truncate text-muted-foreground">https://api.openai.com</span>
      );
    case "apiKey": {
      const value = showApiKeys ? upstream.apiKey : toMaskedApiKey(upstream.apiKey);
      return value.trim() ? (
        <span className="truncate text-foreground">{value}</span>
      ) : (
        <span className="truncate text-muted-foreground">{m.common_optional()}</span>
      );
    }
    case "priority":
      return upstream.priority.trim() ? (
        <span className="text-foreground">{upstream.priority}</span>
      ) : (
        <span className="text-muted-foreground">0</span>
      );
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
  disableDelete: boolean;
  onEdit: () => void;
  onToggleEnabled: () => void;
  onDelete: () => void;
};

function UpstreamRowActions({
  rowLabel,
  enabled,
  disableDelete,
  onEdit,
  onToggleEnabled,
  onDelete,
}: UpstreamRowActionsProps) {
  return (
    <td className="w-[9rem] px-3 py-2 align-top">
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
  index: number;
  columns: readonly UpstreamColumnDefinition[];
  showApiKeys: boolean;
  disableDelete: boolean;
  onEdit: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

function UpstreamsTableRow({
  upstream,
  index,
  columns,
  showApiKeys,
  disableDelete,
  onEdit,
  onToggleEnabled,
  onDelete,
}: UpstreamsTableRowProps) {
  const rowLabel = getUpstreamLabel(index);
  return (
    <tr className="border-b border-border/40 last:border-b-0">
      {columns.map((column) => (
        <td
          key={column.id}
          className={["px-3 py-2 align-top", column.cellClassName].filter(Boolean).join(" ")}
        >
          <div className="flex h-8 items-center">
            {renderUpstreamCell(column.id, upstream, showApiKeys)}
          </div>
        </td>
      ))}
      <UpstreamRowActions
        rowLabel={rowLabel}
        enabled={upstream.enabled}
        disableDelete={disableDelete}
        onEdit={() => onEdit(index)}
        onToggleEnabled={() => onToggleEnabled(index)}
        onDelete={() => onDelete(index)}
      />
    </tr>
  );
}

export type UpstreamsTableProps = {
  upstreams: UpstreamForm[];
  columns: readonly UpstreamColumnDefinition[];
  showApiKeys: boolean;
  disableDelete: boolean;
  onEdit: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

export function UpstreamsTable({
  upstreams,
  columns,
  showApiKeys,
  disableDelete,
  onEdit,
  onToggleEnabled,
  onDelete,
}: UpstreamsTableProps) {
  return (
    <div className="overflow-x-auto rounded-md border border-border/60 bg-background/60">
      <table className="w-full border-collapse text-sm">
        <UpstreamsTableHeader columns={columns} />
        <tbody>
          {upstreams.map((upstream, index) => (
            <UpstreamsTableRow
              key={index}
              upstream={upstream}
              index={index}
              columns={columns}
              showApiKeys={showApiKeys}
              disableDelete={disableDelete}
              onEdit={onEdit}
              onToggleEnabled={onToggleEnabled}
              onDelete={onDelete}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}
