import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Loader2, RefreshCw, Search, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

/** 官方 Codex 目录 slug；官方模型始终保留，不允许作为“额外模型”重复添加。 */
const OFFICIAL_CODEX_MODEL_IDS: ReadonlySet<string> = new Set([
  "gpt-6-astra",
  "gpt-6.1-sol",
  "gpt-6-sol",
  "gpt-6-luna",
  "gpt-5.6-sol",
  "gpt-5.6-terra",
  "gpt-5.6-luna",
  "gpt-daybreak-blue-latest",
  "gpt-daybreak-red-latest",
  "gpt-5.5",
  "codex-auto-review",
]);

const QWEN_MODEL_ID = "qwen3.8-27b-uncensored";

type CodexModelMenuCardProps = {
  models: readonly string[];
  onChange: (models: string[]) => void;
  isDirty: boolean;
};

/** 候选来自代理已缓存的渠道模型目录（只读缓存，不实时请求上游）。 */
async function readCandidates() {
  return await invoke<string[]>("read_codex_extra_model_candidates");
}

export function CodexModelMenuCard({ models, onChange, isDirty }: CodexModelMenuCardProps) {
  const [draft, setDraft] = useState("");
  const [feedback, setFeedback] = useState("");
  const [candidates, setCandidates] = useState<string[]>([]);
  const [search, setSearch] = useState("");
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    let cancelled = false;
    readCandidates()
      .then((next) => {
        if (!cancelled) {
          setCandidates(next);
        }
      })
      .catch((error: unknown) => {
        // 读取失败只影响快速选择，手工输入仍可用。
        console.warn("[codex-model-menu-card] failed to read model candidates", parseError(error));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // 已选模型也放进列表，便于直接取消勾选；官方模型始终保留，不作为候选。
  const options = [...new Set([...candidates, ...models])]
    .filter((model) => !OFFICIAL_CODEX_MODEL_IDS.has(model))
    .sort();
  const keyword = search.trim().toLowerCase();
  const visibleOptions = options.filter((model) => model.toLowerCase().includes(keyword));

  const addModel = (raw: string) => {
    const model = raw.trim();
    if (!model) {
      return;
    }
    if (OFFICIAL_CODEX_MODEL_IDS.has(model)) {
      setFeedback(m.codex_model_menu_already_official({ model }));
      return;
    }
    if (models.includes(model)) {
      setFeedback(m.codex_model_menu_duplicate({ model }));
      return;
    }
    setFeedback("");
    onChange([...models, model]);
  };

  const removeModel = (model: string) => {
    setFeedback("");
    onChange(models.filter((item) => item !== model));
  };

  const refreshCandidates = async () => {
    setRefreshing(true);
    setFeedback("");
    try {
      // 先让代理重新探测各渠道模型目录，再读取最新缓存。
      await invoke("refresh_dashboard_model_discovery");
      setCandidates(await readCandidates());
    } catch (error) {
      setFeedback(m.codex_model_menu_refresh_failed({ error: parseError(error) }));
    } finally {
      setRefreshing(false);
    }
  };

  const handleAddDraft = () => {
    if (!draft.trim()) {
      return;
    }
    addModel(draft);
    setDraft("");
  };

  return (
    <Card data-slot="codex-model-menu-card">
      <CardHeader>
        <CardTitle>{m.codex_model_menu_title()}</CardTitle>
        <CardDescription>{m.codex_model_menu_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <p className="text-xs text-muted-foreground">{m.codex_model_menu_official_hint()}</p>

        {models.length ? (
          <div className="flex flex-wrap items-center gap-2" role="list">
            {models.map((model) => (
              <Badge key={model} variant="secondary" className="gap-1 pr-1" role="listitem">
                <span className="max-w-56 truncate font-mono">{model}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  className="size-5 rounded-full"
                  aria-label={m.codex_model_menu_remove({ model })}
                  onClick={() => removeModel(model)}
                >
                  <X className="size-3" aria-hidden="true" />
                </Button>
              </Badge>
            ))}
          </div>
        ) : (
          <p className="text-sm text-muted-foreground">{m.codex_model_menu_empty()}</p>
        )}

        <div className="grid gap-2">
          <div className="flex items-center gap-2">
            <div className="relative min-w-0 flex-1">
              <Search
                className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
                aria-hidden="true"
              />
              <Input
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder={m.codex_model_menu_search_placeholder()}
                aria-label={m.codex_model_menu_search_placeholder()}
                className="pl-8"
              />
            </div>
            <Button
              type="button"
              variant="outline"
              size="icon"
              aria-label={m.codex_model_menu_refresh()}
              title={m.codex_model_menu_refresh()}
              disabled={refreshing}
              onClick={() => void refreshCandidates()}
            >
              {refreshing ? (
                <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              ) : (
                <RefreshCw className="size-4" aria-hidden="true" />
              )}
            </Button>
          </div>
          <ScrollArea className="h-40 rounded-md border">
            <div className="divide-y" data-slot="codex-model-candidates">
              {visibleOptions.length ? (
                visibleOptions.map((model) => (
                  <Label
                    key={model}
                    className="flex min-h-10 cursor-pointer items-center gap-3 px-3 py-2 font-normal hover:bg-muted/50"
                  >
                    <Checkbox
                      checked={models.includes(model)}
                      onCheckedChange={(value) =>
                        value === true ? addModel(model) : removeModel(model)
                      }
                    />
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">{model}</span>
                  </Label>
                ))
              ) : (
                <p className="px-3 py-6 text-center text-sm text-muted-foreground">
                  {options.length
                    ? m.available_models_no_matches()
                    : m.codex_model_menu_no_candidates()}
                </p>
              )}
            </div>
          </ScrollArea>
        </div>

        <div className="grid gap-2">
          <Label htmlFor="codex-extra-model-input">{m.codex_model_menu_input_label()}</Label>
          <div className="flex items-center gap-2">
            <Input
              id="codex-extra-model-input"
              value={draft}
              placeholder={m.codex_model_menu_input_placeholder()}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  handleAddDraft();
                }
              }}
            />
            <Button type="button" variant="outline" onClick={handleAddDraft} disabled={!draft.trim()}>
              {m.codex_model_menu_add()}
            </Button>
          </div>
          <div>
            <Button
              type="button"
              size="sm"
              variant="secondary"
              onClick={() => addModel(QWEN_MODEL_ID)}
              disabled={models.includes(QWEN_MODEL_ID)}
            >
              {m.codex_model_menu_add_qwen()}
            </Button>
          </div>
          {feedback ? <p className="text-xs text-destructive">{feedback}</p> : null}
        </div>

        {isDirty ? (
          <p className="text-xs text-muted-foreground" data-slot="codex-model-menu-save-hint">
            {m.codex_model_menu_save_first()}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
