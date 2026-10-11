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
  createAutoUpstreamId,
  isAccountBackedProviderSet,
  isAccountCredentialUpstream,
  isAccountProviderKind,
  normalizeProviders,
  pruneConvertFromMap,
  providersEqual,
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
  /** 统一添加弹窗：内部分流 API Key 编辑 / 账户登录导入。 */
  const [addUpstreamOpen, setAddUpstreamOpen] = useState(false);
  const [editor, setEditor] = useState<UpstreamEditorState>({ open: false });
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState>({ open: false });
  const columns = useMemo(
    () => UPSTREAM_COLUMNS.filter((column) => columnVisibility[column.id]),
    [columnVisibility]
  );
  const apiKeyVisible = columnVisibility.apiKeys;
  // 生成 ID 时排除正在编辑的渠道自身，避免清空后重新生成出 "-2"。
  const getOtherUpstreams = useCallback(
    (state: UpstreamEditorState) =>
      state.open && state.mode === "edit"
        ? upstreams.filter((_, index) => index !== state.index)
        : upstreams,
    [upstreams],
  );

  // 更新 draft：provider 变化时收敛 account 字段 / openai 专属开关。
  const updateDraft = useCallback(
    (patch: Partial<UpstreamForm>) => {
      // ID 跟随规则：手动输入即停止跟随，清空则恢复；跟随期间 Base URL/provider 变化时重新生成。
      const withAutoId = (
        prev: Extract<UpstreamEditorState, { open: true }>,
        next: Extract<UpstreamEditorState, { open: true }>,
      ): UpstreamEditorState => {
        if (patch.id !== undefined) {
          return { ...next, autoId: !patch.id.trim() };
        }
        const sourceChanged =
          next.draft.baseUrl !== prev.draft.baseUrl ||
          !providersEqual(next.draft.providers, prev.draft.providers);
        if (!next.autoId || !sourceChanged) {
          return next;
        }
        const id = createAutoUpstreamId(next.draft, getOtherUpstreams(next));
        return { ...next, draft: { ...next.draft, id } };
      };

      setEditor((prev) => {
        if (!prev.open) return prev;

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

          return withAutoId(prev, {
            ...prev,
            draft: {
              ...prev.draft,
              ...patch,
              providers: nextProviders,
              baseUrl,
              filterPromptCacheRetention,
              filterSafetyIdentifier,
              useChatCompletionsForResponses,
              rewriteDeveloperRoleToSystem,
              accountId,
              convertFromMap,
            },
          });
        }
        return withAutoId(prev, { ...prev, draft: { ...prev.draft, ...patch } });
      });
    },
    [getOtherUpstreams],
  );

  const openCreateDialog = () => {
    const empty = createEmptyUpstream();
    const draft = { ...empty, providers: normalizeProviders(empty.providers) };
    const id = createAutoUpstreamId(draft, upstreams);
    setEditor({ open: true, mode: "create", draft: { ...draft, id }, autoId: true });
  };

  const openEditDialog = (index: number) => {
    const upstream = upstreams[index];
    if (!upstream) {
      return;
    }
    // 既有渠道 ID 被 API Key 绑定/统计引用，编辑时默认不跟随 Base URL。
    setEditor({ open: true, mode: "edit", index, draft: cloneUpstreamDraft(upstream), autoId: false });
  };

  const openCopyDialog = (index: number) => {
    const upstream = upstreams[index];
    if (!upstream || isAccountCredentialUpstream(upstream)) {
      // 账户型禁止复制同 credential。
      return;
    }
    // 复制通常是为了换个地址，ID 跟随 Base URL，直到用户手动修改。
    const draft = cloneUpstreamDraft(upstream);
    const id = createAutoUpstreamId(draft, upstreams);
    setEditor({ open: true, mode: "create", draft: { ...draft, id }, autoId: true });
  };

  const saveDraft = () => {
    if (!editor.open) {
      return;
    }

    // ID 留空时按占位提示的自动 ID 落盘，而不是保存后才报必填。
    const draft = editor.draft.id.trim()
      ? editor.draft
      : { ...editor.draft, id: createAutoUpstreamId(editor.draft, getOtherUpstreams(editor)) };
    if (editor.mode === "create") {
      onAdd(draft);
    } else {
      onChange(editor.index, draft);
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
          onAddClick={() => {
            console.debug("[upstreams-card] open add-upstream dialog");
            setAddUpstreamOpen(true);
          }}
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
            onPriorityChange={(index, priority) => {
              console.debug("[upstreams-card] inline priority change", { index, priority });
              onChange(index, { priority });
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
        idPlaceholder={
          editor.open ? createAutoUpstreamId(editor.draft, getOtherUpstreams(editor)) : ""
        }
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
        open={addUpstreamOpen}
        onOpenChange={setAddUpstreamOpen}
        onAccountsChanged={onConfigReload}
        onSelectApiKey={() => {
          // 选 API Key 类型后关闭统一弹窗，进入现有上游编辑器。
          console.debug("[upstreams-card] add-upstream kind=api_key");
          setAddUpstreamOpen(false);
          openCreateDialog();
        }}
      />
    </Card>
  );
}
