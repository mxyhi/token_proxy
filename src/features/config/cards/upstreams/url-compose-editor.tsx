import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  type UrlComposeConfig,
  type UrlComposeEndpoint,
  type UrlComposeFamily,
} from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

// ══════════ MY-URL-COMPOSE PATCH G3 START ══════════
// 「接口地址组合」面板：与 UI Demo（demos/260922-01-channel-url-compose）交互一致。
// 语义与后端 my_url_compose 模块一一对应：出站 URL = 基础地址 + 特殊拼接 +
// 版本段替换后的客户端路径；后端为纯拼接，重复段仅在此处提示、不做修正。

const FAMILIES: readonly UrlComposeFamily[] = [
  "openai",
  "openai-response",
  "anthropic",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  "dashscope",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
];

const DEFAULT_SUFFIX: Record<UrlComposeFamily, string> = {
  openai: "/v1/chat/completions",
  "openai-response": "/v1/responses",
  anthropic: "/v1/messages",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  dashscope: "/v1/services/aigc/text-generation/generation",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
};

const FAMILY_LABEL: Record<UrlComposeFamily, string> = {
  openai: m.url_compose_family_openai(),
  "openai-response": m.url_compose_family_openai_response(),
  anthropic: m.url_compose_family_anthropic(),
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  dashscope: m.url_compose_family_dashscope(),
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
};

const FAMILY_TAG: Record<UrlComposeFamily, string> = {
  openai: m.url_compose_family_openai_tag(),
  "openai-response": m.url_compose_family_openai_response_tag(),
  anthropic: m.url_compose_family_anthropic_tag(),
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  dashscope: m.url_compose_family_dashscope_tag(),
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
};

/** 前缀输入提示示例：openai 系展示 /openai，anthropic 展示 /anthropic。 */
const PREFIX_EXAMPLE: Record<UrlComposeFamily, string> = {
  openai: "/openai",
  "openai-response": "/openai",
  anthropic: "/anthropic",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  dashscope: "/api",
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
};

const MAP_EXAMPLES: Record<UrlComposeFamily, string[]> = {
  openai: ["/v1/chat/completions", "/v1/models", "/v1/embeddings", "/v1/completions"],
  "openai-response": ["/v1/responses", "/v1/responses/resp_123", "/v1/responses/input_tokens", "/v1/models"],
  anthropic: ["/v1/messages", "/v1/messages/count_tokens", "/v1/models", "/v1/complete"],
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════
  dashscope: [
    "/v1/services/aigc/text-generation/generation",
    "/v1/services/aigc/multimodal-generation/generation",
    "/v1/services/embeddings/text-embedding/text-embedding",
    "/v1/services/rerank/text-rerank/text-rerank",
  ],
  // ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════
};

function normalizeSegment(value: string): string {
  const trimmed = value.trim().replace(/\/+$/, "");
  if (!trimmed) {
    return "";
  }
  return trimmed.startsWith("/") ? trimmed : `/${trimmed}`;
}

/** suffix 第一段即版本段（`/v1`、`/v3`、`/abc` 皆可）；空 suffix 回退 `/v1`。 */
function versionSegment(suffix: string): string {
  const normalized = normalizeSegment(suffix);
  if (!normalized) {
    return "/v1";
  }
  const segment = normalized.split("/").find((part) => part !== "");
  return segment ? `/${segment}` : "/v1";
}

function findDuplicateSegment(url: string): string | null {
  try {
    const segments = new URL(url).pathname.split("/").filter(Boolean);
    for (let index = 1; index < segments.length; index += 1) {
      if (segments[index] === segments[index - 1]) {
        return `/${segments[index]}`;
      }
    }
  } catch {
    // 非法 URL 时不提示
  }
  return null;
}

type ComposePreview = {
  base: string;
  prefix: string;
  suffix: string;
  version: string;
  full: string;
};

function composePreview(baseUrl: string, endpoint: UrlComposeEndpoint): ComposePreview {
  const base = baseUrl.trim().replace(/\/+$/, "");
  const prefix = normalizeSegment(endpoint.prefix);
  const suffix = normalizeSegment(endpoint.suffix);
  const version = versionSegment(suffix);
  return {
    base,
    prefix,
    suffix,
    version,
    // 纯拼接：主端点出站 URL = base + prefix + suffix（version/tail 仅用于分色显示）。
    full: `${base}${prefix}${suffix}`,
  };
}

