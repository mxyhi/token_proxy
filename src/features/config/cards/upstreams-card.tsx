import { useCallback, useMemo, useState } from "react";

import { HelpCircle } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import {
  createDefaultColumnVisibility,
  mergeProviderOptions,
  UPSTREAM_COLUMNS,
} from "@/features/config/cards/upstreams/constants";
import {
  cloneUpstreamDraft,
  coerceProviderSelection,
  createAutoUpstreamId,
  createCopiedUpstreamId,
  findIdleAntigravityAccount,
  findIdleCodexAccount,
  findIdleKiroAccount,
  hasProvider,
  normalizeProviders,
  pruneConvertFromMap,
  providersEqual,
  resolveUpstreamIdForProviderChange,
  stripJsonSuffix,
} from "@/features/config/cards/upstreams/upstream-editor-helpers";
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
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useAntigravityAccounts } from "@/features/antigravity/use-antigravity-accounts";
import type { UpstreamForm, UpstreamStrategy } from "@/features/config/types";
import { m } from "@/paraglide/messages.js";

type UpstreamsCardProps = {
  upstreams: UpstreamForm[];
  appProxyUrl: string;
  strategy: UpstreamStrategy;
  showApiKeys: boolean;
  providerOptions: string[];
  onToggleApiKeys: () => void;
  onStrategyChange: (value: UpstreamStrategy) => void;
  onAdd: (upstream: UpstreamForm) => void;
  onRemove: (index: number) => void;
  onChange: (index: number, patch: Partial<UpstreamForm>) => void;
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
}: UpstreamsCardProps) {
  const mergedProviderOptions = useMemo(
    () => mergeProviderOptions(providerOptions),
    [providerOptions]
  );
  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibility>(() =>
    createDefaultColumnVisibility()
  );
  const [columnsOpen, setColumnsOpen] = useState(false);
  const [editor, setEditor] = useState<UpstreamEditorState>({ open: false });
  const [deleteDialog, setDeleteDialog] = useState<DeleteDialogState>({ open: false });
  const {
    accounts: kiroAccounts,
    loading: kiroAccountsLoading,
    error: kiroAccountsError,
    refresh: refreshKiroAccounts,
  } = useKiroAccounts();
  const {
    accounts: codexAccounts,
    loading: codexAccountsLoading,
    error: codexAccountsError,
    refresh: refreshCodexAccounts,
  } = useCodexAccounts();
  const {
    accounts: antigravityAccounts,
    loading: antigravityAccountsLoading,
    error: antigravityAccountsError,
    refresh: refreshAntigravityAccounts,
  } = useAntigravityAccounts();

  const columns = useMemo(
    () => UPSTREAM_COLUMNS.filter((column) => columnVisibility[column.id]),
    [columnVisibility]
  );
  const apiKeyVisible = columnVisibility.apiKey;
  const kiroAccountMap = useMemo(() => {
    const map = new Map(kiroAccounts.map((account) => [account.account_id, account]));
    return map;
  }, [kiroAccounts]);
  const codexAccountMap = useMemo(() => {
    const map = new Map(codexAccounts.map((account) => [account.account_id, account]));
    return map;
  }, [codexAccounts]);
  const antigravityAccountMap = useMemo(() => {
    const map = new Map(antigravityAccounts.map((account) => [account.account_id, account]));
    return map;
  }, [antigravityAccounts]);

  // 更新 draft，处理 provider 变化时的自动逻辑
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
        const nextPrimary = nextProviders[0] ?? "";

        // 如果 provider 变化，处理账户绑定 + ID 自动逻辑：
        // - 新增：根据 provider 自动生成 ID
        // - 编辑：普通 provider 不自动改 ID（避免统计/引用被拆分），仅 kiro/codex/antigravity 会随账户同步
        if (providersChanged) {
          let kiroAccountId = prev.draft.kiroAccountId;
          let codexAccountId = prev.draft.codexAccountId;
          let antigravityAccountId = prev.draft.antigravityAccountId;
          // openai-response 专属开关：切换到其它 provider 时清零，避免把无效字段写进配置。
          let filterPromptCacheRetention = prev.draft.filterPromptCacheRetention;
          let filterSafetyIdentifier = prev.draft.filterSafetyIdentifier;
          let useChatCompletionsForResponses = prev.draft.useChatCompletionsForResponses;
          let rewriteDeveloperRoleToSystem = prev.draft.rewriteDeveloperRoleToSystem;
          let convertFromMap = patch.convertFromMap ?? prev.draft.convertFromMap;

          if (nextPrimary === "kiro") {
            const idleAccount = findIdleKiroAccount(kiroAccounts, upstreams, editingIndex);
            kiroAccountId = idleAccount?.account_id ?? "";
            codexAccountId = "";
            antigravityAccountId = "";
          } else if (nextPrimary === "codex") {
            const idleAccount = findIdleCodexAccount(codexAccounts, upstreams, editingIndex);
            codexAccountId = idleAccount?.account_id ?? "";
            kiroAccountId = "";
            antigravityAccountId = "";
          } else if (nextPrimary === "antigravity") {
            const idleAccount = findIdleAntigravityAccount(
              antigravityAccounts,
              upstreams,
              editingIndex
            );
            antigravityAccountId = idleAccount?.account_id ?? "";
            kiroAccountId = "";
            codexAccountId = "";
          } else {
            if (currentProviders.includes("kiro")) {
              kiroAccountId = "";
            }
            if (currentProviders.includes("codex")) {
              codexAccountId = "";
            }
            if (currentProviders.includes("antigravity")) {
              antigravityAccountId = "";
            }
          }

          if (!nextProviders.includes("openai-response")) {
            filterPromptCacheRetention = false;
            filterSafetyIdentifier = false;
            useChatCompletionsForResponses = false;
          }
          if (!nextProviders.some((provider) => provider === "openai" || provider === "openai-response")) {
            rewriteDeveloperRoleToSystem = false;
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

          convertFromMap = pruneConvertFromMap(convertFromMap, nextProviders);

          const id = resolveUpstreamIdForProviderChange({
            mode: prev.mode,
            currentId: prev.draft.id,
            currentProviders,
            nextProviders,
            upstreams,
            editingIndex,
            kiroAccountId,
            codexAccountId,
            antigravityAccountId,
          });

          return {
            ...prev,
            draft: {
              ...prev.draft,
              ...patch,
              providers: nextProviders,
              id,
              kiroAccountId,
              codexAccountId,
              antigravityAccountId,
              filterPromptCacheRetention,
              filterSafetyIdentifier,
              useChatCompletionsForResponses,
              rewriteDeveloperRoleToSystem,
              convertFromMap,
            },
          };
        }

        // 如果是 kiro provider 且 kiroAccountId 变化，同步更新 ID（去掉 .json）
        if (
          hasProvider(prev.draft, "kiro") &&
          patch.kiroAccountId !== undefined &&
          patch.kiroAccountId !== prev.draft.kiroAccountId
        ) {
          const newId = patch.kiroAccountId ? stripJsonSuffix(patch.kiroAccountId) : prev.draft.id;
          return {
            ...prev,
            draft: { ...prev.draft, ...patch, id: newId },
          };
        }
        if (
          hasProvider(prev.draft, "codex") &&
          patch.codexAccountId !== undefined &&
          patch.codexAccountId !== prev.draft.codexAccountId
        ) {
          const newId = patch.codexAccountId ? stripJsonSuffix(patch.codexAccountId) : prev.draft.id;
          return {
            ...prev,
            draft: { ...prev.draft, ...patch, id: newId },
          };
        }
        if (
          hasProvider(prev.draft, "antigravity") &&
          patch.antigravityAccountId !== undefined &&
          patch.antigravityAccountId !== prev.draft.antigravityAccountId
        ) {
          const newId = patch.antigravityAccountId
            ? stripJsonSuffix(patch.antigravityAccountId)
            : prev.draft.id;
          return {
            ...prev,
            draft: { ...prev.draft, ...patch, id: newId },
          };
        }

        return { ...prev, draft: { ...prev.draft, ...patch } };
      });
    },
    [upstreams, kiroAccounts, codexAccounts, antigravityAccounts],
  );

  const openCreateDialog = () => {
    const draft = createEmptyUpstream();
    const nextProviders = normalizeProviders(draft.providers);
    const id = createAutoUpstreamId(nextProviders, upstreams);
    setEditor({ open: true, mode: "create", draft: { ...draft, id } });
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
    if (!upstream) {
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
    onRemove(deleteDialog.index);
    setDeleteDialog({ open: false });
  };

  return (
    <Card data-slot="upstreams-card">
      <CardHeader>
        <CardTitle className="inline-flex items-center gap-1">
          {m.upstreams_title()}
          <Tooltip>
            <TooltipTrigger asChild>
              <HelpCircle className="size-4 text-muted-foreground cursor-help" />
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-xs">
              {m.upstreams_desc()}
            </TooltipContent>
          </Tooltip>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <UpstreamsToolbar
          apiKeyVisible={apiKeyVisible}
          showApiKeys={showApiKeys}
          onToggleApiKeys={onToggleApiKeys}
          onAddClick={openCreateDialog}
          onColumnsClick={() => setColumnsOpen(true)}
          strategy={strategy}
          onStrategyChange={onStrategyChange}
        />
        {upstreams.length ? (
          <UpstreamsTable
            upstreams={upstreams}
            columns={columns}
            showApiKeys={showApiKeys}
            kiroAccounts={kiroAccountMap}
            codexAccounts={codexAccountMap}
            antigravityAccounts={antigravityAccountMap}
            disableDelete={false}
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
        kiroAccounts={kiroAccounts}
        kiroAccountsLoading={kiroAccountsLoading}
        kiroAccountsError={kiroAccountsError}
        onRefreshKiroAccounts={refreshKiroAccounts}
        codexAccounts={codexAccounts}
        codexAccountsLoading={codexAccountsLoading}
        codexAccountsError={codexAccountsError}
        onRefreshCodexAccounts={refreshCodexAccounts}
        antigravityAccounts={antigravityAccounts}
        antigravityAccountsLoading={antigravityAccountsLoading}
        antigravityAccountsError={antigravityAccountsError}
        onRefreshAntigravityAccounts={refreshAntigravityAccounts}
      />
      <DeleteUpstreamDialog
        dialog={deleteDialog}
        onOpenChange={(open) => !open && setDeleteDialog({ open: false })}
        onConfirm={confirmDelete}
      />
    </Card>
  );
}
