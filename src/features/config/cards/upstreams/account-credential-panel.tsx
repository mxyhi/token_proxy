/**
 * 账户型 Upstream 编辑器：展示只读 credential 身份、额度与 token 刷新操作。
 * account_id 缺失或 summary 找不到时提示需 reconcile，不静默创建。
 */
import { useCallback, useEffect, useMemo, useState } from "react";

import { RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { formatDateLabel } from "@/features/config/cards/upstreams/account-date";
import { isAccountProviderKind } from "@/features/config/cards/upstreams/upstream-editor-helpers";
import type { AccountProviderKind, UpstreamForm } from "@/features/config/types";
import { useCodexAccounts } from "@/features/codex/use-codex-accounts";
import { useCodexQuotas } from "@/features/codex/use-codex-quotas";
import { useKiroAccounts } from "@/features/kiro/use-kiro-accounts";
import { useKiroQuotas } from "@/features/kiro/use-kiro-quotas";
import { useXaiAccounts } from "@/features/xai/use-xai-accounts";
import { useXaiQuotas } from "@/features/xai/use-xai-quotas";
import { parseError } from "@/lib/error";
import { m } from "@/paraglide/messages.js";

type AccountCredentialPanelProps = {
  draft: UpstreamForm;
};

type AccountSummaryView = {
  accountId: string;
  email: string;
  status: string;
  expiresAt: string;
  autoRefreshEnabled: boolean | null;
};

type QuotaItemView = {
  name: string;
  percentage: number;
  used: number | null;
  limit: number | null;
  resetAt: string | null;
};

function resolveAccountProvider(providers: readonly string[]): AccountProviderKind | null {
  const normalized = providers.map((value) => value.trim()).filter(Boolean);
  if (normalized.length !== 1) {
    return null;
  }
  const provider = normalized[0];
  return provider && isAccountProviderKind(provider) ? provider : null;
}

function formatStatusLabel(status: string) {
  if (status === "active") {
    return m.kiro_account_status_active();
  }
  if (status === "disabled") {
    return m.common_disabled();
  }
  if (status === "expired") {
    return m.kiro_account_status_expired();
  }
  if (status === "invalid") {
    return m.codex_account_status_invalid();
  }
  if (status === "cooling_down") {
    return m.providers_account_status_cooling_down();
  }
  return status;
}

function QuotaList({
  items,
  loading,
  error,
}: {
  items: QuotaItemView[];
  loading: boolean;
  error: string;
}) {
  if (loading && !items.length) {
    return <p className="text-xs text-muted-foreground">{m.providers_quota_loading()}</p>;
  }
  if (error) {
    return <p className="text-xs text-destructive">{error}</p>;
  }
  if (!items.length) {
    return <p className="text-xs text-muted-foreground">{m.providers_quota_empty()}</p>;
  }
  return (
    <ul className="space-y-1.5">
      {items.map((item) => {
        const resetLabel = item.resetAt ? formatDateLabel(item.resetAt) : "";
        return (
          <li
            key={item.name}
            className="flex flex-wrap items-baseline justify-between gap-2 text-xs"
          >
            <span className="font-medium text-foreground">{item.name}</span>
            <span className="font-mono text-muted-foreground">
              {item.used !== null && item.limit !== null
                ? m.providers_quota_usage({
                    used: String(item.used),
                    limit: String(item.limit),
                  })
                : `${item.percentage}%`}
              {resetLabel ? ` · ${m.providers_quota_resets({ date: resetLabel })}` : ""}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

export function AccountCredentialPanel({ draft }: AccountCredentialPanelProps) {
  const provider = resolveAccountProvider(draft.providers);
  const accountId = draft.accountId.trim();
  const isKiro = provider === "kiro";
  const isCodex = provider === "codex";
  const isXai = provider === "xai";

  const kiroAccounts = useKiroAccounts({ autoLoad: isKiro });
  const codexAccounts = useCodexAccounts({ autoLoad: isCodex });
  const xaiAccounts = useXaiAccounts({ autoLoad: isXai });
  // 配额按当前 provider 按需加载，避免编辑一个账户时打三家 quota API。
  const kiroQuotas = useKiroQuotas({ autoLoad: isKiro && !!accountId });
  const codexQuotas = useCodexQuotas({ autoLoad: isCodex && !!accountId });
  const xaiQuotas = useXaiQuotas();

  const [quotaBusy, setQuotaBusy] = useState(false);
  const [tokenBusy, setTokenBusy] = useState(false);

  // xAI quota hook 无 autoLoad；账户型编辑打开时事件式拉取。
  useEffect(() => {
    if (!isXai || !accountId) {
      return;
    }
    void xaiQuotas.refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- refresh on account identity only
  }, [accountId, isXai]);

  const summary = useMemo((): AccountSummaryView | null => {
    if (!provider || !accountId) {
      return null;
    }
    if (isKiro) {
      const account = kiroAccounts.accounts.find((item) => item.account_id === accountId);
      if (!account) {
        return null;
      }
      return {
        accountId: account.account_id,
        email: account.email?.trim() || "",
        status: account.status,
        expiresAt: account.expires_at ?? "",
        autoRefreshEnabled: null,
      };
    }
    if (isCodex) {
      const account = codexAccounts.accounts.find((item) => item.account_id === accountId);
      if (!account) {
        return null;
      }
      return {
        accountId: account.account_id,
        email: account.email?.trim() || "",
        status: account.status,
        expiresAt: account.expires_at ?? "",
        autoRefreshEnabled: account.auto_refresh_enabled ?? null,
      };
    }
    if (isXai) {
      const account = xaiAccounts.accounts.find((item) => item.account_id === accountId);
      if (!account) {
        return null;
      }
      return {
        accountId: account.account_id,
        email: account.email?.trim() || "",
        status: account.status,
        expiresAt: account.expires_at ?? "",
        autoRefreshEnabled: account.auto_refresh_enabled,
      };
    }
    return null;
  }, [
    accountId,
    codexAccounts.accounts,
    isCodex,
    isKiro,
    isXai,
    kiroAccounts.accounts,
    provider,
    xaiAccounts.accounts,
  ]);

  const quotaItems = useMemo((): QuotaItemView[] => {
    if (!accountId) {
      return [];
    }
    if (isKiro) {
      const match = kiroQuotas.quotas.find((item) => item.account_id === accountId);
      return (match?.quotas ?? []).map((item) => ({
        name: item.name,
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        resetAt: item.reset_at,
      }));
    }
    if (isCodex) {
      const match = codexQuotas.quotas.find((item) => item.account_id === accountId);
      return (match?.quotas ?? []).map((item) => ({
        name: item.name,
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        resetAt: item.reset_at,
      }));
    }
    if (isXai) {
      const match = xaiQuotas.quotas.find((item) => item.account_id === accountId);
      return (match?.quotas ?? []).map((item) => ({
        name: item.name,
        percentage: item.percentage,
        used: item.used,
        limit: item.limit,
        resetAt: item.reset_at,
      }));
    }
    return [];
  }, [
    accountId,
    codexQuotas.quotas,
    isCodex,
    isKiro,
    isXai,
    kiroQuotas.quotas,
    xaiQuotas.quotas,
  ]);

  const accountsLoading =
    (isKiro && kiroAccounts.loading) ||
    (isCodex && codexAccounts.loading) ||
    (isXai && xaiAccounts.loading);
  const quotaLoading =
    (isKiro && kiroQuotas.loading) ||
    (isCodex && codexQuotas.loading) ||
    (isXai && xaiQuotas.loading);
  const quotaError =
    (isKiro && kiroQuotas.error) ||
    (isCodex && codexQuotas.error) ||
    (isXai && xaiQuotas.error) ||
    "";

  const refreshQuota = useCallback(async () => {
    if (!provider || !accountId) {
      return;
    }
    setQuotaBusy(true);
    try {
      console.debug("[upstream-account] refresh quota", { provider, accountId });
      if (isKiro) {
        await kiroAccounts.refreshQuotaNow(accountId);
        await kiroQuotas.refresh();
      } else if (isCodex) {
        await codexAccounts.refreshQuotaNow(accountId);
        await codexQuotas.refresh();
      } else if (isXai) {
        await xaiAccounts.refreshQuotaNow(accountId);
        await xaiQuotas.refresh();
      }
      toast.success(m.upstreams_account_quota_refreshed());
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setQuotaBusy(false);
    }
  }, [
    accountId,
    codexAccounts,
    codexQuotas,
    isCodex,
    isKiro,
    isXai,
    kiroAccounts,
    kiroQuotas,
    provider,
    xaiAccounts,
    xaiQuotas,
  ]);

  const refreshToken = useCallback(async () => {
    if (!accountId || (!isCodex && !isXai)) {
      return;
    }
    setTokenBusy(true);
    try {
      console.debug("[upstream-account] refresh token", { provider, accountId });
      if (isCodex) {
        await codexAccounts.refreshAccount(accountId);
      } else {
        await xaiAccounts.refreshAccount(accountId);
      }
      toast.success(m.upstreams_account_token_refreshed());
    } catch (error) {
      toast.error(parseError(error));
    } finally {
      setTokenBusy(false);
    }
  }, [accountId, codexAccounts, isCodex, isXai, provider, xaiAccounts]);

  const toggleAutoRefresh = useCallback(
    async (enabledNext: boolean) => {
      if (!accountId || (!isCodex && !isXai)) {
        return;
      }
      setTokenBusy(true);
      try {
        console.debug("[upstream-account] set auto refresh", {
          provider,
          accountId,
          enabled: enabledNext,
        });
        if (isCodex) {
          await codexAccounts.setAutoRefresh(accountId, enabledNext);
        } else {
          await xaiAccounts.setAutoRefresh(accountId, enabledNext);
        }
      } catch (error) {
        toast.error(parseError(error));
      } finally {
        setTokenBusy(false);
      }
    },
    [accountId, codexAccounts, isCodex, isXai, provider, xaiAccounts],
  );

  if (!provider) {
    return null;
  }

  if (!accountId) {
    return (
      <div
        data-slot="upstream-account-missing"
        className="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive"
      >
        {m.upstreams_account_missing()}
      </div>
    );
  }

  if (accountsLoading && !summary) {
    return (
      <p data-slot="upstream-account-loading" className="text-xs text-muted-foreground">
        {m.providers_accounts_loading()}
      </p>
    );
  }

  if (!summary) {
    return (
      <div
        data-slot="upstream-account-reconcile"
        className="rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-amber-700 dark:text-amber-300"
      >
        {m.upstreams_account_needs_reconcile({ accountId })}
      </div>
    );
  }

  const expiresLabel = summary.expiresAt ? formatDateLabel(summary.expiresAt) : "";

  return (
    <div
      data-slot="upstream-account-credential"
      className="space-y-3 rounded-md border border-border/60 bg-muted/20 p-3"
    >
      <div className="space-y-1">
        <p className="text-xs font-medium text-foreground">{m.upstreams_account_section()}</p>
        <p className="text-xs text-muted-foreground">{m.upstreams_account_section_desc()}</p>
      </div>

      <dl className="grid grid-cols-[minmax(5rem,auto)_1fr] gap-x-3 gap-y-1.5 text-xs">
        <dt className="text-muted-foreground">{m.providers_table_account_id()}</dt>
        <dd className="font-mono text-foreground">{summary.accountId}</dd>
        {summary.email ? (
          <>
            <dt className="text-muted-foreground">{m.providers_table_account()}</dt>
            <dd className="text-foreground">{summary.email}</dd>
          </>
        ) : null}
        <dt className="text-muted-foreground">{m.providers_table_status()}</dt>
        <dd className="text-foreground">{formatStatusLabel(summary.status)}</dd>
        {expiresLabel ? (
          <>
            <dt className="text-muted-foreground">{m.providers_table_expires()}</dt>
            <dd className="text-foreground">{expiresLabel}</dd>
          </>
        ) : null}
      </dl>

      <div className="space-y-2 border-t border-border/50 pt-2">
        <div className="flex items-center justify-between gap-2">
          <Label className="text-xs font-medium">{m.providers_table_quota()}</Label>
          <Button
            type="button"
            size="icon-sm"
            variant="outline"
            disabled={quotaBusy || quotaLoading}
            onClick={() => {
              void refreshQuota();
            }}
            aria-label={m.providers_account_refresh_quota()}
          >
            <RefreshCw
              className={["size-3.5", quotaBusy || quotaLoading ? "animate-spin" : ""]
                .filter(Boolean)
                .join(" ")}
              aria-hidden="true"
            />
          </Button>
        </div>
        <QuotaList items={quotaItems} loading={quotaLoading} error={quotaError} />
      </div>

      {isCodex || isXai ? (
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-border/50 pt-2">
          <Button
            type="button"
            size="sm"
            variant="secondary"
            disabled={tokenBusy}
            onClick={() => {
              void refreshToken();
            }}
          >
            {m.providers_account_refresh_token()}
          </Button>
          {summary.autoRefreshEnabled !== null ? (
            <Label className="flex items-center gap-2 text-xs font-normal">
              <span>{m.providers_account_auto_refresh()}</span>
              <Switch
                checked={summary.autoRefreshEnabled}
                disabled={tokenBusy}
                onCheckedChange={(checked) => {
                  void toggleAutoRefresh(checked);
                }}
                aria-label={m.providers_account_auto_refresh()}
              />
            </Label>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
