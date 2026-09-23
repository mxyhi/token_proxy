import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogBody,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogDescription,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { UpstreamEditorFields } from "@/features/config/cards/upstreams/editor-dialog-form";
import type { UpstreamEditorState } from "@/features/config/cards/upstreams/types";
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
}: UpstreamEditorDialogProps) {
  const title = editor.open
    ? editor.mode === "create"
      ? m.upstreams_editor_title_add()
      : m.upstreams_editor_title_edit()
    : m.upstreams_editor_title_default();
  return (
    <AlertDialog open={editor.open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-w-3xl">
        <AlertDialogHeader className="flex-row items-start justify-between gap-6 text-left">
          <div className="space-y-2">
            <AlertDialogTitle>{title}</AlertDialogTitle>
            <AlertDialogDescription>{m.upstreams_editor_subtitle()}</AlertDialogDescription>
          </div>
          {editor.open ? (
            <Label className="flex shrink-0 items-center gap-2 font-normal">
              <span>{editor.draft.enabled ? m.common_enabled() : m.common_disabled()}</span>
              <Switch
                checked={editor.draft.enabled}
                onCheckedChange={(enabled) => onChangeDraft({ enabled })}
                aria-label={m.field_status()}
              />
            </Label>
          ) : null}
        </AlertDialogHeader>
        <AlertDialogBody className="pr-2">
          {editor.open ? (
            <UpstreamEditorFields
              draft={editor.draft}
              providerOptions={providerOptions}
              appProxyUrl={appProxyUrl}
              showApiKeys={showApiKeys}
              onToggleApiKeys={onToggleApiKeys}
              onChangeDraft={onChangeDraft}
              prefillSuffixDefaults={editor.mode === "create"}
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
