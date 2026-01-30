import type { ReactNode } from "react";

import type { AgentToolId } from "@/features/config/AppView";

import {
  PlaintextWarning,
  ToolDetailsFallback,
  ToolSetupPanel,
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
  selectedTool: AgentToolId;
};

type ToolPanelItem = {
  id: string;
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
}: ToolBuildBaseArgs & ToolBuildActionArgs): ToolPanelItem {
  return {
    id: "claude",
    content: setup ? (
      <ClaudeSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApply,
    isWorking,
    onApply,
  };
}

function buildCodexTool({
  setup,
  previewState,
  previewMessage,
  canApply,
  isWorking,
  action,
  onApply,
}: ToolBuildBaseArgs & ToolBuildActionArgs): ToolPanelItem {
  return {
    id: "codex",
    content: setup ? (
      <CodexSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApply,
    isWorking,
    onApply,
  };
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
  isWorking,
  action,
  onApply,
}: OpenCodeToolArgs): ToolPanelItem {
  return {
    id: "opencode",
    content: setup ? (
      <OpenCodeSetupDetails setup={setup} />
    ) : (
      <ToolDetailsFallback previewState={previewState} previewMessage={previewMessage} />
    ),
    action,
    canApply: Boolean(setup) && canApplyOpenCode,
    isWorking,
    onApply,
  };
}

export function ClientSetupCard({ savedAt, isDirty, selectedTool }: ClientSetupCardProps) {
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

  // 根据 selectedTool 构建对应的工具面板
  const toolBuilders: Record<AgentToolId, () => ToolPanelItem> = {
    claude: () => buildClaudeTool({ ...baseArgs, action: claude.action, onApply: claude.apply }),
    codex: () => buildCodexTool({ ...baseArgs, action: codex.action, onApply: codex.apply }),
    opencode: () => buildOpenCodeTool({
      ...baseArgs,
      action: opencode.action,
      onApply: opencode.apply,
      openCodeModelCount,
      canApplyOpenCode,
    }),
  };

  const selectedToolItem = toolBuilders[selectedTool]();

  return (
    <>
      <ToolSetupPanel
        action={selectedToolItem.action}
        canApply={selectedToolItem.canApply}
        isWorking={selectedToolItem.isWorking}
        onApply={selectedToolItem.onApply}
      >
        {selectedToolItem.content}
      </ToolSetupPanel>
      <PlaintextWarning />
    </>
  );
}
