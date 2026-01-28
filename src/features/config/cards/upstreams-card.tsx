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
import { createNativeInboundFormatSet, removeInboundFormatsInSet } from "@/features/config/inbound-formats";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import type { CodexAccountSummary } from "@/features/codex/types";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import type { KiroAccountSummary } from "@/features/kiro/types";
import { useAntigravityAccounts } from "@/features/antigravity/use-antigravity-accounts";
import type { AntigravityAccountSummary } from "@/features/antigravity/types";
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
 * 基于 providers 自动生成唯一 ID
 * - 单 provider：openai-1, openai-2
 * - 多 provider：仍以第一个 provider 作为前缀（避免 id 频繁变化）
 */
function createAutoUpstreamId(
  providers: readonly string[],
  upstreams: readonly UpstreamForm[],
  editingIndex?: number,
) {
  const base = providers[0]?.trim() || "upstream";
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

function normalizeProviders(values: readonly string[]) {
  const output: string[] = [];
  const seen = new Set<string>();
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) {
      continue;
    }
    if (seen.has(trimmed)) {
      continue;
    }
    seen.add(trimmed);
    output.push(trimmed);
  }
  return output;
}

function providersEqual(left: readonly string[], right: readonly string[]) {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    if (left[index] !== right[index]) {
      return false;
    }
  }
  return true;
}

function coerceProviderSelection(next: readonly string[]) {
  const normalized = normalizeProviders(next);
  const special = normalized.find((provider) =>
    provider === "kiro" || provider === "codex" || provider === "antigravity",
  );
  if (!special) {
    return normalized;
  }
  return [special];
}

function hasProvider(upstream: UpstreamForm, provider: string) {
  return upstream.providers.some((value) => value.trim() === provider);
}

function pruneConvertFromMap(
  map: UpstreamForm["convertFromMap"],
  providers: readonly string[],
) {
  if (!Object.keys(map).length) {
    return map;
  }
  const providerSet = new Set(providers);
  const nativeFormatsInUpstream = createNativeInboundFormatSet(providers);
  const output: UpstreamForm["convertFromMap"] = {};
  for (const [provider, formats] of Object.entries(map)) {
    if (!providerSet.has(provider)) {
      continue;
    }
    const filtered = removeInboundFormatsInSet(formats, nativeFormatsInUpstream);
    if (!filtered.length) {
      continue;
    }
    output[provider] = filtered;
  }
  return output;
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
        return hasProvider(upstream, "kiro") && upstream.kiroAccountId.trim();
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

/**
 * 找到第一个未被其他上游使用的空闲 codex 账户
 * 优先返回 active 状态的账户
 */
function findIdleCodexAccount(
  accounts: CodexAccountSummary[],
  upstreams: readonly UpstreamForm[],
  editingIndex?: number,
): CodexAccountSummary | undefined {
  const usedAccountIds = new Set(
    upstreams
      .filter((upstream, index) => {
        if (index === editingIndex) return false;
        return hasProvider(upstream, "codex") && upstream.codexAccountId.trim();
      })
      .map((upstream) => upstream.codexAccountId.trim()),
  );

  const activeIdle = accounts.find(
    (account) => account.status === "active" && !usedAccountIds.has(account.account_id),
  );
  if (activeIdle) return activeIdle;

  return accounts.find((account) => !usedAccountIds.has(account.account_id));
}

/**
 * 找到第一个未被其他上游使用的空闲 antigravity 账户
 * 优先返回 active 状态的账户
 */
function findIdleAntigravityAccount(
  accounts: AntigravityAccountSummary[],
  upstreams: readonly UpstreamForm[],
  editingIndex?: number,
): AntigravityAccountSummary | undefined {
  const usedAccountIds = new Set(
    upstreams
      .filter((upstream, index) => {
        if (index === editingIndex) return false;
        return (
          hasProvider(upstream, "antigravity") &&
          upstream.antigravityAccountId.trim()
        );
      })
      .map((upstream) => upstream.antigravityAccountId.trim()),
  );

  const activeIdle = accounts.find(
    (account) => account.status === "active" && !usedAccountIds.has(account.account_id),
  );
  if (activeIdle) return activeIdle;

  return accounts.find((account) => !usedAccountIds.has(account.account_id));
}

function cloneUpstreamDraft(upstream: UpstreamForm): UpstreamForm {
  const providers = normalizeProviders(upstream.providers);
  return {
    ...upstream,
    // provider 必选：编辑/复制时也保证至少有一个 provider，避免 UI 出现“看起来有默认值但实际为空”的不同步体验
    providers: providers.length ? providers : ["openai"],
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
        const currentPrimary = currentProviders[0] ?? "";
        const nextPrimary = nextProviders[0] ?? "";

        // 如果 provider 变化，自动生成新 ID 并处理账户绑定
        if (providersChanged) {
          let kiroAccountId = prev.draft.kiroAccountId;
          let codexAccountId = prev.draft.codexAccountId;
          let antigravityAccountId = prev.draft.antigravityAccountId;
          // openai-response 专属开关：切换到其它 provider 时清零，避免把无效字段写进配置。
          let filterPromptCacheRetention = prev.draft.filterPromptCacheRetention;
          let filterSafetyIdentifier = prev.draft.filterSafetyIdentifier;
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
          }
          if (patch.filterPromptCacheRetention !== undefined) {
            filterPromptCacheRetention = patch.filterPromptCacheRetention;
          }
          if (patch.filterSafetyIdentifier !== undefined) {
            filterSafetyIdentifier = patch.filterSafetyIdentifier;
          }

          convertFromMap = pruneConvertFromMap(convertFromMap, nextProviders);

          const shouldAutoId = nextPrimary !== currentPrimary && !!nextPrimary;
          const autoId = shouldAutoId
            ? createAutoUpstreamId(nextProviders, upstreams, editingIndex)
            : prev.draft.id;
          const id =
            nextPrimary === "kiro" && kiroAccountId
              ? stripJsonSuffix(kiroAccountId)
              : nextPrimary === "codex" && codexAccountId
                ? stripJsonSuffix(codexAccountId)
                : nextPrimary === "antigravity" && antigravityAccountId
                  ? stripJsonSuffix(antigravityAccountId)
                  : autoId;

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
