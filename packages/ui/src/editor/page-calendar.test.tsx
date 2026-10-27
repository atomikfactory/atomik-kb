// The calendar layout (ADR-0054, section 5), rendered with props over a store: the month grid and its navigation, the
// locale's first day of the week, the chips of a span and of a time, "+k more", opening a page, creating a page on a day,
// dragging a card to another day (the time kept, the end moved, two sequential writes, an error shown), the "No date" tray,
// the card's "Change date…", the keyboard, the empty states, the read-only editor and the created-time source, then the
// settings section, and at the end the calendar through the editor (the block, its toolbar and a drag into the core).

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, PropertyDef, PropertyValue, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { loadLanguage, resetLanguage, t, tp } from "../i18n";
import { createAppStore } from "../store";
import type { AppState } from "../store";
import { StoreContext } from "../store-context";
import { pid } from "../test/editor";
import { beginDrag, FakeDataTransfer, layOut } from "../test/dnd";
import { idsOf, openPage, renderPages } from "../test/page";
import { NOW } from "../test/render";
import { addDays, firstDayOfWeek, fullDate, monthGrid, monthTitle, todayIso, weekdayNames } from "./dates";
import { PageCalendarSettings, PageCalendarView } from "./PageCalendarView";
import { EMPTY_FILTER_GROUP } from "./page-list-filter-tree";
import type { PageListLayoutProps } from "./page-list-layout-types";
import type { PageListViewPatch, PageListViewSpec } from "./page-list-views";

afterEach(() => {
  resetLanguage();
  vi.restoreAllMocks();
});

const START = "de000000-0000-4000-8000-00000000d001";
const END = "de000000-0000-4000-8000-00000000d002";
const NOTES = "de000000-0000-4000-8000-00000000d003";
const OTHER = "de000000-0000-4000-8000-00000000d004";

function def(id: string, name: string, kind: PropertyDef["kind"] = "date"): PropertyDef {
  return { id, name, kind, options: [], createdAt: 0 };
}

const SCHEMA: PropertyDef[] = [def("tags", "Tags", "multi_select"), def(START, "Start"), def(END, "End"), def(OTHER, "Other date"), def(NOTES, "Notes", "text")];

function dateValue(date: string, time?: string): PropertyValue {
  return { kind: "date", value: time === undefined ? { date } : { date, time } };
}

interface PageSpec {
  start?: string;
  startTime?: string;
  end?: string;
  endTime?: string;
  notes?: string;
  createdAt?: number;
  icon?: string;
}

function page(title: string, spec: PageSpec = {}): PageInfo {
  const properties: [string, PropertyValue][] = [];
  if (spec.start !== undefined) {
    properties.push([START, dateValue(spec.start, spec.startTime)]);
  }
  if (spec.end !== undefined) {
    properties.push([END, dateValue(spec.end, spec.endTime)]);
  }
  if (spec.notes !== undefined) {
    properties.push([NOTES, { kind: "text", value: spec.notes }]);
  }
  return {
    id: `page-${title}`,
    spaceId: "space",
    title,
    createdAt: spec.createdAt ?? 0,
    updatedAt: spec.createdAt ?? 0,
    ...(spec.icon === undefined ? {} : { icon: spec.icon }),
    ...(properties.length > 0 ? { properties } : {}),
  };
}

/** Today is Wednesday 2026-10-07: in English the grid runs from Sunday 2026-09-27 to Saturday 2026-11-07. */
const TODAY = "2026-10-07";

function viewOf(dates: { dateBy?: string; endBy?: string } = { dateBy: START, endBy: END }): PageListViewSpec {
  return {
    id: "v",
    name: "",
    layout: "calendar",
    properties: [],
    sort: { by: "title", dir: "asc" },
    sorts: [{ by: "title", dir: "asc" }],
    filter: EMPTY_FILTER_GROUP,
    ...dates,
  };
}

const PAGES = [
  page("Alpha", { start: "2026-10-07" }),
  page("Beta", { start: "2026-10-07", startTime: "14:30" }),
  page("Trip", { start: "2026-10-12", startTime: "08:00", end: "2026-10-14", endTime: "17:00" }),
  page("Dec", { start: "2026-12-25" }),
  page("Loose"),
  page("Late", { end: "2026-10-09" }),
];

interface Rendered {
  user: ReturnType<typeof userEvent.setup>;
  props: PageListLayoutProps;
  setPageProperty: ReturnType<typeof vi.fn<AppState["setPageProperty"]>>;
  onOpen: ReturnType<typeof vi.fn<PageListLayoutProps["onOpen"]>>;
  onCreatePage: ReturnType<typeof vi.fn<NonNullable<PageListLayoutProps["onCreatePage"]>>>;
  rerender: (next?: Partial<PageListLayoutProps>) => void;
}

function renderCalendar(overrides: Partial<PageListLayoutProps> = {}): Rendered {
  const store = createAppStore(new CoreClient(new MemoryTransport({ now: () => NOW })));
  const setPageProperty = vi.fn<AppState["setPageProperty"]>(() => Promise.resolve());
  store.setState({ setPageProperty });
  const onOpen = vi.fn<PageListLayoutProps["onOpen"]>();
  const onCreatePage = vi.fn<NonNullable<PageListLayoutProps["onCreatePage"]>>();
  const base: PageListLayoutProps = {
    rows: PAGES,
    view: viewOf(),
    schema: SCHEMA,
    columns: [],
    spaceId: "space",
    editable: true,
    today: TODAY,
    onOpen,
    onUpdateView: vi.fn(),
    onCreatePage,
    creating: false,
  };
  let props = { ...base, ...overrides };
  const ui = (next: PageListLayoutProps) => (
    <StoreContext value={store}>
      <PageCalendarView {...next} />
    </StoreContext>
  );
  const view = render(ui(props));
  return {
    user: userEvent.setup(),
    props,
    setPageProperty,
    onOpen,
    onCreatePage,
    rerender: (next = {}) => {
      props = { ...props, ...next };
      view.rerender(ui(props));
    },
  };
}

function grid(): HTMLElement {
  return screen.getByRole("grid");
}

function cell(day: string): HTMLElement {
  const found = grid().querySelector<HTMLElement>(`[data-day="${day}"]`);
  if (found === null) {
    throw new Error(`no cell for ${day}`);
  }
  return found;
}

function hasCell(day: string): boolean {
  return grid().querySelector(`[data-day="${day}"]`) !== null;
}

/** The titles of the chips of a day, in order. */
function chipsOf(day: string): string[] {
  return Array.from(cell(day).querySelectorAll(".page-calendar-chip .page-list-name")).map((element) => element.textContent ?? "");
}

function chip(day: string, title: string): HTMLElement {
  const found = Array.from(cell(day).querySelectorAll<HTMLElement>(".page-calendar-chip")).find((element) => element.querySelector(".page-list-name")?.textContent === title);
  if (found === undefined) {
    throw new Error(`no chip ${title} on ${day}`);
  }
  return found;
}

function trayChip(title: string): HTMLElement {
  const found = Array.from(screen.getByRole("region", { name: t("pageCalendar.noDate") }).querySelectorAll<HTMLElement>(".page-calendar-chip")).find(
    (element) => element.querySelector(".page-list-name")?.textContent === title,
  );
  if (found === undefined) {
    throw new Error(`no tray chip ${title}`);
  }
  return found;
}

function trayTitles(): string[] {
  return Array.from(screen.getByRole("region", { name: t("pageCalendar.noDate") }).querySelectorAll(".page-calendar-chip .page-list-name")).map(
    (element) => element.textContent ?? "",
  );
}

function openButton(item: HTMLElement): HTMLElement {
  return item.querySelector<HTMLElement>(".page-calendar-link") as HTMLElement;
}

/** The title a bar shows. */
function barTitle(element: Element): string {
  return element.querySelector(".page-list-name")?.textContent ?? "";
}

/** Every bar of the grid (a page of several days, one bar per week row it touches), in the order of the document. */
function allBars(): HTMLElement[] {
  return Array.from(grid().querySelectorAll<HTMLElement>(".page-calendar-bar"));
}

/** The bars of a page, in the order of the document (top week row first). */
function barsOf(title: string): HTMLElement[] {
  return allBars().filter((element) => barTitle(element) === title);
}

/** The week row (`.page-calendar-weekrow`) a day is in. */
function weekRow(day: string): HTMLElement {
  return cell(day).closest<HTMLElement>(".page-calendar-weekrow") as HTMLElement;
}

/** The bar of `title` in the week row of `day`. */
function bar(title: string, day: string): HTMLElement {
  const row = weekRow(day);
  const found = barsOf(title).find((element) => row.contains(element));
  if (found === undefined) {
    throw new Error(`no bar ${title} in the week row of ${day}`);
  }
  return found;
}

/** The days a bar covers: its columns over the seven cells of its week row. */
function daysOfBar(element: HTMLElement): string[] {
  const cells = Array.from(element.closest(".page-calendar-weekrow")?.querySelectorAll<HTMLElement>("[role='gridcell']") ?? []);
  const column = Number(element.dataset.column);
  return cells.slice(column - 1, column - 1 + Number(element.dataset.span)).map((item) => item.dataset.day ?? "");
}

