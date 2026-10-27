// The calendar layout of a database view (ADR-0054, `docs/architecture/database-views.md` section 5): a month
// grid of the pages by date, with the toolbar's settings section (the date source and the optional end date).
//
// A page of one day is a chip in its day's cell; a page of several days (a Date property with an optional end-date
// property) is one bar per week row it touches, laid in lanes at the top of the row (`layoutWeek`) and drawn by a CSS
// grid over the row's seven cells (`grid-column: start / span n`), with a flat edge and a "‹" or "›" on the side that
// continues. A created or last-edited time is always one day. Only the visible grid is materialised (`placePages` clips
// to it). A chip or a bar (from any of its segments) drags to another day to reschedule the page (HTML5 drag and drop
// as the board's; the whole span moves, the time is kept, the end moves by the same days, the two writes are
// sequential), "Change date…" in the card's menu does the same for the keyboard, a click on a day's empty area or its
// "+" makes a page dated that day (from the view's default template when it has one; the menu next to the "+", `NewPageMenu`,
// picks another template or an empty page), and the "No date" tray lists the undated pages, which can be dropped on a day. A
// created or last-edited source is read only. The view writes nothing but page values (through the store) and asks the
// block for everything else (`onOpen`, `onCreatePage`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo, PropertyDef } from "@atomik-kb/core-client";
import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, DragEvent, KeyboardEvent, MouseEvent } from "react";

import { anchorBelow, FloatingPanel } from "../components/FloatingPanel";
import type { Anchor } from "../components/FloatingPanel";
import { ChevronIcon, MoreIcon, PlusIcon } from "../components/icons";
import { Menu } from "../components/Menu";
import { PageIcon } from "../components/PageIcon";
import { activeLocale, t, tp } from "../i18n";
import { dispositionFromKey, openGestures } from "../page-open";
import { useAppStore } from "../store-context";
import type { OpenDisposition } from "../store-tabs";
import { addDays, addMonths, firstDayOfWeek, fullDate, isIsoDate, mediumDate, monthGrid, monthTitle, partsOf, weekdayNames, weekdayOf } from "./dates";
import { DatePicker } from "./DatePicker";
import { NewPageMenu } from "./NewPageMenu";
import {
  changeDateChanges,
  formatTimeOfDay,
  layoutWeek,
  placePages,
  rescheduleChanges,
  resolveCalendarSource,
  spanOf,
  undatedPages,
} from "./page-calendar-model";
import type { CalendarBar, CalendarChange, CalendarEntry, CalendarSourceOk, CalendarSpan } from "./page-calendar-model";
import type { NewPageInit, PageListLayoutProps, PageListLayoutSettingsProps } from "./page-list-layout-types";
import { valueOfPage } from "./page-list-model";
import { PageListField } from "./PageListField";
import { PageListValue } from "./PageListValue";

/** The drag type of a card of the calendar (the board has its own). */
const DRAG_TYPE = "application/x-atomik-calendar-card";

/** The "No date" tray draws this many pages, then a button for the rest. */
const TRAY_LIMIT = 100;

const NO_EVENTS: readonly CalendarEntry[] = [];

/** The first day of the month `iso` is in. */
function firstOfMonth(iso: string): string {
  return `${iso.slice(0, 7)}-01`;
}

/** The title a page shows: its own, or "Untitled". */
function titleOf(page: PageInfo): string {
  return page.title === "" ? t("page.untitled") : page.title;
}

/** What the grid is on when it shows `month`: today if that is in it, else the first of the month. */
function defaultCursor(month: string, today: string): string {
  return today.slice(0, 7) === month.slice(0, 7) ? today : month;
}

/** The card being dragged: the page and the day it was grabbed on (`null`: from the tray, it had none). */
interface Drag {
  readonly pageId: string;
  readonly from: string | null;
}

/** An open card menu, or the date picker it leads to. */
interface CardPanel {
  readonly pageId: string;
  readonly anchor: Anchor;
  readonly trigger: Element;
}

