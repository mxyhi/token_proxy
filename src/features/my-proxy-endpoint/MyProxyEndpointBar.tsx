import { useCallback, useEffect, useRef, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Check, Copy, Info, KeyRound, Plug } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

import { readMyProxyEndpoint, selectMyProxyEndpoint } from "./api";
import {
  composeUrl,
  formatSuffix,
  ipKind,
  isMyProxyEndpointFormat,
  isMyProxyEndpointSnapshot,
  maskKey,
  resolveSelectedIp,
  type IpKind,
} from "./endpoint";
import {
  MY_PROXY_ENDPOINT_FORMATS,
  type MyProxyEndpointCopyTarget,
  type MyProxyEndpointFormat,
  type MyProxyEndpointSnapshot,
  type MyProxyEndpointStatus,
} from "./types";

const FORMAT_LABELS: Record<MyProxyEndpointFormat, () => string> = {
  openai: () => m.my_proxy_endpoint_format_openai(),
  anthropic: () => m.my_proxy_endpoint_format_anthropic(),
  gemini: () => m.my_proxy_endpoint_format_gemini(),
};

const IP_KIND_LABELS: Record<IpKind, () => string> = {
  loopback: () => m.my_proxy_endpoint_ip_kind_loopback(),
  lan: () => m.my_proxy_endpoint_ip_kind_lan(),
  virtual: () => m.my_proxy_endpoint_ip_kind_virtual(),
};

/** 端口 / 后缀分段着色（沿用 demo 定稿的暖色 + 绿色）；host 用主题 primary 色。 */
const PORT_CLASS =
  "font-semibold text-amber-600 dark:text-amber-400";
const SUFFIX_CLASS =
  "font-semibold text-emerald-600 dark:text-emerald-400";

/**
 * 仪表盘顶部「代理接入地址」栏（紧凑密度）。
 *
 * 只使用既有 UI 组件与尺寸变体（不改 components/ui），Key 只渲染掩码、
 * 明文仅经剪贴板复制；命令失败时静默降级，不影响其下仪表盘渲染。
 */
