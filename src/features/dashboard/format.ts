const DASHBOARD_TIME_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  dateStyle: "short",
  timeStyle: "medium",
};

const DASHBOARD_TIME_MINUTE_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  dateStyle: "short",
  timeStyle: "short",
};

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
