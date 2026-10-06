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
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PasswordInput } from "@/components/ui/password-input";
import { Switch } from "@/components/ui/switch";
import type { LocalApiKey, UpstreamForm } from "../types";
import { m } from "@/paraglide/messages.js";

type Props = {
  draft: LocalApiKey;
  upstreams: UpstreamForm[];
  error: string | null;
  setDraft: (key: LocalApiKey) => void;
  onSave: () => void;
  onClose: () => void;
};

export function ApiKeyEditor({
  draft,
  upstreams,
  error,
  setDraft,
  onSave,
  onClose,
}: Props) {
  const [search, setSearch] = useState("");
  const [draftVisible, setDraftVisible] = useState(false);
  const selected =
    draft.scope.type === "selected" ? draft.scope.upstream_ids : [];
  const upstreamIds = new Set(upstreams.map((upstream) => upstream.id));
  const missing = selected.filter((id) => !upstreamIds.has(id));
  const filtered = upstreams.filter((upstream) =>
    `${upstream.id} ${upstream.providers.join(" ")}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  function select(id: string, checked: boolean) {
    setDraft({
      ...draft,
      scope: {
        type: "selected",
        upstream_ids: checked
          ? [...selected, id]
          : selected.filter((value) => value !== id),
      },
    });
  }

  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <AlertDialogContent className="max-w-2xl">
        <AlertDialogHeader>
          <AlertDialogTitle>{m.api_keys_editor_title()}</AlertDialogTitle>
          <AlertDialogDescription>
            {m.api_keys_editor_description()}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogBody className="space-y-4">
          <>
            <div className="grid gap-2">
              <Label htmlFor="local-key-name">{m.api_keys_name()}</Label>
              <Input
                id="local-key-name"
                value={draft.name}
                onChange={(event) =>
                  setDraft({ ...draft, name: event.target.value })
                }
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="local-key-value">{m.api_keys_value()}</Label>
              <PasswordInput
                id="local-key-value"
                autoComplete="off"
                value={draft.key}
                visible={draftVisible}
                onVisibilityChange={() => setDraftVisible((value) => !value)}
                onChange={(event) =>
                  setDraft({ ...draft, key: event.target.value })
                }
              />
            </div>
            <Label className="flex items-center justify-between">
              {m.common_enabled()}
              <Switch
                checked={draft.enabled}
                onCheckedChange={(enabled) => setDraft({ ...draft, enabled })}
              />
            </Label>
            <Label className="flex items-center justify-between">
              Auto
              <Switch
                checked={draft.scope.type === "auto"}
                onCheckedChange={(auto) =>
                  setDraft({
                    ...draft,
                    scope: auto
                      ? { type: "auto" }
                      : { type: "selected", upstream_ids: [] },
                  })
                }
              />
            </Label>
            <p className="text-xs text-muted-foreground">
              {m.api_keys_auto_help()}
            </p>
            {draft.scope.type === "selected" && (
              <div className="space-y-3">
                <Input
                  aria-label={m.api_keys_search()}
                  placeholder={m.api_keys_search()}
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                />
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() =>
                      setDraft({
                        ...draft,
                        scope: {
                          type: "selected",
                          upstream_ids: [
                            ...new Set([
                              ...selected,
                              ...filtered.map((upstream) => upstream.id),
                            ]),
                          ],
                        },
                      })
                    }
                  >
                    {m.api_keys_select_all()}
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      setDraft({
                        ...draft,
                        scope: { type: "selected", upstream_ids: [] },
                      })
                    }
                  >
                    {m.api_keys_clear()}
                  </Button>
                </div>
                <div className="max-h-64 space-y-3 overflow-auto">
                  {filtered.map((upstream) => (
                    <Label
                      key={upstream.id}
                      className="flex items-center gap-2"
                    >
                      <Checkbox
                        checked={selected.includes(upstream.id)}
                        onCheckedChange={(checked) =>
                          select(upstream.id, checked === true)
                        }
                      />
                      <span className="break-all">{upstream.id}</span>
                      <span className="text-xs text-muted-foreground">
                        {upstream.providers.join(", ")}
                        {!upstream.enabled && ` · ${m.common_disabled()}`}
                      </span>
                    </Label>
                  ))}
                  {missing.map((id) => (
                    <Label
                      key={id}
                      className="flex items-center gap-2 text-destructive"
                    >
                      <Checkbox
                        checked
                        onCheckedChange={() => select(id, false)}
                      />
                      {id} · {m.api_keys_expired()}
                    </Label>
                  ))}
                </div>
              </div>
            )}
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
          </>
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
