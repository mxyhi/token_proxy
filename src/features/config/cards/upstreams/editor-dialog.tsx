import { HelpCircle } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogBody,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { getUpstreamLabel } from "@/features/config/cards/upstreams/constants";
import { UpstreamEditorFields } from "@/features/config/cards/upstreams/editor-dialog-form";
import type { UpstreamEditorState } from "@/features/config/cards/upstreams/types";
import type { AntigravityAccountSummary } from "@/features/antigravity/types";
import type { CodexAccountSummary } from "@/features/codex/types";
import type { KiroAccountSummary } from "@/features/kiro/types";
import type { UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamEditorDialogProps = {
  editor: UpstreamEditorState;
  providerOptions: readonly string[];
  appProxyUrl: string;
  showApiKeys: boolean;
  onToggleApiKeys: () => void;
  onOpenChange: (open: boolean) => void;
  onChangeDraft: (patch: Partial<UpstreamForm>) => void;
  onSave: () => void;
  kiroAccounts: KiroAccountSummary[];
  kiroAccountsLoading: boolean;
  kiroAccountsError: string;
  onRefreshKiroAccounts: () => void;
  codexAccounts: CodexAccountSummary[];
  codexAccountsLoading: boolean;
  codexAccountsError: string;
  onRefreshCodexAccounts: () => void;
  antigravityAccounts: AntigravityAccountSummary[];
  antigravityAccountsLoading: boolean;
  antigravityAccountsError: string;
  onRefreshAntigravityAccounts: () => void;
};

export function UpstreamEditorDialog({
  editor,
  providerOptions,
  appProxyUrl,
  showApiKeys,
  onToggleApiKeys,
  onOpenChange,
  onChangeDraft,
  onSave,
  kiroAccounts,
  kiroAccountsLoading,
  kiroAccountsError,
  onRefreshKiroAccounts,
  codexAccounts,
  codexAccountsLoading,
  codexAccountsError,
  onRefreshCodexAccounts,
  antigravityAccounts,
  antigravityAccountsLoading,
  antigravityAccountsError,
  onRefreshAntigravityAccounts,
}: UpstreamEditorDialogProps) {
  const title = editor.open
    ? editor.mode === "create"
      ? m.upstreams_editor_title_add()
      : m.upstreams_editor_title_edit()
    : m.upstreams_editor_title_default();
  const description =
    editor.open && editor.mode === "edit"
      ? m.upstreams_editor_description_edit({
          rowLabel: getUpstreamLabel(editor.index),
        })
      : m.upstreams_editor_description_create();

  return (
    <AlertDialog open={editor.open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-w-xl">
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            {title}
            <Tooltip>
              <TooltipTrigger asChild>
                <HelpCircle className="size-4 text-muted-foreground cursor-help" />
              </TooltipTrigger>
              <TooltipContent side="right" className="max-w-xs">
                {description}
              </TooltipContent>
            </Tooltip>
          </AlertDialogTitle>
        </AlertDialogHeader>
        <AlertDialogBody className="space-y-4 pr-1">
          {editor.open ? (
            <UpstreamEditorFields
              draft={editor.draft}
              providerOptions={providerOptions}
              appProxyUrl={appProxyUrl}
              showApiKeys={showApiKeys}
              onToggleApiKeys={onToggleApiKeys}
              onChangeDraft={onChangeDraft}
              kiroAccounts={kiroAccounts}
              kiroAccountsLoading={kiroAccountsLoading}
              kiroAccountsError={kiroAccountsError}
              onRefreshKiroAccounts={onRefreshKiroAccounts}
              codexAccounts={codexAccounts}
              codexAccountsLoading={codexAccountsLoading}
              codexAccountsError={codexAccountsError}
              onRefreshCodexAccounts={onRefreshCodexAccounts}
              antigravityAccounts={antigravityAccounts}
              antigravityAccountsLoading={antigravityAccountsLoading}
              antigravityAccountsError={antigravityAccountsError}
              onRefreshAntigravityAccounts={onRefreshAntigravityAccounts}
            />
          ) : null}
        </AlertDialogBody>
        <AlertDialogFooter>
          <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
          <AlertDialogAction onClick={onSave}>
            {m.common_save()}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

