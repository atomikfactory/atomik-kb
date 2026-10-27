// The data of the calendar layout (ADR-0054, `docs/architecture/database-views.md` section 5): which pages are on
// which days, and what moving a card changes. Pure functions over `PageInfo` and the space's schema; the view
// (`PageCalendarView.tsx`) draws what they answer.
//
// The rules are the ones of section 5: a Date property or the page's own created or last-edited time is the
// source, an optional end-date property makes a page span several days, an end before the start is one day, the
// start's time is shown on the first day of a span, dropping a card on another day moves the start and the end by
// the same number of days and keeps both times, and a created or last-edited time cannot be moved. The stored data
// model does not change: a date value is still one day (`DateValue { date, time? }`).
//
// The month grid is laid out by week row (`layoutWeek`): a page of several days is one bar per week row it touches
// (cut at the edge of the row or of the visible grid, with the side that continues marked), the bars are packed into
// lanes at the top of the row (`packLanes`: earliest start first, longer first), the single-day cards are chips under
// the lanes, and a day shows at most `CALENDAR_MAX_CARDS_PER_DAY` items, a bar crossing it counting as one.

import type { DateValue, PageInfo, PropertyDef, PropertyId, PropertyValue } from "@atomik-kb/core-client";

import { activeLocale } from "../i18n";
import { addDays, daysBetween, isIsoDate, isIsoTime, todayIso } from "./dates";
import { valueOfPage } from "./page-list-model";
import type { PageListViewSpec } from "./page-list-views";

/** A day shows at most this many cards, then "+k more". */
export const CALENDAR_MAX_CARDS_PER_DAY = 3;

/** Where a page's date comes from: a Date property (the date can be changed), or the page's own times (it cannot). */
export type CalendarSource =
  | { readonly kind: "property"; readonly def: PropertyDef }
  | { readonly kind: "created" }
  | { readonly kind: "updated" };

/** What the view's `dateBy` and `endBy` resolve to against the schema. */
export type CalendarSourceState =
  /** Usable. `end` is the end-date property, `null` when none is chosen or the chosen one is not a Date property of the space. */
  | { readonly kind: "ok"; readonly source: CalendarSource; readonly end: PropertyDef | null }
  /** `dateBy` is absent: the view asks to choose one and writes nothing until the person does. */
  | { readonly kind: "unset" }
  /** `dateBy` names a property the space no longer has. */
  | { readonly kind: "gone" }
  /** `dateBy` names a property that is not a Date property. */
  | { readonly kind: "kind"; readonly def: PropertyDef };

/** The usable form of a `CalendarSourceState`. */
export type CalendarSourceOk = Extract<CalendarSourceState, { kind: "ok" }>;

/** The days a page covers, as local calendar days `YYYY-MM-DD` (`end >= start`), and the start's time of day if it has one. */
export interface CalendarSpan {
  readonly start: string;
  readonly end: string;
  readonly startTime: string | null;
}

/** One chip of a page on one day. */
export interface CalendarEntry {
  readonly page: PageInfo;
  readonly day: string;
  /** Where the day is in the page's span: the only day, the first, a middle one, the last. */
  readonly position: "single" | "start" | "middle" | "end";
  /** The start time of day, on the first day of the span (or the only day) when the date has one. */
  readonly time: string | null;
}

/** A change a drop makes to a page: a Date property gets a value. */
export interface CalendarChange {
  readonly propertyId: PropertyId;
  readonly value: PropertyValue;
}

/** The value of a Date property of `page` when it holds a usable day (`YYYY-MM-DD`); `null` for none or anything else. */
function dateValueOf(page: PageInfo, def: PropertyDef): DateValue | null {
  const value = valueOfPage(page, def.id);
  if (value?.kind !== "date" || !isIsoDate(value.value.date)) {
    return null;
  }
  return value.value;
}

/** A date value of a day and an optional time (the time key is left out for none, as the core writes it). */
function dateOf(date: string, time: string | null | undefined): PropertyValue {
  return { kind: "date", value: time === null || time === undefined ? { date } : { date, time } };
}

