import { useState } from "react";
import { X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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

export function CodexModelMenuCard({ models, onChange, isDirty }: CodexModelMenuCardProps) {
  const [draft, setDraft] = useState("");
  const [feedback, setFeedback] = useState("");

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