/**
 * The day under the pointer in a week row's overlay (seven equal columns), from the pointer's x; `fallback` when the
 * overlay has no width (no layout).
 */
function dayAtPointer(clientX: number, overlay: Element | null, week: readonly string[], fallback: string | null): string | null {
  if (overlay === null) {
    return fallback;
  }
  const { left, width } = overlay.getBoundingClientRect();
  if (!(width > 0)) {
    return fallback;
  }
  const column = Math.min(6, Math.max(0, Math.floor(((clientX - left) / width) * 7)));
  const rtl = getComputedStyle(overlay).direction === "rtl";
  return week[rtl ? 6 - column : column] ?? fallback;
}

/** The bar's drag from the week row: the day under the pointer is the day it was grabbed on, kept inside the bar. */
function grabbedDay(event: DragEvent<HTMLElement>, week: readonly string[], bar: CalendarBar): string {
  const day = dayAtPointer(event.clientX, event.currentTarget.closest(".page-calendar-bars"), week, bar.first);
  return day === null || day < bar.first ? bar.first : day > bar.last ? bar.last : day;
}

interface ChipProps {
  page: PageInfo;
  /** The day's chip, `null` for a page of the tray or a bar. */
  entry: CalendarEntry | null;
  /** The bar of a page of several days in its week row (drawn in the row's overlay), `null` for a chip. */
  bar: CalendarBar | null;
  span: CalendarSpan | null;
  columns: readonly PropertyDef[];
  schema: readonly PropertyDef[];
  today: string;
  /** The chip drags (a Date property source, an editable editor). */
  canDrag: boolean;
  /** The card has its menu ("Change date…"): the same condition. */
  canMenu: boolean;
  dragging: boolean;
  /** 0 for a chip in the tab order, -1 for one reached by moving to its day first. */
  tabIndex: 0 | -1;
  onOpen: (page: PageInfo, disposition: OpenDisposition) => void;
  /** `from` is the day the card was grabbed on (a chip's day, a bar's first day; the view refines a bar's from the pointer). */
  onDragStart: (page: PageInfo, from: string | null, event: DragEvent<HTMLElement>) => void;
  onDragEnd: () => void;
  onMenu: (page: PageInfo, trigger: Element) => void;
}

/**
 * One page on one day (or in the tray), or one bar of a page of several days in a week row: its time and title as the
 * button that opens it, the card's menu, the shown values (a bar is one line high and shows none; its title is cut
 * with an ellipsis and whole in its tooltip).
 */