type UrlComposeEditorProps = {
  providers: string[];
  baseUrl: string;
  value: UrlComposeConfig;
  onChange: (value: UrlComposeConfig) => void;
  /** 新建渠道（含复制）时为 true：已勾选家族缺失时预填默认后缀；编辑既有渠道不传。 */
  prefillSuffixDefaults?: boolean;
};

export function UrlComposeEditor({
  providers,
  baseUrl,
  value,
  onChange,
  prefillSuffixDefaults = false,
}: UrlComposeEditorProps) {
  const [expanded, setExpanded] = useState(false);
  const [mapFamily, setMapFamily] = useState<UrlComposeFamily | null>(null);
  // 已预填默认后缀的家族：用户清空后不再回填；对话框关闭即卸载、自然重置。
  const prefilledFamiliesRef = useRef<Set<UrlComposeFamily>>(new Set());

  const selected = useMemo(
    () => FAMILIES.filter((family) => providers.includes(family)),
    [providers]
  );

  useEffect(() => {
    if (!prefillSuffixDefaults) {
      return;
    }
    const missing = selected.filter(
      (family) => !value[family] && !prefilledFamiliesRef.current.has(family)
    );
    // 出现过的家族一律记入已见集合：无论默认值来自预填还是草稿自带（复制新建），
    // 用户手动清空后都不再回填。
    for (const family of selected) {
      prefilledFamiliesRef.current.add(family);
    }
    if (!missing.length) {
      return;
    }
    const next: UrlComposeConfig = { ...value };
    for (const family of missing) {
      next[family] = { prefix: "", suffix: DEFAULT_SUFFIX[family] };
    }
    onChange(next);
    // value/onChange 每次渲染身份都会变化，重复执行由 missing 为空短路，不会造成循环或回填。
  }, [onChange, prefillSuffixDefaults, selected, value]);

  const endpointOf = useCallback(
    (family: UrlComposeFamily): UrlComposeEndpoint => value[family] ?? { prefix: "", suffix: "" },
    [value]
  );

  const update = useCallback(
    (family: UrlComposeFamily, patch: Partial<UrlComposeEndpoint>) => {
      const next: UrlComposeConfig = { ...value };
      const merged = { ...endpointOf(family), ...patch };
      if (!merged.prefix && !merged.suffix) {
        delete next[family];
      } else {
        next[family] = merged;
      }
      onChange(next);
    },
    [endpointOf, onChange, value]
  );

  const configuredCount = selected.filter((family) => {
    const endpoint = endpointOf(family);
    const prefix = normalizeSegment(endpoint.prefix);
    const suffix = normalizeSegment(endpoint.suffix);
    // 已定制 = prefix 非空，或 suffix 非空且不等于该家族默认后缀。
    return prefix !== "" || (suffix !== "" && suffix !== DEFAULT_SUFFIX[family]);
  }).length;

  const summary = !selected.length
    ? m.url_compose_summary_disabled()
    : configuredCount
      ? m.url_compose_summary_configured({ count: configuredCount })
      : m.url_compose_summary_default();

  // 子路径映射弹窗打开时：Escape 仅关闭弹窗（capture + stopPropagation，
  // 避免冒泡到 Radix 连带关闭编辑渠道对话框）。
  useEffect(() => {
    if (!mapFamily) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        setMapFamily(null);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [mapFamily]);

  const renderPreview = (family: UrlComposeFamily) => {
    const preview = composePreview(baseUrl, endpointOf(family));
    if (!preview.base) {
      return (
        <span className="text-muted-foreground text-xs italic">{m.url_compose_preview_empty_base()}</span>
      );
    }
    const duplicate = findDuplicateSegment(preview.full);
    const tail = preview.suffix.slice(preview.version.length);
    return (
      <span className="break-all">
        <span className="mr-1 rounded border border-primary/40 px-1 text-[10px] font-bold text-primary">POST</span>
        <span className="text-primary font-semibold">{preview.base}</span>
        <span className="text-amber-700 dark:text-amber-400 font-semibold">{preview.prefix}</span>
        {preview.suffix ? (
          <>
            <span className="text-violet-700 dark:text-violet-300 font-bold">{preview.version}</span>
            <span className="text-emerald-700 dark:text-emerald-400 font-semibold">{tail}</span>
          </>
        ) : (
          <span className="text-muted-foreground italic">{m.url_compose_preview_empty_suffix()}</span>
        )}
        {duplicate ? (
          <span className="mt-1 flex items-center gap-1.5 rounded-md bg-amber-500/10 px-2 py-1 text-[11px] text-amber-700 dark:text-amber-300">
            ⚠ {m.url_compose_dup_warning({ segment: duplicate })}
          </span>
        ) : null}
      </span>
    );
  };

  return (
    <div className="rounded-lg border border-primary/25 bg-primary/[0.035] p-3.5">
      <button
        type="button"
        className="flex w-full cursor-pointer items-center gap-2 bg-transparent p-0 text-left"
        onClick={() => setExpanded((current) => !current)}
      >
        <span className="text-[13px] font-semibold">{m.url_compose_toggle()}</span>
        {!expanded ? (
          <span className="ml-auto truncate text-[11px] text-muted-foreground">{summary}</span>
        ) : null}
        <span
          aria-hidden
          className={`text-muted-foreground transition-transform ${expanded ? "rotate-180" : ""}`}
        >
          ▾
        </span>
      </button>

      {expanded ? (
        <>
          <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">
            {m.url_compose_description()}
          </p>
          {selected.map((family) => {
            const endpoint = endpointOf(family);
            const version = versionSegment(endpoint.suffix);
            const suffixNormalized = normalizeSegment(endpoint.suffix);
            const head = suffixNormalized ? suffixNormalized.slice(0, version.length) : "";
            const rest = suffixNormalized.slice(head.length);
            const isDefaultSuffix =
              normalizeSegment(endpoint.suffix) === DEFAULT_SUFFIX[family];
            return (
              <div key={family} className="mt-2.5 border-t border-dashed pt-3">
                <div className="mb-2 flex items-center gap-2">
                  <span className="text-xs font-semibold">{FAMILY_LABEL[family]}</span>
                  <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                    {FAMILY_TAG[family]}
                  </span>
                </div>
                <div className="grid grid-cols-[minmax(0,5fr)_minmax(0,7fr)_36px] items-end gap-2">
                  <div>
                    <label className="mb-1 block text-[11px] text-muted-foreground">
                      {m.url_compose_prefix_label()}
                    </label>
                    <Input
                      className="font-mono text-xs placeholder:text-[10px]"
                      value={endpoint.prefix}
                      placeholder={m.url_compose_prefix_placeholder({ example: PREFIX_EXAMPLE[family] })}
                      onChange={(event) => update(family, { prefix: event.target.value })}
                      onBlur={(event) => {
                        const normalized = normalizeSegment(event.target.value);
                        if (normalized !== event.target.value) {
                          update(family, { prefix: normalized });
                        }
                      }}
                    />
                  </div>
                  <div>
                    <label className="mb-1 block text-[11px] text-muted-foreground">
                      {m.url_compose_suffix_label()}
                    </label>
                    <div className="relative">
                      <div
                        aria-hidden
                        className="pointer-events-none absolute inset-0 flex items-center overflow-hidden whitespace-pre px-[13px] font-mono text-xs"
                      >
                        {head || rest ? (
                          <>
                            <span className="rounded bg-violet-500/15 font-bold text-violet-700 dark:text-violet-300">
                              {head}
                            </span>
                            <span>{rest}</span>
                          </>
                        ) : null}
                      </div>
                      {/* 真实文本透明、由镜像层着色：两层字号/起点/空白必须完全一致，
                          否则光标与所见彩色文本错位。md:text-xs 中和 shadcn 基类的
                          md:text-sm（tailwind-merge 视为不同 variant 不会去重）；
                          px-[13px] = 边框 1px + 输入内边距 12px。 */}
                      <Input
                        className="relative font-mono text-xs text-transparent caret-foreground selection:bg-primary/20 placeholder:text-muted-foreground placeholder:text-[10px] md:text-xs"
                        value={endpoint.suffix}
                        onChange={(event) => update(family, { suffix: event.target.value })}
                        onBlur={(event) => {
                          const normalized = normalizeSegment(event.target.value);
                          if (normalized !== event.target.value) {
                            update(family, { suffix: normalized });
                          }
                        }}
                        onScroll={(event) => {
                          // 超长后缀横向滚动时镜像层同步跟随，保持逐字对齐。
                          const mirror = event.currentTarget.previousElementSibling as HTMLDivElement | null;
                          if (mirror) {
                            mirror.scrollLeft = event.currentTarget.scrollLeft;
                          }
                        }}
                      />
                    </div>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon"
                    className="h-9 w-9"
                    disabled={isDefaultSuffix}
                    title={m.url_compose_reset_title({ default: DEFAULT_SUFFIX[family] })}
                    onClick={() => update(family, { suffix: DEFAULT_SUFFIX[family] })}
                  >
                    <svg
                      width="14"
                      height="14"
                      viewBox="0 0 24 24"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                      <path d="M3 3v5h5" />
                    </svg>
                  </Button>
                </div>
                <div className="mt-2 flex items-center gap-1.5">
                  <div className="min-w-0 flex-1 break-all font-mono text-[11px] leading-relaxed">
                    {renderPreview(family)}
                  </div>
                  <button
                    type="button"
                    aria-label={m.url_compose_map_open()}
                    aria-expanded={mapFamily === family}
                    className="flex h-[17px] w-[17px] flex-none items-center justify-center rounded-full border-[1.2px] border-muted-foreground text-[10px] font-bold text-muted-foreground hover:border-violet-500 hover:text-violet-600"
                    onClick={() => setMapFamily((current) => (current === family ? null : family))}
                  >
                    !
                  </button>
                </div>
              </div>
            );
          })}
          {providers.includes("gemini") ? (
            <p className="mt-2.5 border-t border-dashed pt-2.5 text-xs text-muted-foreground">
              {m.url_compose_gemini_hint()}
            </p>
          ) : null}
          {/* ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) START ══════════ */}
          {providers.includes("dashscope") ? (
            <p className="mt-2.5 border-t border-dashed pt-2.5 text-xs text-muted-foreground">
              {m.url_compose_dashscope_hint()}
            </p>
          ) : null}
          {/* ══════════ MY-DASHSCOPE-PASSTHROUGH PATCH 3 (editor) END ══════════ */}
          <p className="mt-3 text-[11px] leading-relaxed text-muted-foreground">
            {m.url_compose_footnote()}
          </p>
        </>
      ) : null}

      {mapFamily ? (
        // 全窗口遮罩：点击弹窗外任意区域关闭，并拦截穿透点击（下层元素收不到事件）
        <div className="fixed inset-0 z-50 bg-black/30" onClick={() => setMapFamily(null)}>
          <div
            role="dialog"
            aria-modal="false"
            aria-label={m.url_compose_map_title({ name: FAMILY_LABEL[mapFamily] })}
            className="fixed left-1/2 top-[40%] z-50 max-h-[80vh] w-[min(400px,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto rounded-lg border bg-card p-3 text-[11px] shadow-xl"
            onClick={(event) => event.stopPropagation()}
          >
          <div className="mb-1 text-xs font-semibold">
            {m.url_compose_map_title({ name: FAMILY_LABEL[mapFamily] })}
          </div>
          {(() => {
            const preview = composePreview(baseUrl, endpointOf(mapFamily));
            if (!preview.base) {
              return <div className="text-muted-foreground">{m.url_compose_map_no_base()}</div>;
            }
            if (!preview.suffix) {
              return <div className="text-muted-foreground">{m.url_compose_map_no_suffix()}</div>;
            }
            return (
              <>
                <div className="mb-2 text-muted-foreground">
                  {m.url_compose_map_note({ version: preview.version })}
                </div>
                {MAP_EXAMPLES[mapFamily].map((clientPath) => {
                  const rest = clientPath.replace(/^\/[^/]+/, "");
                  return (
                    <div
                      key={clientPath}
                      className="grid grid-cols-[minmax(0,150px)_14px_minmax(0,1fr)] items-baseline gap-1.5 border-t border-dashed py-1 font-mono text-[11px]"
                    >
                      <span className="text-muted-foreground break-all">{clientPath}</span>
                      <span className="text-muted-foreground">→</span>
                      <span className="break-all">
                        <span className="text-primary font-semibold">{preview.base}</span>
                        <span className="text-amber-700 dark:text-amber-400 font-semibold">{preview.prefix}</span>
                        <span className="text-violet-700 dark:text-violet-300 font-bold">{preview.version}</span>
                        {rest}
                      </span>
                    </div>
                  );
                })}
              </>
            );
          })()}
          </div>
        </div>
      ) : null}
    </div>
  );
}
// ══════════ MY-URL-COMPOSE PATCH G3 END ══════════
