// Dates of date mentions (ADR-0016, amendment of 2026-10-07): a mention holds a floating local
// date, `YYYY-MM-DD`, a calendar day with no time zone, and an optional `HH:mm`. Everything here
// is calendar arithmetic on those texts: UTC dates are used only as a way to count days (no
// daylight saving, no zone), and the day that is "today" is the one on this device's clock. The
// texts are checked by `isIsoDate` and `isIsoTime` of core-client, the same checks the core makes.

import { isIsoDate, isIsoTime, reminderFireAtLocal } from "@atomik-kb/core-client";

import { activeLocale, t } from "../i18n";
import type { MessageKey } from "../i18n";

export { isIsoDate, isIsoTime };

/** A day of the calendar; `month` counts from 1. */
export interface DateParts {
  year: number;
  month: number;
  day: number;
}

const DAY_MS = 86_400_000;

function pad(value: number, width: number): string {
  return String(value).padStart(width, "0");
}

/** The parts of a valid ISO date, `null` for anything else. */
export function partsOf(iso: string): DateParts | null {
  if (!isIsoDate(iso)) {
    return null;
  }
  return { year: Number(iso.slice(0, 4)), month: Number(iso.slice(5, 7)), day: Number(iso.slice(8, 10)) };
}

/** `YYYY-MM-DD` of a day (the parts must be a real day). */
export function isoOf({ year, month, day }: DateParts): string {
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}`;
}

/** The day `date` falls on a clock, in UTC (the year is kept as it is, also below 100). */
function utc(year: number, month: number, day: number, hours = 0, minutes = 0): Date {
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  date.setUTCHours(hours, minutes, 0, 0);
  return date;
}

function fromUtc(date: Date): string {
  return isoOf({ year: date.getUTCFullYear(), month: date.getUTCMonth() + 1, day: date.getUTCDate() });
}

/** The day of this device's clock (`now` for tests): its local calendar day, not the UTC one. */
export function todayIso(now: Date = new Date()): string {
  return isoOf({ year: now.getFullYear(), month: now.getMonth() + 1, day: now.getDate() });
}

/** The day `days` after `iso` (before it when negative); `iso` itself when it is not a valid date. */
export function addDays(iso: string, days: number): string {
  const parts = partsOf(iso);
  return parts === null ? iso : fromUtc(utc(parts.year, parts.month, parts.day + days));
}

export function daysInMonth(year: number, month: number): number {
  return utc(year, month + 1, 0).getUTCDate();
}

/** The same day `months` later (earlier when negative), the last day of the month when it has no such day. */
export function addMonths(iso: string, months: number): string {
  const parts = partsOf(iso);
  if (parts === null) {
    return iso;
  }
  const index = parts.year * 12 + (parts.month - 1) + months;
  const year = Math.floor(index / 12);
  const month = (index % 12) + 1;
  // Years stay inside the range a date may have (0001 to 9999).
  if (year < 1 || year > 9999) {
    return iso;
  }
  return isoOf({ year, month, day: Math.min(parts.day, daysInMonth(year, month)) });
}

/** The day of the week of `iso`: 0 for Sunday to 6 for Saturday. */
export function weekdayOf(iso: string): number {
  const parts = partsOf(iso);
  return parts === null ? 0 : utc(parts.year, parts.month, parts.day).getUTCDay();
}

/** How many days `iso` is after `from` (negative before it). */
export function daysBetween(from: string, iso: string): number {
  const a = partsOf(from);
  const b = partsOf(iso);
  if (a === null || b === null) {
    return 0;
  }
  return Math.round((utc(b.year, b.month, b.day).getTime() - utc(a.year, a.month, a.day).getTime()) / DAY_MS);
}

/**
 * The first day of the week in a locale: 0 for Sunday to 6 for Saturday. It comes from the
 * locale's week info when the runtime has it (`Intl.Locale`), else Monday.
 */
export function firstDayOfWeek(locale: string = activeLocale()): number {
  try {
    const intl = new Intl.Locale(locale) as unknown as {
      getWeekInfo?: () => { firstDay?: number };
      weekInfo?: { firstDay?: number };
    };
    const info = typeof intl.getWeekInfo === "function" ? intl.getWeekInfo() : intl.weekInfo;
    const first = info?.firstDay; // 1 for Monday to 7 for Sunday
    if (typeof first === "number" && Number.isInteger(first) && first >= 1 && first <= 7) {
      return first % 7;
    }
  } catch {
    // An invalid locale tag or a runtime without week info: Monday.
  }
  return 1;
}

/**
 * The six weeks (rows of seven days, from `firstDay`) that show the month of `iso`, with the
 * days of the months next to it filling the first and the last row.
 */
export function monthGrid(iso: string, firstDay: number): string[][] {
  const parts = partsOf(iso);
  if (parts === null) {
    return [];
  }
  const first = isoOf({ ...parts, day: 1 });
  const lead = (weekdayOf(first) - firstDay + 7) % 7;
  const start = addDays(first, -lead);
  const rows: string[][] = [];
  for (let row = 0; row < 6; row += 1) {
    rows.push(Array.from({ length: 7 }, (_, column) => addDays(start, row * 7 + column)));
  }
  return rows;
}

/** The days of a week in the order they are shown, with their short and long names. */
export function weekdayNames(firstDay: number, locale: string = activeLocale()): { short: string; long: string }[] {
  const short = new Intl.DateTimeFormat(locale, { weekday: "short", timeZone: "UTC" });
  const long = new Intl.DateTimeFormat(locale, { weekday: "long", timeZone: "UTC" });
  return Array.from({ length: 7 }, (_, offset) => {
    // 2023-01-01 was a Sunday.
    const date = utc(2023, 1, 1 + ((firstDay + offset) % 7));
    return { short: short.format(date), long: long.format(date) };
  });
}

/** "October 2026": the title of the month a grid shows. */
export function monthTitle(iso: string, locale: string = activeLocale()): string {
  const parts = partsOf(iso);
  if (parts === null) {
    return "";
  }
  return new Intl.DateTimeFormat(locale, { month: "long", year: "numeric", timeZone: "UTC" }).format(
    utc(parts.year, parts.month, 1),
  );
}

/** "Oct 7, 2026": the medium date of the locale, the day as it is written in a mention that is neither today nor next to it. */
export function mediumDate(iso: string, locale: string = activeLocale()): string {
  const parts = partsOf(iso);
  if (parts === null) {
    return iso;
  }
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }).format(
    utc(parts.year, parts.month, parts.day),
  );
}

/** "Wednesday, October 7, 2026": the full name of a day, for assistive technology. */
export function fullDate(iso: string, locale: string = activeLocale()): string {
  const parts = partsOf(iso);
  if (parts === null) {
    return iso;
  }
  return new Intl.DateTimeFormat(locale, { dateStyle: "full", timeZone: "UTC" }).format(
    utc(parts.year, parts.month, parts.day),
  );
}

/**
 * What a date mention shows: "Today", "Tomorrow" or "Yesterday" for those days, else the medium
 * date of the locale ("Oct 7, 2026"); with a time, after it (the locale's short time:
 * "Today, 2:30 PM", "Oct 7, 2026, 2:30 PM"). `today` is the day of the device's clock
 * (`todayIso()`): the chip shows relative words, so it has to be redrawn when the day changes.
 * A date that is not valid shows as it is stored.
 */
export function formatDateMention(date: string, time: string | null, today: string, locale: string = activeLocale()): string {
  const parts = partsOf(date);
  if (parts === null) {
    return date;
  }
  const clock = time !== null && isIsoTime(time) ? { hours: Number(time.slice(0, 2)), minutes: Number(time.slice(3, 5)) } : null;
  const day = utc(parts.year, parts.month, parts.day, clock?.hours ?? 0, clock?.minutes ?? 0);
  const offset = partsOf(today) === null ? null : daysBetween(today, date);
  const relative = offset === 0 ? t("date.today") : offset === 1 ? t("date.tomorrow") : offset === -1 ? t("date.yesterday") : null;
  if (relative === null) {
    return clock === null
      ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeZone: "UTC" }).format(day)
      : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" }).format(day);
  }
  if (clock === null) {
    return relative;
  }
  return t("date.relativeTime", {
    day: relative,
    time: new Intl.DateTimeFormat(locale, { timeStyle: "short", timeZone: "UTC" }).format(day),
  });
}

// ----- reminders (ADR-0024) ---------------------------------------------------------------------

/**
 * The reminders the date picker offers, in minutes before the moment of the mention (`null`: no
 * reminder): at the time, 5, 15 and 30 minutes, 1 and 2 hours, 1 and 2 days. Any non-negative
 * integer is valid in a document (another device, an import), and the picker shows it too.
 */
export const REMINDER_CHOICES: readonly (number | null)[] = [null, 0, 5, 15, 30, 60, 120, 1440, 2880];

/** The labels of the presets: for a mention with a time, and for one of a whole day (its moment is 09:00). */
const REMINDER_LABELS: Readonly<Record<number, readonly [MessageKey, MessageKey]>> = {
  0: ["date.reminder.atTime", "date.reminder.atNine"],
  5: ["date.reminder.m5", "date.reminder.m5Nine"],
  15: ["date.reminder.m15", "date.reminder.m15Nine"],
  30: ["date.reminder.m30", "date.reminder.m30Nine"],
  60: ["date.reminder.m60", "date.reminder.m60Nine"],
  120: ["date.reminder.m120", "date.reminder.m120Nine"],
  1440: ["date.reminder.m1440", "date.reminder.m1440Nine"],
  2880: ["date.reminder.m2880", "date.reminder.m2880Nine"],
};

/**
 * What a reminder is called: "None", "At time", "15 minutes before", "1 day before"... For a
 * mention of a whole day (`time` is `null`) the moment is 09:00 and the labels say so ("At 9:00",
 * "15 minutes before 9:00", "1 day before, at 9:00"). A value that is not a preset reads as its
 * minutes.
 */
export function reminderLabel(reminder: number | null, time: string | null): string {
  if (reminder === null) {
    return t("date.reminder.none");
  }
  const wholeDay = time === null || !isIsoTime(time);
  const preset = REMINDER_LABELS[reminder];
  if (preset !== undefined) {
    return t(wholeDay ? preset[1] : preset[0]);
  }
  return t(wholeDay ? "date.reminder.customNine" : "date.reminder.custom", { count: reminder });
}

/**
 * When a reminder fires, as an instant on this machine's clock: the mention's moment (`date` at
 * `time`, 09:00 when there is none) minus `reminder` minutes, counted on the wall clock (a mention
 * is a floating local date, so the reminder is due when the clock reads that time, whatever the
 * zone). `null` for a date that is not valid. The same number the core stores
 * (`reminderFireAtLocal` of core-client), read as local fields.
 */
export function reminderFireTime(date: string, time: string | null, reminder: number): Date | null {
  const seconds = reminderFireAtLocal(date, time, reminder);
  if (seconds === null) {
    return null;
  }
  const wall = new Date(seconds * 1000);
  const fire = new Date(0);
  fire.setFullYear(wall.getUTCFullYear(), wall.getUTCMonth(), wall.getUTCDate());
  fire.setHours(wall.getUTCHours(), wall.getUTCMinutes(), 0, 0);
  return fire;
}
