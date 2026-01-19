import { useCallback, useMemo, useState } from "react";

import { HelpCircle } from "lucide-react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import type { KiroAccountSummary } from "@/features/kiro/types";
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

function createCopiedUpstreamId(sourceId: string, upstreams: readonly UpstreamForm[]) {
  const base = sourceId.trim() || "upstream";
  const taken = new Set(
    upstreams
      .map((upstream) => upstream.id.trim())
      .filter((id) => id),
  );

  const prefix = `${base}-copy`;
  if (!taken.has(prefix)) {
    return prefix;
  }

  let suffix = 2;
  while (taken.has(`${prefix}-${suffix}`)) {
    suffix += 1;
  }
  return `${prefix}-${suffix}`;
}

/**
 * 基于 provider 自动生成唯一 ID
 * 例如：openai-1, openai-2, kiro-1 等
 */
function createAutoUpstreamId(
  provider: string,
  upstreams: readonly UpstreamForm[],
  editingIndex?: number,
) {
  const base = provider.trim() || "upstream";
  const taken = new Set(
    upstreams
      .filter((_, index) => index !== editingIndex)
      .map((upstream) => upstream.id.trim())
      .filter((id) => id),
  );

  // 先尝试 provider-1
  let suffix = 1;
  while (taken.has(`${base}-${suffix}`)) {
    suffix += 1;
  }
  return `${base}-${suffix}`;
}

/**
 * 去除 account_id 的 .json 后缀，用于生成更简洁的 upstream ID
 */
function stripJsonSuffix(accountId: string): string {
  return accountId.endsWith(".json") ? accountId.slice(0, -5) : accountId;
}

/**
 * 找到第一个未被其他上游使用的空闲 kiro 账户
 * 优先返回 active 状态的账户
 */
function findIdleKiroAccount(
  accounts: KiroAccountSummary[],
  upstreams: readonly UpstreamForm[],
  editingIndex?: number,
): KiroAccountSummary | undefined {
  // 收集已被使用的 kiro account id
  const usedAccountIds = new Set(
    upstreams
      .filter((upstream, index) => {
        if (index === editingIndex) return false;
        return upstream.provider.trim() === "kiro" && upstream.kiroAccountId.trim();
      })
      .map((upstream) => upstream.kiroAccountId.trim()),
  );

  // 先找 active 状态的空闲账户
  const activeIdle = accounts.find(
    (account) => account.status === "active" && !usedAccountIds.has(account.account_id),
  );
  if (activeIdle) return activeIdle;

  // 如果没有 active 的，找任意空闲账户
  return accounts.find((account) => !usedAccountIds.has(account.account_id));
}

function cloneUpstreamDraft(upstream: UpstreamForm): UpstreamForm {
  return {
    ...upstream,
    modelMappings: upstream.modelMappings.map((mapping) => ({ ...mapping })),
    overrides: {
      header: upstream.overrides.header.map((entry) => ({ ...entry })),
    },
  };
}

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

  const columns = useMemo(
    () => UPSTREAM_COLUMNS.filter((column) => columnVisibility[column.id]),
    [columnVisibility]
  );
  const apiKeyVisible = columnVisibility.apiKey;
  const kiroAccountMap = useMemo(() => {
    const map = new Map(kiroAccounts.map((account) => [account.account_id, account]));
    return map;
  }, [kiroAccounts]);

  // 更新 draft，处理 provider 变化时的自动逻辑
  const updateDraft = useCallback(
    (patch: Partial<UpstreamForm>) => {
      setEditor((prev) => {
        if (!prev.open) return prev;

        const editingIndex = prev.mode === "edit" ? prev.index : undefined;
        const currentProvider = prev.draft.provider.trim();
        const newProvider = patch.provider?.trim();

        // 如果 provider 变化，自动生成新 ID 并处理 kiro 账户
        if (newProvider !== undefined && newProvider !== currentProvider) {
          let kiroAccountId = prev.draft.kiroAccountId;
          let autoId: string;

          // 如果切换到 kiro，自动选择空闲账户，并用账户 ID（去掉 .json）作为 upstream ID
          if (newProvider === "kiro") {
            const idleAccount = findIdleKiroAccount(kiroAccounts, upstreams, editingIndex);
            kiroAccountId = idleAccount?.account_id ?? "";
            autoId = kiroAccountId ? stripJsonSuffix(kiroAccountId) : createAutoUpstreamId(newProvider, upstreams, editingIndex);
          } else {
            // 其他 provider 用 provider-n 格式
            autoId = createAutoUpstreamId(newProvider, upstreams, editingIndex);
            if (currentProvider === "kiro") {
              kiroAccountId = "";
            }
          }

          return {
            ...prev,
            draft: { ...prev.draft, ...patch, id: autoId, kiroAccountId },
          };
        }

        // 如果是 kiro provider 且 kiroAccountId 变化，同步更新 ID（去掉 .json）
        if (
          prev.draft.provider.trim() === "kiro" &&
          patch.kiroAccountId !== undefined &&
          patch.kiroAccountId !== prev.draft.kiroAccountId
        ) {
          const newId = patch.kiroAccountId ? stripJsonSuffix(patch.kiroAccountId) : prev.draft.id;
          return {
            ...prev,
            draft: { ...prev.draft, ...patch, id: newId },
          };
        }

        return { ...prev, draft: { ...prev.draft, ...patch } };
      });
    },
    [upstreams, kiroAccounts],
  );

  const openCreateDialog = () =>
    setEditor({ open: true, mode: "create", draft: createEmptyUpstream() });

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
      />
      <DeleteUpstreamDialog
        dialog={deleteDialog}
        onOpenChange={(open) => !open && setDeleteDialog({ open: false })}
        onConfirm={confirmDelete}
      />
    </Card>
  );
}
