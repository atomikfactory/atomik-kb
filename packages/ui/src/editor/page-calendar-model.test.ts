// The data of the calendar layout (ADR-0054, section 5): what `dateBy` and `endBy` resolve to, the span of a page (a Date
// property, an optional end, an end before the start, the created and last-edited times on the local calendar), the chips
// of the visible days and their order, the undated pages, what a drop writes (the time kept, the end moved by the same
// days, an undated page dated, nothing for a built-in source) and what "Change date…" writes, then the week rows: the
// packing of intervals into lanes, the bars of a span split at week boundaries and at the edges of the grid, and the
// chips and "+k more" of a day when bars count as items.

import type { PageInfo, PropertyDef, PropertyValue } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { addDays } from "./dates";
import {
  CALENDAR_MAX_CARDS_PER_DAY,
  changeDateChanges,
  formatTimeOfDay,
  layoutWeek,
  packLanes,
  placePages,
  rescheduleChanges,
  resolveCalendarSource,
  spanOf,
  undatedPages,
} from "./page-calendar-model";
import type { CalendarSourceOk, CalendarWeekLayout } from "./page-calendar-model";

const START = "de000000-0000-4000-8000-00000000c001";
const END = "de000000-0000-4000-8000-00000000c002";
const TEXT = "de000000-0000-4000-8000-00000000c003";

function def(id: string, name: string, kind: PropertyDef["kind"] = "date"): PropertyDef {
  return { id, name, kind, options: [], createdAt: 0 };
}

const START_DEF = def(START, "Start");
const END_DEF = def(END, "End");
const TEXT_DEF = def(TEXT, "Notes", "text");
const SCHEMA = [START_DEF, END_DEF, TEXT_DEF];

function dateValue(date: string, time?: string): PropertyValue {
  return { kind: "date", value: time === undefined ? { date } : { date, time } };
}

interface PageSpec {
  start?: string;
  startTime?: string;
  end?: string;
  endTime?: string;
  createdAt?: number;
  updatedAt?: number;
}

function page(title: string, spec: PageSpec = {}): PageInfo {
  const properties: [string, PropertyValue][] = [];
  if (spec.start !== undefined) {
    properties.push([START, dateValue(spec.start, spec.startTime)]);
  }
  if (spec.end !== undefined) {
    properties.push([END, dateValue(spec.end, spec.endTime)]);
  }
  return {
    id: `page-${title}`,
    spaceId: "space",
    title,
    createdAt: spec.createdAt ?? 0,
    updatedAt: spec.updatedAt ?? 0,
    ...(properties.length > 0 ? { properties } : {}),
  };
}

function okState(view: { dateBy?: string; endBy?: string }, schema: readonly PropertyDef[] = SCHEMA): CalendarSourceOk {
  const state = resolveCalendarSource(view, schema);
  if (state.kind !== "ok") {
    throw new Error(`expected a usable source, got ${state.kind}`);
  }
  return state;
}

const RANGE = { first: "2026-10-01", last: "2026-11-11" };

/** The titles on each day of a placement, `day: [title@position/time]`. */
function listing(placed: Map<string, { page: PageInfo; position: string; time: string | null }[]>): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const [day, entries] of placed) {
    out[day] = entries.map((entry) => `${entry.page.title}@${entry.position}${entry.time === null ? "" : `/${entry.time}`}`);
  }
  return out;
}

describe("the source", () => {
  it("asks to choose while `dateBy` is absent, and writes nothing", () => {
    expect(resolveCalendarSource({}, SCHEMA)).toEqual({ kind: "unset" });
    expect(resolveCalendarSource({ dateBy: "" }, SCHEMA)).toEqual({ kind: "unset" });
    // An end alone is not a source.
    expect(resolveCalendarSource({ endBy: END }, SCHEMA)).toEqual({ kind: "unset" });
    expect(resolveCalendarSource({}, [])).toEqual({ kind: "unset" });
  });

  it("resolves a Date property, with its end when that is another Date property of the space", () => {
    expect(resolveCalendarSource({ dateBy: START }, SCHEMA)).toEqual({ kind: "ok", source: { kind: "property", def: START_DEF }, end: null });
    expect(resolveCalendarSource({ dateBy: START, endBy: END }, SCHEMA)).toEqual({
      kind: "ok",
      source: { kind: "property", def: START_DEF },
      end: END_DEF,
    });
  });

  it("ignores an end that is not a Date property of the space, or is the start itself, and does not lose the view's own value", () => {
    const view = { dateBy: START, endBy: "gone-id" };
    expect(resolveCalendarSource(view, SCHEMA)).toMatchObject({ kind: "ok", end: null });
    expect(resolveCalendarSource({ dateBy: START, endBy: TEXT }, SCHEMA)).toMatchObject({ kind: "ok", end: null });
    expect(resolveCalendarSource({ dateBy: START, endBy: START }, SCHEMA)).toMatchObject({ kind: "ok", end: null });
    expect(view).toEqual({ dateBy: START, endBy: "gone-id" });
  });

  it("takes the created and last-edited times as read-only sources with no end", () => {
    expect(resolveCalendarSource({ dateBy: "created", endBy: END }, SCHEMA)).toEqual({ kind: "ok", source: { kind: "created" }, end: null });
    expect(resolveCalendarSource({ dateBy: "updated" }, SCHEMA)).toEqual({ kind: "ok", source: { kind: "updated" }, end: null });
  });

  it("tells a property the space no longer has from one of another kind", () => {
    expect(resolveCalendarSource({ dateBy: "deleted-id" }, SCHEMA)).toEqual({ kind: "gone" });
    expect(resolveCalendarSource({ dateBy: TEXT }, SCHEMA)).toEqual({ kind: "kind", def: TEXT_DEF });
    expect(resolveCalendarSource({ dateBy: "created" }, [])).toMatchObject({ kind: "ok" });
  });
});

