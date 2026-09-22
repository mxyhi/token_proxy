import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { AlertCircle, Check, Copy } from "lucide-react";
import { toast } from "sonner";

import { DataTable } from "@/features/dashboard/components/data-table";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import {
  DashboardFilters,
  RECENT_PAGE_SIZE,
  useDashboardSnapshot,
} from "@/features/dashboard/snapshot";
import {
  createDashboardTimeFormatter,
  formatDashboardClientIp,
  formatDashboardProviderLabel,
  formatDashboardTimestamp,
  formatInteger,
  formatNanoUsdCost,
} from "@/features/dashboard/format";
import {
  readRequestDetailCapture,
  readRequestLogDetail,
  setRequestDetailCapture,
} from "@/features/logs/api";
import {
  UpstreamResponseModelLine,
  describeUpstreamResponseModel,
} from "@/features/logs/UpstreamResponseModelLine";
import type {
  RequestDetailCaptureState,
  RequestLogDetail,
} from "@/features/logs/types";
import { useI18n } from "@/lib/i18n";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

const DETAIL_PLACEHOLDER = "—";
const REQUEST_DETAIL_CAPTURE_EVENT = "request-detail-capture-changed";
const CAPTURE_COUNTDOWN_TICK_MS = 1_000;
const DETAIL_FIELD_ROW_CLASS = "grid grid-cols-[11rem_minmax(0,1fr)] items-start gap-x-3 py-1";
const DETAIL_FIELD_LABEL_CLASS = "text-xs leading-snug text-muted-foreground";
const DETAIL_FIELD_VALUE_CLASS = "min-w-0 text-sm text-foreground justify-self-start";
const IDLE_CAPTURE_STATE: RequestDetailCaptureState = {
  enabled: false,
  expiresAtMs: null,
};

type DetailStatus = "idle" | "loading" | "error";

type RequestDetailCaptureEvent = RequestDetailCaptureState;

type BadgeVariant = "default" | "secondary" | "destructive" | "outline";

function statusToVariant(status: number): BadgeVariant {
  if (status >= 200 && status < 300) return "default";
  if (status >= 400) return "destructive";
  if (status >= 300) return "secondary";
  return "outline";
}

function isCaptureWindowActive(state: RequestDetailCaptureState, nowMs: number) {
  if (!state.enabled) {
    return false;
  }
  if (state.expiresAtMs === null) {
    return true;
  }
  return state.expiresAtMs > nowMs;
}

function getCaptureRemainingSeconds(state: RequestDetailCaptureState, nowMs: number) {
  if (!state.enabled || state.expiresAtMs === null) {
    return null;
  }
  const remainingMs = state.expiresAtMs - nowMs;
  if (remainingMs <= 0) {
    return null;
  }
  return Math.max(1, Math.ceil(remainingMs / CAPTURE_COUNTDOWN_TICK_MS));
}

function getResponseDetailValue(detail: RequestLogDetail) {
  // 空白响应体没有排障价值，优先回退到错误摘要。
  return detail.responseBody?.trim() ? detail.responseBody : detail.responseError;
}

type DetailFieldProps = {
  label: string;
  value: string | null | undefined;
};

function DetailField({ label, value }: DetailFieldProps) {
  return (
    <div className={DETAIL_FIELD_ROW_CLASS}>
      <span className={DETAIL_FIELD_LABEL_CLASS}>{label}</span>
      <span className={`${DETAIL_FIELD_VALUE_CLASS} truncate`}>
        {value?.trim() || DETAIL_PLACEHOLDER}
      </span>
    </div>
  );
}

type BasicInfoSectionProps = {
  detail: RequestLogDetail;
  formatter: Intl.DateTimeFormat;
};

