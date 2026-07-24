import { useCallback, useMemo, useState } from "react";

import { Card, CardContent } from "@/components/ui/card";
import {
  createDefaultColumnVisibility,
  mergeProviderOptions,
  UPSTREAM_COLUMNS,
} from "@/features/config/cards/upstreams/constants";
import {
  cloneUpstreamDraft,
  coerceProviderSelection,
  createCopiedUpstreamId,
  isAccountBackedProviderSet,
  isAccountCredentialUpstream,
  isAccountProviderKind,
  normalizeProviders,
  pruneConvertFromMap,
  providersEqual,
  resolveUpstreamIdForProviderChange,
} from "@/features/config/cards/upstreams/upstream-editor-helpers";
import { AddAccountDialog } from "@/features/config/cards/upstreams/add-account-dialog";
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
import type { ConfigForm, UpstreamForm } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsCardProps = {
  upstreams: UpstreamForm[];
  appProxyUrl: string;
  strategy: ConfigForm["upstreamStrategy"];
  showApiKeys: boolean;
  providerOptions: string[];
  onToggleApiKeys: () => void;
  onStrategyChange: (value: ConfigForm["upstreamStrategy"]) => void;
  onAdd: (upstream: UpstreamForm) => void;
  onRemove: (index: number) => void;
  onChange: (index: number, patch: Partial<UpstreamForm>) => void;
  /** 登录/导入账户后 reload config，使后端 reconcile 写入 account Upstream。 */
  onConfigReload: () => void;
};

