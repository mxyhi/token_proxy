import { Loader2, Plus, RefreshCw, Search, X } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { isAccountProviderKind } from "@/features/config/cards/upstreams/upstream-editor-helpers";
import type { UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";
import { invoke } from "@tauri-apps/api/core";

type AvailableModelsEditorProps = {
  draft: UpstreamForm;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
};

function mergeModels(...groups: readonly (readonly string[])[]) {
  const models = new Set<string>();
  for (const group of groups) {
    for (const value of group) {
      const model = value.trim();
      if (model) {
        models.add(model);
      }
    }
  }
  return [...models].sort();
}

function firstApiKey(value: string) {
  return value
    .split(/[,\n]/)
    .map((item) => item.trim())
    .find(Boolean) ?? "";
}

export function AvailableModelsEditor({
  draft,
  onChangeDraft,
}: AvailableModelsEditorProps) {
  const [options, setOptions] = useState(() => mergeModels(draft.availableModels));
  const [search, setSearch] = useState("");
  const [customModel, setCustomModel] = useState("");
  const [fetching, setFetching] = useState(false);
  const [feedback, setFeedback] = useState("");
  const provider = draft.providers[0]?.trim() ?? "";
  const isAccount = isAccountProviderKind(provider);
  const canFetch = isAccount
    ? !!draft.accountId.trim()
    : ["openai", "openai-response", "anthropic", "gemini"].includes(provider) &&
      !!draft.baseUrl.trim();
  const selectedModels = mergeModels(draft.availableModels);
  const selectedModelSet = new Set(selectedModels);
  const visibleOptions = mergeModels(options, selectedModels).filter((model) =>
    model.toLowerCase().includes(search.trim().toLowerCase()),
  );
  const visibleSelectedCount = visibleOptions.filter((model) =>
    selectedModelSet.has(model),
  ).length;
  const allVisibleModelsSelected =
    visibleOptions.length > 0 && visibleSelectedCount === visibleOptions.length;
  const visibleModelsSelection = allVisibleModelsSelected
    ? true
    : visibleSelectedCount > 0
      ? "indeterminate"
      : false;
  const visibleModelsBulkLabel = allVisibleModelsSelected
    ? m.available_models_clear_all()
    : m.available_models_select_all();

  const updateSelectedModels = (models: readonly string[]) => {
    onChangeDraft({ availableModels: mergeModels(models) });
  };

  const toggleModel = (model: string, checked: boolean) => {
    if (checked) {
      updateSelectedModels([...selectedModels, model]);
      return;
    }
    updateSelectedModels(selectedModels.filter((candidate) => candidate !== model));
  };

  const toggleVisibleModels = () => {
    if (!visibleOptions.length) {
      return;
    }
    if (allVisibleModelsSelected) {
      const visibleModelSet = new Set(visibleOptions);
      updateSelectedModels(selectedModels.filter((model) => !visibleModelSet.has(model)));
      return;
    }
    updateSelectedModels([...selectedModels, ...visibleOptions]);
  };

  const addCustomModel = () => {
    const model = customModel.trim();
    if (!model) {
      return;
    }
    // 手工模型同时进入候选与已选集合，避免用户还要再勾选一次。
    setOptions((current) => mergeModels(current, [model]));
    updateSelectedModels([...selectedModels, model]);
    setCustomModel("");
    setFeedback("");
  };

  const fetchModels = async () => {
    if (fetching) {
      return;
    }
    if (!canFetch) {
      setFeedback(
        isAccount ? m.upstreams_account_missing() : m.available_models_base_url_required(),
      );
      return;
    }
    setFetching(true);
    setFeedback("");
    try {
      const models = await invoke<string[]>("fetch_upstream_models", {
        provider,
        baseUrl: draft.baseUrl.trim(),
        apiKey: firstApiKey(draft.apiKeys),
        // 账户鉴权由后端解析固定身份；凭据不进入前端或自定义 Base URL。
        ...(isAccount
          ? { accountId: draft.accountId.trim(), proxyUrl: draft.proxyUrl.trim() }
          : {}),
      });
      if (!models.length) {
        setFeedback(m.available_models_sync_empty());
        return;
      }
      setOptions((current) => mergeModels(current, models));
      setFeedback(m.available_models_sync_success({ count: models.length }));
      console.info("[upstream-models] fetched model candidates", {
        provider,
        count: models.length,
      });
    } catch (error) {
      console.error("[upstream-models] failed to fetch model candidates", {
        provider,
        error,
      });
      setFeedback(String(error));
    } finally {
      setFetching(false);
    }
  };

  return (
    <div data-slot="available-models-editor" className="space-y-4">
      <ToggleGroup
        type="single"
        variant="outline"
        value={draft.availableModelsMode}
        onValueChange={(value) => {
          if (value === "all" || value === "selected") {
            onChangeDraft({ availableModelsMode: value });
          }
        }}
        aria-label={m.field_available_models()}
      >
        <ToggleGroupItem value="all">{m.available_models_all()}</ToggleGroupItem>
        <ToggleGroupItem value="selected">
          {m.available_models_selected()}
        </ToggleGroupItem>
      </ToggleGroup>

      {draft.availableModelsMode === "all" ? (
        <p className="text-sm text-muted-foreground">{m.available_models_all_desc()}</p>
      ) : (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-muted-foreground">
              {m.available_models_selected_count({ count: selectedModels.length })}
            </span>
            {selectedModels.map((model) => (
              <Badge key={model} variant="secondary" className="gap-1 pr-1">
                <span className="max-w-48 truncate">{model}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  className="size-5 rounded-full"
                  aria-label={m.available_models_remove({ model })}
                  onClick={() => toggleModel(model, false)}
                >
                  <X className="size-3" aria-hidden="true" />
                </Button>
              </Badge>
            ))}
          </div>

          <div className="flex items-center gap-2">
            <div className="relative min-w-0 flex-1">
              <Search
                className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <Input
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder={m.available_models_search_placeholder()}
                className="pl-8"
              />
            </div>
            <Button
              type="button"
              variant="outline"
              size="icon"
              aria-label={m.available_models_sync()}
              title={m.available_models_sync()}
              disabled={!canFetch || fetching}
              onClick={fetchModels}
            >
              {fetching ? (
                <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              ) : (
                <RefreshCw className="size-4" aria-hidden="true" />
              )}
            </Button>
          </div>

          {feedback ? <p className="text-xs text-muted-foreground">{feedback}</p> : null}

          <ScrollArea className="h-40 rounded-md border">
            <div className="divide-y">
              {visibleOptions.length ? (
                <>
                  <Label className="sticky top-0 z-10 flex min-h-9 cursor-pointer items-center gap-3 bg-muted px-3 py-2 font-normal">
                    <Checkbox
                      checked={visibleModelsSelection}
                      onCheckedChange={toggleVisibleModels}
                      aria-label={visibleModelsBulkLabel}
                    />
                    <span className="text-xs font-medium">{visibleModelsBulkLabel}</span>
                  </Label>
                  {visibleOptions.map((model) => (
                    <Label
                      key={model}
                      className="flex min-h-10 cursor-pointer items-center gap-3 px-3 py-2 font-normal hover:bg-muted/50"
                    >
                      <Checkbox
                        checked={selectedModelSet.has(model)}
                        onCheckedChange={(value) => toggleModel(model, value === true)}
                      />
                      <span className="min-w-0 flex-1 truncate font-mono text-xs">{model}</span>
                    </Label>
                  ))}
                </>
              ) : (
                <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                  {options.length
                    ? m.available_models_no_matches()
                    : m.available_models_no_options()}
                </p>
              )}
            </div>
          </ScrollArea>

          <div className="flex items-center gap-2">
            <Input
              value={customModel}
              onChange={(event) => setCustomModel(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  addCustomModel();
                }
              }}
              placeholder={m.available_models_custom_placeholder()}
            />
            <Button type="button" variant="outline" onClick={addCustomModel}>
              <Plus className="size-4" aria-hidden="true" />
              {m.available_models_add_custom()}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