// 基础信息区域：展示表格中的字段
function BasicInfoSection({ detail, formatter }: BasicInfoSectionProps) {
  const timestamp = formatDashboardTimestamp(detail.tsMs, formatter);
  const streamText = detail.stream ? m.logs_detail_stream_yes() : m.logs_detail_stream_no();
  const providerText = formatDashboardProviderLabel(
    detail.upstreamId,
    detail.provider,
    detail.accountId,
  );
  // 只有当 mappedModel 与 model 不同时才展示（相同说明没有实际映射）
  const hasMappedModel =
    detail.mappedModel?.trim() &&
    detail.model?.trim() &&
    detail.mappedModel.trim() !== detail.model.trim();

  return (
    <div className="space-y-2">
      <p className="text-sm font-medium text-foreground">{m.logs_detail_basic_info()}</p>
      <div className="rounded-lg border border-border/60 bg-muted/20 p-3 space-y-1">
        <DetailField label="ID" value={String(detail.id)} />
        <DetailField label={m.dashboard_table_time()} value={timestamp} />
        <DetailField
          label={m.dashboard_table_ip()}
          value={formatDashboardClientIp(detail.clientIp)}
        />
        <DetailField label={m.dashboard_table_path()} value={detail.path} />
        <DetailField label={m.dashboard_table_provider()} value={providerText} />
        {/* Model 展示逻辑与表格一致：主模型在上，映射模型在下 */}
        <div className={DETAIL_FIELD_ROW_CLASS}>
          <span className={DETAIL_FIELD_LABEL_CLASS}>{m.dashboard_table_model()}</span>
          <div className="flex min-w-0 flex-col items-start">
            <span className="w-full truncate text-sm text-foreground">
              {detail.model?.trim() || DETAIL_PLACEHOLDER}
            </span>
            {hasMappedModel ? (
              <span className="w-full truncate text-xs text-muted-foreground">
                {detail.mappedModel}
              </span>
            ) : null}
            <UpstreamResponseModelLine
              model={detail.model}
              mappedModel={detail.mappedModel}
              upstreamResponseModel={detail.upstreamResponseModel}
            />
          </div>
        </div>
        <div className={DETAIL_FIELD_ROW_CLASS}>
          <span className={DETAIL_FIELD_LABEL_CLASS}>{m.dashboard_table_status()}</span>
          <Badge variant={statusToVariant(detail.status)} className="justify-self-start">
            {detail.status}
          </Badge>
        </div>
        <DetailField label={m.logs_detail_stream()} value={streamText} />
        <DetailField
          label={m.dashboard_table_cost()}
          value={formatNanoUsdCost(detail.costNanoUsd)}
        />
        <DetailField
          label={m.logs_detail_image_output_tokens()}
          value={formatOptionalInteger(detail.imageOutputTokens)}
        />
        <DetailField
          label={m.logs_detail_pricing_model()}
          value={detail.pricingModel}
        />
        <DetailField
          label={m.logs_detail_pricing_context_tier()}
          value={formatPricingContextTier(detail.pricingContextTier)}
        />
        <DetailField
          label={m.logs_detail_pricing_version()}
          value={detail.pricingVersion}
        />
        <DetailField
          label={m.dashboard_table_latency_ms()}
          value={formatInteger(detail.latencyMs)}
        />
        <DetailField
          label={m.logs_timing_upstream_response_headers_ms()}
          value={formatOptionalInteger(detail.upstreamResponseHeadersMs)}
        />
        <DetailField
          label={m.logs_timing_upstream_first_body_chunk_ms()}
          value={formatOptionalInteger(detail.upstreamFirstBodyChunkMs ?? detail.upstreamFirstByteMs)}
        />
        <DetailField
          label={m.logs_timing_first_client_flush_ms()}
          value={formatOptionalInteger(detail.firstClientFlushMs)}
        />
        <DetailField
          label={m.logs_timing_first_output_ms()}
          value={formatOptionalInteger(detail.firstOutputMs)}
        />
        <DetailField label={m.logs_detail_upstream_request_id()} value={detail.upstreamRequestId} />
      </div>
    </div>
  );
}

type DetailSectionProps = {
  title: string;
  value: string | null;
};

function DetailSection({ title, value }: DetailSectionProps) {
  const content = value?.trim() ? value : null;
  return (
    <div className="space-y-2">
      <p className="text-sm font-medium text-foreground">{title}</p>
      {content ? (
        <pre className="rounded-lg border border-border/60 bg-muted/20 p-3 text-xs whitespace-pre-wrap break-words">
          {content}
        </pre>
      ) : (
        <p className="text-xs text-muted-foreground">{DETAIL_PLACEHOLDER}</p>
      )}
    </div>
  );
}

