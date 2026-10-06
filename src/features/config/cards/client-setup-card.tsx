import { useState, type ReactNode } from "react";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

import { m } from "@/paraglide/messages.js";

import {
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
  const { previewState, previewMessage, setup, loadPreview } = useClientSetupPreview(savedAt);

  const [chosenKeyId, setChosenKeyId] = useState<string | null>(null);
  const keys = setup?.enabled_api_keys ?? [];
  // 用户选过的 Key 失效时保留无效选择，不悄悄替换成另一条。
  const keyId = chosenKeyId ?? (keys.length === 1 ? keys[0].id : null);
  const validSelection = keyId !== null && keys.some((key) => key.id === keyId);
  const canApply = !isDirty && (!setup?.local_auth_required || validSelection);
  const writeKeyId = setup?.local_auth_required ? keyId : null;
  const claude = useWriteAction("write_claude_code_settings", loadPreview, writeKeyId);
  const codex = useWriteAction("write_codex_config", loadPreview, writeKeyId);

  const isWorking =
    previewState === "working" ||
    claude.action.state === "working" ||
    codex.action.state === "working";

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
  ];

  return (
    <>
      {setup?.local_auth_required && <div className="grid gap-2">
        <Label htmlFor="client-api-key">{m.api_keys_client_select()}</Label>
        <Select value={validSelection ? keyId ?? "" : ""} onValueChange={setChosenKeyId}>
          <SelectTrigger id="client-api-key"><SelectValue placeholder={m.api_keys_client_required()} /></SelectTrigger>
          <SelectContent>{keys.map((key) => <SelectItem key={key.id} value={key.id}>{key.name}</SelectItem>)}</SelectContent>
        </Select>
        {keys.length === 0 && <p className="text-sm text-destructive">{m.api_keys_all_disabled()}</p>}
      </div>}
      <ToolCards tools={tools} />
      <PlaintextWarning />
    </>
  );
}
