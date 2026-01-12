import { useMemo, useState } from "react";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  createDefaultColumnVisibility,
  mergeProviderOptions,
  UPSTREAM_COLUMNS,
} from "@/features/config/cards/upstreams/constants";
import { ColumnsDialog } from "@/features/config/cards/upstreams/columns-dialog";
import { DeleteUpstreamDialog } from "@/features/config/cards/upstreams/delete-dialog";
import { UpstreamEditorDialog } from "@/features/config/cards/upstreams/editor-dialog";
import { UpstreamsTable, UpstreamsToolbar } from "@/features/config/cards/upstreams/table";
import type {
  ColumnVisibility,
  DeleteDialogState,
  UpstreamEditorState,
} from "@/features/config/cards/upstreams/types";
import { createEmptyUpstream } from "@/features/config/form";
import type { UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsCardProps = {
  upstreams: UpstreamForm[];
  appProxyUrl: string;
  showApiKeys: boolean;
  providerOptions: string[];
  onToggleApiKeys: () => void;
  onAdd: (upstream: UpstreamForm) => void;
  onRemove: (index: number) => void;
  onChange: (index: number, patch: Partial<UpstreamForm>) => void;
};

export function UpstreamsCard({ upstreams, appProxyUrl, showApiKeys, providerOptions, onToggleApiKeys, onAdd, onRemove, onChange }: UpstreamsCardProps) {
  const mergedProviderOptions = useMemo(() => mergeProviderOptions(providerOptions), [providerOptions]);
  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibility>(() => createDefaultColumnVisibility());
  const [columnsOpen, setColumnsOpen] = useState(false);
  const [editor, setEditor] = useState<UpstreamEditorState>({ open: false });
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState>({ open: false });

  const columns = useMemo(() => UPSTREAM_COLUMNS.filter((column) => columnVisibility[column.id]), [columnVisibility]);
  const disableDelete = upstreams.length <= 1;
  const apiKeyVisible = columnVisibility.apiKey;

  const updateDraft = (patch: Partial<UpstreamForm>) => setEditor((prev) => (prev.open ? { ...prev, draft: { ...prev.draft, ...patch } } : prev));
  const saveDraft = () => { if (!editor.open) return; editor.mode === "create" ? onAdd(editor.draft) : onChange(editor.index, editor.draft); setEditor({ open: false }); };
  const confirmDelete = () => { if (!deleteDialog.open) return; onRemove(deleteDialog.index); setDeleteDialog({ open: false }); };

  return (
    <Card data-slot="upstreams-card">
      <CardHeader>
        <CardTitle>{m.upstreams_title()}</CardTitle>
        <CardDescription>{m.upstreams_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <UpstreamsToolbar apiKeyVisible={apiKeyVisible} showApiKeys={showApiKeys} onToggleApiKeys={onToggleApiKeys} onAddClick={() => setEditor({ open: true, mode: "create", draft: createEmptyUpstream() })} onColumnsClick={() => setColumnsOpen(true)} />
        {upstreams.length ? (
          <UpstreamsTable upstreams={upstreams} columns={columns} showApiKeys={showApiKeys} disableDelete={disableDelete} onEdit={(index) => upstreams[index] && setEditor({ open: true, mode: "edit", index, draft: { ...upstreams[index] } })} onToggleEnabled={(index) => upstreams[index] && onChange(index, { enabled: !upstreams[index].enabled })} onDelete={(index) => setDeleteDialog({ open: true, index })} />
        ) : (
          <p className="text-sm text-muted-foreground">{m.upstreams_empty()}</p>
        )}
        <p className="text-xs text-muted-foreground">{m.upstreams_tip()}</p>
      </CardContent>

      <ColumnsDialog open={columnsOpen} visibility={columnVisibility} onOpenChange={setColumnsOpen} onToggleColumn={(columnId) => setColumnVisibility((prev) => ({ ...prev, [columnId]: !prev[columnId] }))} />
      <UpstreamEditorDialog editor={editor} providerOptions={mergedProviderOptions} appProxyUrl={appProxyUrl} showApiKeys={showApiKeys} onToggleApiKeys={onToggleApiKeys} onOpenChange={(open) => !open && setEditor({ open: false })} onChangeDraft={updateDraft} onSave={saveDraft} />
      <DeleteUpstreamDialog dialog={deleteDialog} onOpenChange={(open) => !open && setDeleteDialog({ open: false })} onConfirm={confirmDelete} />
    </Card>
  );
}
