import { ApiKeyEditor } from "./api-key-editor";
import { useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { toast } from "sonner";

import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { createLocalApiKey, validateLocalApiKeys } from "../local-api-keys";
import type { ConfigForm, LocalApiKey } from "../types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type Props = {
  form: ConfigForm;
  onChange: (patch: Partial<ConfigForm>) => void;
};

export function ApiKeysCard({ form, onChange }: Props) {
  const [draft, setDraft] = useState<LocalApiKey | null>(null);
  const [visible, setVisible] = useState<ReadonlySet<string>>(new Set());
  const [deleting, setDeleting] = useState<LocalApiKey | null>(null);
  const keys = form.localApiKeys;
  const upstreamIds = new Set(form.upstreams.map((upstream) => upstream.id));
  const error = draft
    ? validateLocalApiKeys([
        ...keys.filter((key) => key.id !== draft.id),
        draft,
      ])
    : null;

  function edit(key: LocalApiKey) {
    setDraft(key);
  }

  function save() {
    if (!draft || error) return;
    onChange({
      localApiKeys: keys.some((key) => key.id === draft.id)
        ? keys.map((key) => (key.id === draft.id ? draft : key))
        : [...keys, draft],
    });
    setDraft(null);
  }

  async function copy(key: LocalApiKey) {
    try {
      await writeText(key.key);
      toast.success(m.api_keys_copied());
    } catch (error) {
      toast.error(parseError(error));
    }
  }

  return (
    <>
      {form.localApiKeysMigrated && (
        <Alert>
          <AlertDescription>{m.api_keys_migrated()}</AlertDescription>
        </Alert>
      )}
      <Card>
        <CardHeader className="flex flex-row items-center justify-between gap-4">
          <div className="space-y-1.5">
            <CardTitle>{m.api_keys_title()}</CardTitle>
            <CardDescription>{m.api_keys_description()}</CardDescription>
          </div>
          <Button onClick={() => edit(createLocalApiKey())}>
            {m.api_keys_add()}
          </Button>
        </CardHeader>
        <CardContent className="space-y-3">
          {!keys.length && (
            <p className="text-sm text-muted-foreground">
              {m.api_keys_empty()}
            </p>
          )}
          {keys.length > 0 && keys.every((key) => !key.enabled) && (
            <Alert>
              <AlertDescription>{m.api_keys_all_disabled()}</AlertDescription>
            </Alert>
          )}
          {keys.map((key) => (
            <div
              key={key.id}
              className="flex flex-wrap items-center gap-3 rounded-lg border p-4"
            >
              <div className="min-w-0 flex-1 space-y-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{key.name}</span>
                  <Badge variant="secondary">
                    {key.scope.type === "auto"
                      ? "Auto"
                      : m.api_keys_selected_count({
                          count: key.scope.upstream_ids.length,
                        })}
                  </Badge>
                  {key.scope.type === "selected" &&
                    key.scope.upstream_ids.some(
                      (id) => !upstreamIds.has(id),
                    ) && (
                      <Badge variant="destructive">
                        {m.api_keys_expired()}
                      </Badge>
                    )}
                </div>
                <code className="block break-all text-xs text-muted-foreground">
                  {visible.has(key.id) ? key.key : "••••••••••••••••"}
                </code>
              </div>
              <Switch
                aria-label={`${key.name} ${m.field_status()}`}
                checked={key.enabled}
                onCheckedChange={(enabled) =>
                  onChange({
                    localApiKeys: keys.map((item) =>
                      item.id === key.id ? { ...item, enabled } : item,
                    ),
                  })
                }
              />
              <Button
                variant="ghost"
                size="sm"
                onClick={() =>
                  setVisible((current) => {
                    const next = new Set(current);
                    if (next.has(key.id)) next.delete(key.id);
                    else next.add(key.id);
                    return next;
                  })
                }
              >
                {visible.has(key.id) ? m.common_hide() : m.common_show()}
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={() => void copy(key)}
              >
                {m.api_keys_copy()}
              </Button>
              <Button variant="outline" size="sm" onClick={() => edit(key)}>
                {m.common_edit()}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setDeleting(key)}
              >
                {m.common_delete()}
              </Button>
            </div>
          ))}
        </CardContent>
      </Card>
      {draft && (
        <ApiKeyEditor
          key={draft.id}
          draft={draft}
          upstreams={form.upstreams}
          error={error}
          setDraft={setDraft}
          onSave={save}
          onClose={() => setDraft(null)}
        />
      )}
      <AlertDialog
        open={deleting !== null}
        onOpenChange={(open) => {
          if (!open) setDeleting(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{m.api_keys_delete_title()}</AlertDialogTitle>
            <AlertDialogDescription>
              {keys.length === 1
                ? m.api_keys_delete_last()
                : m.api_keys_delete_description()}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{m.common_cancel()}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                onChange({
                  localApiKeys: keys.filter((key) => key.id !== deleting?.id),
                });
                setDeleting(null);
              }}
            >
              {m.common_delete()}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