describe("the span of a page", () => {
  const state = okState({ dateBy: START, endBy: END });

  it("is the start day alone without an end, and keeps the start's time", () => {
    expect(spanOf(page("A", { start: "2026-10-07" }), state)).toEqual({ start: "2026-10-07", end: "2026-10-07", startTime: null });
    expect(spanOf(page("A", { start: "2026-10-07", startTime: "14:30" }), okState({ dateBy: START }))).toEqual({
      start: "2026-10-07",
      end: "2026-10-07",
      startTime: "14:30",
    });
  });

  it("runs to the end date when it is after the start", () => {
    expect(spanOf(page("A", { start: "2026-10-07", startTime: "09:00", end: "2026-10-10", endTime: "17:00" }), state)).toEqual({
      start: "2026-10-07",
      end: "2026-10-10",
      startTime: "09:00",
    });
  });

  it("is one day for an end on the start day or before it", () => {
    expect(spanOf(page("A", { start: "2026-10-07", end: "2026-10-07" }), state)).toMatchObject({ start: "2026-10-07", end: "2026-10-07" });
    expect(spanOf(page("A", { start: "2026-10-07", end: "2026-10-02" }), state)).toMatchObject({ start: "2026-10-07", end: "2026-10-07" });
  });

  it("is nothing for a page with no start, even with an end", () => {
    expect(spanOf(page("A"), state)).toBeNull();
    expect(spanOf(page("A", { end: "2026-10-10" }), state)).toBeNull();
  });

  it("is nothing for a start that is not a real day", () => {
    const broken: PageInfo = { ...page("A"), properties: [[START, { kind: "date", value: { date: "2026-13-45" } }]] };
    expect(spanOf(broken, state)).toBeNull();
    const text: PageInfo = { ...page("B"), properties: [[START, { kind: "text", value: "2026-10-07" }]] };
    expect(spanOf(text, state)).toBeNull();
  });

  it("ignores a time that is not a time of day", () => {
    const odd: PageInfo = { ...page("A"), properties: [[START, { kind: "date", value: { date: "2026-10-07", time: "25:99" } }]] };
    expect(spanOf(odd, state)).toEqual({ start: "2026-10-07", end: "2026-10-07", startTime: null });
  });

  it("is the local calendar day of the created or last-edited time, one day, with no time", () => {
    // 23:30 on the viewer's own calendar, whatever the zone of the machine: the day is the local one, not the UTC one.
    const late = new Date(2026, 9, 7, 23, 30).getTime();
    const early = new Date(2026, 9, 8, 0, 20).getTime();
    const p = page("A", { createdAt: late, updatedAt: early });
    expect(spanOf(p, okState({ dateBy: "created" }))).toEqual({ start: "2026-10-07", end: "2026-10-07", startTime: null });
    expect(spanOf(p, okState({ dateBy: "updated" }))).toEqual({ start: "2026-10-08", end: "2026-10-08", startTime: null });
  });
});