/** Resolves the view's `dateBy` and `endBy` (see `CalendarSourceState`). */
export function resolveCalendarSource(view: Pick<PageListViewSpec, "dateBy" | "endBy">, schema: readonly PropertyDef[]): CalendarSourceState {
  const { dateBy, endBy } = view;
  if (dateBy === undefined || dateBy === "") {
    return { kind: "unset" };
  }
  if (dateBy === "created") {
    return { kind: "ok", source: { kind: "created" }, end: null };
  }
  if (dateBy === "updated") {
    return { kind: "ok", source: { kind: "updated" }, end: null };
  }
  const def = schema.find((candidate) => candidate.id === dateBy);
  if (def === undefined) {
    return { kind: "gone" };
  }
  if (def.kind !== "date") {
    return { kind: "kind", def };
  }
  // An end that is not a Date property of the space is ignored (and kept in the view); so is the start property itself.
  const end = endBy === undefined || endBy === dateBy ? undefined : schema.find((candidate) => candidate.id === endBy);
  return { kind: "ok", source: { kind: "property", def }, end: end !== undefined && end.kind === "date" ? end : null };
}

/** The local calendar day of a time in Unix milliseconds (the viewer's own calendar, as "today" is). */
function localDay(ms: number): string {
  return todayIso(new Date(ms));
}

/** The span of a page, `null` when it has no date (a Date source without a value; the page is "undated"). */
export function spanOf(page: PageInfo, state: CalendarSourceOk): CalendarSpan | null {
  const { source } = state;
  if (source.kind === "created" || source.kind === "updated") {
    const day = localDay(source.kind === "created" ? page.createdAt : page.updatedAt);
    return { start: day, end: day, startTime: null };
  }
  const start = dateValueOf(page, source.def);
  if (start === null) {
    return null;
  }
  const startTime = start.time !== undefined && isIsoTime(start.time) ? start.time : null;
  const end = state.end === null ? null : dateValueOf(page, state.end);
  // An end before the start is one day: the page stays on its start day.
  const last = end !== null && daysBetween(start.date, end.date) > 0 ? end.date : start.date;
  return { start: start.date, end: last, startTime };
}

/** The chips of one page on the days of `range` (clipped to it), in day order. */
function entriesOf(page: PageInfo, span: CalendarSpan, range: { readonly first: string; readonly last: string }): CalendarEntry[] {
  if (span.end < range.first || span.start > range.last) {
    return [];
  }
  const entries: CalendarEntry[] = [];
  const from = span.start < range.first ? range.first : span.start;
  const to = span.end > range.last ? range.last : span.end;
  const several = span.start !== span.end;
  const count = daysBetween(from, to);
  for (let offset = 0; offset <= count; offset += 1) {
    const day = addDays(from, offset);
    if (!isIsoDate(day)) {
      break; // the end of the calendar (year 9999)
    }
    const position = !several ? "single" : day === span.start ? "start" : day === span.end ? "end" : "middle";
    entries.push({ page, day, position, time: day === span.start ? span.startTime : null });
  }
  return entries;
}

/** The order of the chips of a day: pages of several days (0), then pages of one day without a time (1), then timed ones (2). */
function groupOf(span: CalendarSpan): 0 | 1 | 2 {
  if (span.start !== span.end) {
    return 0;
  }
  return span.startTime === null ? 1 : 2;
}

/**
 * The chips of the pages on the days from `range.first` to `range.last` (the visible grid; a span is clipped to it, so
 * a span of years costs nothing), each day's chips in the order of section 5: pages of several days first, then the
 * others without a time, then timed ones by time, each group in the order of `pages` (the view's sort).
 */
export function placePages(
  pages: readonly PageInfo[],
  state: CalendarSourceOk,
  range: { readonly first: string; readonly last: string },
): Map<string, CalendarEntry[]> {
  const days = new Map<string, { entry: CalendarEntry; group: 0 | 1 | 2; order: number }[]>();
  pages.forEach((page, order) => {
    const span = spanOf(page, state);
    if (span === null) {
      return;
    }
    for (const entry of entriesOf(page, span, range)) {
      const list = days.get(entry.day);
      const item = { entry, group: groupOf(span), order };
      if (list === undefined) {
        days.set(entry.day, [item]);
      } else {
        list.push(item);
      }
    }
  });
  const placed = new Map<string, CalendarEntry[]>();
  for (const [day, items] of days) {
    items.sort(
      (a, b) =>
        a.group - b.group ||
        (a.group === 2 ? (a.entry.time ?? "").localeCompare(b.entry.time ?? "") : 0) ||
        a.order - b.order,
    );
    placed.set(
      day,
      items.map((item) => item.entry),
    );
  }
  return placed;
}

/**
 * One page of several days inside one week row of the grid: a bar. A span that crosses a week boundary (or the edge of
 * the visible grid) is one bar per week row it touches, each marked on the side that continues.
 */