/** The titles of the bars of a week row, lane by lane. */
function lanesOf(day: string): string[] {
  return Array.from(weekRow(day).querySelectorAll(".page-calendar-bar")).map((element) => `${barTitle(element)}@${(element as HTMLElement).dataset.lane}`);
}

/** Gives the bars' overlay of the week row of `day` the geometry of seven 100px columns, so that a pointer's x names a day. */
function layOutRow(day: string): void {
  const overlay = weekRow(day).querySelector(".page-calendar-bars");
  if (overlay === null) {
    throw new Error(`no bars over the week of ${day}`);
  }
  overlay.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, right: 700, top: 0, bottom: 22, width: 700, height: 22, toJSON: () => ({}) });
}

/** The x of the middle of column `index` (0 to 6) of a laid-out week row. */
const columnX = (index: number): number => index * 100 + 50;

/** A drag that starts at `clientX` on `source` (on a bar, the x names the day it is grabbed on) and can go over bars and cells. */
function grab(source: Element, clientX: number) {
  const dataTransfer = new FakeDataTransfer();
  fireEvent.dragStart(source, { dataTransfer, clientX });
  return {
    /** Over a bar (the x names the day) or a cell; answers whether dropping there is allowed. */
    over(target: Element, x = 0): boolean {
      if (target.classList.contains("page-calendar-day")) {
        layOut(target);
      }
      return !fireEvent.dragOver(target, { dataTransfer, clientX: x });
    },
    drop(target: Element, x = 0): void {
      if (target.classList.contains("page-calendar-day")) {
        layOut(target);
      }
      fireEvent.drop(target, { dataTransfer, clientX: x });
      fireEvent.dragEnd(source, { dataTransfer });
    },
    end(): void {
      fireEvent.dragEnd(source, { dataTransfer });
    },
  };
}

/** A promise settled from the outside, for holding a write back. */
function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => undefined;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** Drags `source` onto the cell of `day` and drops it; answers whether the day accepted the drop. */
function dragTo(source: Element, day: string): boolean {
  const drag = beginDrag(source);
  const allowed = drag.over(cell(day), "inside");
  if (allowed) {
    drag.drop(cell(day), "inside");
  } else {
    drag.end();
  }
  return allowed;
}

describe("the grid", () => {
  it("shows the month of today as six weeks of seven days from the locale's first day, with today marked and the other months dimmed", () => {
    renderCalendar();
    expect(grid()).toHaveAccessibleName(t("pageCalendar.grid", { month: monthTitle(TODAY) }));
    expect(screen.getByText(monthTitle(TODAY))).toBeInTheDocument();
    expect(screen.getAllByRole("columnheader").map((header) => header.getAttribute("aria-label"))).toEqual(
      weekdayNames(firstDayOfWeek()).map((name) => name.long),
    );
    expect(grid().querySelectorAll("[role='row']")).toHaveLength(7); // the names, then six weeks
    expect(grid().querySelectorAll("[role='gridcell']")).toHaveLength(42);
    expect(cell("2026-09-27")).toBeInTheDocument();
    expect(cell("2026-11-07")).toBeInTheDocument();
    expect(cell(TODAY)).toHaveAttribute("aria-current", "date");
    expect(grid().querySelectorAll("[aria-current='date']")).toHaveLength(1);
    expect(cell("2026-09-30")).toHaveAttribute("data-outside");
    expect(cell("2026-11-01")).toHaveAttribute("data-outside");
    expect(cell("2026-10-15")).not.toHaveAttribute("data-outside");
  });

  it("starts the week on Monday in a locale that does", async () => {
    const { rerender } = renderCalendar();
    // After the store, which sets the language it starts with.
    await act(async () => {
      await loadLanguage("fr", () => Promise.resolve({}), ["fr-FR"]);
    });
    rerender();
    expect(screen.getAllByRole("columnheader").map((header) => header.getAttribute("aria-label"))).toEqual(
      weekdayNames(1, "fr-FR").map((name) => name.long),
    );
    expect(monthGrid("2026-10-01", 1)[0]?.[0]).toBe("2026-09-28");
    expect(grid().querySelector("[role='gridcell']")).toHaveAttribute("data-day", "2026-09-28");
    expect(grid().querySelectorAll("[role='gridcell']")[41]).toHaveAttribute("data-day", "2026-11-08");
    expect(screen.getByText(monthTitle(TODAY, "fr-FR"))).toBeInTheDocument();
  });

  it("goes to the next and the previous month and back to today", async () => {
    const { user } = renderCalendar();
    await user.click(screen.getByRole("button", { name: t("pageCalendar.next") }));
    expect(screen.getByText(monthTitle("2026-11-01"))).toBeInTheDocument();
    expect(hasCell("2026-11-30")).toBe(true);
    expect(hasCell(TODAY)).toBe(false);
    await user.click(screen.getByRole("button", { name: t("pageCalendar.next") }));
    expect(screen.getByText(monthTitle("2026-12-01"))).toBeInTheDocument();
    expect(chipsOf("2026-12-25")).toEqual(["Dec"]);
    await user.click(screen.getByRole("button", { name: t("pageCalendar.today") }));
    expect(screen.getByText(monthTitle(TODAY))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: t("pageCalendar.prev") }));
    expect(screen.getByText(monthTitle("2026-09-01"))).toBeInTheDocument();
    expect(cell("2026-09-27")).not.toHaveAttribute("data-outside");
  });

  it("draws only the days of the visible grid: the chips of other months are not in the document", async () => {
    const { user } = renderCalendar();
    expect(screen.queryByText("Dec")).toBeNull();
    expect(grid().querySelectorAll(".page-calendar-chip")).toHaveLength(3); // Alpha, Beta and the one bar of Trip; Loose and Late are in the tray
    await user.click(screen.getByRole("button", { name: t("pageCalendar.next") }));
    await user.click(screen.getByRole("button", { name: t("pageCalendar.next") }));
    expect(chipsOf("2026-12-25")).toEqual(["Dec"]);
    expect(screen.queryByText("Alpha")).toBeNull();
  });

  it("keeps one day in the tab order, and gives it to today first", () => {
    renderCalendar();
    const tabbable = Array.from(grid().querySelectorAll<HTMLElement>("[role='gridcell']")).filter((element) => element.tabIndex === 0);
    expect(tabbable.map((element) => element.dataset.day)).toEqual([TODAY]);
    // The chips and bars of the other days are reached by moving to their day first.
    expect(openButton(bar("Trip", "2026-10-12")).tabIndex).toBe(-1);
    expect(openButton(chip(TODAY, "Alpha")).tabIndex).toBe(0);
  });
});