export function MyProxyEndpointBar() {
  const [snapshot, setSnapshot] = useState<MyProxyEndpointSnapshot | null>(null);
  const [status, setStatus] = useState<MyProxyEndpointStatus>("loading");
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState<MyProxyEndpointCopyTarget | null>(null);
  const copyTimer = useRef<number | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const next = await readMyProxyEndpoint();
        if (!cancelled) {
          if (!isMyProxyEndpointSnapshot(next)) {
            throw new Error("malformed my_proxy_endpoint snapshot");
          }
          setSnapshot(next);
          setStatus("ready");
        }
      } catch (error) {
        if (!cancelled) {
          console.debug("[my-proxy-endpoint] snapshot failed", parseError(error));
          setStatus("degraded");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    return () => {
      if (copyTimer.current !== null) {
        window.clearTimeout(copyTimer.current);
      }
    };
  }, []);

  const flashCopied = useCallback((target: MyProxyEndpointCopyTarget) => {
    setCopied(target);
    if (copyTimer.current !== null) {
      window.clearTimeout(copyTimer.current);
    }
    copyTimer.current = window.setTimeout(() => setCopied(null), 1400);
  }, []);

  const copy = useCallback(
    async (
      text: string,
      target: MyProxyEndpointCopyTarget,
      successMessage: string
    ) => {
      try {
        await writeText(text);
        flashCopied(target);
        toast.success(successMessage);
      } catch (error) {
        console.debug("[my-proxy-endpoint] copy failed", parseError(error));
        toast.error(m.my_proxy_endpoint_copy_failed());
      }
    },
    [flashCopied]
  );

  const persist = useCallback(
    async (ip: string, format: MyProxyEndpointFormat) => {
      if (snapshot === null) {
        return;
      }
      const previous = snapshot;
      setSnapshot({ ...snapshot, effectiveIp: ip, effectiveFormat: format });
      setSaving(true);
      try {
        const next = await selectMyProxyEndpoint(ip, format);
        setSnapshot(next);
      } catch (error) {
        setSnapshot(previous);
        console.debug("[my-proxy-endpoint] select failed", parseError(error));
        toast.error(m.my_proxy_endpoint_update_failed());
      } finally {
        setSaving(false);
      }
    },
    [snapshot]
  );

  if (status !== "ready" || snapshot === null) {
    return (
      <div className="px-4 lg:px-6">
        <Card
          className="gap-0 border-border/60 bg-background/70 py-0"
          data-testid="my-proxy-endpoint-bar"
        >
          <CardContent className="py-3 text-xs text-muted-foreground">
            {status === "degraded"
              ? m.my_proxy_endpoint_unavailable()
              : m.my_proxy_endpoint_loading()}
          </CardContent>
        </Card>
      </div>
    );
  }

  const selectedIp = resolveSelectedIp(snapshot);
  const url = composeUrl(selectedIp, snapshot.port, snapshot.effectiveFormat);
  const suffix = formatSuffix(snapshot.effectiveFormat);
  const ipOptions = snapshot.scanEnabled ? snapshot.availableIps : [selectedIp];

  return (
    <div className="px-4 lg:px-6">
      <Card className="gap-0 border-border/60 bg-background/70 py-0" data-testid="my-proxy-endpoint-bar">
        <CardContent className="flex flex-col gap-2 py-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <span className="flex items-center gap-1.5">
              <span className="flex size-6 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
                <Plug className="size-3.5" aria-hidden="true" />
              </span>
              <span className="text-xs font-medium whitespace-nowrap">
                {m.my_proxy_endpoint_title()}
              </span>
            </span>

            <Select
              value={selectedIp}
              onValueChange={(value) => {
                void persist(value, snapshot.effectiveFormat);
              }}
              disabled={!snapshot.scanEnabled || saving}
            >
              <SelectTrigger
                id="my-proxy-endpoint-ip"
                data-testid="my-proxy-endpoint-ip-trigger"
                aria-label={m.my_proxy_endpoint_ip_label()}
                className="h-7 w-auto min-w-32 gap-1 px-2 text-xs"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ipOptions.map((ip) => (
                  <SelectItem key={ip} value={ip} className="text-xs">
                    <span className="font-mono">{ip}</span>
                    <span className="text-muted-foreground">
                      {IP_KIND_LABELS[ipKind(ip)]()}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              className="size-7"
              aria-label={m.my_proxy_endpoint_copy_ip()}
              data-testid="my-proxy-endpoint-copy-ip"
              onClick={() => {
                void copy(selectedIp, "ip", m.my_proxy_endpoint_copied_ip());
              }}
            >
              {copied === "ip" ? (
                <Check className="size-3.5" aria-hidden="true" />
              ) : (
                <Copy className="size-3.5" aria-hidden="true" />
              )}
            </Button>

            <Select
              value={snapshot.effectiveFormat}
              onValueChange={(value) => {
                if (isMyProxyEndpointFormat(value)) {
                  void persist(selectedIp, value);
                }
              }}
              disabled={saving}
            >
              <SelectTrigger
                id="my-proxy-endpoint-format"
                data-testid="my-proxy-endpoint-format-trigger"
                aria-label={m.my_proxy_endpoint_format_label()}
                className="h-7 w-auto min-w-32 gap-1 px-2 text-xs"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {MY_PROXY_ENDPOINT_FORMATS.map((format) => (
                  <SelectItem key={format} value={format} className="text-xs">
                    {FORMAT_LABELS[format]()}
                    <span className="font-mono text-muted-foreground">
                      {formatSuffix(format) || m.my_proxy_endpoint_no_suffix()}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>

            <div className="flex min-w-0 flex-1 basis-72 items-center gap-1.5">
              <button
                type="button"
                data-testid="my-proxy-endpoint-url"
                title={m.my_proxy_endpoint_copy_url()}
                onClick={() => {
                  void copy(url, "url", m.my_proxy_endpoint_copied());
                }}
                className="flex h-7 min-w-0 flex-1 items-center overflow-hidden rounded-md border bg-muted/40 px-2 font-mono text-xs"
              >
                <span className="text-muted-foreground">http://</span>
                <span className="font-semibold text-primary">{selectedIp}</span>
                <span className={PORT_CLASS}>{":" + snapshot.port}</span>
                {suffix.length > 0 ? (
                  <span className={SUFFIX_CLASS}>{suffix}</span>
                ) : null}
              </button>
              <Button
                type="button"
                size="sm"
                className="h-7 gap-1 px-2 text-xs"
                data-testid="my-proxy-endpoint-copy-url"
                onClick={() => {
                  void copy(url, "url", m.my_proxy_endpoint_copied());
                }}
              >
                {copied === "url" ? (
                  <Check className="size-3.5" aria-hidden="true" />
                ) : (
                  <Copy className="size-3.5" aria-hidden="true" />
                )}
                <span>{copied === "url" ? m.my_proxy_endpoint_copied() : m.my_proxy_endpoint_copy_url()}</span>
              </Button>
            </div>

            <span className="ml-auto rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground whitespace-nowrap">
              {m.my_proxy_endpoint_listen_label()} {snapshot.listenHost}:{snapshot.port}
            </span>
          </div>

          <div className="flex flex-wrap items-center gap-2 border-t border-dashed pt-2">
            <span className="inline-flex items-center gap-1 text-[11px] font-medium text-muted-foreground">
              <KeyRound className="size-3" aria-hidden="true" />
              {m.my_proxy_endpoint_key_label()}
            </span>
            <span
              data-testid="my-proxy-endpoint-key-mask"
              className="rounded-md border border-dashed px-2 py-0.5 font-mono text-xs text-muted-foreground"
            >
              {snapshot.apiKeyConfigured
                ? maskKey(true)
                : m.my_proxy_endpoint_key_empty()}
            </span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 gap-1 px-2 text-xs"
              data-testid="my-proxy-endpoint-copy-key"
              disabled={!snapshot.apiKeyConfigured || (snapshot.apiKey ?? "").length === 0}
              onClick={() => {
                void copy(snapshot.apiKey ?? "", "key", m.my_proxy_endpoint_copied_key());
              }}
            >
              {copied === "key" ? (
                <Check className="size-3.5" aria-hidden="true" />
              ) : (
                <Copy className="size-3.5" aria-hidden="true" />
              )}
              <span>{copied === "key" ? m.my_proxy_endpoint_copied() : m.my_proxy_endpoint_copy_key()}</span>
            </Button>
            <span className="ml-auto flex min-w-0 flex-1 basis-64 items-center gap-1 text-[11px] text-muted-foreground">
              <Info className="size-3 shrink-0" aria-hidden="true" />
              <span className="truncate">
                {snapshot.scanEnabled
                  ? m.my_proxy_endpoint_hint_scan({ port: snapshot.port })
                  : m.my_proxy_endpoint_hint_loopback()}
              </span>
            </span>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
