// Dates (section 3.5): a local "floating" date-time is a count of milliseconds since 1970-01-01T00:00 local time plus a
// flag for "has a time". All the calendar arithmetic is integer math on days (the civil-from-days algorithms), so the
// host's time zone and `Date` quirks never leak in. A date outside years 1..9999 is not a date (it is empty).

import type { DateValue } from "../generated/DateValue";
import type { DateVal } from "./value";

export const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;
const MINUTE_MS = 60_000;

export function daysFromCivil(year: number, month: number, day: number): number {
  const y = month <= 2 ? year - 1 : year;
  const era = Math.floor(y / 400);
  const yoe = y - era * 400;
  const doy = Math.floor((153 * (month > 2 ? month - 3 : month + 9) + 2) / 5) + day - 1;
  const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
  return era * 146_097 + doe - 719_468;
}

export function civilFromDays(days: number): { y: number; m: number; d: number } {
  const z = days + 719_468;
  const era = Math.floor(z / 146_097);
  const doe = z - era * 146_097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36_524) - Math.floor(doe / 146_096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const d = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const m = mp < 10 ? mp + 3 : mp - 9;
  return { y: yoe + era * 400 + (m <= 2 ? 1 : 0), m, d };
}

export function isLeapYear(year: number): boolean {
  return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
}

export function daysInMonth(year: number, month: number): number {
  switch (month) {
    case 2:
      return isLeapYear(year) ? 29 : 28;
    case 4:
    case 6:
    case 9:
    case 11:
      return 30;
    default:
      return 31;
  }
}

/** 0001-01-01T00:00 .. 9999-12-31T23:59:59.999, in local ms. */
const MIN_MS = daysFromCivil(1, 1, 1) * DAY_MS;
const MAX_MS = daysFromCivil(10_000, 1, 1) * DAY_MS - 1;

/** A date from local ms, or `null` when it is outside years 1..9999 (or not a number). */
export function makeDate(ms: number, time: boolean): DateVal | null {
  if (!Number.isFinite(ms) || ms < MIN_MS || ms > MAX_MS) {
    return null;
  }
  return { k: "date", ms: ms === 0 ? 0 : ms, time };
}

/** The local ms of a Unix ms and the device's offset. */
export function localMs(unixMs: number, offsetSeconds: number): number {
  return unixMs + offsetSeconds * 1000;
}

export interface Parts {
  /** Whole days since 1970-01-01. */
  readonly days: number;
  /** Milliseconds since local midnight. */
  readonly msOfDay: number;
  readonly y: number;
  readonly m: number;
  readonly d: number;
}

export function partsOf(ms: number): Parts {
  const days = Math.floor(ms / DAY_MS);
  const msOfDay = ms - days * DAY_MS;
  return { days, msOfDay, ...civilFromDays(days) };
}

/** ISO 8601 day of the week: 1 (Monday) to 7 (Sunday). */
export function isoWeekday(days: number): number {
  return ((((days + 3) % 7) + 7) % 7) + 1;
}

/** ISO 8601 week number, 1..53. */
export function isoWeek(days: number): number {
  const thursday = days - isoWeekday(days) + 4;
  const { y } = civilFromDays(thursday);
  return Math.floor((thursday - daysFromCivil(y, 1, 1)) / 7) + 1;
}

export function hourOf(p: Parts): number {
  return Math.floor(p.msOfDay / HOUR_MS);
}

export function minuteOf(p: Parts): number {
  return Math.floor(p.msOfDay / MINUTE_MS) % 60;
}

export function secondOf(p: Parts): number {
  return Math.floor(p.msOfDay / 1000) % 60;
}

function pad(n: number, width: number): string {
  return String(n).padStart(width, "0");
}

// --- DateValue <-> DateVal ---