describe("the cards of a day", () => {
  it("draws a page of several days as one bar over its days, not as a chip in each day", () => {
    renderCalendar();
    expect(chipsOf("2026-10-12")).toEqual([]);
    expect(chipsOf("2026-10-13")).toEqual([]);
    expect(chipsOf("2026-10-14")).toEqual([]);
    expect(barsOf("Trip")).toHaveLength(1);
    const trip = bar("Trip", "2026-10-12");
    expect(daysOfBar(trip)).toEqual(["2026-10-12", "2026-10-13", "2026-10-14"]);
    // The bar is not in a day's cell: it is in the overlay of its week row, over the cells.
    expect(cell("2026-10-12").contains(trip)).toBe(false);
    expect(weekRow("2026-10-12").querySelector(".page-calendar-bars")).toContainElement(trip);
  });

  it("names a bar by its whole span, marks no side as continuing when the span ends inside the week row, and shows the full title in the tooltip", () => {
    renderCalendar();
    const trip = bar("Trip", "2026-10-12");
    expect(openButton(trip)).toHaveAccessibleName(t("pageCalendar.span", { title: "Trip", from: "Oct 12, 2026", to: "Oct 14, 2026" }));
    expect(trip).toHaveAttribute("title", "Trip");
    expect(trip).not.toHaveAttribute("data-continues-before");
    expect(trip).not.toHaveAttribute("data-continues-after");
    expect(trip.querySelectorAll(".page-calendar-arrow")).toHaveLength(0);
  });

  it("shows the start's time on the first day only, as the locale writes it, and orders a day: untimed first, then by time", () => {
    renderCalendar();
    expect(chipsOf(TODAY)).toEqual(["Alpha", "Beta"]);
    expect(chip(TODAY, "Beta").querySelector(".page-calendar-time")?.textContent?.replace(/\s/gu, " ")).toBe("2:30 PM");
    expect(chip(TODAY, "Alpha").querySelector(".page-calendar-time")).toBeNull();
    expect(bar("Trip", "2026-10-12").querySelector(".page-calendar-time")?.textContent?.replace(/\s/gu, " ")).toBe("8:00 AM");
    // The end's time is never shown.
    expect(screen.queryByText(/5:00/u)).toBeNull();
  });

  it("puts the pages of several days in lanes above the single-day chips of a day", () => {
    renderCalendar({ rows: [page("Plain", { start: "2026-10-13" }), page("Timed", { start: "2026-10-13", startTime: "01:00" }), ...PAGES] });
    // The chips of the day are the single-day pages; the page of several days is a bar of the row (lane 0) over them.
    expect(chipsOf("2026-10-13")).toEqual(["Plain", "Timed"]);
    expect(lanesOf("2026-10-13")).toEqual(["Trip@0"]);
    expect(cell("2026-10-13")).toHaveAccessibleName(tp("pageCalendar.day", 3, { date: fullDate("2026-10-13") }));
  });

  it("treats an end before the start as one day", () => {
    renderCalendar({ rows: [page("Odd", { start: "2026-10-20", end: "2026-10-15" })] });
    expect(chipsOf("2026-10-20")).toEqual(["Odd"]);
    expect(chipsOf("2026-10-15")).toEqual([]);
    expect(chip("2026-10-20", "Odd")).toHaveAttribute("data-position", "single");
  });

  it("ignores the end when the view has none chosen", () => {
    renderCalendar({ view: viewOf({ dateBy: START }) });
    expect(chipsOf("2026-10-12")).toEqual(["Trip"]);
    expect(chipsOf("2026-10-13")).toEqual([]);
  });

  it("shows at most three cards, then '+k more' that opens the whole list in a popover, and opens a page from it", async () => {
    const busy = ["a", "b", "c", "d", "e"].map((title) => page(title, { start: "2026-10-20" }));
    const { user, onOpen } = renderCalendar({ rows: busy });
    expect(chipsOf("2026-10-20")).toEqual(["a", "b", "c"]);
    const more = within(cell("2026-10-20")).getByRole("button", { name: tp("pageCalendar.moreLabel", 2, { date: fullDate("2026-10-20") }) });
    expect(more).toHaveTextContent(t("pageCalendar.more", { count: 2 }));
    await user.click(more);
    const popover = await screen.findByRole("dialog", { name: tp("pageCalendar.day", 5, { date: fullDate("2026-10-20") }) });
    expect(Array.from(popover.querySelectorAll(".page-list-name")).map((element) => element.textContent)).toEqual(["a", "b", "c", "d", "e"]);
    await user.click(within(popover).getByRole("button", { name: "e" }));
    expect(onOpen).toHaveBeenCalledWith(busy[4], "current");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("gives a day with exactly three cards no '+k more'", () => {
    renderCalendar({ rows: ["a", "b", "c"].map((title) => page(title, { start: "2026-10-20" })) });
    expect(chipsOf("2026-10-20")).toEqual(["a", "b", "c"]);
    expect(within(cell("2026-10-20")).queryByRole("button", { name: /more/iu })).toBeNull();
  });

  it("names a day by its date and how many pages it holds", () => {
    renderCalendar();
    expect(cell(TODAY)).toHaveAccessibleName(tp("pageCalendar.day", 2, { date: fullDate(TODAY) }));
    expect(cell("2026-10-20")).toHaveAccessibleName(tp("pageCalendar.day", 0, { date: fullDate("2026-10-20") }));
    expect(cell("2026-10-12")).toHaveAccessibleName(tp("pageCalendar.day", 1, { date: fullDate("2026-10-12") }));
  });

  it("shows the view's properties under the title of a card", () => {
    const notes = SCHEMA.find((candidate) => candidate.id === NOTES) as PropertyDef;
    renderCalendar({ rows: [page("Alpha", { start: TODAY, notes: "Bring the slides" }), page("Beta", { start: TODAY })], columns: [notes] });
    expect(within(chip(TODAY, "Alpha")).getByRole("group", { name: "Notes" })).toHaveTextContent("Bring the slides");
    expect(within(chip(TODAY, "Beta")).queryByRole("group", { name: "Notes" })).toBeNull();
  });

  it("calls an untitled page 'Untitled'", () => {
    renderCalendar({ rows: [page("", { start: TODAY })] });
    expect(chipsOf(TODAY)).toEqual([t("page.untitled")]);
  });
});

describe("the bars of pages of several days", () => {
  /** Friday 2026-10-09 to Tuesday 2026-10-13: it crosses from the week row of Sunday 4 to the one of Sunday 11. */
  const crossing = page("Trip", { start: "2026-10-09", startTime: "08:00", end: "2026-10-13" });

  it("draws one element per week row a span touches, over the right columns", () => {
    renderCalendar({ rows: [crossing] });
    expect(barsOf("Trip")).toHaveLength(2);
    const first = bar("Trip", "2026-10-09");
    const second = bar("Trip", "2026-10-13");
    expect(daysOfBar(first)).toEqual(["2026-10-09", "2026-10-10"]);
    expect(daysOfBar(second)).toEqual(["2026-10-11", "2026-10-12", "2026-10-13"]);
    // `grid-column: start / span n` in the overlay of the week row (Sunday is the first column), in the lane's row.
    expect(first.style.gridColumn).toBe("6 / span 2");
    expect(first.style.gridRow).toBe("1");
    expect(second.style.gridColumn).toBe("1 / span 3");
    expect(second.style.gridRow).toBe("1");
    expect(first).toHaveAttribute("data-column", "6");
    expect(first).toHaveAttribute("data-span", "2");
    expect(second).toHaveAttribute("data-column", "1");
    expect(second).toHaveAttribute("data-span", "3");
    // The pages are not drawn again as chips in the cells.
    expect(grid().querySelectorAll(".page-calendar-day .page-calendar-chip")).toHaveLength(0);
  });

  it("marks the side that continues into the other week row, with a flat edge and an arrow, and names each bar", () => {
    renderCalendar({ rows: [crossing] });
    const first = bar("Trip", "2026-10-09");
    const second = bar("Trip", "2026-10-13");
    expect(first).toHaveAttribute("data-continues-after");
    expect(first).not.toHaveAttribute("data-continues-before");
    expect(second).toHaveAttribute("data-continues-before");
    expect(second).not.toHaveAttribute("data-continues-after");
    expect(Array.from(first.querySelectorAll(".page-calendar-arrow")).map((arrow) => arrow.textContent)).toEqual(["›"]);
    expect(Array.from(second.querySelectorAll(".page-calendar-arrow")).map((arrow) => arrow.textContent)).toEqual(["‹"]);
    expect(openButton(first)).toHaveAccessibleName(t("pageCalendar.span", { title: "Trip", from: "Oct 9, 2026", to: "Oct 13, 2026" }));
    expect(openButton(second)).toHaveAccessibleName(t("pageCalendar.continued", { title: "Trip" }));
    // The title is shown once per bar, whole in the tooltip (the stylesheet cuts it with an ellipsis), the time on the first only.
    expect(first).toHaveAttribute("title", "Trip");
    expect(second).toHaveAttribute("title", "Trip");
    expect(first.querySelector(".page-calendar-time")?.textContent?.replace(/\s/gu, " ")).toBe("8:00 AM");
    expect(second.querySelector(".page-calendar-time")).toBeNull();
  });

  it("puts the whole title in the tooltip of a long one", () => {
    const title = "A very long title for a trip that cannot fit in the three days of its bar";
    renderCalendar({ rows: [page(title, { start: "2026-10-13", end: "2026-10-15" })] });
    expect(bar(title, "2026-10-13")).toHaveAttribute("title", title);
    expect(bar(title, "2026-10-13").querySelector(".page-list-name")).toHaveTextContent(title);
  });

  it("marks both sides of a bar over the edges of the visible grid, and only one when the span ends inside", () => {
    renderCalendar({ rows: [page("Long", { start: "2026-09-20", end: "2026-11-20" }), page("Early", { start: "2026-09-20", end: "2026-09-29" })] });
    // The grid is six week rows, the first from Sunday 2026-09-27: the span runs through all of them.
    const long = barsOf("Long");
    expect(long).toHaveLength(6);
    for (const item of long) {
      expect(item).toHaveAttribute("data-column", "1");
      expect(item).toHaveAttribute("data-span", "7");
      expect(item).toHaveAttribute("data-continues-before");
      expect(item).toHaveAttribute("data-continues-after");
      expect(item.querySelectorAll(".page-calendar-arrow")).toHaveLength(2);
    }
    // It starts before the grid and ends in the first row: the bar covers Sunday to Tuesday and continues before only.
    const early = bar("Early", "2026-09-28");
    expect(daysOfBar(early)).toEqual(["2026-09-27", "2026-09-28", "2026-09-29"]);
    expect(early).toHaveAttribute("data-continues-before");
    expect(early).not.toHaveAttribute("data-continues-after");
    expect(barsOf("Early")).toHaveLength(1);
  });

  it("draws only the week rows of the visible grid, whatever the length of a span", () => {
    renderCalendar({ rows: [page("Epoch", { start: "2000-01-01", end: "2999-12-31" })] });
    expect(barsOf("Epoch")).toHaveLength(6);
    expect(grid().querySelectorAll(".page-calendar-chip")).toHaveLength(6);
  });

  it("lays bars out in lanes at the top of the week row: overlapping ones apart, ones that follow each other in the same lane", () => {
    renderCalendar({
      rows: [
        page("A", { start: "2026-10-13", end: "2026-10-15" }),
        page("B", { start: "2026-10-14", end: "2026-10-16" }),
        page("C", { start: "2026-10-16", end: "2026-10-17" }),
      ],
    });
    expect(lanesOf("2026-10-13")).toEqual(["A@0", "C@0", "B@1"]);
    expect(bar("A", "2026-10-13").style.gridRow).toBe("1");
    expect(bar("C", "2026-10-13").style.gridRow).toBe("1");
    expect(bar("B", "2026-10-13").style.gridRow).toBe("2");
    expect(daysOfBar(bar("C", "2026-10-16"))).toEqual(["2026-10-16", "2026-10-17"]);
    // The cells of a row keep room for its lanes; a row with no bar keeps none.
    expect(weekRow("2026-10-13").style.getPropertyValue("--calendar-lanes")).toBe("2");
    expect(weekRow("2026-10-20").style.getPropertyValue("--calendar-lanes")).toBe("0");
    expect(weekRow("2026-10-20").querySelector(".page-calendar-bars")).toBeNull();
  });

  it("keeps the single-day chips in their cell, under the lanes", () => {
    renderCalendar({ rows: [page("Span", { start: "2026-10-13", end: "2026-10-15" }), page("Solo", { start: "2026-10-14" })] });
    expect(chipsOf("2026-10-14")).toEqual(["Solo"]);
    expect(lanesOf("2026-10-14")).toEqual(["Span@0"]);
    expect(cell("2026-10-14").contains(bar("Span", "2026-10-14"))).toBe(false);
  });

  it("counts the bars passing through a day as items: '+k more', and a popover that lists everything on the day", async () => {
    const rows = [
      page("Bar1", { start: "2026-10-13", end: "2026-10-15" }),
      page("Bar2", { start: "2026-10-13", end: "2026-10-14" }),
      page("c1", { start: "2026-10-14" }),
      page("c2", { start: "2026-10-14" }),
      page("c3", { start: "2026-10-14" }),
    ];
    const { user, onOpen } = renderCalendar({ rows });
    // Two bars cross the 14th and a chip is drawn: five items, three shown, two left.
    expect(lanesOf("2026-10-14")).toEqual(["Bar1@0", "Bar2@1"]);
    expect(chipsOf("2026-10-14")).toEqual(["c1"]);
    const more = within(cell("2026-10-14")).getByRole("button", { name: tp("pageCalendar.moreLabel", 2, { date: fullDate("2026-10-14") }) });
    expect(more).toHaveTextContent(t("pageCalendar.more", { count: 2 }));
    // The days that hold at most three items have none.
    expect(within(cell("2026-10-13")).queryByRole("button", { name: /more/iu })).toBeNull();
    expect(within(cell("2026-10-15")).queryByRole("button", { name: /more/iu })).toBeNull();
    expect(cell("2026-10-14")).toHaveAccessibleName(tp("pageCalendar.day", 5, { date: fullDate("2026-10-14") }));
    await user.click(more);
    const popover = await screen.findByRole("dialog", { name: tp("pageCalendar.day", 5, { date: fullDate("2026-10-14") }) });
    expect(Array.from(popover.querySelectorAll(".page-list-name")).map((element) => element.textContent)).toEqual(["Bar1", "Bar2", "c1", "c2", "c3"]);
    // A bar's page opens from the popover like any card.
    await user.click(popover.querySelector<HTMLElement>("[data-page='page-Bar2'] .page-calendar-link") as HTMLElement);
    expect(onOpen).toHaveBeenCalledWith(rows[1], "current");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("draws three lanes at most: a fourth bar is left out and counted on the days it crosses", async () => {
    const rows = ["Bar1", "Bar2", "Bar3", "Bar4"].map((title) => page(title, { start: "2026-10-13", end: "2026-10-14" }));
    const { user } = renderCalendar({ rows });
    expect(lanesOf("2026-10-13")).toEqual(["Bar1@0", "Bar2@1", "Bar3@2"]);
    expect(barsOf("Bar4")).toHaveLength(0);
    for (const day of ["2026-10-13", "2026-10-14"]) {
      expect(within(cell(day)).getByRole("button", { name: tp("pageCalendar.moreLabel", 1, { date: fullDate(day) }) })).toHaveTextContent(t("pageCalendar.more", { count: 1 }));
    }
    await user.click(within(cell("2026-10-13")).getByRole("button", { name: /more/iu }));
    const popover = await screen.findByRole("dialog", { name: tp("pageCalendar.day", 4, { date: fullDate("2026-10-13") }) });
    expect(Array.from(popover.querySelectorAll(".page-list-name")).map((element) => element.textContent)).toEqual(["Bar1", "Bar2", "Bar3", "Bar4"]);
  });

  it("shows three bars on a day with no '+k more'", () => {
    renderCalendar({ rows: ["Bar1", "Bar2", "Bar3"].map((title) => page(title, { start: "2026-10-13", end: "2026-10-14" })) });
    expect(allBars()).toHaveLength(3);
    expect(within(cell("2026-10-13")).queryByRole("button", { name: /more/iu })).toBeNull();
  });

  it("opens the page through onOpen from any bar of it, with the disposition of the gesture, and does not create a page", async () => {
    const { user, onOpen, onCreatePage } = renderCalendar({ rows: [crossing] });
    await user.click(openButton(bar("Trip", "2026-10-09")));
    expect(onOpen).toHaveBeenLastCalledWith(crossing, "current");
    fireEvent.click(openButton(bar("Trip", "2026-10-13")), { ctrlKey: true });
    expect(onOpen).toHaveBeenLastCalledWith(crossing, "background");
    expect(onOpen).toHaveBeenCalledTimes(2);
    expect(onCreatePage).not.toHaveBeenCalled();
  });

  it("puts the bars that cross the focused day in the tab order, and the others out of it", () => {
    renderCalendar();
    expect(openButton(bar("Trip", "2026-10-12")).tabIndex).toBe(-1);
    act(() => {
      cell("2026-10-13").focus();
    });
    expect(openButton(bar("Trip", "2026-10-12")).tabIndex).toBe(0);
    act(() => {
      cell("2026-10-15").focus();
    });
    expect(openButton(bar("Trip", "2026-10-12")).tabIndex).toBe(-1);
  });

  it("opens the first page of a day on Enter, and that is a bar's when the day has one", async () => {
    const { user, onOpen } = renderCalendar();
    act(() => {
      cell("2026-10-13").focus();
    });
    await user.keyboard("{Enter}");
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[2], "current");
  });

  it("is not draggable, has no menu and takes no drop in a read-only editor", () => {
    renderCalendar({ editable: false, onCreatePage: null });
    const trip = bar("Trip", "2026-10-12");
    expect(trip).toHaveAttribute("draggable", "false");
    expect(within(trip).queryByRole("button", { name: /^Menu of/u })).toBeNull();
    expect(openButton(trip)).toBeInTheDocument();
    const over = fireEvent.dragOver(trip, { dataTransfer: new FakeDataTransfer(), clientX: 150 });
    expect(over).toBe(true);
  });

  it("makes no bars for the created and last-edited times, which are one day, whatever the dates of the page say", () => {
    const made = page("Made", { createdAt: new Date(2026, 9, 7, 12).getTime(), start: "2026-10-12", end: "2026-10-14" });
    renderCalendar({ rows: [made], view: viewOf({ dateBy: "created", endBy: END }) });
    expect(allBars()).toHaveLength(0);
    expect(chipsOf(TODAY)).toEqual(["Made"]);
  });

  describe("dragging a bar", () => {
    it("moves the whole span from a bar of another week row, keeping its length and its time", async () => {
      const { setPageProperty } = renderCalendar({ rows: [crossing] });
      layOutRow("2026-10-12");
      // The second bar (Sunday 11 to Tuesday 13), grabbed on Monday the 12th and dropped on Tuesday the 20th: eight days later.
      const drag = grab(bar("Trip", "2026-10-13"), columnX(1));
      const target = cell("2026-10-20");
      expect(drag.over(target)).toBe(true);
      drag.drop(target);
      await waitFor(() => {
        expect(setPageProperty).toHaveBeenCalledTimes(2);
      });
      expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-17", "08:00"));
      expect(setPageProperty).toHaveBeenNthCalledWith(2, "page-Trip", END, dateValue("2026-10-21"));
      expect(await screen.findByText(t("pageCalendar.moved", { title: "Trip", date: "Oct 20, 2026" }))).toBeInTheDocument();
    });

    it("moves the whole span from the first bar too, measured from the day under the pointer", async () => {
      const { setPageProperty } = renderCalendar({ rows: [crossing] });
      layOutRow("2026-10-09");
      const drag = grab(bar("Trip", "2026-10-09"), columnX(6)); // Saturday the 10th
      drag.drop(cell("2026-10-11")); // the next day
      await waitFor(() => {
        expect(setPageProperty).toHaveBeenCalledTimes(2);
      });
      expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-10", "08:00"));
      expect(setPageProperty).toHaveBeenNthCalledWith(2, "page-Trip", END, dateValue("2026-10-14"));
    });

    it("marks every bar of the page while it is dragged, and none after", () => {
      renderCalendar({ rows: [crossing] });
      const drag = grab(bar("Trip", "2026-10-09"), 0);
      expect(barsOf("Trip").map((item) => item.hasAttribute("data-dragging"))).toEqual([true, true]);
      drag.end();
      expect(barsOf("Trip").map((item) => item.hasAttribute("data-dragging"))).toEqual([false, false]);
    });

    it("finds the day under a card dragged over a bar from the pointer, marks it, and drops there", async () => {
      const { setPageProperty } = renderCalendar();
      layOutRow("2026-10-12");
      const trip = bar("Trip", "2026-10-12");
      const drag = grab(chip(TODAY, "Alpha"), 0);
      expect(drag.over(trip, columnX(2))).toBe(true); // Tuesday the 13th
      expect(cell("2026-10-13")).toHaveAttribute("data-over");
      expect(drag.over(trip, columnX(3))).toBe(true); // Wednesday the 14th
      expect(cell("2026-10-13")).not.toHaveAttribute("data-over");
      expect(cell("2026-10-14")).toHaveAttribute("data-over");
      drag.drop(trip, columnX(3));
      expect(cell("2026-10-14")).not.toHaveAttribute("data-over");
      await waitFor(() => {
        expect(setPageProperty).toHaveBeenCalledTimes(1);
      });
      expect(setPageProperty).toHaveBeenCalledWith("page-Alpha", START, dateValue("2026-10-14"));
    });

    it("lets a bar be dropped on its own days", async () => {
      const { setPageProperty } = renderCalendar();
      layOutRow("2026-10-12");
      const trip = bar("Trip", "2026-10-12");
      const drag = grab(trip, columnX(1)); // Monday the 12th
      expect(drag.over(trip, columnX(2))).toBe(true);
      drag.drop(trip, columnX(2)); // Tuesday the 13th: one day later
      await waitFor(() => {
        expect(setPageProperty).toHaveBeenCalledTimes(2);
      });
      expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-13", "08:00"));
      expect(setPageProperty).toHaveBeenNthCalledWith(2, "page-Trip", END, dateValue("2026-10-15", "17:00"));
    });

    it("clears the mark of the day when the card leaves the bars", () => {
      renderCalendar();
      layOutRow("2026-10-12");
      const trip = bar("Trip", "2026-10-12");
      const drag = grab(chip(TODAY, "Alpha"), 0);
      drag.over(trip, columnX(2));
      expect(cell("2026-10-13")).toHaveAttribute("data-over");
      fireEvent.dragLeave(trip, { relatedTarget: document.body });
      expect(cell("2026-10-13")).not.toHaveAttribute("data-over");
      drag.end();
    });

    it("does not take a drag over a bar that is not a card of the calendar", () => {
      renderCalendar();
      layOutRow("2026-10-12");
      expect(fireEvent.dragOver(bar("Trip", "2026-10-12"), { dataTransfer: new FakeDataTransfer(), clientX: columnX(2) })).toBe(true);
    });
  });
});

describe("opening a page", () => {
  it("opens it through onOpen with the disposition of the gesture, and never itself", async () => {
    const { user, onOpen } = renderCalendar();
    const alpha = openButton(chip(TODAY, "Alpha"));
    await user.click(alpha);
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "current");
    fireEvent.click(alpha, { ctrlKey: true });
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "background");
    fireEvent.click(alpha, { ctrlKey: true, shiftKey: true });
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "foreground");
    fireEvent(alpha, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "background");
    expect(onOpen).toHaveBeenCalledTimes(4);
  });

  it("opens a page of the tray, and a page of several days from its bar", async () => {
    const { user, onOpen } = renderCalendar();
    await user.click(openButton(trayChip("Loose")));
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[4], "current");
    await user.click(openButton(bar("Trip", "2026-10-13")));
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[2], "current");
  });

  it("does not create a page when a card is clicked", async () => {
    const { user, onCreatePage } = renderCalendar();
    await user.click(openButton(chip(TODAY, "Alpha")));
    expect(onCreatePage).not.toHaveBeenCalled();
  });
});