export interface CalendarBar {
  readonly page: PageInfo;
  /** 0 is the top lane of the week row. */
  readonly lane: number;
  /** The grid column of the bar's first day in the week row, 1 to 7 (`grid-column: column / span span`). */
  readonly column: number;
  /** How many days of the week row the bar covers, 1 to 7. */
  readonly span: number;
  /** The first and the last day of the bar. */
  readonly first: string;
  readonly last: string;
  /** The page's span began before this bar (an earlier week row, or before the visible grid). */
  readonly continuesBefore: boolean;
  /** The page's span goes on after this bar (a later week row, or after the visible grid). */
  readonly continuesAfter: boolean;
  /** The start's time of day, on the bar that holds the first day of the span. */
  readonly time: string | null;
  /** Its lane is past the day limit, so it is not drawn (the days it crosses count it in "+k more"). */
  readonly hidden: boolean;
}

/** One day of a week row: what the day holds, what its cell draws under the bars, and what is left for "+k more". */
export interface CalendarDayLayout {
  readonly day: string;
  /** Everything on the day, the bars' pages first (the list of "+k more", the day's count). */
  readonly entries: readonly CalendarEntry[];
  /** The single-day chips the cell draws, below the lanes. */
  readonly chips: readonly CalendarEntry[];
  /** Items of the day that are neither drawn as a bar nor as a chip. */
  readonly hidden: number;
}

/** A week row of the grid: its bars in lanes, how many lanes it draws, and its seven days. */
export interface CalendarWeekLayout {
  /** Every bar of the row, hidden ones included, by lane then column. */
  readonly bars: readonly CalendarBar[];
  /** The lanes drawn above the chips (the lanes of the drawn bars, at most the limit of a day). */
  readonly lanes: number;
  readonly days: readonly CalendarDayLayout[];
}

/**
 * Greedy interval packing: the lane (0, 1, ...) of each inclusive interval, in the order given. The earliest start goes
 * first, a longer interval before a shorter one that starts with it, then the order given; an interval takes the first
 * lane whose last interval ended before it starts (one that ends the day before shares the lane).
 */
export function packLanes(intervals: readonly { readonly start: number; readonly end: number }[]): number[] {
  const order = intervals.map((_, index) => index);
  order.sort((a, b) => {
    const left = intervals[a];
    const right = intervals[b];
    return (left?.start ?? 0) - (right?.start ?? 0) || (right?.end ?? 0) - (left?.end ?? 0) || a - b;
  });
  const lanes = new Array<number>(intervals.length).fill(0);
  const ends: number[] = [];
  for (const index of order) {
    const interval = intervals[index];
    if (interval === undefined) {
      continue;
    }
    const free = ends.findIndex((end) => end < interval.start);
    const lane = free === -1 ? ends.length : free;
    ends[lane] = interval.end;
    lanes[index] = lane;
  }
  return lanes;
}

/**
 * Lays out one week row (seven consecutive days) from the placement of the visible grid: the pages of several days
 * become bars in lanes at the top of the row (`packLanes` over the week's columns), the single-day cards stay chips
 * below them. A day shows at most `maxPerDay` items, the bars crossing it counting as items: a bar in a lane past the
 * limit is not drawn (every day it crosses counts it as hidden), and the chips of a day fill what its drawn bars leave.
 */
export function layoutWeek(
  week: readonly string[],
  placed: ReadonlyMap<string, readonly CalendarEntry[]>,
  maxPerDay: number = CALENDAR_MAX_CARDS_PER_DAY,
): CalendarWeekLayout {
  // The bar of each page of several days, from its entries in the row's days (they are consecutive: a span is clipped, not cut).
  interface Draft {
    page: PageInfo;
    first: number;
    last: number;
    before: boolean;
    after: boolean;
    time: string | null;
  }
  const drafts = new Map<string, Draft>();
  week.forEach((day, column) => {
    for (const entry of placed.get(day) ?? []) {
      if (entry.position === "single") {
        continue;
      }
      const draft = drafts.get(entry.page.id);
      if (draft === undefined) {
        drafts.set(entry.page.id, {
          page: entry.page,
          first: column,
          last: column,
          before: entry.position !== "start",
          after: entry.position !== "end",
          time: entry.time,
        });
      } else {
        draft.last = column;
        draft.after = entry.position !== "end";
      }
    }
  });
  const found = [...drafts.values()];
  const lanes = packLanes(found.map((draft) => ({ start: draft.first, end: draft.last })));
  const bars: CalendarBar[] = found.map((draft, index) => ({
    page: draft.page,
    lane: lanes[index] ?? 0,
    column: draft.first + 1,
    span: draft.last - draft.first + 1,
    first: week[draft.first] ?? "",
    last: week[draft.last] ?? "",
    continuesBefore: draft.before,
    continuesAfter: draft.after,
    time: draft.time,
    hidden: (lanes[index] ?? 0) >= maxPerDay,
  }));
  bars.sort((a, b) => a.lane - b.lane || a.column - b.column);
  const drawn = bars.filter((bar) => !bar.hidden);
  const days = week.map((day, column): CalendarDayLayout => {
    const entries = placed.get(day) ?? [];
    const crossing = drawn.filter((bar) => bar.column - 1 <= column && column <= bar.column - 2 + bar.span).length;
    const chips = entries.filter((entry) => entry.position === "single").slice(0, Math.max(0, maxPerDay - crossing));
    return { day, entries, chips, hidden: entries.length - crossing - chips.length };
  });
  return { bars, lanes: drawn.reduce((count, bar) => Math.max(count, bar.lane + 1), 0), days };
}

