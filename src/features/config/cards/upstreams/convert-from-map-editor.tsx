import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { EditorField } from "@/features/config/cards/upstreams/editor-fields";
import type { InboundApiFormat, UpstreamForm } from "@/features/config/types";

const INBOUND_FORMAT_OPTIONS: ReadonlyArray<{
  value: InboundApiFormat;
  label: string;
}> = [
  { value: "openai_chat", label: "/v1/chat/completions" },
  { value: "openai_responses", label: "/v1/responses" },
  { value: "anthropic_messages", label: "/v1/messages" },
  { value: "gemini", label: "Gemini (/v1beta/...)" },
];

type ConvertFromMapEditorProps = {
  providers: readonly string[];
  value: UpstreamForm["convertFromMap"];
  onChange: (next: UpstreamForm["convertFromMap"]) => void;
};

function normalizeProviders(values: readonly string[]) {
  const output: string[] = [];
  const seen = new Set<string>();
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) {
      continue;
    }
    if (seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    output.push(trimmed);
  }
  return output;
}

function toggleInboundFormat(
  current: readonly InboundApiFormat[],
  format: InboundApiFormat,
  checked: boolean,
) {
  if (!checked) {
    return current.filter((value) => value !== format);
  }
  if (current.includes(format)) {
    return [...current];
  }
  return [...current, format];
}

export function ConvertFromMapEditor({
  providers,
  value,
  onChange,
}: ConvertFromMapEditorProps) {
  const normalizedProviders = normalizeProviders(providers);
  if (!normalizedProviders.length) {
    return null;
  }

  return (
    <div data-slot="convert-from-map-editor" className="contents">
      <EditorField
        label="可转格式"
        tooltip="声明允许从哪些入站 API 格式转换后再使用该 provider。未勾选则仅支持该 provider 的 native 格式。"
      >
        <div className="space-y-3">
          {normalizedProviders.map((provider, index) => {
            const selected = value[provider] ?? [];
            return (
              <div key={provider} className="space-y-2">
                <div className="text-sm font-medium text-foreground">{provider}</div>
                <div className="grid gap-2 sm:grid-cols-2">
                  {INBOUND_FORMAT_OPTIONS.map((option) => {
                    const checked = selected.includes(option.value);
                    return (
                      <Label
                        key={option.value}
                        className="inline-flex items-center gap-2 text-sm font-normal"
                      >
                        <Checkbox
                          checked={checked}
                          onCheckedChange={(nextChecked) => {
                            const nextFormats = toggleInboundFormat(
                              selected,
                              option.value,
                              nextChecked === true,
                            );
                            const next: UpstreamForm["convertFromMap"] = {
                              ...value,
                              [provider]: nextFormats,
                            };
                            if (!nextFormats.length) {
                              delete next[provider];
                            }
                            onChange(next);
                          }}
                        />
                        <span className="text-muted-foreground">{option.label}</span>
                      </Label>
                    );
                  })}
                </div>
                {index + 1 < normalizedProviders.length ? <Separator /> : null}
              </div>
            );
          })}
        </div>
      </EditorField>
    </div>
  );
}
