import { getLocale } from "@/paraglide/runtime";

const DATE_FORMAT_OPTIONS: Intl.DateTimeFormatOptions = {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
};

function parseDateValue(value: string): Date | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }

  const numeric = Number(trimmed);
  if (!Number.isNaN(numeric)) {
    // Treat small numeric timestamps as seconds (e.g. 1769904000.0).
    const ms = numeric < 100_000_000_000 ? numeric * 1000 : numeric;
    const date = new Date(ms);
    return Number.isNaN(date.getTime()) ? null : date;
  }

  const date = new Date(trimmed);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function formatDateLabel(value: string | null) {
  if (!value) {
    return "";
  }
  const date = parseDateValue(value);
  if (!date) {
    return value;
  }
  const locale = getLocale();
  return new Intl.DateTimeFormat(locale, DATE_FORMAT_OPTIONS).format(date);
}
