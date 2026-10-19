import { useId, useState } from "react";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { m } from "@/paraglide/messages.js";
import type { CodexModelCapabilities } from "../types";

const REASONING_EFFORTS = ["none", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
const MAX_CONTEXT_WINDOW = 4294967295;

type ReasoningEffort = CodexModelCapabilities["reasoning_effort"];

function isReasoningEffort(value: string): value is ReasoningEffort {
  return REASONING_EFFORTS.some((effort) => effort === value);
}

export interface CodexModelCapabilitiesEditorProps {
  model: string;
  value: CodexModelCapabilities;
  onChange: (value: CodexModelCapabilities) => void;
}

export function CodexModelCapabilitiesEditor({
  model,
  value,
  onChange,
}: CodexModelCapabilitiesEditorProps) {
  const contextId = useId();
  const visionId = useId();
  const reasoningId = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);

  const commitContextWindow = () => {
    if (draft === null) return;
    const text = draft.trim();
    const next = /^\d+$/.test(text) ? Number(text) : Number.NaN;
    if (!Number.isInteger(next) || next < 1 || next > MAX_CONTEXT_WINDOW) {
      setInvalid(true);
      return;
    }
    setInvalid(false);
    setDraft(null);
    if (next !== value.context_window) {
      onChange({ ...value, context_window: next });
    }
  };

  return (
    <section aria-label={model} className="grid gap-4 sm:grid-cols-3">
      <p className="text-sm font-medium sm:col-span-3">{model}</p>
      <div className="grid gap-2">
        <Label htmlFor={contextId}>{m.codex_model_capabilities_context()}</Label>
        <Input
          id={contextId}
          type="number"
          inputMode="numeric"
          min={1}
          max={MAX_CONTEXT_WINDOW}
          step={1}
          value={draft ?? String(value.context_window)}
          aria-invalid={invalid}
          aria-describedby={invalid ? `${contextId}-error` : undefined}
          onChange={(event) => {
            setDraft(event.target.value);
            if (invalid) setInvalid(false);
          }}
          onBlur={commitContextWindow}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitContextWindow();
            }
          }}
        />
        {invalid && (
          <p id={`${contextId}-error`} role="alert" className="text-sm text-destructive">
            {m.codex_model_capabilities_invalid_context()}
          </p>
        )}
      </div>
      <div className="grid gap-2">
        <Label htmlFor={visionId}>{m.codex_model_capabilities_vision()}</Label>
        <Switch
          id={visionId}
          checked={value.supports_vision}
          onCheckedChange={(checked) => onChange({ ...value, supports_vision: checked })}
        />
      </div>
      <div className="grid gap-2">
        <Label htmlFor={reasoningId}>{m.codex_model_capabilities_reasoning()}</Label>
        <Select
          value={value.reasoning_effort}
          onValueChange={(next) => {
            if (isReasoningEffort(next)) {
              onChange({ ...value, reasoning_effort: next });
            }
          }}
        >
          <SelectTrigger id={reasoningId}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {REASONING_EFFORTS.map((effort) => (
              <SelectItem key={effort} value={effort}>
                {effort}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </section>
  );
}