// 将详情格式化为可复制的文本
function formatDetailAsText(detail: RequestLogDetail, formatter: Intl.DateTimeFormat): string {
  const lines: string[] = [];
  const providerText = formatDashboardProviderLabel(
    detail.upstreamId,
    detail.provider,
    detail.accountId,
  );
  const hasMappedModel =
    detail.mappedModel?.trim() &&
    detail.model?.trim() &&
    detail.mappedModel.trim() !== detail.model.trim();

  lines.push(`ID: ${detail.id}`);
  lines.push(`${m.dashboard_table_time()}: ${formatDashboardTimestamp(detail.tsMs, formatter)}`);
  lines.push(`${m.dashboard_table_ip()}: ${formatDashboardClientIp(detail.clientIp)}`);
  lines.push(`${m.dashboard_table_path()}: ${detail.path}`);
  lines.push(`${m.dashboard_table_provider()}: ${providerText}`);
  lines.push(`${m.dashboard_table_model()}: ${detail.model?.trim() || DETAIL_PLACEHOLDER}`);
  if (hasMappedModel) {
    lines.push(`${m.logs_detail_model_mapped()}: ${detail.mappedModel}`);
  }
  const upstreamResponseLine = describeUpstreamResponseModel(
    detail.model,
    detail.mappedModel,
    detail.upstreamResponseModel,
  );
  if (upstreamResponseLine) {
    lines.push(upstreamResponseLine);
  }
  lines.push(`${m.dashboard_table_status()}: ${detail.status}`);
  lines.push(`${m.logs_detail_stream()}: ${detail.stream ? m.logs_detail_stream_yes() : m.logs_detail_stream_no()}`);
  lines.push(`${m.dashboard_table_cost()}: ${formatNanoUsdCost(detail.costNanoUsd)}`);
  lines.push(`${m.logs_detail_image_output_tokens()}: ${formatOptionalInteger(detail.imageOutputTokens)}`);
  lines.push(`${m.logs_detail_pricing_model()}: ${detail.pricingModel?.trim() || DETAIL_PLACEHOLDER}`);
  lines.push(`${m.logs_detail_pricing_context_tier()}: ${formatPricingContextTier(detail.pricingContextTier)}`);
  lines.push(`${m.logs_detail_pricing_version()}: ${detail.pricingVersion?.trim() || DETAIL_PLACEHOLDER}`);
  lines.push(`${m.dashboard_table_latency_ms()}: ${formatInteger(detail.latencyMs)}`);
  lines.push(`${m.logs_timing_upstream_response_headers_ms()}: ${formatOptionalInteger(detail.upstreamResponseHeadersMs)}`);
  lines.push(`${m.logs_timing_upstream_first_body_chunk_ms()}: ${formatOptionalInteger(detail.upstreamFirstBodyChunkMs ?? detail.upstreamFirstByteMs)}`);
  lines.push(`${m.logs_timing_first_client_flush_ms()}: ${formatOptionalInteger(detail.firstClientFlushMs)}`);
  lines.push(`${m.logs_timing_first_output_ms()}: ${formatOptionalInteger(detail.firstOutputMs)}`);
  lines.push(`${m.logs_detail_upstream_request_id()}: ${detail.upstreamRequestId?.trim() || DETAIL_PLACEHOLDER}`);

  if (detail.usageJson?.trim()) {
    lines.push("");
    lines.push(`--- ${m.logs_detail_usage_json()} ---`);
    lines.push(detail.usageJson);
  }

  if (detail.requestHeaders?.trim()) {
    lines.push("");
    lines.push(`--- ${m.logs_detail_headers()} ---`);
    lines.push(detail.requestHeaders);
  }

  if (detail.requestBody?.trim()) {
    lines.push("");
    lines.push(`--- ${m.logs_detail_body()} ---`);
    lines.push(detail.requestBody);
  }

  if (detail.responseBody?.trim()) {
    lines.push("");
    lines.push(`--- ${m.logs_detail_response()} ---`);
    lines.push(detail.responseBody);
  }

  if (detail.responseError?.trim()) {
    lines.push("");
    lines.push("--- response_error ---");
    lines.push(detail.responseError);
  }

  return lines.join("\n");
}

function formatOptionalInteger(value: number | null | undefined) {
  return value == null ? DETAIL_PLACEHOLDER : formatInteger(value);
}

function formatPricingContextTier(tier: string | null | undefined) {
  if (tier === "long") {
    return m.logs_detail_pricing_context_long();
  }
  if (tier === "short") {
    return m.logs_detail_pricing_context_short();
  }
  return DETAIL_PLACEHOLDER;
}

type RequestDetailSheetProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  status: DetailStatus;
  statusMessage: string;
  detail: RequestLogDetail | null;
  formatter: Intl.DateTimeFormat;
};