/** The pages of `pages` with no date (the "No date" tray), in order. A created or last-edited source has none. */
export function undatedPages(pages: readonly PageInfo[], state: CalendarSourceOk): PageInfo[] {
  if (state.source.kind !== "property") {
    return [];
  }
  return pages.filter((page) => spanOf(page, state) === null);
}

/**
 * What dropping `page` on `toDay` writes: the start moves by `toDay - grabbedDay` days with its time kept and the
 * end (when the page has one) by the same number of days, its time kept; an undated page gets `toDay` as its start
 * (no time) and its end is left as it is. Nothing (an empty list) for a created or last-edited source, or when the
 * page is dropped where it already is. The start comes first: the writes are made one after the other.
 */
export function rescheduleChanges(page: PageInfo, state: CalendarSourceOk, toDay: string, grabbedDay: string): CalendarChange[] {
  const { source } = state;
  if (source.kind !== "property" || !isIsoDate(toDay)) {
    return [];
  }
  const start = dateValueOf(page, source.def);
  if (start === null) {
    return [{ propertyId: source.def.id, value: dateOf(toDay, null) }];
  }
  if (!isIsoDate(grabbedDay)) {
    return [];
  }
  const delta = daysBetween(grabbedDay, toDay);
  if (delta === 0) {
    return [];
  }
  return shiftedChanges(page, state, start, delta);
}

/** The start moved by `delta` days (its time kept) and the end by the same number of days, when the page has one. */
function shiftedChanges(page: PageInfo, state: CalendarSourceOk, start: DateValue, delta: number): CalendarChange[] {
  if (state.source.kind !== "property") {
    return [];
  }
  const movedStart = addDays(start.date, delta);
  if (!isIsoDate(movedStart) || movedStart === start.date) {
    return [];
  }
  const changes: CalendarChange[] = [{ propertyId: state.source.def.id, value: dateOf(movedStart, start.time) }];
  const end = state.end === null ? null : dateValueOf(page, state.end);
  if (state.end !== null && end !== null) {
    const movedEnd = addDays(end.date, delta);
    if (isIsoDate(movedEnd) && movedEnd !== end.date) {
      changes.push({ propertyId: state.end.id, value: dateOf(movedEnd, end.time) });
    }
  }
  return changes;
}

/**
 * What "Change date…" writes: the start becomes `date` with `time` (`null`: no time); when the day changed, the end
 * moves by the same number of days, its time kept. Nothing for a created or last-edited source or when nothing changed.
 */
export function changeDateChanges(page: PageInfo, state: CalendarSourceOk, date: string, time: string | null): CalendarChange[] {
  const { source } = state;
  if (source.kind !== "property" || !isIsoDate(date)) {
    return [];
  }
  const start = dateValueOf(page, source.def);
  if (start === null) {
    return [{ propertyId: source.def.id, value: dateOf(date, time) }];
  }
  const shifted = shiftedChanges(page, state, start, daysBetween(start.date, date));
  const keptTime = start.time !== undefined && isIsoTime(start.time) ? start.time : null;
  if (shifted.length === 0 && time === keptTime) {
    return [];
  }
  // The start takes the new day and time; the end (when the day changed) is the shift `shiftedChanges` made of it.
  return [{ propertyId: source.def.id, value: dateOf(date, time) }, ...shifted.slice(1)];
}

/** The time of day `HH:mm` as the locale writes it ("2:30 PM", "14:30"); the text as it is when it is not a valid time. */
export function formatTimeOfDay(time: string, locale: string = activeLocale()): string {
  if (!isIsoTime(time)) {
    return time;
  }
  const moment = new Date(Date.UTC(2000, 0, 1, Number(time.slice(0, 2)), Number(time.slice(3, 5))));
  return new Intl.DateTimeFormat(locale, { timeStyle: "short", timeZone: "UTC" }).format(moment);
}