export function UpstreamsCard({
  upstreams,
  appProxyUrl,
  strategy,
  showApiKeys,
  providerOptions,
  onToggleApiKeys,
  onStrategyChange,
  onAdd,
  onRemove,
  onChange,
  onConfigReload,
}: UpstreamsCardProps) {
  const mergedProviderOptions = useMemo(
    () => mergeProviderOptions(providerOptions),
    [providerOptions]
  );
  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibility>(() =>
    createDefaultColumnVisibility()
  );
  const [columnsOpen, setColumnsOpen] = useState(false);
  const [addAccountOpen, setAddAccountOpen] = useState(false);
  const [editor, setEditor] = useState<UpstreamEditorState>({ open: false });
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState>({ open: false });
  const columns = useMemo(
    () => UPSTREAM_COLUMNS.filter((column) => columnVisibility[column.id]),
    [columnVisibility]
  );
  const apiKeyVisible = columnVisibility.apiKeys;

  // 更新 draft：provider 变化时收敛 account 字段 / openai 专属开关。
  const updateDraft = useCallback(
    (patch: Partial<UpstreamForm>) => {
      setEditor((prev) => {
        if (!prev.open) return prev;

        const editingIndex = prev.mode === "edit" ? prev.index : undefined;
        const currentProviders = normalizeProviders(prev.draft.providers);
        const nextProviders =
          patch.providers === undefined
            ? currentProviders
            : coerceProviderSelection(patch.providers);
        const providersChanged =
          patch.providers !== undefined &&
          !providersEqual(nextProviders, currentProviders);

        if (providersChanged) {
          let filterPromptCacheRetention = prev.draft.filterPromptCacheRetention;
          let filterSafetyIdentifier = prev.draft.filterSafetyIdentifier;
          let useChatCompletionsForResponses = prev.draft.useChatCompletionsForResponses;
          let rewriteDeveloperRoleToSystem = prev.draft.rewriteDeveloperRoleToSystem;
          let accountId = prev.draft.accountId;
          let baseUrl = patch.baseUrl ?? prev.draft.baseUrl;
          let convertFromMap = patch.convertFromMap ?? prev.draft.convertFromMap;

          if (!nextProviders.includes("openai-response")) {
            filterPromptCacheRetention = false;
            filterSafetyIdentifier = false;
            useChatCompletionsForResponses = false;
          }
          if (!nextProviders.some((provider) => provider === "openai" || provider === "openai-response")) {
            rewriteDeveloperRoleToSystem = false;
          }
          // 离开账户型 provider 时清空 accountId；进入账户型时清空 baseUrl/apiKeys。
          const nextAccountProvider = nextProviders[0];
          if (
            nextProviders.length !== 1 ||
            !nextAccountProvider ||
            !isAccountProviderKind(nextAccountProvider)
          ) {
            accountId = "";
          }
          if (isAccountBackedProviderSet(nextProviders)) {
            baseUrl = "";
            patch.apiKeys = "";
          }
          if (patch.filterPromptCacheRetention !== undefined) {
            filterPromptCacheRetention = patch.filterPromptCacheRetention;
          }
          if (patch.filterSafetyIdentifier !== undefined) {
            filterSafetyIdentifier = patch.filterSafetyIdentifier;
          }
          if (patch.useChatCompletionsForResponses !== undefined) {
            useChatCompletionsForResponses = patch.useChatCompletionsForResponses;
          }
          if (patch.rewriteDeveloperRoleToSystem !== undefined) {
            rewriteDeveloperRoleToSystem = patch.rewriteDeveloperRoleToSystem;
          }
          if (patch.accountId !== undefined) {
            accountId = patch.accountId;
          }

          convertFromMap = pruneConvertFromMap(convertFromMap, nextProviders);

          const id = resolveUpstreamIdForProviderChange({
            mode: prev.mode,
            currentId: prev.draft.id,
            currentProviders,
            nextProviders,
            upstreams,
            editingIndex,
          });

          return {
            ...prev,
            draft: {
              ...prev.draft,
              ...patch,
              providers: nextProviders,
              id,
              baseUrl,
              filterPromptCacheRetention,
              filterSafetyIdentifier,
              useChatCompletionsForResponses,
              rewriteDeveloperRoleToSystem,
              accountId,
              convertFromMap,
            },
          };
        }
        return { ...prev, draft: { ...prev.draft, ...patch } };
      });
    },
    [upstreams],
  );

  const openCreateDialog = () => {
    const draft = createEmptyUpstream();
    const nextProviders = normalizeProviders(draft.providers);
    const hasDuplicateId = upstreams.some((upstream) => upstream.id.trim() === draft.id.trim());
    const nextId = hasDuplicateId ? createCopiedUpstreamId(draft.id, upstreams) : draft.id;
    setEditor({ open: true, mode: "create", draft: { ...draft, id: nextId, providers: nextProviders } });
  };

  const openEditDialog = (index: number) => {
    const upstream = upstreams[index];
    if (!upstream) {
      return;
    }
    setEditor({ open: true, mode: "edit", index, draft: cloneUpstreamDraft(upstream) });
  };

  const openCopyDialog = (index: number) => {
    const upstream = upstreams[index];
    if (!upstream || isAccountCredentialUpstream(upstream)) {
      // 账户型禁止复制同 credential。
      return;
    }
    const nextId = createCopiedUpstreamId(upstream.id, upstreams);
    const draft: UpstreamForm = {
      ...cloneUpstreamDraft(upstream),
      id: nextId,
    };
    setEditor({ open: true, mode: "create", draft });
  };

  const saveDraft = () => {
    if (!editor.open) {
      return;
    }

    if (editor.mode === "create") {
      onAdd(editor.draft);
    } else {
      onChange(editor.index, editor.draft);
    }
    setEditor({ open: false });
  };

  const confirmDelete = () => {
    if (!deleteDialog.open) {
      return;
    }
    console.debug("[upstreams-card] delete upstream", { index: deleteDialog.index });
    onRemove(deleteDialog.index);
    setDeleteDialog({ open: false });
  };

  const deleteUpstream =
    deleteDialog.open && deleteDialog.index >= 0
      ? upstreams[deleteDialog.index]
      : undefined;

  return (
    <Card data-slot="upstreams-card">
      <CardContent className="space-y-4">
        <UpstreamsToolbar
          apiKeyVisible={apiKeyVisible}
          showApiKeys={showApiKeys}
          onToggleApiKeys={onToggleApiKeys}
          onAddClick={openCreateDialog}
          onAddAccountClick={() => setAddAccountOpen(true)}
          onColumnsClick={() => setColumnsOpen(true)}
          strategy={strategy}
          onStrategyChange={onStrategyChange}
        />
        {upstreams.length ? (
          <UpstreamsTable
            upstreams={upstreams}
            columns={columns}
            showApiKeys={showApiKeys}
            disableDelete={false}
            isCopyDisabled={isAccountCredentialUpstream}
            isDeleteDisabled={() => false}
            onEdit={openEditDialog}
            onCopy={openCopyDialog}
            onToggleEnabled={(index) => {
              const upstream = upstreams[index];
              if (!upstream) {
                return;
              }
              onChange(index, { enabled: !upstream.enabled });
            }}
            onDelete={(index) => setDeleteDialog({ open: true, index })}
          />
        ) : (
          <p className="text-sm text-muted-foreground">{m.upstreams_empty()}</p>
        )}
        <p className="text-xs text-muted-foreground">{m.upstreams_tip()}</p>
      </CardContent>

      <ColumnsDialog
        open={columnsOpen}
        visibility={columnVisibility}
        onOpenChange={setColumnsOpen}
        onToggleColumn={(columnId) =>
          setColumnVisibility((prev) => ({ ...prev, [columnId]: !prev[columnId] }))
        }
      />
      <UpstreamEditorDialog
        editor={editor}
        providerOptions={mergedProviderOptions}
        appProxyUrl={appProxyUrl}
        showApiKeys={showApiKeys}
        onToggleApiKeys={onToggleApiKeys}
        onOpenChange={(open) => !open && setEditor({ open: false })}
        onChangeDraft={updateDraft}
        onSave={saveDraft}
      />
      <DeleteUpstreamDialog
        dialog={deleteDialog}
        accountBacked={deleteUpstream ? isAccountCredentialUpstream(deleteUpstream) : false}
        onOpenChange={(open) => !open && setDeleteDialog({ open: false })}
        onConfirm={confirmDelete}
      />
      <AddAccountDialog
        open={addAccountOpen}
        onOpenChange={setAddAccountOpen}
        onAccountsChanged={onConfigReload}
      />
    </Card>
  );
}