function RequestDetailSheet({
  open,
  onOpenChange,
  status,
  statusMessage,
  detail,
  formatter,
}: RequestDetailSheetProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    if (!detail) return;
    const text = formatDetailAsText(detail, formatter);
    try {
      await writeText(text);
      setCopied(true);
      toast.success(m.logs_detail_copied());
    } catch {
      toast.error(m.logs_detail_copy_failed());
    }
  }, [detail, formatter]);

  // 重置复制状态当 sheet 关闭时，并清理 timeout
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen) {
        setCopied(false);
      }
      onOpenChange(nextOpen);
    },
    [onOpenChange]
  );

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent className="sm:max-w-2xl">
        <SheetHeader>
          <div className="flex items-center gap-2">
            <SheetTitle>{m.logs_detail_title()}</SheetTitle>
            {status === "idle" && detail ? (
              <Button
                variant="outline"
                size="icon"
                onClick={handleCopy}
                className="size-7"
              >
                {copied ? (
                  <Check className="size-3.5" aria-hidden="true" />
                ) : (
                  <Copy className="size-3.5" aria-hidden="true" />
                )}
                <span className="sr-only">
                  {copied ? m.logs_detail_copied() : m.logs_detail_copy()}
                </span>
              </Button>
            ) : null}
          </div>
          <SheetDescription>{m.logs_detail_desc()}</SheetDescription>
        </SheetHeader>
        <ScrollArea className="flex-1">
          <div className="space-y-4 px-4 pb-6">
            {status === "loading" ? (
              <p className="text-sm text-muted-foreground">{m.logs_detail_loading()}</p>
            ) : null}
            {status === "error" ? (
              <Alert variant="destructive">
                <AlertCircle className="size-4" aria-hidden="true" />
                <div>
                  <AlertTitle>{m.logs_detail_error()}</AlertTitle>
                  <AlertDescription>{statusMessage}</AlertDescription>
                </div>
              </Alert>
            ) : null}
            {status === "idle" && detail ? (
              <div className="space-y-4">
                <BasicInfoSection detail={detail} formatter={formatter} />
                <DetailSection
                  title={m.logs_detail_usage_json()}
                  value={detail.usageJson}
                />
                <DetailSection
                  title={m.logs_detail_headers()}
                  value={detail.requestHeaders}
                />
                <DetailSection
                  title={m.logs_detail_body()}
                  value={detail.requestBody}
                />
                <DetailSection
                  title={m.logs_detail_response()}
                  value={getResponseDetailValue(detail)}
                />
              </div>
            ) : null}
          </div>
        </ScrollArea>
      </SheetContent>
    </Sheet>
  );
}

