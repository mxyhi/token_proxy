import { useCallback, useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

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
import { Separator } from "@/components/ui/separator";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type ClientSetupInfo = {
  proxy_http_base_url: string;
  claude_settings_path: string;
  claude_base_url: string;
  claude_auth_token_configured: boolean;
  codex_config_path: string;
  codex_auth_path: string;
  codex_disable_response_storage: boolean;
  codex_model: string;
  codex_model_provider: string;
  codex_model_reasoning_effort: string;
  codex_network_access: string;
  codex_preferred_auth_method: string;
  codex_provider_base_url: string;
  codex_provider_name: string;
  codex_provider_requires_openai_auth: boolean;
  codex_provider_wire_api: string;
  codex_api_key_configured: boolean;
  opencode_config_path: string;
  opencode_auth_path: string;
  opencode_provider_id: string;
  opencode_provider_base_url: string;
  opencode_models: string[];
  opencode_api_key_configured: boolean;
};

type ClientConfigWriteResult = {
  paths: string[];
};

type RequestState = "idle" | "working" | "success" | "error";

type ActionState = {
  state: RequestState;
  message: string;
  lastPath: string;
};

function toActionState(): ActionState {
  return { state: "idle", message: "", lastPath: "" };
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

type ClientSetupCardProps = {
  savedAt: string;
  isDirty: boolean;
};

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

function SummaryItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
      <span className="shrink-0 uppercase tracking-[0.2em]">{label}</span>
      <span className="min-w-0 truncate font-mono text-foreground/80">{value}</span>
    </div>
  );
}

function DetailSection({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-1">
      <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">{label}</p>
      {children}
    </div>
  );
}

function MonoBlock({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-3">
      {children}
    </div>
  );
}

function PathList({ paths }: { paths: readonly string[] }) {
  return (
    <MonoBlock>
      <div className="space-y-1 font-mono text-xs text-foreground/80 break-all">
        {paths.map((path, index) => (
          <div key={index}>{path}</div>
        ))}
      </div>
    </MonoBlock>
  );
}

function CodeBlock({ lines }: { lines: readonly string[] }) {
  return (
    <MonoBlock>
      <div className="overflow-x-auto">
        <div className="min-w-max space-y-1 font-mono text-xs text-foreground/80 whitespace-pre">
          {lines.map((line, index) => (
            <div key={index}>{line}</div>
          ))}
        </div>
      </div>
    </MonoBlock>
  );
}

type ToolSetupRowProps = Pick<ToolSetupDialogProps, "title" | "description" | "summary" | "action">;

function ToolSetupRow({ title, description, summary, action }: ToolSetupRowProps) {
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-2">
          <div className="space-y-0.5">
            <p className="truncate text-sm font-medium text-foreground">{title}</p>
            <p className="text-xs text-muted-foreground">{description}</p>
          </div>
          {summary}
        </div>
        <div className="flex items-center gap-2">
          <Badge variant={toBadgeVariant(action.state)}>{toBadgeLabel(action.state)}</Badge>
          <DialogTrigger asChild>
            <Button type="button" variant="outline" size="sm">
              {m.common_show()}
            </Button>
          </DialogTrigger>
        </div>
      </div>
    </div>
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
          <Badge variant={toBadgeVariant(action.state)}>{toBadgeLabel(action.state)}</Badge>
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

function ToolSetupDialog(props: ToolSetupDialogProps) {
  return (
    <Dialog>
      <ToolSetupRow
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

function ClaudeSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  return (
    <div className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.claude_settings_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_env_label()}>
        <CodeBlock
          lines={[
            `ANTHROPIC_BASE_URL=${setup.claude_base_url}`,
            `ANTHROPIC_AUTH_TOKEN=${
              setup.claude_auth_token_configured ? "••••••••" : m.client_setup_not_configured()
            }`,
          ]}
        />
      </DetailSection>
    </div>
  );
}

function CodexSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  return (
    <div className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.codex_config_path, setup.codex_auth_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_config_label()}>
        <CodeBlock
          lines={[
            `disable_response_storage = ${setup.codex_disable_response_storage ? "true" : "false"}`,
            `model = "${setup.codex_model}"`,
            `model_provider = "${setup.codex_model_provider}"`,
            `model_reasoning_effort = "${setup.codex_model_reasoning_effort}"`,
            `network_access = "${setup.codex_network_access}"`,
            `preferred_auth_method = "${setup.codex_preferred_auth_method}"`,
            `[model_providers.${setup.codex_model_provider}]`,
            `base_url = "${setup.codex_provider_base_url}"`,
            `name = "${setup.codex_provider_name}"`,
            `requires_openai_auth = ${setup.codex_provider_requires_openai_auth ? "true" : "false"}`,
            `wire_api = "${setup.codex_provider_wire_api}"`,
            `auth.json: OPENAI_API_KEY = "${
              setup.codex_api_key_configured ? "••••••••" : m.client_setup_not_configured()
            }"`,
          ]}
        />
      </DetailSection>
    </div>
  );
}

