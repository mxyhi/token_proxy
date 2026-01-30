import type { ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { m } from "@/paraglide/messages.js";

import type { ActionState, RequestState } from "./client-setup-state";

// 内联展示工具配置的 props
type ToolSetupPanelProps = {
  action: ActionState;
  canApply: boolean;
  isWorking: boolean;
  onApply: () => void;
  children: ReactNode;
};

function shouldShowBadge(state: RequestState) {
  return state !== "idle";
}

function toBadgeVariant(state: RequestState) {
  if (state === "success") return "default";
  if (state === "error") return "destructive";
  if (state === "working") return "secondary";
  return "outline";
}

function toBadgeLabel(state: RequestState) {
  if (state === "success") return m.client_setup_status_success();
  if (state === "error") return m.client_setup_status_error();
  if (state === "working") return m.client_setup_status_working();
  return m.client_setup_status_idle();
}

export function SummaryItem({ label, value }: { label: string; value: string }) {
  return (
    <div
      data-slot="client-setup-summary-item"
      className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground"
    >
      <span className="shrink-0 uppercase tracking-[0.2em]">{label}</span>
      <span className="min-w-0 truncate font-mono text-foreground/80">{value}</span>
    </div>
  );
}

export function DetailSection({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div data-slot="client-setup-detail-section" className="space-y-1">
      <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">{label}</p>
      {children}
    </div>
  );
}

export function MonoBlock({ children }: { children: ReactNode }) {
  return (
    <div data-slot="client-setup-mono-block" className="rounded-md border border-border/60 bg-background/60 p-3">
      {children}
    </div>
  );
}

export function PathList({ paths }: { paths: readonly string[] }) {
  return (
    <div data-slot="client-setup-path-list">
      <MonoBlock>
        <div className="space-y-1 font-mono text-xs text-foreground/80 break-all">
          {paths.map((path, index) => (
            <div key={index}>{path}</div>
          ))}
        </div>
      </MonoBlock>
    </div>
  );
}

export function CodeBlock({ lines }: { lines: readonly string[] }) {
  return (
    <div data-slot="client-setup-code-block">
      <MonoBlock>
        <div className="overflow-x-auto">
          <div className="min-w-max space-y-1 font-mono text-xs text-foreground/80 whitespace-pre">
            {lines.map((line, index) => (
              <div key={index}>{line}</div>
            ))}
          </div>
        </div>
      </MonoBlock>
    </div>
  );
}

/** 内联展示工具配置面板（无弹窗） */
export function ToolSetupPanel({
  action,
  canApply,
  isWorking,
  onApply,
  children,
}: ToolSetupPanelProps) {
  return (
    <Card data-slot="client-setup-tool-panel">
      <CardContent className="space-y-4 pt-6">
        {/* 详细配置内容 */}
        {children}

        {/* 操作状态消息 */}
        {action.message ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {action.message}
          </div>
        ) : null}

        {/* 备份提示 */}
        <p className="text-xs text-muted-foreground">{m.client_setup_backup_hint()}</p>

        {/* 底部操作栏 */}
        <div className="flex items-center justify-between gap-3 pt-2">
          <div className="flex items-center gap-2">
            {shouldShowBadge(action.state) ? (
              <Badge variant={toBadgeVariant(action.state)}>{toBadgeLabel(action.state)}</Badge>
            ) : null}
          </div>
          <Button type="button" onClick={onApply} disabled={!canApply || isWorking}>
            {m.client_setup_apply()}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

export function ToolDetailsFallback({
  previewState,
  previewMessage,
}: {
  previewState: RequestState;
  previewMessage: string;
}) {
  if (previewMessage) {
    return (
      <div
        data-slot="client-setup-details-fallback"
        className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground"
      >
        {previewMessage}
      </div>
    );
  }

  if (previewState === "working" || previewState === "error") {
    return (
      <div
        data-slot="client-setup-details-fallback"
        className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground"
      >
        {toBadgeLabel(previewState)}
      </div>
    );
  }

  return null;
}

export function PlaintextWarning() {
  return (
    <div
      data-slot="client-setup-plaintext-warning"
      className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground"
    >
      {m.client_setup_plaintext_warning()}
    </div>
  );
}