describe("creating a page on a day", () => {
  const dated = (day: string) => ({ values: [[START, { kind: "date", value: { date: day } }]] });

  it("calls onCreatePage with the day as the date property's value when the day's + is pressed", async () => {
    const { user, onCreatePage } = renderCalendar();
    await user.click(within(cell("2026-10-20")).getByRole("button", { name: t("pageCalendar.newOn", { date: fullDate("2026-10-20") }) }));
    expect(onCreatePage).toHaveBeenCalledTimes(1);
    expect(onCreatePage).toHaveBeenCalledWith(dated("2026-10-20"));
  });

  it("does the same for a click on the empty area of a day, and not for a click on its cards", async () => {
    const { user, onCreatePage } = renderCalendar();
    await user.click(cell("2026-10-21"));
    expect(onCreatePage).toHaveBeenLastCalledWith(dated("2026-10-21"));
    await user.click(cell(TODAY).querySelector(".page-calendar-day-number") as HTMLElement);
    expect(onCreatePage).toHaveBeenLastCalledWith(dated(TODAY));
    onCreatePage.mockClear();
    await user.click(chip(TODAY, "Alpha"));
    await user.click(within(chip(TODAY, "Alpha")).getByRole("button", { name: t("pageCalendar.cardMenu", { title: "Alpha" }) }));
    expect(onCreatePage).not.toHaveBeenCalled();
  });

  it("is not offered while a page is being made, nor without onCreatePage", async () => {
    const { user, onCreatePage } = renderCalendar({ creating: true });
    const add = within(cell("2026-10-20")).getByRole("button", { name: t("pageCalendar.newOn", { date: fullDate("2026-10-20") }) });
    expect(add).toBeDisabled();
    await user.click(cell("2026-10-21"));
    expect(onCreatePage).not.toHaveBeenCalled();
  });

  it("has no + when onCreatePage is null", () => {
    renderCalendar({ onCreatePage: null });
    expect(screen.queryByRole("button", { name: /^New page on/u })).toBeNull();
  });
});