/** A `DateValue` (`YYYY-MM-DD`, `HH:mm`) as a date; `null` when the day is not a real day of years 1..9999. */
export function dateFromValue(value: DateValue): DateVal | null {
  const day = /^(\d{4})-(\d{2})-(\d{2})$/u.exec(value.date);
  if (day === null) {
    return null;
  }
  const y = Number(day[1]);
  const m = Number(day[2]);
  const d = Number(day[3]);
  if (y < 1 || m < 1 || m > 12 || d < 1 || d > daysInMonth(y, m)) {
    return null;
  }
  let ms = daysFromCivil(y, m, d) * DAY_MS;
  let time = false;
  if (value.time !== undefined) {
    const t = /^(\d{2}):(\d{2})$/u.exec(value.time);
    if (t !== null && Number(t[1]) <= 23 && Number(t[2]) <= 59) {
      ms += Number(t[1]) * HOUR_MS + Number(t[2]) * MINUTE_MS;
      time = true;
    }
  }
  return makeDate(ms, time);
}

/** A date as a `DateValue` (the seconds are dropped); `null` outside years 1..9999. */
export function dateToValue(date: DateVal): DateValue | null {
  if (makeDate(date.ms, date.time) === null) {
    return null;
  }
  const p = partsOf(date.ms);
  const day = `${pad(p.y, 4)}-${pad(p.m, 2)}-${pad(p.d, 2)}`;
  return date.time ? { date: day, time: `${pad(hourOf(p), 2)}:${pad(minuteOf(p), 2)}` } : { date: day };
}

/** `parseDate`: `YYYY-MM-DD`, `YYYY-MM-DDTHH:mm` or `YYYY-MM-DD HH:mm`, seconds `:ss` allowed; else `null`. */
export function parseDateText(text: string): DateVal | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2}))?)?$/u.exec(text);
  if (m === null) {
    return null;
  }
  const y = Number(m[1]);
  const mo = Number(m[2]);
  const d = Number(m[3]);
  if (y < 1 || mo < 1 || mo > 12 || d < 1 || d > daysInMonth(y, mo)) {
    return null;
  }
  const hasTime = m[4] !== undefined;
  const hh = hasTime ? Number(m[4]) : 0;
  const mm = hasTime ? Number(m[5]) : 0;
  const ss = m[6] !== undefined ? Number(m[6]) : 0;
  if (hh > 23 || mm > 59 || ss > 59) {
    return null;
  }
  return makeDate(daysFromCivil(y, mo, d) * DAY_MS + hh * HOUR_MS + mm * MINUTE_MS + ss * 1000, hasTime);
}

// --- arithmetic ---

/** The units of `dateAdd` and `dateBetween`. */
export type Unit = "years" | "quarters" | "months" | "weeks" | "days" | "hours" | "minutes";

export function parseUnit(text: string): Unit | null {
  switch (text) {
    case "years":
    case "quarters":
    case "months":
    case "weeks":
    case "days":
    case "hours":
    case "minutes":
      return text;
    default:
      return null;
  }
}

/** `dateAdd`: the count is truncated; months, quarters and years clamp the day to the month's end. */
export function addToDate(date: DateVal, count: number, unit: Unit): DateVal | null {
  const n = Math.trunc(count);
  switch (unit) {
    case "years":
    case "quarters":
    case "months": {
      const p = partsOf(date.ms);
      const months = n * (unit === "years" ? 12 : unit === "quarters" ? 3 : 1);
      const total = p.y * 12 + (p.m - 1) + months;
      if (!Number.isSafeInteger(total)) {
        return null;
      }
      const year = Math.floor(total / 12);
      const month = total - year * 12 + 1;
      if (year < 1 || year > 9999) {
        return null;
      }
      const day = Math.min(p.d, daysInMonth(year, month));
      return makeDate(daysFromCivil(year, month, day) * DAY_MS + p.msOfDay, date.time);
    }
    case "weeks":
      return makeDate(date.ms + n * 7 * DAY_MS, date.time);
    case "days":
      return makeDate(date.ms + n * DAY_MS, date.time);
    case "hours":
      return makeDate(date.ms + n * HOUR_MS, true);
    case "minutes":
      return makeDate(date.ms + n * MINUTE_MS, true);
  }
}

