import type { ReactNode } from "react";

import { m } from "@/paraglide/messages.js";

import {
  ClientSetupOverviewCard,
  PlaintextWarning,
  SummaryItem,
  ToolDetailsFallback,
  ToolSetupDialog,
} from "./client-setup-ui";
import {
  useClientSetupPreview,
  useWriteAction,
  type ActionState,
  type ClientSetupInfo,
  type RequestState,
} from "./client-setup-state";
import {
  ClaudeSetupDetails,
  CodexSetupDetails,
  OpenCodeSetupDetails,
} from "./client-setup-details";

type ClientSetupCardProps = {
  savedAt: string;
  isDirty: boolean;
};

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

function ToolCards({ tools }: { tools: readonly ToolListItem[] }) {
  return (
    <>
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
    </>
  );
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
    <>
      <ClientSetupOverviewCard
        previewState={previewState}
        previewMessage={previewMessage}
        setup={setup}
        isDirty={isDirty}
        isWorking={isWorking}
        onRefresh={loadPreview}
      />
      <ToolCards tools={tools} />
      <PlaintextWarning />
    </>
  );
}
