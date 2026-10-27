import { trashDaysLeft } from "@atomik-kb/core-client";

import { activeLocale, t, tp } from "./i18n";

export interface FormattedTimestamp {
  /** ISO 8601 text for the `dateTime` attribute of a `<time>` element. */
  iso: string;
  /** Localised text for people. */
  text: string;
}

/**
 * Formats a Unix-millisecond timestamp from the core. The type admits `bigint` because
 * ts-rs types Rust `i64` that way, although JSON delivers a number. Returns `null`
 * when the value is not a date.
 */
export function formatTimestamp(unixMs: number | bigint): FormattedTimestamp | null {
  const date = new Date(Number(unixMs));
  if (Number.isNaN(date.getTime())) {
    return null;
  }
  return {
    iso: date.toISOString(),
    text: new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium", timeStyle: "short" }).format(date),
  };
}

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;

/**
 * How long ago a Unix-millisecond timestamp was, for people: "just now" (under a minute,
 * or a time slightly ahead of this clock), "2 min ago", "3 h ago" (under a day), "yesterday"
 * (the calendar day before `now`, local time), else the date. `null` when the value is not
 * a date.
 */
export function formatRelativeTime(unixMs: number | bigint, now: number = Date.now()): string | null {
  const date = new Date(Number(unixMs));
  if (Number.isNaN(date.getTime())) {
    return null;
  }
  const elapsed = now - date.getTime();
  if (elapsed < MINUTE_MS) {
    return t("time.justNow");
  }
  if (elapsed < HOUR_MS) {
    return t("time.minutesAgo", { count: Math.floor(elapsed / MINUTE_MS) });
  }
  if (elapsed < 24 * HOUR_MS) {
    return t("time.hoursAgo", { count: Math.floor(elapsed / HOUR_MS) });
  }
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (
    date.getFullYear() === yesterday.getFullYear() &&
    date.getMonth() === yesterday.getMonth() &&
    date.getDate() === yesterday.getDate()
  ) {
    return t("time.yesterday");
  }
  return new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium" }).format(date);
}

const RELATIVE_UNITS: readonly { unit: Intl.RelativeTimeFormatUnit; ms: number }[] = [
  { unit: "year", ms: 365 * 24 * HOUR_MS },
  { unit: "month", ms: 30 * 24 * HOUR_MS },
  { unit: "week", ms: 7 * 24 * HOUR_MS },
  { unit: "day", ms: 24 * HOUR_MS },
  { unit: "hour", ms: HOUR_MS },
  { unit: "minute", ms: MINUTE_MS },
];

/**
 * How long ago a Unix-millisecond timestamp was, in words of the locale ("5 minutes ago",
 * "yesterday", "3 weeks ago"), through `Intl.RelativeTimeFormat`; "now" under a minute or for a
 * time slightly ahead of this clock. `null` when the value is not a date.
 */
export function formatAgo(unixMs: number | bigint, now: number = Date.now()): string | null {
  const then = Number(unixMs);
  if (Number.isNaN(new Date(then).getTime())) {
    return null;
  }
  const formatter = new Intl.RelativeTimeFormat(activeLocale(), { numeric: "auto" });
  const elapsed = now - then;
  for (const { unit, ms } of RELATIVE_UNITS) {
    if (elapsed >= ms) {
      return formatter.format(-Math.floor(elapsed / ms), unit);
    }
  }
  return formatter.format(0, "second");
}

const BYTE_UNITS =["byte", "kilobyte", "megabyte", "gigabyte", "terabyte"] as const;

/** A file size for people, in the largest unit that keeps the number at 1 or more ("12.4 kB"). */
export function formatBytes(bytes: number | bigint): string {
  let value = Math.max(0, Number(bytes));
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return new Intl.NumberFormat(activeLocale(), {
    style: "unit",
    unit: BYTE_UNITS[unit],
    unitDisplay: unit === 0 ? "long" : "short",
    maximumFractionDigits: unit === 0 ? 0 : 1,
  }).format(value);
}

/**
 * When a page in the trash will be deleted by itself (ADR-0040), for the Trash: "Deleted today",
 * "Deleted tomorrow" or "Deleted in 12 days", counted in calendar days of the local time zone from
 * `trashedAt` and the limit `retentionMs` (`null` when trashed pages are kept: then there is nothing
 * to say, and the result is `null`).
 */
export function formatTrashExpiry(
  trashedAt: number | bigint,
  retentionMs: number | null,
  now: number = Date.now(),
): string | null {
  const days = trashDaysLeft(trashedAt, retentionMs, now);
  if (days === null) {
    return null;
  }
  if (days === 0) {
    return t("trash.expires.today");
  }
  return days === 1 ? t("trash.expires.tomorrow") : t("trash.expires.other", { count: days });
}

/** "1 page" or "12 pages": how many pages a space holds. */
export function formatPageCount(count: number): string {
  return tp("spaces.pageCount", count);
}