function CalendarChip({
  page,
  entry,
  bar,
  span,
  columns,
  schema,
  today,
  canDrag,
  canMenu,
  dragging,
  tabIndex,
  onOpen,
  onDragStart,
  onDragEnd,
  onMenu,
}: ChipProps) {
  const title = titleOf(page);
  const position = entry?.position ?? "single";
  const continuesBefore = bar === null ? position === "middle" || position === "end" : bar.continuesBefore;
  const continuesAfter = bar === null ? position === "start" || position === "middle" : bar.continuesAfter;
  const time = bar === null ? (entry?.time ?? null) : bar.time;
  // The name of a chip or a bar of a page of several days says so; a one-day chip is named by what it shows (its time and title).
  const several = bar !== null || position !== "single";
  const label =
    several && !continuesBefore && span !== null
      ? t("pageCalendar.span", { title, from: mediumDate(span.start), to: mediumDate(span.end) })
      : continuesBefore
        ? t("pageCalendar.continued", { title })
        : undefined;
  return (
    <li
      className={bar === null ? "page-calendar-chip" : "page-calendar-chip page-calendar-bar"}
      data-page={page.id}
      data-position={bar === null ? position : undefined}
      data-column={bar?.column}
      data-span={bar?.span}
      data-lane={bar?.lane}
      data-continues-before={bar?.continuesBefore ? "" : undefined}
      data-continues-after={bar?.continuesAfter ? "" : undefined}
      data-dragging={dragging ? "" : undefined}
      title={bar === null ? undefined : title}
      style={bar === null ? undefined : { gridColumn: `${bar.column} / span ${bar.span}`, gridRow: bar.lane + 1 }}
      draggable={canDrag}
      onDragStart={
        canDrag
          ? (event) => {
              event.dataTransfer.setData(DRAG_TYPE, page.id);
              event.dataTransfer.effectAllowed = "move";
              onDragStart(page, entry?.day ?? bar?.first ?? null, event);
            }
          : undefined
      }
      onDragEnd={canDrag ? onDragEnd : undefined}
    >
      <button
        type="button"
        className="page-list-link page-calendar-link"
        tabIndex={tabIndex}
        aria-label={label}
        {...openGestures<HTMLButtonElement>((disposition) => {
          onOpen(page, disposition);
        })}
      >
        {continuesBefore && (
          <span className="page-calendar-arrow" aria-hidden="true">
            ‹
          </span>
        )}
        <PageIcon icon={page.icon} className="page-list-icon" />
        {time !== null && <span className="page-calendar-time">{formatTimeOfDay(time)}</span>}
        <span className="page-list-name">{title}</span>
        {continuesAfter && (
          <span className="page-calendar-arrow" aria-hidden="true">
            ›
          </span>
        )}
      </button>
      {canMenu && (
        <button
          type="button"
          className="page-calendar-more-button"
          tabIndex={tabIndex}
          aria-haspopup="menu"
          aria-label={t("pageCalendar.cardMenu", { title })}
          onClick={(event) => {
            onMenu(page, event.currentTarget);
          }}
        >
          <MoreIcon />
        </button>
      )}
      {(bar === null ? columns : []).map((prop) => {
        const value = valueOfPage(page, prop.id);
        return value === undefined ? null : (
          <div key={prop.id} className="page-calendar-prop" role="group" aria-label={prop.name} data-kind={prop.kind}>
            <PageListValue def={prop} value={value} today={today} schema={schema} />
          </div>
        );
      })}
    </li>
  );
}

const NO_PAGES: readonly PageInfo[] = [];