describe("placing pages on the visible days", () => {
  const state = okState({ dateBy: START, endBy: END });

  it("puts a page on every day of its span, marked first, middle and last, with the time on the first day only", () => {
    const placed = placePages([page("Trip", { start: "2026-10-07", startTime: "08:15", end: "2026-10-09" })], state, RANGE);
    expect(listing(placed)).toEqual({
      "2026-10-07": ["Trip@start/08:15"],
      "2026-10-08": ["Trip@middle"],
      "2026-10-09": ["Trip@end"],
    });
  });

  it("marks a one-day page single and gives it its time", () => {
    const placed = placePages([page("Call", { start: "2026-10-07", startTime: "14:30" }), page("Note", { start: "2026-10-07" })], state, RANGE);
    expect(listing(placed)).toEqual({ "2026-10-07": ["Note@single", "Call@single/14:30"] });
  });

  it("clips a span to the visible days, so a span of years costs nothing", () => {
    const placed = placePages([page("Epoch", { start: "2000-01-01", end: "2999-12-31" })], state, RANGE);
    expect(placed.size).toBe(42);
    expect([...placed.keys()][0]).toBe("2026-10-01");
    expect([...placed.keys()].at(-1)).toBe("2026-11-11");
    // Clipped at the left edge: the first visible day is a middle day of the span, so it carries no time.
    expect(listing(placed)["2026-10-01"]).toEqual(["Epoch@middle"]);
  });

  it("clips a span that starts before the grid and ends inside it, and one that starts inside and ends after", () => {
    const placed = placePages(
      [page("In", { start: "2026-09-28", end: "2026-10-02" }), page("Out", { start: "2026-11-10", startTime: "10:00", end: "2026-11-14" })],
      state,
      RANGE,
    );
    expect(listing(placed)).toEqual({
      "2026-10-01": ["In@middle"],
      "2026-10-02": ["In@end"],
      "2026-11-10": ["Out@start/10:00"],
      "2026-11-11": ["Out@middle"],
    });
  });

  it("leaves out pages outside the grid and pages with no date", () => {
    const placed = placePages(
      [page("Before", { start: "2026-09-30" }), page("After", { start: "2026-11-12" }), page("Undated"), page("Inside", { start: "2026-10-20" })],
      state,
      RANGE,
    );
    expect(listing(placed)).toEqual({ "2026-10-20": ["Inside@single"] });
  });

  it("orders a day: pages of several days, then one-day pages without a time, then timed ones by time, each in the order given", () => {
    const pages = [
      page("late", { start: "2026-10-07", startTime: "18:00" }),
      page("plain-b", { start: "2026-10-07" }),
      page("span-b", { start: "2026-10-06", end: "2026-10-08" }),
      page("early", { start: "2026-10-07", startTime: "07:45" }),
      page("plain-a", { start: "2026-10-07" }),
      page("span-a", { start: "2026-10-07", startTime: "03:00", end: "2026-10-07" }),
      page("span-c", { start: "2026-10-07", end: "2026-10-12" }),
      page("same-time", { start: "2026-10-07", startTime: "07:45" }),
    ];
    const placed = placePages(pages, state, RANGE);
    expect(placed.get("2026-10-07")?.map((entry) => entry.page.title)).toEqual([
      "span-b",
      "span-c",
      "plain-b",
      "plain-a",
      "span-a", // a one-day page with a time (its end is the start day)
      "early",
      "same-time",
      "late",
    ]);
  });

  it("places by the created time on the local day, in the order given", () => {
    const day = (hour: number): number => new Date(2026, 9, 7, hour, 0).getTime();
    const pages = [page("b", { createdAt: day(20) }), page("a", { createdAt: day(1) })];
    const placed = placePages(pages, okState({ dateBy: "created" }), RANGE);
    expect(listing(placed)).toEqual({ "2026-10-07": ["b@single", "a@single"] });
  });

  it("shows at most three cards a day in the view (the model keeps them all)", () => {
    expect(CALENDAR_MAX_CARDS_PER_DAY).toBe(3);
    const pages = ["a", "b", "c", "d", "e"].map((title) => page(title, { start: "2026-10-07" }));
    expect(placePages(pages, state, RANGE).get("2026-10-07")).toHaveLength(5);
  });
});

describe("the undated pages", () => {
  it("lists the pages with no start, in order, for a Date property", () => {
    const state = okState({ dateBy: START, endBy: END });
    const pages = [page("a"), page("b", { start: "2026-10-07" }), page("c", { end: "2026-10-09" }), page("d")];
    expect(undatedPages(pages, state).map((p) => p.title)).toEqual(["a", "c", "d"]);
  });

  it("has none for the created or last-edited time", () => {
    const pages = [page("a"), page("b")];
    expect(undatedPages(pages, okState({ dateBy: "created" }))).toEqual([]);
    expect(undatedPages(pages, okState({ dateBy: "updated" }))).toEqual([]);
  });
});

