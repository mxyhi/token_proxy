import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

export type HttpStatusTone = "success" | "redirect" | "client-error" | "server-error" | "unknown";

export function httpStatusTone(status: number): HttpStatusTone {
  if (status >= 200 && status < 300) return "success";
  if (status >= 300 && status < 400) return "redirect";
  if (status >= 400 && status < 500) return "client-error";
  if (status >= 500 && status < 600) return "server-error";
  return "unknown";
}

// 按状态码段区分颜色：成功绿、重定向蓝、客户端错误琥珀、服务端错误红，其余（1xx/异常值）保持中性。
const TONE_CLASS: Record<HttpStatusTone, string> = {
  success:
    "border-transparent bg-emerald-50 text-emerald-700 dark:bg-emerald-500/15 dark:text-emerald-300",
  redirect: "border-transparent bg-sky-50 text-sky-700 dark:bg-sky-500/15 dark:text-sky-300",
  "client-error":
    "border-transparent bg-amber-50 text-amber-700 dark:bg-amber-500/15 dark:text-amber-300",
  "server-error": "border-transparent bg-red-50 text-red-700 dark:bg-red-500/15 dark:text-red-300",
  unknown: "text-muted-foreground",
};

type HttpStatusBadgeProps = {
  status: number;
  className?: string;
};

export function HttpStatusBadge({ status, className }: HttpStatusBadgeProps) {
  const tone = httpStatusTone(status);
  return (
    <Badge
      variant="outline"
      data-status-tone={tone}
      className={cn("tabular-nums", TONE_CLASS[tone], className)}
    >
      {status}
    </Badge>
  );
}
