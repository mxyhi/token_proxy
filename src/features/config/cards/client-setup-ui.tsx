import type { ReactNode } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { m } from "@/paraglide/messages.js";

import type { ActionState, ClientSetupInfo, RequestState } from "./client-setup-state";

type ToolSetupDialogProps = {
  title: string;
  description: string;
  summary: ReactNode;
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

type ToolSetupCardProps = Pick<ToolSetupDialogProps, "title" | "description" | "summary" | "action">;

function ToolSetupCard({ title, description, summary, action }: ToolSetupCardProps) {
  return (
    <Card data-slot="client-setup-tool-card">
      <CardHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <CardTitle>{title}</CardTitle>
            <CardDescription>{description}</CardDescription>
          </div>
          {shouldShowBadge(action.state) ? (
            <Badge variant={toBadgeVariant(action.state)}>{toBadgeLabel(action.state)}</Badge>
          ) : null}
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        {summary}
        <DialogTrigger asChild>
          <Button type="button" variant="outline" size="sm">
            {m.common_show()}
          </Button>
        </DialogTrigger>
      </CardContent>
    </Card>
  );
}

type ToolSetupModalProps = Omit<ToolSetupDialogProps, "summary">;

function ToolSetupModal({
  title,
  description,
  action,
  canApply,
  isWorking,
  onApply,
  children,
}: ToolSetupModalProps) {
  return (
    <DialogContent className="max-w-2xl">
      <DialogHeader>
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <DialogTitle>{title}</DialogTitle>
            <DialogDescription>{description}</DialogDescription>
          </div>
          {shouldShowBadge(action.state) ? (
            <Badge variant={toBadgeVariant(action.state)}>{toBadgeLabel(action.state)}</Badge>
          ) : null}
        </div>
      </DialogHeader>

      <DialogBody className="space-y-4">
        {children}

        {action.message ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {action.message}
          </div>
        ) : null}

        <p className="text-xs text-muted-foreground">{m.client_setup_backup_hint()}</p>
      </DialogBody>

      <DialogFooter>
        <DialogClose asChild>
          <Button type="button" variant="outline">
            {m.common_close()}
          </Button>
        </DialogClose>
        <Button type="button" onClick={onApply} disabled={!canApply || isWorking}>
          {m.client_setup_apply()}
        </Button>
      </DialogFooter>
    </DialogContent>
  );
}

export function ToolSetupDialog(props: ToolSetupDialogProps) {
  return (
    <Dialog>
      <ToolSetupCard
        title={props.title}
        description={props.description}
        summary={props.summary}
        action={props.action}
      />
      <ToolSetupModal
        title={props.title}
        description={props.description}
        action={props.action}
        canApply={props.canApply}
        isWorking={props.isWorking}
        onApply={props.onApply}
      >
        {props.children}
      </ToolSetupModal>
    </Dialog>
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

type ClientSetupOverviewCardProps = {
  previewState: RequestState;
  previewMessage: string;
  setup: ClientSetupInfo | null;
  isDirty: boolean;
  isWorking: boolean;
  onRefresh: () => void;
};

export function ClientSetupOverviewCard({
  previewState,
  previewMessage,
  setup,
  isDirty,
  isWorking,
  onRefresh,
}: ClientSetupOverviewCardProps) {
  return (
    <Card data-slot="client-setup-overview-card">
      <CardHeader>
        <CardTitle>{m.client_setup_title()}</CardTitle>
        <CardDescription>{m.client_setup_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap items-center gap-2">
          {shouldShowBadge(previewState) ? (
            <Badge variant={toBadgeVariant(previewState)}>{toBadgeLabel(previewState)}</Badge>
          ) : null}
          <Button type="button" variant="outline" size="sm" onClick={onRefresh} disabled={isWorking}>
            {m.common_refresh()}
          </Button>
        </div>

        {previewMessage ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {previewMessage}
          </div>
        ) : null}

        {isDirty ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
            {m.client_setup_dirty_notice()}
          </div>
        ) : null}

        {setup ? (
          <div className="rounded-md border border-border/60 bg-background/60 p-3">
            <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
              {m.client_setup_proxy_base_url_label()}
            </p>
            <p className="mt-1 font-mono text-xs text-foreground/80">
              {setup.proxy_http_base_url}
            </p>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}