function OpenCodeModels({ models }: { models: readonly string[] }) {
  if (models.length === 0) {
    return (
      <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
        {m.client_setup_opencode_models_empty()}
      </div>
    );
  }

  return (
    <div className="flex flex-wrap gap-1.5">
      {models.map((model) => (
        <Badge key={model} variant="outline" className="font-mono text-xs">
          {model}
        </Badge>
      ))}
    </div>
  );
}

function OpenCodeSetupDetails({ setup }: { setup: ClientSetupInfo }) {
  return (
    <div className="space-y-4">
      <DetailSection label={m.client_setup_target_file_label()}>
        <PathList paths={[setup.opencode_config_path, setup.opencode_auth_path]} />
      </DetailSection>
      <DetailSection label={m.client_setup_effective_config_label()}>
        <CodeBlock
          lines={[
            `provider.${setup.opencode_provider_id}.npm = "@ai-sdk/openai-compatible"`,
            `provider.${setup.opencode_provider_id}.options.baseURL = "${setup.opencode_provider_base_url}"`,
            `auth.json: ${setup.opencode_provider_id}.key = "${
              setup.opencode_api_key_configured ? "••••••••" : m.client_setup_not_configured()
            }"`,
          ]}
        />
      </DetailSection>
      <DetailSection label={m.client_setup_opencode_models_label()}>
        <OpenCodeModels models={setup.opencode_models} />
      </DetailSection>
    </div>
  );
}

type WriteCommand = "write_claude_code_settings" | "write_codex_config" | "write_opencode_config";

function useClientSetupPreview(savedAt: string) {
  const [previewState, setPreviewState] = useState<RequestState>("idle");
  const [previewMessage, setPreviewMessage] = useState("");
  const [setup, setSetup] = useState<ClientSetupInfo | null>(null);

  const loadPreview = useCallback(async () => {
    setPreviewState("working");
    setPreviewMessage("");
    try {
      const result = await invoke<ClientSetupInfo>("preview_client_setup");
      setSetup(result);
      setPreviewState("success");
    } catch (error) {
      setPreviewState("error");
      setPreviewMessage(parseError(error));
    }
  }, []);

  useEffect(() => {
    void loadPreview();
  }, [loadPreview, savedAt]);

  return { previewState, previewMessage, setup, loadPreview };
}

function useWriteAction(command: WriteCommand, loadPreview: () => Promise<void>) {
  const [action, setAction] = useState<ActionState>(toActionState);

  const apply = useCallback(async () => {
    setAction({ state: "working", message: "", lastPath: "" });
    try {
      const result = await invoke<ClientConfigWriteResult>(command);
      const path = result.paths.join(", ");
      setAction({ state: "success", message: m.client_setup_apply_success({ path }), lastPath: path });
      await loadPreview();
    } catch (error) {
      setAction({ state: "error", message: parseError(error), lastPath: "" });
    }
  }, [command, loadPreview]);

  return { action, apply };
}

type ToolListItem = {
  id: string;
  title: string;
  description: string;
  summary: ReactNode;
  content: ReactNode;
  action: ActionState;
  canApply: boolean;
  isWorking: boolean;
  onApply: () => void;
};

type ClientSetupOverviewProps = {
  previewState: RequestState;
  previewMessage: string;
  setup: ClientSetupInfo | null;
  isDirty: boolean;
  isWorking: boolean;
  onRefresh: () => void;
};

function ClientSetupOverview({
  previewState,
  previewMessage,
  setup,
  isDirty,
  isWorking,
  onRefresh,
}: ClientSetupOverviewProps) {
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant={toBadgeVariant(previewState)}>{toBadgeLabel(previewState)}</Badge>
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
          <p className="mt-1 font-mono text-xs text-foreground/80">{setup.proxy_http_base_url}</p>
        </div>
      ) : null}
    </div>
  );
}

function ToolDetailsFallback({ previewState, previewMessage }: { previewState: RequestState; previewMessage: string }) {
  if (previewMessage) {
    return (
      <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
        {previewMessage}
      </div>
    );
  }

  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
      {toBadgeLabel(previewState)}
    </div>
  );
}

function ClientSetupToolList({ tools }: { tools: readonly ToolListItem[] }) {
  return (
    <div className="space-y-2">
      {tools.map((tool) => (
        <ToolSetupDialog
          key={tool.id}
          title={tool.title}
          description={tool.description}
          summary={tool.summary}
          action={tool.action}
          canApply={tool.canApply}
          isWorking={tool.isWorking}
          onApply={tool.onApply}
        >
          {tool.content}
        </ToolSetupDialog>
      ))}
    </div>
  );
}

function PlaintextWarning() {
  return (
    <div className="rounded-md border border-border/60 bg-background/60 p-3 text-xs text-muted-foreground">
      {m.client_setup_plaintext_warning()}
    </div>
  );
}