export function LogsPanel() {
  const {
    snapshot,
    status,
    statusMessage,
    rangePreset,
    customRange,
    selectedUpstreamId,
    selectedModel,
    upstreamOptions,
    modelOptions,
    pagination,
    refresh,
    onRangeChange,
    onCustomRangeChange,
    onUpstreamChange,
    onModelChange,
    onPrevPage,
    onNextPage,
  } = useDashboardSnapshot();

  const { locale } = useI18n();
  const formatter = createDashboardTimeFormatter(locale);

  const [captureState, setCaptureState] = useState<RequestDetailCaptureState>(IDLE_CAPTURE_STATE);
  const [captureLoading, setCaptureLoading] = useState(false);
  const [captureNowMs, setCaptureNowMs] = useState(() => Date.now());
  const [detailOpen, setDetailOpen] = useState(false);
  const [detailStatus, setDetailStatus] = useState<DetailStatus>("idle");
  const [detailMessage, setDetailMessage] = useState("");
  const [detail, setDetail] = useState<RequestLogDetail | null>(null);
  const detailRequestSeq = useRef(0);

  const isLoading = status === "loading";
  const captureEnabled = isCaptureWindowActive(captureState, captureNowMs);
  const captureRemainingSeconds = getCaptureRemainingSeconds(captureState, captureNowMs);
  const captureStatusText = captureRemainingSeconds
    ? m.logs_capture_status_countdown({ seconds: captureRemainingSeconds })
    : "";

  const updateCaptureState = useCallback((nextState: RequestDetailCaptureState) => {
    setCaptureState(nextState);
    setCaptureNowMs(Date.now());
  }, []);

  const loadCaptureState = useCallback(async () => {
    try {
      const nextState = await readRequestDetailCapture();
      updateCaptureState(nextState);
    } catch {
      // ignore
    }
  }, [updateCaptureState]);

  useEffect(() => {
    void loadCaptureState();
  }, [loadCaptureState]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | null = null;

    const setupListener = async () => {
      try {
        const stop = await listen<RequestDetailCaptureEvent>(
          REQUEST_DETAIL_CAPTURE_EVENT,
          (event) => {
            if (!active) {
              return;
            }
            updateCaptureState(event.payload);
          }
        );
        if (!active) {
          stop();
          return;
        }
        unlisten = stop;
      } catch {
        // ignore
      }
    };

    void setupListener();

    return () => {
      active = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, [updateCaptureState]);

  useEffect(() => {
    if (!captureEnabled) {
      return;
    }
    setCaptureNowMs(Date.now());
    const timerId = window.setInterval(() => {
      setCaptureNowMs(Date.now());
    }, CAPTURE_COUNTDOWN_TICK_MS);
    return () => {
      window.clearInterval(timerId);
    };
  }, [captureEnabled, captureState.expiresAtMs]);

  useEffect(() => {
    if (!captureState.enabled || captureState.expiresAtMs === null) {
      return;
    }
    const timeoutMs = Math.max(captureState.expiresAtMs - Date.now(), 0) + 50;
    const timeoutId = window.setTimeout(() => {
      void loadCaptureState();
    }, timeoutMs);
    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [captureState.enabled, captureState.expiresAtMs, loadCaptureState]);

  const handleToggleCapture = useCallback(async (nextValue: boolean) => {
    setCaptureLoading(true);
    try {
      const nextState = await setRequestDetailCapture(nextValue);
      updateCaptureState(nextState);
    } catch {
      // ignore
    } finally {
      setCaptureLoading(false);
    }
  }, [updateCaptureState]);

  const handleSelectItem = useCallback(async (itemId: number) => {
    const requestId = detailRequestSeq.current + 1;
    detailRequestSeq.current = requestId;
    setDetailOpen(true);
    setDetail(null);
    setDetailStatus("loading");
    setDetailMessage("");

    try {
      const data = await readRequestLogDetail(itemId);
      if (detailRequestSeq.current === requestId) {
        setDetail(data);
        setDetailStatus("idle");
      }
    } catch (error) {
      if (detailRequestSeq.current === requestId) {
        setDetailMessage(parseError(error));
        setDetailStatus("error");
      }
    }
  }, []);

  const handleDetailOpenChange = useCallback((nextOpen: boolean) => {
    if (nextOpen) {
      setDetailOpen(true);
      return;
    }

    detailRequestSeq.current += 1;
    setDetailOpen(false);
    setDetail(null);
    setDetailStatus("idle");
    setDetailMessage("");
  }, []);

  useEffect(() => {
    return () => {
      detailRequestSeq.current += 1;
    };
  }, []);

  return (
    <div data-testid="logs-panel" className="flex min-h-0 flex-1 flex-col gap-4">
      {status === "error" ? (
        <Alert variant="destructive" className="mx-4 lg:mx-6">
          <AlertCircle className="size-4" aria-hidden="true" />
          <div>
            <AlertTitle>{m.dashboard_load_failed()}</AlertTitle>
            <AlertDescription>{statusMessage}</AlertDescription>
          </div>
        </Alert>
      ) : null}

      <DashboardFilters
        range={rangePreset}
        customRange={customRange}
        upstreamId={selectedUpstreamId}
        upstreamOptions={upstreamOptions}
        model={selectedModel}
        modelOptions={modelOptions}
        loading={isLoading}
        onRangeChange={onRangeChange}
        onCustomRangeChange={onCustomRangeChange}
        onUpstreamChange={onUpstreamChange}
        onModelChange={onModelChange}
        onRefresh={refresh}
        capture={{
          enabled: captureEnabled,
          loading: captureLoading,
          statusText: captureStatusText,
          onToggle: handleToggleCapture,
        }}
      />

      <DataTable
        items={snapshot?.recent ?? []}
        page={pagination.page}
        totalPages={pagination.totalPages}
        totalRequests={pagination.totalRequests}
        pageSize={RECENT_PAGE_SIZE}
        loading={isLoading}
        scrollKey={`${rangePreset}-${pagination.page}`}
        onPrevPage={onPrevPage}
        onNextPage={onNextPage}
        onSelectItem={(item) => handleSelectItem(item.id)}
      />

      <RequestDetailSheet
        open={detailOpen}
        onOpenChange={handleDetailOpenChange}
        status={detailStatus}
        statusMessage={detailMessage}
        detail={detail}
        formatter={formatter}
      />
    </div>
  );
}
