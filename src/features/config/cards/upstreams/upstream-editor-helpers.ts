import { createNativeInboundFormatSet, removeInboundFormatsInSet } from "@/features/config/inbound-formats";
import type { AccountProviderKind, UpstreamForm } from "@/features/config/types";

export const ACCOUNT_BACKED_PROVIDERS = ["kiro", "codex", "xai", "antigravity"] as const;

/** 可绑定 OAuth/账户 credential 的 provider（不含 antigravity 等仅 endpoint 托管型）。 */
export const ACCOUNT_PROVIDER_KINDS = ["kiro", "codex", "xai"] as const satisfies readonly AccountProviderKind[];

export function isAccountBackedProvider(provider: string) {
  return ACCOUNT_BACKED_PROVIDERS.some((value) => value === provider);
}

export function isAccountProviderKind(provider: string): provider is AccountProviderKind {
  return ACCOUNT_PROVIDER_KINDS.some((value) => value === provider);
}

export function isAccountBackedProviderSet(providers: readonly string[]) {
  return providers.length === 1 && providers.some(isAccountBackedProvider);
}

/**
 * 账户型 Upstream：绑定固定 credential，禁止复制同 credential。
 * 删除已启用（删除 Upstream 级联删账户凭据）。
 */
export function isAccountCredentialUpstream(upstream: UpstreamForm) {
  const providers = normalizeProviders(upstream.providers);
  const provider = providers[0];
  return providers.length === 1 && provider !== undefined && isAccountProviderKind(provider);
}

/** @deprecated 使用 isAccountCredentialUpstream；保留别名避免遗漏引用。 */
export function isManagedAccountBackedUpstream(upstream: UpstreamForm) {
  return isAccountCredentialUpstream(upstream);
}

/** 账户型 credential identity 只读：已绑定 account_id 时不可改 provider/account。 */
export function isAccountIdentityLocked(upstream: UpstreamForm) {
  return isAccountCredentialUpstream(upstream) && !!upstream.accountId.trim();
}

export function createCopiedUpstreamId(sourceId: string, upstreams: readonly UpstreamForm[]) {
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
export function createAutoUpstreamId(
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

export function normalizeProviders(values: readonly string[]) {
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

export function providersEqual(left: readonly string[], right: readonly string[]) {
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

export function coerceProviderSelection(next: readonly string[]) {
  const normalized = normalizeProviders(next);
  const special = normalized.find(isAccountBackedProvider);
  if (!special) {
    return normalized;
  }
  return [special];
}

export function hasProvider(upstream: UpstreamForm, provider: string) {
  return upstream.providers.some((value) => value.trim() === provider);
}

export function pruneConvertFromMap(
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
 * 编辑时 ID 的期望：
 * - 编辑：切换 provider 不自动改 ID（避免统计/引用被“拆分”）
 * - 新增：切换 provider 时自动生成 provider 前缀 ID
 */
export function resolveUpstreamIdForProviderChange(args: {
  mode: "create" | "edit";
  currentId: string;
  currentProviders: readonly string[];
  nextProviders: readonly string[];
  upstreams: readonly UpstreamForm[];
  editingIndex?: number;
}) {
  const currentPrimary = args.currentProviders[0]?.trim() ?? "";
  const nextPrimary = args.nextProviders[0]?.trim() ?? "";

  // 仅“新增”才允许根据 provider 自动改 ID；编辑中保持稳定，交给用户手动调整。
  if (args.mode === "edit") {
    return args.currentId;
  }

  const shouldAutoId = nextPrimary !== currentPrimary && !!nextPrimary;
  if (!shouldAutoId) {
    return args.currentId;
  }
  return createAutoUpstreamId(args.nextProviders, args.upstreams, args.editingIndex);
}

export function cloneUpstreamDraft(upstream: UpstreamForm) {
  const providers = normalizeProviders(upstream.providers);
  return {
    ...upstream,
    // provider 必选：编辑/复制时也保证至少有一个 provider，避免 UI 出现“看起来有默认值但实际为空”的不同步体验
    providers: providers.length ? providers : ["openai"],
    availableModels: [...upstream.availableModels],
    modelMappings: upstream.modelMappings.map((mapping) => ({ ...mapping })),
    overrides: {
      header: upstream.overrides.header.map((entry) => ({ ...entry })),
    },
  };
}
