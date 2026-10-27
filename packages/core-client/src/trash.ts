// Trash expiry (ADR-0040 item 3) on the TypeScript side: the device setting that says how long a
// page stays in the trash, read exactly as `kb-core` reads it (`Core::trash_retention_ms`), and
// the arithmetic the in-memory mirror and the UI share (which entries are past the limit; how many
// days a page has left).

import type { JsonValue } from "./generated/serde_json/JsonValue";

/** Setting holding the number of days a page stays in the trash (`kb_core::TRASH_RETENTION_SETTING`). */
export const TRASH_RETENTION_SETTING = "app.trash.retention_days";

/** Days a page stays in the trash when the setting is absent (`kb_core::DEFAULT_TRASH_RETENTION_DAYS`); `0` keeps it forever. */
export const DEFAULT_TRASH_RETENTION_DAYS = 30;

/** How often the shell (and the mirror's reminders tick) expires the trash while the app runs: an hour. */
export const TRASH_EXPIRY_INTERVAL_MS = 60 * 60 * 1000;

const MS_PER_DAY = 24 * 60 * 60 * 1000;

/**
 * The number of days a stored setting value means, as Settings shows it: like the core, a number is
 * a number of days (fractions allowed, a negative one is `0`, which keeps trashed pages forever),
 * and anything else (or nothing) is {@link DEFAULT_TRASH_RETENTION_DAYS}.
 */
export function trashRetentionDays(value: JsonValue | null | undefined): number {
  return typeof value === "number" ? Math.max(0, value) : DEFAULT_TRASH_RETENTION_DAYS;
}

/** The limit in milliseconds that a stored setting value means; `null` keeps trashed pages forever. */
export function trashRetentionMs(value: JsonValue | null | undefined): number | null {
  const days = trashRetentionDays(value);
  if (!Number.isFinite(days) || days <= 0) {
    return null;
  }
  return days * MS_PER_DAY;
}

/** Whether a page trashed at `trashedAt` (Unix ms) is past the limit at `now`: strictly older, as in the core. */
export function isTrashExpired(trashedAt: number | undefined, retentionMs: number, now: number): boolean {
  return trashedAt !== undefined && trashedAt < now - retentionMs;
}

/**
 * How many calendar days (in the local time zone) are left before the page trashed at `trashedAt`
 * is deleted at `now`: `0` for today (or when it is overdue and the next check will take it), `1`
 * for tomorrow. `null` when trashed pages are kept (`retentionMs` is `null`) or the date cannot be
 * worked out.
 */
export function trashDaysLeft(trashedAt: number | bigint, retentionMs: number | null, now: number): number | null {
  if (retentionMs === null) {
    return null;
  }
  const due = new Date(Number(trashedAt) + retentionMs);
  const today = new Date(now);
  if (Number.isNaN(due.getTime()) || Number.isNaN(today.getTime())) {
    return null;
  }
  const dueDay = Date.UTC(due.getFullYear(), due.getMonth(), due.getDate());
  const todayDay = Date.UTC(today.getFullYear(), today.getMonth(), today.getDate());
  return Math.max(0, Math.round((dueDay - todayDay) / MS_PER_DAY));
}