describe("what a drop writes", () => {
  const state = okState({ dateBy: START, endBy: END });

  it("moves the start by the days between the grabbed day and the drop day, keeping its time", () => {
    const p = page("A", { start: "2026-10-07", startTime: "14:30" });
    expect(rescheduleChanges(p, okState({ dateBy: START }), "2026-10-10", "2026-10-07")).toEqual([{ propertyId: START, value: dateValue("2026-10-10", "14:30") }]);
    expect(rescheduleChanges(p, okState({ dateBy: START }), "2026-10-03", "2026-10-07")).toEqual([{ propertyId: START, value: dateValue("2026-10-03", "14:30") }]);
  });

  it("writes no time key for a date with no time", () => {
    const [change] = rescheduleChanges(page("A", { start: "2026-10-07" }), state, "2026-10-08", "2026-10-07");
    expect(change?.value).toEqual({ kind: "date", value: { date: "2026-10-08" } });
    expect(Object.keys((change?.value as { value: object }).value)).toEqual(["date"]);
  });

  it("moves the end by the same days, keeping its time, after the start", () => {
    const p = page("Trip", { start: "2026-10-07", startTime: "08:00", end: "2026-10-09", endTime: "17:30" });
    expect(rescheduleChanges(p, state, "2026-10-14", "2026-10-07")).toEqual([
      { propertyId: START, value: dateValue("2026-10-14", "08:00") },
      { propertyId: END, value: dateValue("2026-10-16", "17:30") },
    ]);
  });

  it("measures from the day the card was grabbed on, which can be a later day of its span", () => {
    const p = page("Trip", { start: "2026-10-07", end: "2026-10-09" });
    // Grabbed on the 9th and dropped on the 12th: three days later.
    expect(rescheduleChanges(p, state, "2026-10-12", "2026-10-09")).toEqual([
      { propertyId: START, value: dateValue("2026-10-10") },
      { propertyId: END, value: dateValue("2026-10-12") },
    ]);
  });

  it("crosses months and years", () => {
    const p = page("A", { start: "2026-12-30", end: "2027-01-02" });
    expect(rescheduleChanges(p, state, "2027-02-01", "2026-12-30")).toEqual([
      { propertyId: START, value: dateValue("2027-02-01") },
      { propertyId: END, value: dateValue("2027-02-04") },
    ]);
  });

  it("moves an end that is before the start by the same days (the value is kept as it is, moved)", () => {
    const p = page("A", { start: "2026-10-07", end: "2026-10-02" });
    expect(rescheduleChanges(p, state, "2026-10-08", "2026-10-07")).toEqual([
      { propertyId: START, value: dateValue("2026-10-08") },
      { propertyId: END, value: dateValue("2026-10-03") },
    ]);
  });

  it("writes only the start when the page has no end value", () => {
    expect(rescheduleChanges(page("A", { start: "2026-10-07" }), state, "2026-10-08", "2026-10-07")).toHaveLength(1);
  });

  it("writes nothing when the page is dropped on the day it was grabbed on", () => {
    expect(rescheduleChanges(page("A", { start: "2026-10-07", end: "2026-10-09" }), state, "2026-10-08", "2026-10-08")).toEqual([]);
  });

  it("dates an undated page: the drop day as its start, no time, its end value left as it is", () => {
    expect(rescheduleChanges(page("A"), state, "2026-10-20", "2026-10-20")).toEqual([{ propertyId: START, value: dateValue("2026-10-20") }]);
    expect(rescheduleChanges(page("B", { end: "2026-10-30", endTime: "10:00" }), state, "2026-10-20", "2026-10-20")).toEqual([
      { propertyId: START, value: dateValue("2026-10-20") },
    ]);
  });

  it("writes nothing for the created and last-edited sources, which cannot be moved", () => {
    for (const dateBy of ["created", "updated"]) {
      expect(rescheduleChanges(page("A"), okState({ dateBy }), "2026-10-20", "2026-10-07")).toEqual([]);
    }
  });

  it("writes nothing for a day that is not a real day", () => {
    expect(rescheduleChanges(page("A", { start: "2026-10-07" }), state, "nope", "2026-10-07")).toEqual([]);
    expect(rescheduleChanges(page("A", { start: "9999-12-31" }), state, "10000-01-01", "9999-12-31")).toEqual([]);
  });
});

describe("what \"Change date…\" writes", () => {
  const state = okState({ dateBy: START, endBy: END });

  it("sets the day and the time and moves the end by the same days", () => {
    const p = page("Trip", { start: "2026-10-07", startTime: "08:00", end: "2026-10-09", endTime: "17:30" });
    expect(changeDateChanges(p, state, "2026-10-08", "08:00")).toEqual([
      { propertyId: START, value: dateValue("2026-10-08", "08:00") },
      { propertyId: END, value: dateValue("2026-10-10", "17:30") },
    ]);
  });

  it("changes the time alone with one write, leaving the end", () => {
    const p = page("Trip", { start: "2026-10-07", startTime: "08:00", end: "2026-10-09" });
    expect(changeDateChanges(p, state, "2026-10-07", "09:15")).toEqual([{ propertyId: START, value: dateValue("2026-10-07", "09:15") }]);
    expect(changeDateChanges(p, state, "2026-10-07", null)).toEqual([{ propertyId: START, value: dateValue("2026-10-07") }]);
  });

  it("writes nothing when nothing changed, and dates an undated page", () => {
    expect(changeDateChanges(page("A", { start: "2026-10-07", startTime: "08:00" }), state, "2026-10-07", "08:00")).toEqual([]);
    expect(changeDateChanges(page("B"), state, "2026-10-07", "10:00")).toEqual([{ propertyId: START, value: dateValue("2026-10-07", "10:00") }]);
  });

  it("writes nothing for a built-in source", () => {
    expect(changeDateChanges(page("A"), okState({ dateBy: "created" }), "2026-10-07", null)).toEqual([]);
  });
});

