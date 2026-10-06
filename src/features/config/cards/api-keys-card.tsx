import { ApiKeyEditor } from "./api-key-editor";
import { ApiKeysTable } from "./api-keys-table";
import { useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Eye, EyeOff, Info, KeyRound, Plus, TriangleAlert } from "lucide-react";
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
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import {
  createLocalApiKey,
  readLocalApiKeyUsage,
  validateLocalApiKeys,
} from "../local-api-keys";
import type { ConfigForm, LocalApiKey, LocalApiKeyUsage } from "../types";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type Props = {
  form: ConfigForm;
  onChange: (patch: Partial<ConfigForm>) => void;
};

export function ApiKeysCard({ form, onChange }: Props) {
  const [draft, setDraft] = useState<LocalApiKey | null>(null);
  const [showKeys, setShowKeys] = useState(false);
  const [deleting, setDeleting] = useState<LocalApiKey | null>(null);
  const [usage, setUsage] = useState<ReadonlyMap<string, LocalApiKeyUsage>>(new Map());
  const keys = form.localApiKeys;
  const enabledCount = keys.filter((key) => key.enabled).length;
  const upstreamIds = new Set(form.upstreams.map((upstream) => upstream.id));
  const isNewDraft = draft !== null && !keys.some((key) => key.id === draft.id);
  const error = draft
    ? validateLocalApiKeys([
        ...keys.filter((key) => key.id !== draft.id),
        draft,
      ])
    : null;

  useEffect(() => {
    let active = true;
    readLocalApiKeyUsage()
      .then((next) => {
        if (active) setUsage(next);
      })
      .catch((error: unknown) => {
        // 用量只是辅助信息，读取失败不影响 Key 管理，列内显示占位。
        console.warn("[api-keys-card] failed to read key usage", parseError(error));
      });
    return () => {
      active = false;
    };
  }, []);

  function save() {
    if (!draft || error) return;
    onChange({
      localApiKeys: isNewDraft
        ? [...keys, draft]
        : keys.map((key) => (key.id === draft.id ? draft : key)),
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
      <Card data-slot="api-keys-card">
        <CardContent className="space-y-4">
          {/* 标题与说明已由页面工具栏展示，这里只保留摘要与操作，对齐渠道页工具栏 */}
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-xs text-muted-foreground">
              {keys.length
                ? m.api_keys_summary({ total: keys.length, enabled: enabledCount })
                : null}
            </p>
            <div className="flex shrink-0 items-center gap-2">
              {keys.length ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => setShowKeys((value) => !value)}
                  aria-label={showKeys ? m.upstreams_hide_api_keys() : m.upstreams_show_api_keys()}
                >
                  {showKeys ? (
                    <EyeOff className="size-4" aria-hidden="true" />
                  ) : (
                    <Eye className="size-4" aria-hidden="true" />
                  )}
                </Button>
              ) : null}
              <Button
                type="button"
                onClick={() => setDraft(createLocalApiKey())}
                aria-label={m.api_keys_add()}
              >
                <Plus className="size-4" aria-hidden="true" />
                {m.common_add()}
              </Button>
            </div>
          </div>
          {keys.length > 0 && enabledCount === 0 ? (
            <Alert variant="destructive">
              <TriangleAlert className="size-4" aria-hidden="true" />
              <AlertDescription>{m.api_keys_all_disabled()}</AlertDescription>
            </Alert>
          ) : null}
          {keys.length ? (
            <ApiKeysTable
              keys={keys}
              upstreamIds={upstreamIds}
              usage={usage}
              showKeys={showKeys}
              onCopy={(key) => void copy(key)}
              onEdit={setDraft}
              onToggleEnabled={(target) =>
                onChange({
                  localApiKeys: keys.map((key) =>
                    key.id === target.id ? { ...key, enabled: !key.enabled } : key,
                  ),
                })
              }
              onDelete={setDeleting}
            />
          ) : (
            <div className="flex flex-col items-center gap-2 rounded-md border border-dashed border-border/60 px-4 py-10 text-center">
              <KeyRound className="size-5 text-muted-foreground" aria-hidden="true" />
              <p className="text-sm text-muted-foreground">{m.api_keys_empty()}</p>
            </div>
          )}
          {form.localApiKeysMigrated ? (
            <p className="flex items-start gap-1.5 text-xs text-muted-foreground">
              <Info className="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
              {m.api_keys_migrated()}
            </p>
          ) : null}
        </CardContent>
      </Card>
      {draft && (
        <ApiKeyEditor
          key={draft.id}
          draft={draft}
          isNew={isNewDraft}
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
