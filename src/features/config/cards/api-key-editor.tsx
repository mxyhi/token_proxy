import { useState } from "react";
import {
  AlertDialog,
  AlertDialogBody,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PasswordInput } from "@/components/ui/password-input";
import { Switch } from "@/components/ui/switch";
import { ApiKeyUpstreamPicker } from "./api-key-upstream-picker";
import type { LocalApiKey, UpstreamForm } from "../types";
import { m } from "@/paraglide/messages.js";

type Props = {
  draft: LocalApiKey;
  isNew: boolean;
  upstreams: UpstreamForm[];
  error: string | null;
  setDraft: (key: LocalApiKey) => void;
  onSave: () => void;
  onClose: () => void;
};

/** 布局对齐渠道编辑弹窗：启用开关在标题右侧，字段左标签右输入。 */
export function ApiKeyEditor({
  draft,
  isNew,
  upstreams,
  error,
  setDraft,
  onSave,
  onClose,
}: Props) {
  const [draftVisible, setDraftVisible] = useState(false);

  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <AlertDialogContent className="max-w-2xl">
        <AlertDialogHeader className="flex-row items-start justify-between gap-6 text-left">
          <div className="space-y-2">
            <AlertDialogTitle>
              {isNew ? m.api_keys_editor_title_add() : m.api_keys_editor_title()}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {m.api_keys_editor_description()}
            </AlertDialogDescription>
          </div>
          <Label className="flex shrink-0 items-center gap-2 font-normal">
            <span>{draft.enabled ? m.common_enabled() : m.common_disabled()}</span>
            <Switch
              checked={draft.enabled}
              onCheckedChange={(enabled) => setDraft({ ...draft, enabled })}
              aria-label={m.field_status()}
            />
          </Label>
        </AlertDialogHeader>
        <AlertDialogBody className="space-y-5 pr-2">
          <div className="grid grid-cols-[minmax(7rem,auto)_1fr] items-center gap-x-4 gap-y-4">
            <Label htmlFor="local-key-name">{m.api_keys_name()}</Label>
            <Input
              id="local-key-name"
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            />
            <Label htmlFor="local-key-value">{m.api_keys_value()}</Label>
            <PasswordInput
              id="local-key-value"
              autoComplete="off"
              className="font-mono md:text-xs"
              value={draft.key}
              visible={draftVisible}
              onVisibilityChange={() => setDraftVisible((value) => !value)}
              onChange={(event) => setDraft({ ...draft, key: event.target.value })}
            />
          </div>
          <section className="space-y-3 border-t pt-5">
            <div className="flex items-start justify-between gap-6">
              <div className="space-y-1">
                <h3 className="text-sm font-semibold">{m.api_keys_scope()}</h3>
                <p className="text-xs text-muted-foreground">{m.api_keys_auto_help()}</p>
              </div>
              <Label className="flex shrink-0 items-center gap-2 font-normal">
                Auto
                <Switch
                  checked={draft.scope.type === "auto"}
                  onCheckedChange={(auto) =>
                    setDraft({
                      ...draft,
                      scope: auto ? { type: "auto" } : { type: "selected", upstream_ids: [] },
                    })
                  }
                />
              </Label>
            </div>
            {draft.scope.type === "selected" ? (
              <ApiKeyUpstreamPicker
                upstreams={upstreams}
                selected={draft.scope.upstream_ids}
                onChange={(upstream_ids) =>
                  setDraft({ ...draft, scope: { type: "selected", upstream_ids } })
                }
              />
            ) : null}
          </section>
          {error ? (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          ) : null}
        </AlertDialogBody>
        <AlertDialogFooter>
          <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
          <Button disabled={!!error} onClick={onSave}>
            {m.common_save()}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