describe("the time of day", () => {
  it("is written as the locale does", () => {
    expect(formatTimeOfDay("14:30", "en-US").replace(/\s/gu, " ")).toBe("2:30 PM");
    expect(formatTimeOfDay("14:30", "fr-FR")).toBe("14:30");
    expect(formatTimeOfDay("00:05", "en-US").replace(/\s/gu, " ")).toBe("12:05 AM");
  });

  it("is the text as it is when it is not a time", () => {
    expect(formatTimeOfDay("late", "en-US")).toBe("late");
  });
});

describe("packing intervals into lanes", () => {
  it("gives nothing no lanes, and an interval alone the first", () => {
    expect(packLanes([])).toEqual([]);
    expect(packLanes([{ start: 2, end: 4 }])).toEqual([0]);
  });

  it("puts intervals that share a day in different lanes", () => {
    expect(
      packLanes([
        { start: 0, end: 2 },
        { start: 2, end: 4 },
        { start: 2, end: 2 },
      ]),
    ).toEqual([0, 1, 2]);
  });

  it("shares a lane between intervals that only follow each other, the second starting the day after the first ends", () => {
    expect(
      packLanes([
        { start: 0, end: 1 },
        { start: 2, end: 3 },
        { start: 4, end: 6 },
      ]),
    ).toEqual([0, 0, 0]);
  });

  it("puts the earliest start first and, for the same start, the longer interval first, whatever the order given", () => {
    // Given out of order: the lanes are answered in the order given.
    expect(
      packLanes([
        { start: 2, end: 3 }, // starts last: the third lane (it meets the first two on day 2 and 3)
        { start: 1, end: 3 }, // the longer of the two that start first: lane 0
        { start: 1, end: 2 },
      ]),
    ).toEqual([2, 0, 1]);
  });

  it("gives the lowest lane that is free", () => {
    expect(
      packLanes([
        { start: 0, end: 1 },
        { start: 0, end: 5 },
        { start: 2, end: 3 },
      ]),
    ).toEqual([1, 0, 1]);
  });

  it("keeps the order given for intervals that are the same", () => {
    expect(
      packLanes([
        { start: 1, end: 3 },
        { start: 1, end: 3 },
      ]),
    ).toEqual([0, 1]);
  });
});

