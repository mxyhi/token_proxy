import type { InboundApiFormat } from "@/features/config/types";

/**
 * 入站 API 格式选项（用于 UI 展示 & 配置序列化）。
 *
 * 注意：
 * - 该项目后端对每个 provider 都有 “native 入站格式” 的定义（无需 convert_from_map 即可命中）。
 * - convert_from_map 只应声明 “非 native 的额外允许格式”，否则会产生冗余、也会让 UI 出现重复含义选项。
 */
export const INBOUND_FORMAT_OPTIONS: ReadonlyArray<{
  value: InboundApiFormat;
  label: string;
}> = [
  { value: "openai_chat", label: "/v1/chat/completions" },
  { value: "openai_responses", label: "/v1/responses" },
  { value: "anthropic_messages", label: "/v1/messages" },
  { value: "gemini", label: "Gemini (/v1beta/...)" },
];

/**
 * 与后端 `native_inbound_formats_for_provider()` 对齐：
 * - 这些格式对对应 provider 来说是“原生支持”，不应在 UI 的“可转格式”中展示。
 */
const PROVIDER_NATIVE_INBOUND_FORMATS: Readonly<
  Partial<Record<string, readonly InboundApiFormat[]>>
> = {
  openai: ["openai_chat"],
  "openai-response": ["openai_responses"],
  anthropic: ["anthropic_messages"],
  gemini: ["gemini"],
  kiro: ["anthropic_messages"],
  codex: ["openai_responses"],
};

export function getProviderNativeInboundFormats(provider: string) {
  return PROVIDER_NATIVE_INBOUND_FORMATS[provider] ?? [];
}

export function createNativeInboundFormatSet(providers: readonly string[]) {
  const set = new Set<InboundApiFormat>();
  for (const rawProvider of providers) {
    const provider = rawProvider.trim();
    if (!provider) continue;
    for (const format of getProviderNativeInboundFormats(provider)) {
      set.add(format);
    }
  }
  return set;
}

export function removeInboundFormatsInSet(
  formats: readonly InboundApiFormat[],
  nativeFormats: ReadonlySet<InboundApiFormat>,
) {
  if (!formats.length) {
    return [];
  }
  if (!nativeFormats.size) {
    return [...formats];
  }
  return formats.filter((format) => !nativeFormats.has(format));
}