type ClientSetupCardLayoutProps = {
  previewState: RequestState;
  previewMessage: string;
  setup: ClientSetupInfo | null;
  isDirty: boolean;
  isWorking: boolean;
  onRefresh: () => void;
  tools: readonly ToolListItem[];
};

function ClientSetupCardLayout({
  previewState,
  previewMessage,
  setup,
  isDirty,
  isWorking,
  onRefresh,
  tools,
}: ClientSetupCardLayoutProps) {
  return (
    <Card data-slot="client-setup-card">
      <CardHeader>
        <CardTitle>{m.client_setup_title()}</CardTitle>
        <CardDescription>{m.client_setup_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <ClientSetupOverview
          previewState={previewState}
          previewMessage={previewMessage}
          setup={setup}
          isDirty={isDirty}
          isWorking={isWorking}
          onRefresh={onRefresh}
        />
        <ClientSetupToolList tools={tools} />
        <Separator />
        <PlaintextWarning />
      </CardContent>
    </Card>
  );
}

type ToolBuildBaseArgs = {
  setup: ClientSetupInfo | null;
  previewState: RequestState;
  previewMessage: string;
  canApply: boolean;
  isWorking: boolean;
};

type ToolBuildActionArgs = {
  action: ActionState;
  onApply: () => void;
};

function buildClaudeTool({
  setup,
  previewState,
  previewMessage,
  canApply,
  isWorking,
  action,
  onApply,
}: ToolBuildBaseArgs & ToolBuildActionArgs) {
  return {
    id: "claude",
    title: m.client_setup_claude_title(),
    description: m.client_setup_claude_desc(),
    summary: (
      <SummaryItem
        label={m.client_setup_target_file_label()}
        value={setup?.claude_settings_path ?? "—"}
      />
    ),
    content: setup ? (
      <ClaudeSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApply,
    isWorking,
    onApply,
  } satisfies ToolListItem;
}

function buildCodexTool({
  setup,
  previewState,
  previewMessage,
  canApply,
  isWorking,
  action,
  onApply,
}: ToolBuildBaseArgs & ToolBuildActionArgs) {
  return {
    id: "codex",
    title: m.client_setup_codex_title(),
    description: m.client_setup_codex_desc(),
    summary: (
      <SummaryItem
        label={m.client_setup_target_file_label()}
        value={setup ? `${setup.codex_config_path} (+1)` : "—"}
      />
    ),
    content: setup ? (
      <CodexSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApply,
    isWorking,
    onApply,
  } satisfies ToolListItem;
}

type OpenCodeToolArgs = ToolBuildBaseArgs & ToolBuildActionArgs & {
  openCodeModelCount: number;
  canApplyOpenCode: boolean;
};

function buildOpenCodeTool({
  setup,
  previewState,
  previewMessage,
  canApplyOpenCode,
  openCodeModelCount,
  isWorking,
  action,
  onApply,
}: OpenCodeToolArgs) {
  return {
    id: "opencode",
    title: m.client_setup_opencode_title(),
    description: m.client_setup_opencode_desc(),
    summary: (
      <div className="space-y-1">
        <SummaryItem
          label={m.client_setup_target_file_label()}
          value={setup ? `${setup.opencode_config_path} (+1)` : "—"}
        />
        <SummaryItem
          label={m.client_setup_opencode_models_label()}
          value={setup ? String(openCodeModelCount) : "—"}
        />
      </div>
    ),
    content: setup ? (
      <OpenCodeSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApplyOpenCode,
    isWorking,
    onApply,
  } satisfies ToolListItem;
}

export function ClientSetupCard({ savedAt, isDirty }: ClientSetupCardProps) {
  const canApply = !isDirty;
  const { previewState, previewMessage, setup, loadPreview } = useClientSetupPreview(savedAt);

  const claude = useWriteAction("write_claude_code_settings", loadPreview);
  const codex = useWriteAction("write_codex_config", loadPreview);
  const opencode = useWriteAction("write_opencode_config", loadPreview);

  const isWorking =
    previewState === "working" ||
    claude.action.state === "working" ||
    codex.action.state === "working" ||
    opencode.action.state === "working";

  const openCodeModelCount = setup?.opencode_models.length ?? 0;
  const canApplyOpenCode = canApply && openCodeModelCount > 0;

  const baseArgs: ToolBuildBaseArgs = {
    setup,
    previewState,
    previewMessage,
    canApply,
    isWorking,
  };

  const tools: ToolListItem[] = [
    buildClaudeTool({ ...baseArgs, action: claude.action, onApply: claude.apply }),
    buildCodexTool({ ...baseArgs, action: codex.action, onApply: codex.apply }),
    buildOpenCodeTool({
      ...baseArgs,
      action: opencode.action,
      onApply: opencode.apply,
      openCodeModelCount,
      canApplyOpenCode,
    }),
  ];

  return (
    <ClientSetupCardLayout
      previewState={previewState}
      previewMessage={previewMessage}
      setup={setup}
      isDirty={isDirty}
      isWorking={isWorking}
      onRefresh={loadPreview}
      tools={tools}
    />
  );
}