describe("dragging a card to another day", () => {
  it("sets the start on the drop day, keeping its time", async () => {
    const { setPageProperty } = renderCalendar();
    const beta = chip(TODAY, "Beta");
    expect(beta).toHaveAttribute("draggable", "true");
    expect(dragTo(beta, "2026-10-10")).toBe(true);
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(1);
    });
    expect(setPageProperty).toHaveBeenCalledWith("page-Beta", START, dateValue("2026-10-10", "14:30"));
    expect(await screen.findByText(t("pageCalendar.moved", { title: "Beta", date: "Oct 10, 2026" }))).toBeInTheDocument();
  });

  it("moves the end by the same days, after the start has been written", async () => {
    const first = deferred();
    const second = deferred();
    const { setPageProperty } = renderCalendar();
    setPageProperty.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    dragTo(bar("Trip", "2026-10-12"), "2026-10-19");
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(1);
    });
    expect(setPageProperty).toHaveBeenLastCalledWith("page-Trip", START, dateValue("2026-10-19", "08:00"));
    // The end waits for the start.
    await act(async () => {
      await Promise.resolve();
    });
    expect(setPageProperty).toHaveBeenCalledTimes(1);
    first.resolve();
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(2);
    });
    expect(setPageProperty).toHaveBeenLastCalledWith("page-Trip", END, dateValue("2026-10-21", "17:00"));
    second.resolve();
    expect(await screen.findByText(t("pageCalendar.moved", { title: "Trip", date: "Oct 19, 2026" }))).toBeInTheDocument();
  });

  it("measures from the day the bar was grabbed on, which can be a later day of its span", async () => {
    const { setPageProperty } = renderCalendar();
    layOutRow("2026-10-12"); // the week row of Sunday 11 to Saturday 17: Wednesday the 14th is the fourth column
    const drag = grab(bar("Trip", "2026-10-12"), columnX(3));
    const target = cell("2026-10-15");
    expect(drag.over(target)).toBe(true);
    drag.drop(target);
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(2);
    });
    // Grabbed on the last day and dropped a day later: the whole span moves one day, keeping its length and its times.
    expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-13", "08:00"));
    expect(setPageProperty).toHaveBeenNthCalledWith(2, "page-Trip", END, dateValue("2026-10-15", "17:00"));
  });

  it("takes the first day of a bar as the day grabbed when the pointer has no place (no layout)", async () => {
    const { setPageProperty } = renderCalendar();
    dragTo(bar("Trip", "2026-10-13"), "2026-10-13"); // dropped on its second day, grabbed on its first: one day later
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(2);
    });
    expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-13", "08:00"));
  });

  it("writes nothing for a drop on the day it was grabbed on", async () => {
    const { setPageProperty } = renderCalendar();
    layOutRow("2026-10-12");
    const drag = grab(bar("Trip", "2026-10-12"), columnX(2)); // Tuesday the 13th
    drag.drop(cell("2026-10-13"));
    await act(async () => {
      await Promise.resolve();
    });
    expect(setPageProperty).not.toHaveBeenCalled();
  });

  it("marks the day under the card and clears the mark when the card leaves or is dropped", () => {
    renderCalendar();
    const drag = beginDrag(chip(TODAY, "Alpha"));
    drag.over(cell("2026-10-09"), "inside");
    expect(cell("2026-10-09")).toHaveAttribute("data-over");
    expect(chip(TODAY, "Alpha")).toHaveAttribute("data-dragging");
    drag.leave(cell("2026-10-09"));
    expect(cell("2026-10-09")).not.toHaveAttribute("data-over");
    drag.over(cell("2026-10-10"), "inside");
    drag.end();
    expect(cell("2026-10-10")).not.toHaveAttribute("data-over");
    expect(chip(TODAY, "Alpha")).not.toHaveAttribute("data-dragging");
  });

  it("shows the error of a write that failed, also the end's", async () => {
    const { setPageProperty } = renderCalendar();
    setPageProperty.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("the core said no"));
    dragTo(bar("Trip", "2026-10-12"), "2026-10-13");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("pageCalendar.error.move", { message: "the core said no" }));
    expect(setPageProperty).toHaveBeenCalledTimes(2);
    expect(screen.queryByText(t("pageCalendar.moved", { title: "Trip", date: "Oct 13, 2026" }))).toBeNull();
  });

  it("does not take a drag that is not a card of the calendar", () => {
    renderCalendar();
    const over = fireEvent.dragOver(cell("2026-10-10"), { dataTransfer: { dropEffect: "none", getData: () => "" } });
    expect(over).toBe(true); // not prevented: no drop here
  });

  it("keeps the drag away from the editor around the block", () => {
    const heard = vi.fn();
    const outer = document.createElement("div");
    outer.addEventListener("dragstart", heard);
    outer.addEventListener("drop", heard);
    document.body.append(outer);
    const host = document.createElement("div");
    host.className = "page-list-node";
    outer.append(host);
    const store = createAppStore(new CoreClient(new MemoryTransport({ now: () => NOW })));
    store.setState({ setPageProperty: vi.fn(() => Promise.resolve()) });
    render(
      <StoreContext value={store}>
        <PageCalendarView
          rows={PAGES}
          view={viewOf()}
          schema={SCHEMA}
          columns={[]}
          spaceId="space"
          editable
          today={TODAY}
          onOpen={vi.fn()}
          onUpdateView={vi.fn()}
          onCreatePage={null}
          creating={false}
        />
      </StoreContext>,
      { container: host },
    );
    dragTo(chip(TODAY, "Alpha"), "2026-10-09");
    expect(heard).not.toHaveBeenCalled();
    outer.remove();
  });
});

