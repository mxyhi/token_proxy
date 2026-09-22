import { m } from "@/paraglide/messages.js";
import { cn } from "@/lib/utils";

import {
  auditUpstreamResponseModel,
  type UpstreamResponseModelAudit,
} from "@/features/logs/upstream-response-model";

type UpstreamResponseModelLineProps = {
  model: string | null | undefined;
  mappedModel: string | null | undefined;
  upstreamResponseModel: string | null | undefined;
};

export function describeUpstreamResponseModel(
  model: string | null | undefined,
  mappedModel: string | null | undefined,
  upstreamResponseModel: string | null | undefined,
) {
  const audit = auditUpstreamResponseModel(model, mappedModel, upstreamResponseModel);
  if (audit.kind === "same") {
    return null;
  }
  const badge = audit.kind === "variant" ? m.logs_model_variant() : m.logs_model_mismatch();
  return `${m.logs_model_upstream_response()}: ${audit.responseModel} (${badge})`;
}

function badgeClass(kind: Exclude<UpstreamResponseModelAudit["kind"], "same">) {
  if (kind === "variant") {
    return "bg-amber-50 text-amber-700 ring-amber-200 dark:bg-amber-500/10 dark:text-amber-300 dark:ring-amber-500/30";
  }
  return "bg-orange-50 text-orange-700 ring-orange-200 dark:bg-orange-500/10 dark:text-orange-300 dark:ring-orange-500/30";
}

export function UpstreamResponseModelLine({
  model,
  mappedModel,
  upstreamResponseModel,
}: UpstreamResponseModelLineProps) {
  const audit = auditUpstreamResponseModel(model, mappedModel, upstreamResponseModel);
  if (audit.kind === "same") {
    return null;
  }
  const variant = audit.kind === "variant";
  return (
    <span
      data-testid="upstream-response-model"
      className={cn(
        "flex w-full min-w-0 items-center gap-1 text-[11px] leading-tight",
        variant ? "text-amber-600 dark:text-amber-400" : "text-orange-600 dark:text-orange-400",
      )}
    >
      <span className="min-w-0 truncate">
        ↳ {m.logs_model_upstream_response()}: {audit.responseModel}
      </span>
      <span
        className={cn(
          "shrink-0 rounded px-1 py-px text-[10px] font-medium ring-1 ring-inset",
          badgeClass(audit.kind),
        )}
      >
        {variant ? m.logs_model_variant() : m.logs_model_mismatch()}
      </span>
    </span>
  );
}
