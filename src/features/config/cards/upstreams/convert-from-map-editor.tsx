import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { EditorField } from "@/features/config/cards/upstreams/editor-fields";
import {
  createNativeInboundFormatSet,
  INBOUND_FORMAT_OPTIONS,
  removeInboundFormatsInSet,
} from "@/features/config/inbound-formats";
import type { InboundApiFormat, UpstreamForm } from "@/features/config/types";

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

  // “可转格式”只展示“非原生格式”的转换授权：
  // - 若该 upstream 已包含某入站格式的原生 provider，则该入站格式无需也不应再被其它 provider 转换兜底（避免误导）。
  const nativeFormatsInUpstream = createNativeInboundFormatSet(normalizedProviders);
  const providersKey = normalizedProviders.join("|");
  const selectedCount = normalizedProviders.reduce((count, provider) => {
    const selected = removeInboundFormatsInSet(value[provider] ?? [], nativeFormatsInUpstream);
    return count + selected.length;
  }, 0);
  const summaryLabel = selectedCount ? `已选 ${selectedCount} 项` : "未开启";

  return (
    <div data-slot="convert-from-map-editor" className="contents">
      <EditorField
        label="可转格式"
        tooltip="声明允许从哪些入站 API 格式转换后再使用该 provider。未勾选则仅支持该 provider 的 native 格式。"
      >
        <details
          key={providersKey}
          className="space-y-2"
          data-slot="convert-from-map-details"
        >
          <summary className="cursor-pointer select-none text-sm text-muted-foreground hover:text-foreground">
            {summaryLabel}
          </summary>
          <div className="space-y-3">
            {normalizedProviders.map((provider, index) => {
              const selected = removeInboundFormatsInSet(
                value[provider] ?? [],
                nativeFormatsInUpstream,
              );
              const visibleOptions = INBOUND_FORMAT_OPTIONS.filter(
                (option) => !nativeFormatsInUpstream.has(option.value),
              );
              return (
                <div key={provider} className="space-y-2">
                  <div className="text-sm font-medium text-foreground">{provider}</div>
                  <div className="space-y-2">
                    {visibleOptions.length ? (
                      visibleOptions.map((option) => {
                        const checked = selected.includes(option.value);
                        return (
                          <Label
                            key={option.value}
                            className="flex items-center gap-2 text-sm font-normal"
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
                      })
                    ) : (
                      <div className="text-xs text-muted-foreground">
                        无可用转换选项（该 upstream 已包含对应原生 provider）。
                      </div>
                    )}
                  </div>
                  {index + 1 < normalizedProviders.length ? <Separator /> : null}
                </div>
              );
            })}
          </div>
        </details>
      </EditorField>
    </div>
  );
}