describe("the No date tray", () => {
  it("lists the pages with no start, even one that has an end", () => {
    renderCalendar();
    expect(trayTitles()).toEqual(["Loose", "Late"]);
    expect(within(screen.getByRole("region", { name: t("pageCalendar.noDate") })).getByText("2")).toBeInTheDocument();
    expect(screen.getByText(t("pageCalendar.noDate.hint"))).toBeInTheDocument();
  });

  it("gives a page a date when it is dropped on a day, leaving its end value as it is", async () => {
    const { setPageProperty } = renderCalendar();
    expect(dragTo(trayChip("Late"), "2026-10-22")).toBe(true);
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(1);
    });
    expect(setPageProperty).toHaveBeenCalledWith("page-Late", START, dateValue("2026-10-22"));
  });

  it("says that every page has a date when the tray is empty", () => {
    renderCalendar({ rows: [page("Alpha", { start: TODAY })] });
    expect(screen.getByText(t("pageCalendar.noDate.empty"))).toBeInTheDocument();
    expect(screen.queryByText(t("pageCalendar.noDate.hint"))).toBeNull();
  });

  it("shows the first hundred pages and the rest on request", async () => {
    const many = Array.from({ length: 130 }, (_, index) => page(`p${String(index).padStart(3, "0")}`));
    const { user } = renderCalendar({ rows: many });
    expect(trayTitles()).toHaveLength(100);
    const rest = within(screen.getByRole("region", { name: t("pageCalendar.noDate") })).getByRole("button", { name: t("pageCalendar.more", { count: 30 }) });
    await user.click(rest);
    expect(trayTitles()).toHaveLength(130);
    expect(within(screen.getByRole("region", { name: t("pageCalendar.noDate") })).queryByRole("button", { name: /more/iu })).toBeNull();
  });
});

describe("Change date… in the card's menu", () => {
  async function openPicker(user: Rendered["user"], title: string, card: HTMLElement) {
    await user.click(within(card).getByRole("button", { name: t("pageCalendar.cardMenu", { title }) }));
    const menu = await screen.findByRole("menu", { name: t("pageCalendar.cardMenu", { title }) });
    await user.click(within(menu).getByRole("menuitem", { name: t("pageCalendar.changeDate") }));
    return screen.findByRole("dialog", { name: t("date.pickerTitle") });
  }

  it("lists the one command and opens the date picker on the page's date, which writes the new day", async () => {
    const { user, setPageProperty } = renderCalendar();
    const picker = await openPicker(user, "Beta", chip(TODAY, "Beta"));
    expect(picker.querySelector(`[data-date="${TODAY}"]`)?.closest("td")).toHaveAttribute("aria-selected", "true");
    await user.click(within(picker).getByRole("button", { name: fullDate("2026-10-20") }));
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(1);
    });
    // The time is kept.
    expect(setPageProperty).toHaveBeenCalledWith("page-Beta", START, dateValue("2026-10-20", "14:30"));
    expect(await screen.findByText(t("pageCalendar.moved", { title: "Beta", date: "Oct 20, 2026" }))).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("date.pickerTitle") })).toBeNull();
    });
  });

  it("is on a bar too, and moves the end by the same days", async () => {
    const { user, setPageProperty } = renderCalendar();
    const picker = await openPicker(user, "Trip", bar("Trip", "2026-10-12"));
    await user.click(within(picker).getByRole("button", { name: fullDate("2026-10-15") }));
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledTimes(2);
    });
    expect(setPageProperty).toHaveBeenNthCalledWith(1, "page-Trip", START, dateValue("2026-10-15", "08:00"));
    expect(setPageProperty).toHaveBeenNthCalledWith(2, "page-Trip", END, dateValue("2026-10-17", "17:00"));
  });

  it("is on every card, also of the tray, and also in the '+k more' list", async () => {
    const busy = ["a", "b", "c", "d"].map((title) => page(title, { start: "2026-10-20" }));
    const { user, setPageProperty } = renderCalendar({ rows: [...busy, page("Loose")] });
    expect(within(trayChip("Loose")).getByRole("button", { name: t("pageCalendar.cardMenu", { title: "Loose" }) })).toBeInTheDocument();
    await user.click(within(cell("2026-10-20")).getByRole("button", { name: tp("pageCalendar.moreLabel", 1, { date: fullDate("2026-10-20") }) }));
    const popover = await screen.findByRole("dialog", { name: /4 pages/u });
    await user.click(within(popover).getByRole("button", { name: t("pageCalendar.cardMenu", { title: "d" }) }));
    const menu = await screen.findByRole("menu");
    await user.click(within(menu).getByRole("menuitem", { name: t("pageCalendar.changeDate") }));
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    await user.click(within(picker).getByRole("button", { name: fullDate("2026-10-21") }));
    await waitFor(() => {
      expect(setPageProperty).toHaveBeenCalledWith("page-d", START, dateValue("2026-10-21"));
    });
  });
});

describe("the keyboard", () => {
  it("moves between days with the arrows, a month with Page Up and Page Down, and the week with Home and End", async () => {
    const { user } = renderCalendar();
    act(() => {
      cell(TODAY).focus();
    });
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(cell("2026-10-08"));
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toBe(cell("2026-10-15"));
    await user.keyboard("{ArrowLeft}{ArrowUp}");
    expect(document.activeElement).toBe(cell("2026-10-07"));
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(cell("2026-10-04")); // Sunday
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(cell("2026-10-10")); // Saturday
    await user.keyboard("{PageDown}");
    expect(screen.getByText(monthTitle("2026-11-01"))).toBeInTheDocument();
    expect(document.activeElement).toBe(cell("2026-11-10"));
    await user.keyboard("{PageUp}{PageUp}");
    expect(screen.getByText(monthTitle("2026-09-01"))).toBeInTheDocument();
    expect(document.activeElement).toBe(cell("2026-09-10"));
  });

  it("changes the month when an arrow leaves the grid, and keeps one day in the tab order", async () => {
    const { user } = renderCalendar();
    act(() => {
      cell("2026-11-07").focus();
    });
    await user.keyboard("{ArrowRight}");
    expect(screen.getByText(monthTitle("2026-11-01"))).toBeInTheDocument();
    expect(document.activeElement).toBe(cell("2026-11-08"));
    const tabbable = Array.from(grid().querySelectorAll<HTMLElement>("[role='gridcell']")).filter((element) => element.tabIndex === 0);
    expect(tabbable.map((element) => element.dataset.day)).toEqual(["2026-11-08"]);
  });

  it("opens the first card of the day on Enter (Ctrl+Enter in front), and makes a page when the day is empty", async () => {
    const { user, onOpen, onCreatePage } = renderCalendar();
    act(() => {
      cell(TODAY).focus();
    });
    await user.keyboard("{Enter}");
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "current");
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onOpen).toHaveBeenLastCalledWith(PAGES[0], "foreground");
    await user.keyboard("{ArrowRight}{Enter}");
    expect(onCreatePage).toHaveBeenCalledWith({ values: [[START, { kind: "date", value: { date: "2026-10-08" } }]] });
  });

  it("leaves the keys of a card's own buttons alone", async () => {
    const { user, onOpen, onCreatePage } = renderCalendar();
    act(() => {
      openButton(bar("Trip", "2026-10-12")).focus();
    });
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(openButton(bar("Trip", "2026-10-12")));
    await user.keyboard("{Enter}");
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onCreatePage).not.toHaveBeenCalled();
  });

  it("does not make a page on Enter in a read-only calendar", async () => {
    const { user, onCreatePage } = renderCalendar({ editable: false, onCreatePage: null });
    act(() => {
      cell("2026-10-08").focus();
    });
    await user.keyboard("{Enter}");
    expect(onCreatePage).not.toHaveBeenCalled();
  });
});