/** `dateBetween(a, b, unit)`: whole units of `a - b`, truncated toward zero (section 3.5). */
export function dateDifference(a: DateVal, b: DateVal, unit: Unit): number {
  const diff = a.ms - b.ms;
  switch (unit) {
    case "minutes":
      return Math.trunc(diff / MINUTE_MS);
    case "hours":
      return Math.trunc(diff / HOUR_MS);
    case "days":
      return Math.trunc(diff / DAY_MS);
    case "weeks":
      return Math.trunc(diff / (7 * DAY_MS));
    case "months":
    case "quarters":
    case "years": {
      const pa = partsOf(a.ms);
      const pb = partsOf(b.ms);
      let months = (pa.y - pb.y) * 12 + (pa.m - pb.m);
      // The rest of each date: the day of the month, then the time of day.
      const rest = pa.d !== pb.d ? (pa.d < pb.d ? -1 : 1) : pa.msOfDay < pb.msOfDay ? -1 : pa.msOfDay > pb.msOfDay ? 1 : 0;
      if (months > 0 && rest < 0) {
        months -= 1;
      } else if (months < 0 && rest > 0) {
        months += 1;
      }
      return Math.trunc(months / (unit === "months" ? 1 : unit === "quarters" ? 3 : 12));
    }
  }
}

// --- formatting ---

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];
const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

function ordinal(n: number): string {
  const rest = n % 100;
  if (rest >= 11 && rest <= 13) {
    return `${String(n)}th`;
  }
  switch (n % 10) {
    case 1:
      return `${String(n)}st`;
    case 2:
      return `${String(n)}nd`;
    case 3:
      return `${String(n)}rd`;
    default:
      return `${String(n)}th`;
  }
}

/** The tokens of `formatDate`, longest first. */
const TOKENS = [
  "YYYY",
  "MMMM",
  "dddd",
  "MMM",
  "ddd",
  "YY",
  "MM",
  "DD",
  "Do",
  "HH",
  "hh",
  "mm",
  "ss",
  "Q",
  "M",
  "D",
  "d",
  "H",
  "h",
  "m",
  "A",
  "a",
  "s",
];

function tokenText(token: string, p: Parts): string {
  const hour = hourOf(p);
  const weekday = isoWeekday(p.days) % 7;
  switch (token) {
    case "YYYY":
      return pad(p.y, 4);
    case "YY":
      return pad(((p.y % 100) + 100) % 100, 2);
    case "Q":
      return String(Math.floor((p.m - 1) / 3) + 1);
    case "MMMM":
      return MONTHS[p.m - 1] ?? "";
    case "MMM":
      return (MONTHS[p.m - 1] ?? "").slice(0, 3);
    case "MM":
      return pad(p.m, 2);
    case "M":
      return String(p.m);
    case "DD":
      return pad(p.d, 2);
    case "D":
      return String(p.d);
    case "Do":
      return ordinal(p.d);
    case "dddd":
      return WEEKDAYS[weekday] ?? "";
    case "ddd":
      return (WEEKDAYS[weekday] ?? "").slice(0, 3);
    case "d":
      return String(weekday);
    case "HH":
      return pad(hour, 2);
    case "H":
      return String(hour);
    case "hh":
      return pad(hour % 12 === 0 ? 12 : hour % 12, 2);
    case "h":
      return String(hour % 12 === 0 ? 12 : hour % 12);
    case "mm":
      return pad(minuteOf(p), 2);
    case "m":
      return String(minuteOf(p));
    case "A":
      return hour < 12 ? "AM" : "PM";
    case "a":
      return hour < 12 ? "am" : "pm";
    case "ss":
      return pad(secondOf(p), 2);
    case "s":
      return String(secondOf(p));
    default:
      return token;
  }
}

/** `formatDate`: the tokens of section 3.5, English names; `[text]` is copied as is, anything else too. */
export function formatDate(date: DateVal, format: string): string {
  const p = partsOf(date.ms);
  const chars = Array.from(format);
  let out = "";
  let i = 0;
  while (i < chars.length) {
    const c = chars[i] ?? "";
    if (c === "[") {
      const close = chars.indexOf("]", i + 1);
      if (close !== -1) {
        out += chars.slice(i + 1, close).join("");
        i = close + 1;
        continue;
      }
    }
    const token = TOKENS.find((t) => chars.slice(i, i + t.length).join("") === t);
    if (token !== undefined) {
      out += tokenText(token, p);
      i += token.length;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/** What `format()` writes for a date: `MMMM D, YYYY`, with `h:mm A` when it has a time. */
export function formatDateDefault(date: DateVal): string {
  return formatDate(date, date.time ? "MMMM D, YYYY h:mm A" : "MMMM D, YYYY");
}
