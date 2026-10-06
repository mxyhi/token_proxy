import { useState } from "react";
import { Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { sortUpstreamsByPriority } from "@/features/config/cards/upstreams/constants";
import type { UpstreamForm } from "../types";
import { m } from "@/paraglide/messages.js";

type Props = {
  upstreams: readonly UpstreamForm[];
  selected: readonly string[];
  onChange: (ids: string[]) => void;
};

const GRID_CLASS = "grid grid-cols-[auto_minmax(0,1fr)_minmax(0,10rem)_4rem] items-center gap-x-3 px-3";

/** 指定渠道选择器：顺序与渠道表一致（优先级降序，同级保持配置顺序）。 */
export function ApiKeyUpstreamPicker({ upstreams, selected, onChange }: Props) {
  const [search, setSearch] = useState("");
  const selectedSet = new Set(selected);
  const upstreamIds = new Set(upstreams.map((upstream) => upstream.id));
  // 已删除渠道仍保留在绑定中，单独列出以便用户手动移除。
  const missing = selected.filter((id) => !upstreamIds.has(id));
  const query = search.trim().toLowerCase();
  const filtered = sortUpstreamsByPriority(upstreams)
    .map((entry) => entry.upstream)
    .filter((upstream) =>
      `${upstream.id} ${upstream.providers.join(" ")}`.toLowerCase().includes(query),
    );
  const filteredSelected = filtered.filter((upstream) => selectedSet.has(upstream.id)).length;
  const headerChecked =
    filtered.length > 0 && filteredSelected === filtered.length
      ? true
      : filteredSelected > 0
        ? "indeterminate"
        : false;

  function toggle(id: string, checked: boolean) {
    onChange(checked ? [...selected, id] : selected.filter((value) => value !== id));
  }

  function toggleFiltered(checked: boolean) {
    const ids = new Set(filtered.map((upstream) => upstream.id));
    onChange(
      checked
        ? [...new Set([...selected, ...ids])]
        : selected.filter((id) => !ids.has(id)),
    );
  }

  return (
    <div className="overflow-hidden rounded-md border border-border/60 bg-background/60">
      <div className="flex items-center gap-3 border-b border-border/60 p-2">
        <div className="relative min-w-0 flex-1">
          <Search
            className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            aria-label={m.api_keys_search()}
            placeholder={m.api_keys_search()}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            className="h-8 pl-8"
          />
        </div>
        <span className="shrink-0 pr-1 text-xs tabular-nums text-muted-foreground">
          {m.api_keys_selected_summary({
            selected: selected.length - missing.length,
            total: upstreams.length,
          })}
        </span>
      </div>
      <div
        className={`${GRID_CLASS} border-b border-border/60 bg-background/40 py-2 text-xs font-medium text-muted-foreground`}
      >
        <Checkbox
          aria-label={m.api_keys_select_all()}
          checked={headerChecked}
          disabled={!filtered.length}
          onCheckedChange={(checked) => toggleFiltered(checked === true)}
        />
        <span>{m.upstreams_column_id()}</span>
        <span>{m.upstreams_column_provider()}</span>
        <span className="text-right">{m.upstreams_column_priority()}</span>
      </div>
      <div className="max-h-64 overflow-y-auto">
        {missing.map((id) => (
          <label
            key={id}
            className={`${GRID_CLASS} cursor-pointer border-b border-border/40 py-2 text-sm hover:bg-muted/50`}
          >
            <Checkbox checked onCheckedChange={() => toggle(id, false)} />
            <span className="flex min-w-0 items-center gap-2">
              <span className="truncate font-medium text-destructive">{id}</span>
              <Badge variant="destructive">{m.api_keys_expired()}</Badge>
            </span>
            <span />
            <span />
          </label>
        ))}
        {filtered.map((upstream) => (
          <label
            key={upstream.id}
            className={`${GRID_CLASS} cursor-pointer border-b border-border/40 py-2 text-sm last:border-b-0 hover:bg-muted/50`}
          >
            <Checkbox
              checked={selectedSet.has(upstream.id)}
              onCheckedChange={(checked) => toggle(upstream.id, checked === true)}
            />
            <span className="flex min-w-0 items-center gap-2">
              <span
                className={
                  upstream.enabled
                    ? "truncate font-medium"
                    : "truncate font-medium text-muted-foreground"
                }
              >
                {upstream.id}
              </span>
              {upstream.enabled ? null : (
                <Badge variant="secondary">{m.common_disabled()}</Badge>
              )}
            </span>
            <span className="truncate text-xs text-muted-foreground">
              {upstream.providers.join(", ")}
            </span>
            <span className="text-right text-xs tabular-nums text-muted-foreground">
              {upstream.priority.trim() || "0"}
            </span>
          </label>
        ))}
        {filtered.length ? null : (
          <p className="px-3 py-6 text-center text-sm text-muted-foreground">
            {m.api_keys_no_match()}
          </p>
        )}
      </div>
    </div>
  );
}