describe("what it asks to choose", () => {
  it("asks for a date property while `dateBy` is absent, and draws no grid", () => {
    renderCalendar({ view: viewOf({}) });
    expect(screen.getByRole("status")).toHaveTextContent(t("pageCalendar.choose"));
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("says so when the space has no date property at all, but still offers the built-in times through the text", () => {
    renderCalendar({ view: viewOf({}), schema: [def("tags", "Tags", "multi_select"), def(NOTES, "Notes", "text")] });
    expect(screen.getByRole("status")).toHaveTextContent(t("pageCalendar.noDateProperty"));
  });

  it("asks to choose, and does not claim there is no property, while the schema is still loading", () => {
    renderCalendar({ view: viewOf({}), schema: [] });
    expect(screen.getByRole("status")).toHaveTextContent(t("pageCalendar.choose"));
  });

  it("says that the date property is gone, or is not a date property", () => {
    const { rerender } = renderCalendar({ view: viewOf({ dateBy: "deleted-id" }) });
    expect(screen.getByRole("status")).toHaveTextContent(t("pageCalendar.dateGone"));
    rerender({ view: viewOf({ dateBy: NOTES }) });
    expect(screen.getByRole("status")).toHaveTextContent(t("pageCalendar.dateKind", { name: "Notes" }));
    rerender({ view: viewOf({ dateBy: START }) });
    expect(screen.getByRole("grid")).toBeInTheDocument();
  });
});

describe("a read-only editor", () => {
  it("shows everything and offers no write: nothing draggable, no +, no menu, no click that creates", async () => {
    const { user, onOpen, onCreatePage, setPageProperty } = renderCalendar({ editable: false, onCreatePage: null });
    expect(chipsOf(TODAY)).toEqual(["Alpha", "Beta"]);
    expect(trayTitles()).toEqual(["Loose", "Late"]);
    expect(document.querySelectorAll("[draggable='true']")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: /^New page on/u })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Menu of/u })).toBeNull();
    expect(screen.queryByText(t("pageCalendar.noDate.hint"))).toBeNull();
    await user.click(cell("2026-10-21"));
    expect(onCreatePage).not.toHaveBeenCalled();
    // Opening works, and the month can be changed.
    await user.click(openButton(chip(TODAY, "Alpha")));
    expect(onOpen).toHaveBeenCalledWith(PAGES[0], "current");
    await user.click(screen.getByRole("button", { name: t("pageCalendar.next") }));
    expect(screen.getByText(monthTitle("2026-11-01"))).toBeInTheDocument();
    expect(setPageProperty).not.toHaveBeenCalled();
  });

  it("does not accept a drop: a day gives no drop target", () => {
    renderCalendar({ editable: false, onCreatePage: null });
    const over = fireEvent.dragOver(cell("2026-10-09"), { dataTransfer: { dropEffect: "none" } });
    expect(over).toBe(true);
  });
});

describe("the created and last-edited sources", () => {
  const day = (d: number, hour: number): number => new Date(2026, 9, d, hour, 15).getTime();
  const rows = [page("Made", { createdAt: day(7, 23) }), page("Early", { createdAt: day(7, 1), start: "2026-10-30" }), page("Next", { createdAt: day(8, 0) })];

  it("places each page on the local day of its time, without a time on the chip, and with no tray", () => {
    renderCalendar({ rows, view: viewOf({ dateBy: "created" }) });
    expect(chipsOf(TODAY)).toEqual(["Made", "Early"]);
    expect(chipsOf("2026-10-08")).toEqual(["Next"]);
    expect(chipsOf("2026-10-30")).toEqual([]);
    expect(document.querySelector(".page-calendar-time")).toBeNull();
    expect(screen.queryByRole("region", { name: t("pageCalendar.noDate") })).toBeNull();
  });

  it("cannot be moved, and cannot create: no drag, no menu, no +, and says why", async () => {
    const { user, onOpen, onCreatePage, setPageProperty } = renderCalendar({ rows, view: viewOf({ dateBy: "created" }) });
    expect(screen.getByText(t("pageCalendar.fixedSource"))).toBeInTheDocument();
    expect(document.querySelectorAll("[draggable='true']")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: /^Menu of/u })).toBeNull();
    expect(screen.queryByRole("button", { name: /^New page on/u })).toBeNull();
    await user.click(cell("2026-10-21"));
    expect(onCreatePage).not.toHaveBeenCalled();
    expect(dragTo(chip(TODAY, "Made"), "2026-10-10")).toBe(false);
    expect(setPageProperty).not.toHaveBeenCalled();
    // Opening still works.
    await user.click(openButton(chip(TODAY, "Made")));
    expect(onOpen).toHaveBeenCalledWith(rows[0], "current");
  });

  it("uses the last-edited time for 'updated'", () => {
    renderCalendar({ rows: [{ ...page("Edited"), updatedAt: day(12, 9) }], view: viewOf({ dateBy: "updated" }) });
    expect(chipsOf("2026-10-12")).toEqual(["Edited"]);
  });
});

