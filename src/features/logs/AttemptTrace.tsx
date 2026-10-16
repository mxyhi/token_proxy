import type { DashboardRequestItem } from "@/features/dashboard/types";
import { httpStatusTone, type HttpStatusTone } from "@/features/logs/HttpStatusBadge";
import { cn } from "@/lib/utils";
import { m } from "@/paraglide/messages.js";

/** final：最终返回给客户端的 attempt（计费）；attempt：被重试或 failover 取代的中间 attempt。 */
export type AttemptDotVariant = "final" | "attempt";

// 颜色与 HttpStatusBadge 的状态码段保持一致：边框区分状态，实心填充只给最终返回。
const DOT_TONE_CLASS: Record<HttpStatusTone, { border: string; fill: string }> = {
  success: { border: "border-emerald-500", fill: "bg-emerald-500" },
  redirect: { border: "border-sky-500", fill: "bg-sky-500" },
  "client-error": { border: "border-amber-500", fill: "bg-amber-500" },
  "server-error": { border: "border-red-500", fill: "bg-red-500" },
  unknown: { border: "border-muted-foreground", fill: "bg-muted-foreground" },
};

type AttemptDotProps = {
  status: number;
  variant: AttemptDotVariant;
  className?: string;
};

// 实心点 = 最终返回；空心点 = 中间 attempt。
export function AttemptDot({ status, variant, className }: AttemptDotProps) {
  const tone = httpStatusTone(status);
  return (
    <span
      aria-hidden="true"
      data-attempt-dot={variant}
      data-status-tone={tone}
      className={cn(
        "relative z-[1] inline-block size-2.5 shrink-0 rounded-full border-2",
        DOT_TONE_CLASS[tone].border,
        variant === "final" ? DOT_TONE_CLASS[tone].fill : "bg-background",
        className,
      )}
    />
  );
}

type AttemptTraceCellProps = AttemptDotProps & {
  lineAbove?: boolean;
  lineBelow?: boolean;
};

// 竖线把最终请求和它下面的中间 attempt 串成一组；单元格需撑满行高才能跨行连线。
export function AttemptTraceCell({ lineAbove, lineBelow, ...dot }: AttemptTraceCellProps) {
  return (
    <div className="relative flex h-full items-center justify-center">
      {lineAbove ? (
        <span aria-hidden="true" className="absolute top-0 left-1/2 h-1/2 w-px -translate-x-1/2 bg-border" />
      ) : null}
      {lineBelow ? (
        <span aria-hidden="true" className="absolute bottom-0 left-1/2 h-1/2 w-px -translate-x-1/2 bg-border" />
      ) : null}
      <AttemptDot {...dot} />
    </div>
  );
}

export function AttemptLegend() {
  return (
    <span data-slot="attempt-legend" className="inline-flex items-center gap-3 text-xs text-muted-foreground">
      <span className="inline-flex items-center gap-1.5">
        <AttemptDot status={200} variant="final" />
        {m.logs_attempt_final()}
      </span>
      <span className="inline-flex items-center gap-1.5">
        <AttemptDot status={503} variant="attempt" />
        {m.logs_attempt_intermediate()}
      </span>
    </span>
  );
}

/** 按发送先后列出渠道与状态，如 `a 503 → a 503 → b 200`。 */
export function formatAttemptChain(item: DashboardRequestItem) {
  const attempts = item.attempts ?? [];
  return [...attempts]
    .reverse()
    .map((attempt) => `${attempt.upstreamId} ${attempt.status}`)
    .concat(`${item.upstreamId} ${item.status}`)
    .join(" → ");
}

/** attempt 序号从 0 开始；旧日志没有序号时不显示编号。 */
export function attemptNumber(attemptIndex: number | null | undefined) {
  return attemptIndex == null ? null : attemptIndex + 1;
}

const HTTP_ERROR_PREFIX = /^HTTP \d{3}:\s*/;

function readErrorMessage(value: unknown): string | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const record = value as Record<string, unknown>;
  if (typeof record.message === "string" && record.message.trim()) {
    return record.message.trim();
  }
  return readErrorMessage(record.error);
}

/**
 * 表格内的错误摘要：优先取上游 JSON 里的 `error.message`，状态码已由徽章展示。
 * 摘要在后端截断后可能不是完整 JSON，此时回退原文。
 */
export function summarizeAttemptError(raw: string | null | undefined) {
  const trimmed = raw?.trim();
  if (!trimmed) {
    return null;
  }
  const body = trimmed.replace(HTTP_ERROR_PREFIX, "");
  if (!body.startsWith("{")) {
    return trimmed;
  }
  try {
    return readErrorMessage(JSON.parse(body)) ?? trimmed;
  } catch {
    return trimmed;
  }
}