/** The calendar. */
export function PageCalendarView({
  rows,
  view,
  schema,
  columns,
  editable,
  today,
  onOpen,
  onUpdateView,
  onCreatePage,
  creating,
}: PageListLayoutProps) {
  const setPageProperty = useAppStore((state) => state.setPageProperty);
  const locale = activeLocale();
  const firstDay = useMemo(() => firstDayOfWeek(locale), [locale]);
  const state = useMemo(() => resolveCalendarSource(view, schema), [view, schema]);
  const ok: CalendarSourceOk | null = state.kind === "ok" ? state : null;
  const [month, setMonth] = useState(() => firstOfMonth(today));
  const [cursor, setCursor] = useState(today);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [over, setOver] = useState<string | null>(null);
  const [more, setMore] = useState<{ day: string; anchor: Anchor; trigger: Element } | null>(null);
  const [menu, setMenu] = useState<CardPanel | null>(null);
  const [picker, setPicker] = useState<CardPanel | null>(null);
  const [trayAll, setTrayAll] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const root = useRef<HTMLDivElement>(null);
  // Set by a key that moved the cursor: the day it moved to takes the focus once it is drawn.
  const moveFocus = useRef(false);

  const weeks = useMemo(() => monthGrid(month, firstDay), [month, firstDay]);
  const first = weeks[0]?.[0] ?? month;
  const last = weeks[weeks.length - 1]?.[6] ?? month;
  const placed = useMemo(() => (ok === null ? new Map<string, CalendarEntry[]>() : placePages(rows, ok, { first, last })), [rows, ok, first, last]);
  // Each week row: its bars in lanes, the chips and the "+k more" of its days.
  const weekRows = useMemo(() => weeks.map((week) => ({ week, layout: layoutWeek(week, placed) })), [weeks, placed]);
  const undated = useMemo(() => (ok === null ? NO_PAGES : undatedPages(rows, ok)), [rows, ok]);
  const spans = useMemo(() => {
    const found = new Map<string, CalendarSpan | null>();
    if (ok !== null) {
      for (const entries of placed.values()) {
        for (const { page } of entries) {
          if (!found.has(page.id)) {
            found.set(page.id, spanOf(page, ok));
          }
        }
      }
    }
    return found;
  }, [placed, ok]);
  const names = useMemo(() => weekdayNames(firstDay, locale), [firstDay, locale]);

  const canMove = editable && ok !== null && ok.source.kind === "property";
  const dateProperty = ok !== null && ok.source.kind === "property" ? ok.source.def : null;
  const canCreate = canMove && dateProperty !== null && onCreatePage !== null;
  const focusDay = cursor >= first && cursor <= last ? cursor : defaultCursor(month, today);
  const drawn = ok !== null;

  // A card's drag is the calendar's own: the editor (its drop cursor, its drag of a selection) must not see these events.
  // React has handled them at the node view's element when they get there, so they are stopped there, for the editor's
  // listeners further up (as the board does).
  useEffect(() => {
    const host = root.current?.closest(".page-list-node");
    if (host === null || host === undefined) {
      return;
    }
    const stop = (event: Event): void => {
      event.stopPropagation();
    };
    const types = ["dragstart", "dragenter", "dragover", "dragleave", "drop", "dragend"];
    for (const type of types) {
      host.addEventListener(type, stop);
    }
    return () => {
      for (const type of types) {
        host.removeEventListener(type, stop);
      }
    };
  }, [drawn]);

  useEffect(() => {
    if (!moveFocus.current) {
      return;
    }
    moveFocus.current = false;
    root.current?.querySelector<HTMLElement>(`[data-day="${focusDay}"]`)?.focus();
  }, [focusDay]);

  if (ok === null) {
    const noProperty = state.kind === "unset" && schema.length > 0 && !schema.some((def) => def.kind === "date");
    const text =
      state.kind === "gone"
        ? t("pageCalendar.dateGone")
        : state.kind === "kind"
          ? t("pageCalendar.dateKind", { name: state.def.name })
          : noProperty
            ? t("pageCalendar.noDateProperty")
            : t("pageCalendar.choose");
    return (
      <div ref={root} className="page-calendar" data-state={state.kind}>
        <p className="page-list-empty" role="status">
          {text}
        </p>
      </div>
    );
  }

  const source = ok;
  const byId = (id: string): PageInfo | undefined => rows.find((page) => page.id === id);

  async function apply(page: PageInfo, changes: readonly CalendarChange[], day: string): Promise<void> {
    if (changes.length === 0) {
      return;
    }
    setError(null);
    try {
      // One after the other: the end only after the start was written.
      for (const change of changes) {
        await setPageProperty(page.id, change.propertyId, change.value);
      }
      setAnnouncement(t("pageCalendar.moved", { title: titleOf(page), date: mediumDate(day) }));
    } catch (cause) {
      setError(t("pageCalendar.error.move", { message: toCoreError(cause).message }));
    }
  }

  function dropOn(day: string): void {
    const current = drag;
    setDrag(null);
    setOver(null);
    const page = current === null ? undefined : byId(current.pageId);
    if (current === null || page === undefined) {
      return;
    }
    void apply(page, rescheduleChanges(page, source, day, current.from ?? day), day);
  }

  function dragOver(event: DragEvent<HTMLElement>, day: string): void {
    if (drag === null) {
      return;
    }
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    setOver(day);
  }

  function create(day: string): void {
    if (dateProperty === null || onCreatePage === null || creating) {
      return;
    }
    onCreatePage(dayInit(day));
  }

  /** What a page made on `day` starts with: that date in the calendar's date property. */
  function dayInit(day: string): NewPageInit | undefined {
    return dateProperty === null ? undefined : { values: [[dateProperty.id, { kind: "date", value: { date: day } }]] };
  }

  function goTo(next: string): void {
    const start = firstOfMonth(next);
    setMonth(start);
    setCursor(defaultCursor(start, today));
  }

  function moveCursor(next: string): void {
    if (!isIsoDate(next)) {
      return;
    }
    setCursor(next);
    if (next < first || next > last) {
      setMonth(firstOfMonth(next));
    }
    moveFocus.current = next !== focusDay;
  }

  function onDayKeyDown(event: KeyboardEvent<HTMLElement>, day: string, entries: readonly CalendarEntry[]): void {
    if (event.target !== event.currentTarget || event.altKey) {
      return;
    }
    const weekday = (weekdayOf(day) - firstDay + 7) % 7;
    switch (event.key) {
      case "ArrowLeft":
        moveCursor(addDays(day, -1));
        break;
      case "ArrowRight":
        moveCursor(addDays(day, 1));
        break;
      case "ArrowUp":
        moveCursor(addDays(day, -7));
        break;
      case "ArrowDown":
        moveCursor(addDays(day, 7));
        break;
      case "PageUp":
        moveCursor(addMonths(day, event.shiftKey ? -12 : -1));
        break;
      case "PageDown":
        moveCursor(addMonths(day, event.shiftKey ? 12 : 1));
        break;
      case "Home":
        moveCursor(addDays(day, -weekday));
        break;
      case "End":
        moveCursor(addDays(day, 6 - weekday));
        break;
      case "Enter": {
        const entry = entries[0];
        if (entry !== undefined) {
          onOpen(entry.page, dispositionFromKey(event));
        } else if (canCreate) {
          create(day);
        } else {
          return;
        }
        break;
      }
      default:
        return;
    }
    event.preventDefault();
  }

  function openMenu(page: PageInfo, trigger: Element): void {
    setMore(null);
    setMenu((current) => (current?.pageId === page.id ? null : { pageId: page.id, anchor: anchorBelow(trigger), trigger }));
  }

  const chipProps = (page: PageInfo, entry: CalendarEntry | null, tabIndex: 0 | -1) => ({
    page,
    entry,
    bar: null,
    span: spans.get(page.id) ?? null,
    columns,
    schema,
    today,
    canDrag: canMove,
    canMenu: canMove,
    dragging: drag?.pageId === page.id,
    tabIndex,
    onOpen,
    onDragStart: (dragged: PageInfo, from: string | null) => {
      setDrag({ pageId: dragged.id, from });
    },
    onDragEnd: () => {
      setDrag(null);
      setOver(null);
    },
    onMenu: openMenu,
  });

  const menuPage = menu === null ? undefined : byId(menu.pageId);
  const pickerPage = picker === null ? undefined : byId(picker.pageId);
  const pickerSpan = pickerPage === undefined ? null : spanOf(pickerPage, source);
  const trayPages = trayAll ? undated : undated.slice(0, TRAY_LIMIT);
  const moreEntries = more === null ? NO_EVENTS : (placed.get(more.day) ?? NO_EVENTS);

  return (
    <div ref={root} className="page-calendar" data-state="ok" data-source={source.source.kind}>
      <div className="page-calendar-header">
        <span className="page-calendar-title" aria-live="polite">
          {monthTitle(month, locale)}
        </span>
        <button
          type="button"
          className="page-calendar-nav is-previous"
          aria-label={t("pageCalendar.prev")}
          title={t("pageCalendar.prev")}
          onClick={() => {
            goTo(addMonths(month, -1));
          }}
        >
          <ChevronIcon />
        </button>
        <button
          type="button"
          className="page-calendar-today"
          onClick={() => {
            goTo(today);
          }}
        >
          {t("pageCalendar.today")}
        </button>
        <button
          type="button"
          className="page-calendar-nav"
          aria-label={t("pageCalendar.next")}
          title={t("pageCalendar.next")}
          onClick={() => {
            goTo(addMonths(month, 1));
          }}
        >
          <ChevronIcon />
        </button>
      </div>
      <div className="page-calendar-grid" role="grid" aria-label={t("pageCalendar.grid", { month: monthTitle(month, locale) })}>
        <div className="page-calendar-week page-calendar-weekdays" role="row">
          {names.map((name) => (
            <div key={name.long} className="page-calendar-weekday" role="columnheader" aria-label={name.long}>
              {name.short}
            </div>
          ))}
        </div>
        {weekRows.map(({ week, layout }) => (
          // A week row is its seven cells (the `row`) with the overlay of its bars over them; `--calendar-lanes` is the room
          // the cells keep for the lanes above their chips.
          <div key={week[0]} className="page-calendar-weekrow" role="presentation" style={{ "--calendar-lanes": layout.lanes } as CSSProperties}>
            <div className="page-calendar-week" role="row">
              {layout.days.map(({ day, entries, chips, hidden }) => {
                const inTabOrder = day === focusDay;
                const tabIndex = inTabOrder ? 0 : -1;
                const dayNumber = partsOf(day)?.day ?? 0;
                return (
                  <div
                    key={day}
                    className="page-calendar-day"
                    role="gridcell"
                    data-day={day}
                    data-today={day === today ? "" : undefined}
                    data-outside={day.slice(0, 7) === month.slice(0, 7) ? undefined : ""}
                    data-over={over === day ? "" : undefined}
                    aria-current={day === today ? "date" : undefined}
                    aria-label={tp("pageCalendar.day", entries.length, { date: fullDate(day, locale) })}
                    tabIndex={tabIndex}
                    onFocus={(event) => {
                      if (event.target === event.currentTarget) {
                        setCursor(day);
                      }
                    }}
                    onKeyDown={(event) => {
                      onDayKeyDown(event, day, entries);
                    }}
                    onClick={(event: MouseEvent<HTMLElement>) => {
                      setCursor(day);
                      if (!canCreate || !(event.target instanceof Element)) {
                        return;
                      }
                      // A click on a card or a button of the day is theirs; the rest of the day makes a page.
                      const hit = event.target.closest("button, li");
                      if (hit === null || !event.currentTarget.contains(hit)) {
                        create(day);
                      }
                    }}
                    onDragOver={canMove ? (event) => dragOver(event, day) : undefined}
                    onDragLeave={
                      canMove
                        ? (event) => {
                            const into = event.relatedTarget;
                            if (!(into instanceof Node && event.currentTarget.contains(into))) {
                              setOver((current) => (current === day ? null : current));
                            }
                          }
                        : undefined
                    }
                    onDrop={
                      canMove
                        ? (event) => {
                            event.preventDefault();
                            dropOn(day);
                          }
                        : undefined
                    }
                  >
                    <div className="page-calendar-day-head">
                      <span className="page-calendar-day-number" aria-hidden="true">
                        {dayNumber}
                      </span>
                      {canCreate && onCreatePage !== null && (
                        <span className="page-calendar-adders">
                          <button
                            type="button"
                            className="page-calendar-add"
                            tabIndex={tabIndex}
                            disabled={creating}
                            aria-label={t("pageCalendar.newOn", { date: fullDate(day, locale) })}
                            title={t("pageCalendar.newOn", { date: fullDate(day, locale) })}
                            onClick={() => {
                              setCursor(day);
                              create(day);
                            }}
                          >
                            <PlusIcon />
                          </button>
                          <NewPageMenu
                            view={view}
                            onCreatePage={onCreatePage}
                            onUpdateView={onUpdateView}
                            init={dayInit(day)}
                            tabIndex={tabIndex}
                            disabled={creating}
                            label={t("databaseTemplate.menuOn", { date: fullDate(day, locale) })}
                          />
                        </span>
                      )}
                    </div>
                    {chips.length > 0 && (
                      <ul className="page-calendar-chips">
                        {chips.map((entry) => (
                          <CalendarChip key={entry.page.id} {...chipProps(entry.page, entry, tabIndex)} />
                        ))}
                      </ul>
                    )}
                    {hidden > 0 && (
                      <button
                        type="button"
                        className="page-calendar-more"
                        tabIndex={tabIndex}
                        aria-haspopup="dialog"
                        aria-expanded={more?.day === day}
                        aria-label={tp("pageCalendar.moreLabel", hidden, { date: fullDate(day, locale) })}
                        onClick={(event) => {
                          const opener = event.currentTarget;
                          setCursor(day);
                          setMore((current) => (current?.day === day ? null : { day, anchor: anchorBelow(opener), trigger: opener }));
                        }}
                      >
                        {t("pageCalendar.more", { count: hidden })}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
            {layout.bars.some((bar) => !bar.hidden) && (
              // The bars sit over the cells in a grid of the same seven columns; only the bars take the pointer, so the rest of
              // the overlay lets a click or a drop through to the day below. A bar is as wide as several days, so a drag over one
              // finds its day from the pointer.
              <ul
                className="page-calendar-bars"
                role="presentation"
                onDragOver={
                  canMove
                    ? (event) => {
                        const day = dayAtPointer(event.clientX, event.currentTarget, week, null);
                        if (day !== null) {
                          dragOver(event, day);
                        }
                      }
                    : undefined
                }
                onDragLeave={
                  canMove
                    ? (event) => {
                        const into = event.relatedTarget;
                        if (!(into instanceof Node && event.currentTarget.contains(into))) {
                          setOver(null);
                        }
                      }
                    : undefined
                }
                onDrop={
                  canMove
                    ? (event) => {
                        const day = dayAtPointer(event.clientX, event.currentTarget, week, null);
                        if (day !== null) {
                          event.preventDefault();
                          dropOn(day);
                        }
                      }
                    : undefined
                }
              >
                {layout.bars
                  .filter((bar) => !bar.hidden)
                  .map((bar) => (
                    <CalendarChip
                      key={bar.page.id}
                      {...chipProps(bar.page, null, bar.first <= focusDay && focusDay <= bar.last ? 0 : -1)}
                      bar={bar}
                      onDragStart={(dragged, _from, event) => {
                        setDrag({ pageId: dragged.id, from: grabbedDay(event, week, bar) });
                      }}
                    />
                  ))}
              </ul>
            )}
          </div>
        ))}
      </div>
      {dateProperty === null ? (
        <p className="page-calendar-note" role="note">
          {t("pageCalendar.fixedSource")}
        </p>
      ) : (
        <section className="page-calendar-tray" aria-label={t("pageCalendar.noDate")}>
          <header className="page-calendar-tray-head">
            <span className="page-calendar-tray-title">{t("pageCalendar.noDate")}</span>
            <span className="page-calendar-tray-count">{undated.length}</span>
          </header>
          {undated.length === 0 ? (
            <p className="page-list-empty">{t("pageCalendar.noDate.empty")}</p>
          ) : (
            <>
              {canMove && <p className="page-list-empty">{t("pageCalendar.noDate.hint")}</p>}
              <ul className="page-calendar-tray-chips">
                {trayPages.map((page) => (
                  <CalendarChip key={page.id} {...chipProps(page, null, 0)} />
                ))}
              </ul>
              {undated.length > trayPages.length && (
                <button
                  type="button"
                  className="page-calendar-more"
                  onClick={() => {
                    setTrayAll(true);
                  }}
                >
                  {t("pageCalendar.more", { count: undated.length - trayPages.length })}
                </button>
              )}
            </>
          )}
        </section>
      )}
      {error !== null && (
        <p className="page-list-error" role="alert">
          {error}
        </p>
      )}
      <div className="visually-hidden" role="status">
        {announcement}
      </div>
      {more !== null && (
        <FloatingPanel
          anchor={more.anchor}
          anchorElement={more.trigger}
          role="dialog"
          label={tp("pageCalendar.day", moreEntries.length, { date: fullDate(more.day, locale) })}
          className="page-calendar-popover"
          onClose={() => {
            setMore(null);
          }}
        >
          <p className="page-calendar-popover-title" aria-hidden="true">
            {fullDate(more.day, locale)}
          </p>
          <ul className="page-calendar-popover-chips">
            {moreEntries.map((entry) => (
              <CalendarChip
                key={entry.page.id}
                {...chipProps(entry.page, entry, 0)}
                canDrag={false}
                onOpen={(page, disposition) => {
                  setMore(null);
                  onOpen(page, disposition);
                }}
              />
            ))}
          </ul>
        </FloatingPanel>
      )}
      {menu !== null && menuPage !== undefined && (
        <Menu
          anchor={menu.anchor}
          anchorElement={menu.trigger}
          label={t("pageCalendar.cardMenu", { title: titleOf(menuPage) })}
          items={[{ id: "change", label: t("pageCalendar.changeDate") }]}
          onClose={() => {
            setMenu(null);
          }}
          onSelect={() => {
            setPicker({ pageId: menu.pageId, anchor: menu.anchor, trigger: menu.trigger });
          }}
        />
      )}
      {picker !== null && pickerPage !== undefined && (
        <DatePicker
          anchor={picker.anchor}
          anchorElement={picker.trigger}
          date={pickerSpan?.start ?? ""}
          time={pickerSpan?.startTime ?? null}
          onChange={(date, time) => {
            void apply(pickerPage, changeDateChanges(pickerPage, source, date, time), date);
          }}
          onClose={() => {
            setPicker(null);
          }}
        />
      )}
    </div>
  );
}

/** The calendar's section of the view settings menu: the date source and the end date, each with its visible label. Nothing is preselected and nothing is written until a choice. */
export function PageCalendarSettings({ view, schema, onUpdateView }: PageListLayoutSettingsProps) {
  const dates = schema.filter((def) => def.kind === "date");
  const state = resolveCalendarSource(view, schema);
  const startDef = state.kind === "ok" && state.source.kind === "property" ? state.source.def : null;
  const dateValue = state.kind === "ok" ? (startDef === null ? state.source.kind : startDef.id) : "";
  const endValue = state.kind === "ok" && state.end !== null ? state.end.id : "";
  return (
    <>
      <PageListField label={t("pageCalendar.dateBy")}>
        <select
          className="page-list-select"
          value={dateValue}
          onChange={(event) => {
            if (event.target.value !== "" && event.target.value !== dateValue) {
              onUpdateView({ dateBy: event.target.value });
            }
          }}
        >
          <option value="" disabled>
            {t("pageCalendar.dateBy.choose")}
          </option>
          {dates.map((def) => (
            <option key={def.id} value={def.id}>
              {def.name}
            </option>
          ))}
          <option value="created">{t("pageCalendar.dateBy.created")}</option>
          <option value="updated">{t("pageCalendar.dateBy.updated")}</option>
        </select>
      </PageListField>
      {startDef !== null && (
        <PageListField label={t("pageCalendar.endBy")}>
          <select
            className="page-list-select"
            value={endValue}
            onChange={(event) => {
              if (event.target.value !== endValue) {
                onUpdateView({ endBy: event.target.value === "" ? null : event.target.value });
              }
            }}
          >
            <option value="">{t("pageCalendar.endBy.none")}</option>
            {dates
              .filter((def) => def.id !== startDef.id)
              .map((def) => (
                <option key={def.id} value={def.id}>
                  {def.name}
                </option>
              ))}
          </select>
        </PageListField>
      )}
    </>
  );
}