describe("the settings section", () => {
  function renderSettings(view: PageListViewSpec, schema: readonly PropertyDef[] = SCHEMA) {
    const onUpdateView = vi.fn<(patch: PageListViewPatch) => void>();
    const user = userEvent.setup();
    const result = render(<PageCalendarSettings view={view} schema={schema} onUpdateView={onUpdateView} />);
    return { user, onUpdateView, ...result };
  }

  function dateSelect(): HTMLSelectElement {
    return screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageCalendar.dateBy") });
  }

  it("preselects nothing, not even visually, and writes nothing until a choice", () => {
    const { onUpdateView } = renderSettings(viewOf({}));
    expect(dateSelect().value).toBe("");
    expect(dateSelect().selectedOptions[0]).toHaveTextContent(t("pageCalendar.dateBy.choose"));
    expect(dateSelect().selectedOptions[0]).toBeDisabled();
    expect(screen.queryByRole("combobox", { name: t("pageCalendar.endBy") })).toBeNull();
    expect(onUpdateView).not.toHaveBeenCalled();
  });

  it("shows a visible label above each select, as the other settings do", () => {
    renderSettings(viewOf({ dateBy: START }));
    for (const key of ["pageCalendar.dateBy", "pageCalendar.endBy"] as const) {
      const select = screen.getByRole("combobox", { name: t(key) });
      const field = select.closest("label.page-list-field");
      expect(field).not.toBeNull();
      expect(field?.querySelector(".page-list-field-label")).toHaveTextContent(t(key));
      expect(select).not.toHaveAttribute("aria-label");
    }
  });

  it("lists the Date properties of the space, then the created and last-edited times", () => {
    renderSettings(viewOf({}));
    expect(Array.from(dateSelect().options).map((option) => [option.value, option.textContent])).toEqual([
      ["", t("pageCalendar.dateBy.choose")],
      [START, "Start"],
      [END, "End"],
      [OTHER, "Other date"],
      ["created", t("pageCalendar.dateBy.created")],
      ["updated", t("pageCalendar.dateBy.updated")],
    ]);
  });

  it("offers the built-in times even when the space has no Date property", () => {
    renderSettings(viewOf({}), [def("tags", "Tags", "multi_select")]);
    expect(Array.from(dateSelect().options).map((option) => option.value)).toEqual(["", "created", "updated"]);
  });

  it("writes `dateBy` when one is chosen", async () => {
    const { user, onUpdateView } = renderSettings(viewOf({}));
    await user.selectOptions(dateSelect(), "Start");
    expect(onUpdateView).toHaveBeenCalledWith({ dateBy: START });
    await user.selectOptions(dateSelect(), t("pageCalendar.dateBy.updated"));
    expect(onUpdateView).toHaveBeenLastCalledWith({ dateBy: "updated" });
  });

  it("shows the chosen source, and the end date select only for a Date property", () => {
    const { rerender } = renderSettings(viewOf({ dateBy: START }));
    expect(dateSelect().value).toBe(START);
    const end = screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageCalendar.endBy") });
    expect(end.value).toBe("");
    // The end lists the other Date properties and "No end date" (the start itself is no end).
    expect(Array.from(end.options).map((option) => [option.value, option.textContent])).toEqual([
      ["", t("pageCalendar.endBy.none")],
      [END, "End"],
      [OTHER, "Other date"],
    ]);
    rerender(<PageCalendarSettings view={viewOf({ dateBy: "created", endBy: END })} schema={SCHEMA} onUpdateView={vi.fn()} />);
    expect(dateSelect().value).toBe("created");
    expect(screen.queryByRole("combobox", { name: t("pageCalendar.endBy") })).toBeNull();
  });

  it("writes `endBy` when an end is chosen, and removes it for 'No end date'", async () => {
    const first = renderSettings(viewOf({ dateBy: START }));
    await first.user.selectOptions(screen.getByRole("combobox", { name: t("pageCalendar.endBy") }), "End");
    expect(first.onUpdateView).toHaveBeenLastCalledWith({ endBy: END });
    first.unmount();
    const second = renderSettings(viewOf({ dateBy: START, endBy: END }));
    expect(screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageCalendar.endBy") }).value).toBe(END);
    await second.user.selectOptions(screen.getByRole("combobox", { name: t("pageCalendar.endBy") }), t("pageCalendar.endBy.none"));
    expect(second.onUpdateView).toHaveBeenLastCalledWith({ endBy: null });
  });

  it("shows nothing chosen for a date property that is gone or of another kind, and keeps the attribute (it writes nothing)", () => {
    const gone = renderSettings(viewOf({ dateBy: "deleted-id" }));
    expect(dateSelect().value).toBe("");
    expect(screen.queryByRole("combobox", { name: t("pageCalendar.endBy") })).toBeNull();
    expect(gone.onUpdateView).not.toHaveBeenCalled();
    gone.unmount();
    renderSettings(viewOf({ dateBy: NOTES }));
    expect(dateSelect().value).toBe("");
  });

  it("shows 'No end date' for an end that is not a Date property of the space, and keeps what is stored (it writes nothing)", () => {
    const { onUpdateView } = renderSettings(viewOf({ dateBy: START, endBy: "deleted-id" }));
    expect(screen.getByRole<HTMLSelectElement>("combobox", { name: t("pageCalendar.endBy") }).value).toBe("");
    expect(onUpdateView).not.toHaveBeenCalled();
  });
});

// ----- through the editor ---------------------------------------------------------------------------------

const DUE = "de000000-0000-4000-8000-00000000e001";
const UNTIL = "de000000-0000-4000-8000-00000000e002";

/** The month the real clock is in (the node view's "today" is the device's): days 10, 12 to 14 and 20 of it exist in every month. */
const MONTH = todayIso().slice(0, 7);
const dayOf = (n: number): string => `${MONTH}-${String(n).padStart(2, "0")}`;

function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      {
        type: "pageList",
        attrs: {
          id: pid(2),
          source: { kind: "children" },
          layout: "calendar",
          properties: [],
          sort: { by: "title", dir: "asc" },
          views: [{ id: "cal", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" }, dateBy: DUE }],
          ...list,
        },
      },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

function seed(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    properties: [
      { id: DUE, name: "Due", kind: "date" },
      { id: UNTIL, name: "Until", kind: "date" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", properties: { Due: { date: dayOf(10) } } },
          { title: "Beta", properties: { Due: { date: dayOf(12), time: "09:30" }, Until: { date: dayOf(14) } } },
          { title: "Gamma" },
        ],
      },
    ],
  };
}

function attrsOf(editor: Editor): Record<string, unknown> {
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageList") {
      found = node.attrs;
    }
    return true;
  });
  return found;
}

describe("the calendar in the editor", () => {
  async function openHub(content: JsonValue = hubContent()) {
    const app = await renderPages([seed(content)]);
    const editor = await openPage(app.user, "Hub");
    await screen.findByRole("group", { name: t("pageList.label") });
    return { ...app, editor, client: new CoreClient(app.memory), ids: await idsOf(app.memory) };
  }

  /** One of the calendar's own selects, wherever the toolbar keeps them: in the toolbar, or in the view settings menu, opened if need be. */
  async function settingsSelect(user: Rendered["user"], name: string): Promise<HTMLSelectElement> {
    const inline = screen.queryByRole<HTMLSelectElement>("combobox", { name });
    if (inline !== null) {
      return inline;
    }
    await user.click(within(screen.getByRole("toolbar", { name: t("pageList.toolbar") })).getByRole("button", { name: t("pageList.viewSettings") }));
    return screen.findByRole<HTMLSelectElement>("combobox", { name });
  }

  it("draws the pages on their dates, the undated ones in the tray, and the settings in the toolbar", async () => {
    const { user } = await openHub();
    await waitFor(() => {
      expect(chipsOf(dayOf(10))).toEqual(["Alpha"]);
    });
    expect(chipsOf(dayOf(12))).toEqual(["Beta"]);
    expect(trayTitles()).toEqual(["Gamma"]);
    expect((await settingsSelect(user, t("pageCalendar.dateBy"))).value).toBe(DUE);
    expect(await settingsSelect(user, t("pageCalendar.endBy"))).toBeInTheDocument();
  });

  it("writes the end date through the view, and then draws the page over its days", async () => {
    const { user, editor } = await openHub();
    await waitFor(() => {
      expect(chipsOf(dayOf(12))).toEqual(["Beta"]);
    });
    expect(chipsOf(dayOf(13))).toEqual([]);
    await user.selectOptions(await settingsSelect(user, t("pageCalendar.endBy")), "Until");
    await waitFor(() => {
      expect((attrsOf(editor).views as { endBy?: string }[])[0]?.endBy).toBe(UNTIL);
    });
    // One bar over its three days (two, when a week boundary falls inside them), no chip in the cells.
    await waitFor(() => {
      expect(barsOf("Beta").flatMap(daysOfBar)).toEqual([dayOf(12), dayOf(13), dayOf(14)]);
    });
    expect(chipsOf(dayOf(12))).toEqual([]);
    expect(chipsOf(dayOf(13))).toEqual([]);
  });

  it("reschedules a bar by drag and drop into the core: the start and the end move together, the time kept", async () => {
    const { client, ids } = await openHub(
      hubContent({ views: [{ id: "cal", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" }, dateBy: DUE, endBy: UNTIL }] }),
    );
    await waitFor(() => {
      expect(barsOf("Beta").flatMap(daysOfBar)).toEqual([dayOf(12), dayOf(13), dayOf(14)]);
    });
    // Grabbed on its first day (the first bar's first day, as no layout names a day here), dropped on the 20th: eight days later.
    const first = barsOf("Beta")[0] as HTMLElement;
    expect(daysOfBar(first)[0]).toBe(dayOf(12));
    dragTo(first, dayOf(20));
    await waitFor(() => {
      expect(barsOf("Beta").flatMap(daysOfBar)).toEqual([dayOf(20), dayOf(21), dayOf(22)]);
    });
    const values = await client.pagePropertiesGet(ids.page("Beta"));
    expect(values.find(([id]) => id === DUE)?.[1]).toEqual(dateValue(dayOf(20), "09:30"));
    expect(values.find(([id]) => id === UNTIL)?.[1]).toEqual(dateValue(dayOf(22)));
  });

  it("reschedules a card by drag and drop into the core, keeping its time, and the card moves", async () => {
    const { client, ids } = await openHub();
    await waitFor(() => {
      expect(chipsOf(dayOf(12))).toEqual(["Beta"]);
    });
    dragTo(chip(dayOf(12), "Beta"), dayOf(20));
    await waitFor(() => {
      expect(chipsOf(dayOf(20))).toEqual(["Beta"]);
    });
    expect(chipsOf(dayOf(12))).toEqual([]);
    const values = await client.pagePropertiesGet(ids.page("Beta"));
    expect(values.find(([id]) => id === DUE)?.[1]).toEqual(dateValue(dayOf(20), "09:30"));
    // And a page of the tray gets a date by a drop.
    dragTo(trayChip("Gamma"), dayOf(22));
    await waitFor(() => {
      expect(chipsOf(dayOf(22))).toEqual(["Gamma"]);
    });
    expect(trayTitles()).toEqual([]);
    expect((await client.pagePropertiesGet(ids.page("Gamma"))).find(([id]) => id === DUE)?.[1]).toEqual(dateValue(dayOf(22)));
  });

  it("asks to choose when the view has no date source", async () => {
    await openHub(hubContent({ views: [{ id: "cal", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" } }] }));
    expect(await screen.findByText(t("pageCalendar.choose"))).toBeInTheDocument();
    expect(screen.queryByRole("grid")).toBeNull();
  });

  it("has a first week that matches the locale in the real clock's month", async () => {
    await openHub();
    await waitFor(() => {
      expect(chipsOf(dayOf(10))).toEqual(["Alpha"]);
    });
    const first = grid().querySelector("[role='gridcell']")?.getAttribute("data-day") ?? "";
    expect(first).toBe(monthGrid(`${MONTH}-01`, firstDayOfWeek())[0]?.[0]);
    expect(addDays(first, 41)).toBe(grid().querySelectorAll("[role='gridcell']")[41]?.getAttribute("data-day"));
  });
});