describe("laying out a week row", () => {
  const state = okState({ dateBy: START, endBy: END });
  // Sunday 2026-10-04 to Saturday 2026-10-10, then the week after; columns 1 (Sunday) to 7.
  const WEEK_1 = Array.from({ length: 7 }, (_, index) => addDays("2026-10-04", index));
  const WEEK_2 = Array.from({ length: 7 }, (_, index) => addDays("2026-10-11", index));
  const TWO_WEEKS = { first: "2026-10-04", last: "2026-10-17" };

  function layoutOf(pages: PageInfo[], week: readonly string[] = WEEK_1, range = TWO_WEEKS, max?: number): CalendarWeekLayout {
    return layoutWeek(week, placePages(pages, state, range), max);
  }

  /** `title Llane cCOLUMN+SPAN`, with `<` and `>` for a side that continues and `hidden` for a bar that is not drawn. */
  function barsOf(layout: CalendarWeekLayout): string[] {
    return layout.bars.map(
      (bar) => `${bar.page.title} L${bar.lane} c${bar.column}+${bar.span}${bar.continuesBefore ? " <" : ""}${bar.continuesAfter ? " >" : ""}${bar.hidden ? " hidden" : ""}`,
    );
  }

  /** The chips and what is hidden, per day: `day: [chip titles] +hidden`. */
  function daysOf(layout: CalendarWeekLayout): Record<string, string> {
    const out: Record<string, string> = {};
    for (const day of layout.days) {
      out[day.day] = `[${day.chips.map((entry) => entry.page.title).join(",")}] +${day.hidden}`;
    }
    return out;
  }

  describe("the bars of a span", () => {
    it("is one bar over the days of a span inside the week row, with the start's time", () => {
      const layout = layoutOf([page("Trip", { start: "2026-10-06", startTime: "08:15", end: "2026-10-08" })]);
      expect(barsOf(layout)).toEqual(["Trip L0 c3+3"]);
      const [bar] = layout.bars;
      expect(bar).toMatchObject({ first: "2026-10-06", last: "2026-10-08", time: "08:15", continuesBefore: false, continuesAfter: false, hidden: false });
      expect(layout.lanes).toBe(1);
    });

    it("is no bar for a page of one day, an end on the start day, or an end before the start", () => {
      const layout = layoutOf([
        page("One", { start: "2026-10-06" }),
        page("Same", { start: "2026-10-07", end: "2026-10-07" }),
        page("Back", { start: "2026-10-08", end: "2026-10-02" }),
      ]);
      expect(layout.bars).toEqual([]);
      expect(layout.lanes).toBe(0);
      expect(daysOf(layout)["2026-10-06"]).toBe("[One] +0");
    });

    it("covers a whole week row with no mark when the span is exactly that week", () => {
      expect(barsOf(layoutOf([page("Week", { start: "2026-10-04", end: "2026-10-10" })]))).toEqual(["Week L0 c1+7"]);
    });

    it("splits a span that crosses a week boundary into a bar per week row, marked on the side that continues", () => {
      const trip = page("Trip", { start: "2026-10-09", startTime: "08:00", end: "2026-10-13" });
      const first = layoutOf([trip], WEEK_1);
      const second = layoutOf([trip], WEEK_2);
      expect(barsOf(first)).toEqual(["Trip L0 c6+2 >"]);
      expect(barsOf(second)).toEqual(["Trip L0 c1+3 <"]);
      expect(first.bars[0]).toMatchObject({ first: "2026-10-09", last: "2026-10-10" });
      expect(second.bars[0]).toMatchObject({ first: "2026-10-11", last: "2026-10-13" });
      // The time belongs to the bar that holds the first day of the span.
      expect(first.bars[0]?.time).toBe("08:00");
      expect(second.bars[0]?.time).toBeNull();
    });

    it("has a middle week row both continue, for a span of three week rows", () => {
      const range = { first: "2026-10-04", last: "2026-10-24" };
      const trip = page("Trip", { start: "2026-10-10", end: "2026-10-20" });
      const week = (first: string) => Array.from({ length: 7 }, (_, index) => addDays(first, index));
      const placed = placePages([trip], state, range);
      expect(barsOf(layoutWeek(week("2026-10-04"), placed))).toEqual(["Trip L0 c7+1 >"]);
      expect(barsOf(layoutWeek(week("2026-10-11"), placed))).toEqual(["Trip L0 c1+7 < >"]);
      expect(barsOf(layoutWeek(week("2026-10-18"), placed))).toEqual(["Trip L0 c1+3 <"]);
    });

    it("continues before when the span starts before the visible grid, and after when it ends after it", () => {
      const before = page("Before", { start: "2026-09-28", end: "2026-10-06" });
      const after = page("After", { start: "2026-10-15", end: "2026-10-25" });
      expect(barsOf(layoutOf([before, after], WEEK_1))).toEqual(["Before L0 c1+3 <"]);
      expect(barsOf(layoutOf([before, after], WEEK_2))).toEqual(["After L0 c5+3 >"]);
      // The time of a start that is off the grid is not shown (its day is not drawn).
      const timed = page("Timed", { start: "2026-09-30", startTime: "10:00", end: "2026-10-05" });
      expect(layoutOf([timed], WEEK_1).bars[0]?.time).toBeNull();
    });

    it("covers every week row of the grid, both sides continuing, for a span that is longer than the grid", () => {
      const epoch = page("Epoch", { start: "2000-01-01", end: "2999-12-31" });
      expect(barsOf(layoutOf([epoch], WEEK_1))).toEqual(["Epoch L0 c1+7 < >"]);
      expect(barsOf(layoutOf([epoch], WEEK_2))).toEqual(["Epoch L0 c1+7 < >"]);
    });

    it("has no bar in a week row the span does not touch", () => {
      const trip = page("Trip", { start: "2026-10-12", end: "2026-10-14" });
      expect(layoutOf([trip], WEEK_1).bars).toEqual([]);
      expect(layoutOf([trip], WEEK_1).days.every((day) => day.entries.length === 0)).toBe(true);
    });
  });

  describe("the lanes", () => {
    it("puts bars that overlap in different lanes", () => {
      const layout = layoutOf([page("A", { start: "2026-10-05", end: "2026-10-07" }), page("B", { start: "2026-10-06", end: "2026-10-08" })]);
      expect(barsOf(layout)).toEqual(["A L0 c2+3", "B L1 c3+3"]);
      expect(layout.lanes).toBe(2);
    });

    it("puts a bar inside a longer one in the next lane, however the pages are ordered", () => {
      const long = page("Long", { start: "2026-10-04", end: "2026-10-10" });
      const short = page("Short", { start: "2026-10-06", end: "2026-10-07" });
      expect(barsOf(layoutOf([long, short]))).toEqual(["Long L0 c1+7", "Short L1 c3+2"]);
      expect(barsOf(layoutOf([short, long]))).toEqual(["Long L0 c1+7", "Short L1 c3+2"]);
    });

    it("shares a lane between bars that follow each other, one starting the day after the other ends", () => {
      const layout = layoutOf([page("A", { start: "2026-10-05", end: "2026-10-06" }), page("B", { start: "2026-10-07", end: "2026-10-08" })]);
      expect(barsOf(layout)).toEqual(["A L0 c2+2", "B L0 c4+2"]);
      expect(layout.lanes).toBe(1);
    });

    it("does not share a lane when one bar starts on the day the other ends", () => {
      const layout = layoutOf([page("A", { start: "2026-10-05", end: "2026-10-06" }), page("B", { start: "2026-10-06", end: "2026-10-08" })]);
      expect(barsOf(layout)).toEqual(["A L0 c2+2", "B L1 c3+3"]);
    });

    it("takes the earliest start first, then the longer bar, then the order of the rows", () => {
      const a = page("A", { start: "2026-10-06", end: "2026-10-07" });
      const b = page("B", { start: "2026-10-05", end: "2026-10-07" });
      const c = page("C", { start: "2026-10-05", end: "2026-10-06" });
      expect(barsOf(layoutOf([a, b, c]))).toEqual(["B L0 c2+3", "C L1 c2+2", "A L2 c3+2"]);
      const x = page("X", { start: "2026-10-05", end: "2026-10-06" });
      const y = page("Y", { start: "2026-10-05", end: "2026-10-06" });
      expect(barsOf(layoutOf([x, y]))).toEqual(["X L0 c2+2", "Y L1 c2+2"]);
      expect(barsOf(layoutOf([y, x]))).toEqual(["Y L0 c2+2", "X L1 c2+2"]);
    });

    it("gives a later, shorter bar the lowest lane that is free", () => {
      const layout = layoutOf([
        page("Early", { start: "2026-10-04", end: "2026-10-05" }),
        page("All", { start: "2026-10-04", end: "2026-10-09" }),
        page("Late", { start: "2026-10-06", end: "2026-10-07" }),
      ]);
      expect(barsOf(layout)).toEqual(["All L0 c1+6", "Early L1 c1+2", "Late L1 c3+2"]);
    });

    it("lays the bars out by lane, then by column", () => {
      const layout = layoutOf([
        page("Wed", { start: "2026-10-07", end: "2026-10-08" }),
        page("Mon", { start: "2026-10-05", end: "2026-10-06" }),
        page("Sun", { start: "2026-10-04", end: "2026-10-10" }),
      ]);
      expect(barsOf(layout)).toEqual(["Sun L0 c1+7", "Mon L1 c2+2", "Wed L1 c4+2"]);
    });

    it("packs each week row on its own, so a page can sit in another lane in the next row", () => {
      const trip = page("Trip", { start: "2026-10-09", end: "2026-10-13" });
      const mid = page("Mid", { start: "2026-10-08", end: "2026-10-10" });
      expect(barsOf(layoutOf([trip, mid], WEEK_1))).toEqual(["Mid L0 c5+3", "Trip L1 c6+2 >"]);
      expect(barsOf(layoutOf([trip, mid], WEEK_2))).toEqual(["Trip L0 c1+3 <"]);
    });

    it("draws three lanes at most, however many bars overlap, and flags the others hidden", () => {
      const pages = ["p1", "p2", "p3", "p4", "p5"].map((title) => page(title, { start: "2026-10-05", end: "2026-10-06" }));
      const layout = layoutOf(pages);
      expect(barsOf(layout)).toEqual(["p1 L0 c2+2", "p2 L1 c2+2", "p3 L2 c2+2", "p4 L3 c2+2 hidden", "p5 L4 c2+2 hidden"]);
      expect(layout.lanes).toBe(CALENDAR_MAX_CARDS_PER_DAY);
    });

    it("gives each of many overlapping bars its own lane", () => {
      const pages = Array.from({ length: 8 }, (_, index) => page(`p${index}`, { start: "2026-10-05", end: "2026-10-09" }));
      expect(layoutOf(pages, WEEK_1, TWO_WEEKS, 100).bars.map((bar) => bar.lane)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    });
  });

  describe("the chips and \"+k more\" of a day", () => {
    const span = (title: string, start: string, end: string) => page(title, { start, end });

    it("keeps every single-day card as a chip when the day holds at most three items", () => {
      const layout = layoutOf([span("bar", "2026-10-05", "2026-10-07"), page("c1", { start: "2026-10-06" }), page("c2", { start: "2026-10-06" })]);
      expect(daysOf(layout)["2026-10-06"]).toBe("[c1,c2] +0");
      expect(daysOf(layout)["2026-10-05"]).toBe("[] +0");
    });

    it("counts the bars crossing a day as items: two bars leave room for one chip", () => {
      const pages = [
        span("b1", "2026-10-05", "2026-10-07"),
        span("b2", "2026-10-05", "2026-10-06"),
        page("c1", { start: "2026-10-06" }),
        page("c2", { start: "2026-10-06" }),
        page("c3", { start: "2026-10-06" }),
      ];
      const layout = layoutOf(pages);
      expect(barsOf(layout)).toEqual(["b1 L0 c2+3", "b2 L1 c2+2"]);
      // Five items: two bars and a chip are drawn, two are left.
      expect(daysOf(layout)["2026-10-06"]).toBe("[c1] +2");
      // The day after, one bar and no chip.
      expect(daysOf(layout)["2026-10-07"]).toBe("[] +0");
    });

    it("leaves no room for a chip beside three bars", () => {
      const pages = [
        span("b1", "2026-10-05", "2026-10-06"),
        span("b2", "2026-10-05", "2026-10-06"),
        span("b3", "2026-10-05", "2026-10-06"),
        page("c", { start: "2026-10-05" }),
      ];
      expect(daysOf(layoutOf(pages))["2026-10-05"]).toBe("[] +1");
    });

    it("shows exactly three bars with nothing left", () => {
      const pages = [span("b1", "2026-10-05", "2026-10-06"), span("b2", "2026-10-05", "2026-10-06"), span("b3", "2026-10-05", "2026-10-06")];
      const layout = layoutOf(pages);
      expect(daysOf(layout)["2026-10-05"]).toBe("[] +0");
      expect(layout.bars.some((bar) => bar.hidden)).toBe(false);
    });

    it("counts a bar that is not drawn on every day it crosses, even a day with room", () => {
      const pages = [
        span("b1", "2026-10-05", "2026-10-06"),
        span("b2", "2026-10-05", "2026-10-06"),
        span("b3", "2026-10-05", "2026-10-06"),
        span("b4", "2026-10-06", "2026-10-08"), // the fourth lane
        page("c", { start: "2026-10-07" }),
      ];
      const layout = layoutOf(pages);
      expect(barsOf(layout)).toEqual(["b1 L0 c2+2", "b2 L1 c2+2", "b3 L2 c2+2", "b4 L3 c3+3 hidden"]);
      expect(daysOf(layout)["2026-10-06"]).toBe("[] +1"); // b1 to b3 are drawn, b4 is not
      expect(daysOf(layout)["2026-10-07"]).toBe("[c] +1"); // only b4 crosses it, and it is left out
      expect(daysOf(layout)["2026-10-08"]).toBe("[] +1");
      expect(daysOf(layout)["2026-10-05"]).toBe("[] +0");
    });

    it("lists everything on the day, the bars' pages first, whether or not it is drawn", () => {
      const pages = [
        page("c1", { start: "2026-10-06" }),
        span("b1", "2026-10-05", "2026-10-07"),
        span("b2", "2026-10-06", "2026-10-07"),
        page("c2", { start: "2026-10-06", startTime: "09:00" }),
        page("c3", { start: "2026-10-06" }),
      ];
      const day = layoutOf(pages).days[2];
      expect(day?.day).toBe("2026-10-06");
      expect(day?.entries.map((entry) => entry.page.title)).toEqual(["b1", "b2", "c1", "c3", "c2"]);
      expect(day?.chips.map((entry) => entry.page.title)).toEqual(["c1"]);
      expect(day?.hidden).toBe(2);
    });

    it("takes the limit of a day as a parameter", () => {
      const pages = [span("b", "2026-10-05", "2026-10-06"), page("c1", { start: "2026-10-05" }), page("c2", { start: "2026-10-05" })];
      expect(daysOf(layoutOf(pages, WEEK_1, TWO_WEEKS, 2))["2026-10-05"]).toBe("[c1] +1");
      expect(layoutOf(pages, WEEK_1, TWO_WEEKS, 1).lanes).toBe(1);
      expect(daysOf(layoutOf(pages, WEEK_1, TWO_WEEKS, 1))["2026-10-05"]).toBe("[] +2");
    });

    it("has seven days, each named, with nothing on the days of an empty week row", () => {
      const layout = layoutOf([]);
      expect(layout.days.map((day) => day.day)).toEqual(WEEK_1);
      expect(layout.bars).toEqual([]);
      expect(layout.days.every((day) => day.entries.length === 0 && day.chips.length === 0 && day.hidden === 0)).toBe(true);
    });
  });
});
