import { Ban, Check, Columns3, Copy, Eye, EyeOff, Pencil, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  getUpstreamLabel,
  toMaskedApiKey,
  toMaskedProxyUrl,
  toStatusLabel,
} from "@/features/config/cards/upstreams/constants";
import type { UpstreamColumnDefinition, UpstreamColumnId } from "@/features/config/cards/upstreams/types";
import type { CodexAccountSummary } from "@/features/codex/types";
import type { KiroAccountSummary } from "@/features/kiro/types";
import { UPSTREAM_STRATEGIES, type UpstreamForm, type UpstreamStrategy } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsToolbarProps = {
  apiKeyVisible: boolean;
  showApiKeys: boolean;
  strategy: UpstreamStrategy;
  onToggleApiKeys: () => void;
  onStrategyChange: (value: UpstreamStrategy) => void;
  onAddClick: () => void;
  onColumnsClick: () => void;
};

const UPSTREAM_STRATEGY_VALUES: ReadonlySet<string> = new Set(
  UPSTREAM_STRATEGIES.map((strategy) => strategy.value)
);

function toUpstreamStrategy(value: string): UpstreamStrategy | null {
  return UPSTREAM_STRATEGY_VALUES.has(value) ? (value as UpstreamStrategy) : null;
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
      <div className="flex flex-wrap items-center gap-2">
        <div className="flex items-center gap-2">
          <Label
            htmlFor="upstreams-strategy"
            className="text-xs text-muted-foreground"
            title={m.strategy_help()}
          >
            {m.strategy_label()}
          </Label>
          <Select
            value={strategy}
            onValueChange={(value) => {
              const nextStrategy = toUpstreamStrategy(value);
              if (nextStrategy) {
                onStrategyChange(nextStrategy);
              }
            }}
          >
            <SelectTrigger id="upstreams-strategy" className="min-w-[180px]">
              <SelectValue placeholder={m.strategy_placeholder()} />
            </SelectTrigger>
            <SelectContent>
              {UPSTREAM_STRATEGIES.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
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

type KiroAccountMap = Map<string, KiroAccountSummary>;
type CodexAccountMap = Map<string, CodexAccountSummary>;

function renderTextCell(value: string, placeholder: string) {
  return value.trim() ? (
    <span className="truncate text-foreground">{value}</span>
  ) : (
    <span className="truncate text-muted-foreground">{placeholder}</span>
  );
}

function renderPriorityCell(value: string) {
  return value.trim() ? (
    <span className="text-foreground">{value}</span>
  ) : (
    <span className="text-muted-foreground">0</span>
  );
}

function renderAccountCell(
  upstream: UpstreamForm,
  kiroAccounts: KiroAccountMap,
  codexAccounts: CodexAccountMap,
) {
  const provider = upstream.provider.trim();
  if (provider === "kiro") {
    const accountId = upstream.kiroAccountId.trim();
    if (!accountId) {
      return <span className="truncate text-muted-foreground">{m.kiro_account_unset()}</span>;
    }
    const account = kiroAccounts.get(accountId);
    if (!account) {
      return <span className="truncate text-muted-foreground">{m.kiro_account_missing()}</span>;
    }
    return <span className="truncate text-foreground">{account.account_id}</span>;
  }
  if (provider === "codex") {
    const accountId = upstream.codexAccountId.trim();
    if (!accountId) {
      return <span className="truncate text-muted-foreground">{m.codex_account_unset()}</span>;
    }
    const account = codexAccounts.get(accountId);
    if (!account) {
      return <span className="truncate text-muted-foreground">{m.codex_account_missing()}</span>;
    }
    const label = account.email?.trim() ? account.email : account.account_id;
    return <span className="truncate text-foreground">{label}</span>;
  }
  return <span className="truncate text-muted-foreground">—</span>;
}

function renderApiKeyCell(upstream: UpstreamForm, showApiKeys: boolean) {
  const value = showApiKeys ? upstream.apiKey : toMaskedApiKey(upstream.apiKey);
  return value.trim() ? (
    <span className="truncate text-foreground">{value}</span>
  ) : (
    <span className="truncate text-muted-foreground">{m.common_optional()}</span>
  );
}

function renderProxyUrlCell(upstream: UpstreamForm, showApiKeys: boolean) {
  const rawValue = upstream.proxyUrl;
  const value = showApiKeys ? rawValue : toMaskedProxyUrl(rawValue);
  return value.trim() ? (
    <span className="truncate text-foreground">{value}</span>
  ) : (
    <span className="truncate text-muted-foreground">{m.upstreams_proxy_direct()}</span>
  );
}

function renderUpstreamCell(
  columnId: UpstreamColumnId,
  upstream: UpstreamForm,
  showApiKeys: boolean,
  kiroAccounts: KiroAccountMap,
  codexAccounts: CodexAccountMap,
) {
  switch (columnId) {
    case "id":
      return renderTextCell(upstream.id, "openai-default");
    case "provider":
      return renderTextCell(upstream.provider, "openai");
    case "account":
      return renderAccountCell(upstream, kiroAccounts, codexAccounts);
    case "baseUrl":
      return renderTextCell(upstream.baseUrl, "https://api.openai.com");
    case "apiKey":
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
  disableDelete: boolean;
  onEdit: () => void;
  onCopy: () => void;
  onToggleEnabled: () => void;
  onDelete: () => void;
};

function UpstreamRowActions({
  rowLabel,
  enabled,
  disableDelete,
  onEdit,
  onCopy,
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
          onClick={onCopy}
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
  index: number;
  columns: readonly UpstreamColumnDefinition[];
  showApiKeys: boolean;
  kiroAccounts: KiroAccountMap;
  codexAccounts: CodexAccountMap;
  disableDelete: boolean;
  onEdit: (index: number) => void;
  onCopy: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

function UpstreamsTableRow({
  upstream,
  index,
  columns,
  showApiKeys,
  kiroAccounts,
  codexAccounts,
  disableDelete,
  onEdit,
  onCopy,
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
            {renderUpstreamCell(column.id, upstream, showApiKeys, kiroAccounts, codexAccounts)}
          </div>
        </td>
      ))}
      <UpstreamRowActions
        rowLabel={rowLabel}
        enabled={upstream.enabled}
        disableDelete={disableDelete}
        onEdit={() => onEdit(index)}
        onCopy={() => onCopy(index)}
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
  kiroAccounts: KiroAccountMap;
  codexAccounts: CodexAccountMap;
  disableDelete: boolean;
  onEdit: (index: number) => void;
  onCopy: (index: number) => void;
  onToggleEnabled: (index: number) => void;
  onDelete: (index: number) => void;
};

export function UpstreamsTable({
  upstreams,
  columns,
  showApiKeys,
  kiroAccounts,
  codexAccounts,
  disableDelete,
  onEdit,
  onCopy,
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
              kiroAccounts={kiroAccounts}
              codexAccounts={codexAccounts}
              disableDelete={disableDelete}
              onEdit={onEdit}
              onCopy={onCopy}
              onToggleEnabled={onToggleEnabled}
              onDelete={onDelete}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}
