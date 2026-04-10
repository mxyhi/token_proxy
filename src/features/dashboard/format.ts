import { m } from "@/paraglide/messages.js";

const DASHBOARD_TIME_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  dateStyle: "short",
  timeStyle: "medium",
};

const DASHBOARD_TIME_MINUTE_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  dateStyle: "short",
  timeStyle: "short",
};

function normalizeProviderPart(value: string | null | undefined) {
  return value?.trim().toLowerCase().replace(/[^a-z0-9]+/g, " ").trim() ?? "";
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function isLocalProxyRequest(
  upstreamId: string,
  provider: string,
  accountId: string | null | undefined,
) {
  if (accountId?.trim()) {
    return false;
  }
  return (
    normalizeProviderPart(upstreamId) === "local" &&
    normalizeProviderPart(provider) === "proxy"
  );
}

function containsProviderPart(value: string | null | undefined, provider: string) {
  const normalizedValue = normalizeProviderPart(value);
  const normalizedProvider = normalizeProviderPart(provider);
  if (!normalizedValue || !normalizedProvider) {
    return false;
  }
  return new RegExp(`(^| )${escapeRegExp(normalizedProvider)}( |$)`).test(normalizedValue);
}

export function createDashboardTimeFormatter(locale: string) {
  return new Intl.DateTimeFormat(locale, DASHBOARD_TIME_FORMAT_OPTIONS);
}

export function createDashboardMinuteFormatter(locale: string) {
  return new Intl.DateTimeFormat(locale, DASHBOARD_TIME_MINUTE_FORMAT_OPTIONS);
}

export function formatDashboardTimestamp(tsMs: number, formatter: Intl.DateTimeFormat) {
  const date = new Date(tsMs);
  return Number.isNaN(date.getTime()) ? "—" : formatter.format(date);
}

export function formatDashboardProviderLabel(
  upstreamId: string,
  provider: string,
  accountId: string | null | undefined,
) {
  if (isLocalProxyRequest(upstreamId, provider, accountId)) {
    return m.dashboard_provider_local_proxy();
  }

  const trimmedProvider = provider.trim();
  const shouldHideProvider =
    trimmedProvider.length > 0 &&
    (containsProviderPart(upstreamId, trimmedProvider) ||
      containsProviderPart(accountId, trimmedProvider));

  return [upstreamId.trim(), shouldHideProvider ? null : trimmedProvider, accountId?.trim()]
    .filter(Boolean)
    .join(" · ");
}

// 使用逗号作为千位分隔符，便于阅读
export function formatInteger(value: number) {
  return Math.round(value).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

// 紧凑格式，用于空间有限的场景（如 985856 → 986K, 1500000 → 1.5M）
const COMPACT_FORMAT = new Intl.NumberFormat("en-US", {
  notation: "compact",
  maximumFractionDigits: 1,
});

export function formatCompact(value: number) {
  return COMPACT_FORMAT.format(value);
}
