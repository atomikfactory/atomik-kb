// The rules of the table layout that need no React (ADR-0016, amendment of 2026-10-08): the widths
// of the columns, where a column goes when it is moved, which kinds the cells can edit, and which
// rows a long table draws (a window over fixed-height rows).

import type { PropertyId, PropertyKind } from "@atomik-kb/core-client";

import { clampColumnWidth, MAX_COLUMN_WIDTH, MIN_COLUMN_WIDTH, TITLE_COLUMN } from "./page-list-model";

/** Width of the page (title) column when the block has not set one, in pixels. */
export const DEFAULT_TITLE_WIDTH = 280;
/** Width of a property column when the block has not set one, in pixels. */
export const DEFAULT_PROPERTY_WIDTH = 170;

/** Height of every row of the table, header included, in pixels (fixed so that a window of rows can be computed). */
export const ROW_HEIGHT = 34;
/** From how many rows the table draws only the ones in view. */
export const VIRTUALIZE_AFTER = 100;
/** Height of the table's scroll area when it is windowed, in pixels. */
export const WINDOW_HEIGHT = 480;
/** Rows drawn beyond the edge of the view, above and below. */
export const OVERSCAN = 8;

/**
 * The kinds the cells of the table edit, with the editors of the properties panel. The computed
 * kinds (created and last edited time and by, rollup) and a value of a kind from a newer version
 * show as they are and are not edited here.
 */
const EDITABLE_KINDS: readonly string[] = [
  "text",
  "number",
  "url",
  "email",
  "phone",
  "checkbox",
  "date",
  "select",
  "status",
  "multi_select",
  "files",
  "checklist",
  "place",
  "person",
  "relation",
];

export function isEditableKind(kind: PropertyKind): boolean {
  return EDITABLE_KINDS.includes(kind);
}

/** The kinds whose editor is a text field in the cell (the others open a picker, or are a box). */
export function isTextKind(kind: PropertyKind): boolean {
  return ["text", "number", "url", "email", "phone"].includes(kind);
}

/** The kinds whose editor is too big for a cell: it opens in a popover over the cell. */
export function isPopoverKind(kind: PropertyKind): boolean {
  return kind === "files" || kind === "checklist" || kind === "place";
}

/** The width of the column `key` (`"title"` or a property id) as it is drawn. */
export function widthOf(widths: Readonly<Record<string, number>>, key: string): number {
  const stored = widths[key];
  if (stored !== undefined) {
    return clampColumnWidth(stored);
  }
  return key === TITLE_COLUMN ? DEFAULT_TITLE_WIDTH : DEFAULT_PROPERTY_WIDTH;
}

/** A column being resized: its live width, and the width the page column had when the drag began (`null` when unknown). */
export interface ColumnDrag {
  column: string;
  width: number;
  title: number | null;
}

/**
 * The tracks of the grid and the narrowest it can be (ADR-0035). The grid fills its block, and the page (title)
 * column takes the room the others leave: its track is `minmax(<its width>px, 1fr)`, every other column keeps its
 * exact width. The minimum is the sum of the widths: a block narrower than that keeps the sum and scrolls sideways.
 * While a column is dragged every track is fixed, the page column at the width it was drawn with when the drag
 * began, so that the edge under the pointer is the edge that is drawn.
 */
export function gridLayout(
  keys: readonly string[],
  widths: Readonly<Record<string, number>>,
  drag: ColumnDrag | null,
): { template: string; minWidth: number } {
  const widthFor = (key: string): number => {
    if (drag?.column === key) {
      return drag.width;
    }
    return key === TITLE_COLUMN && drag?.title != null ? drag.title : widthOf(widths, key);
  };
  const tracks = keys.map((key) =>
    key === TITLE_COLUMN && drag === null ? `minmax(${String(widthFor(key))}px, 1fr)` : `${String(widthFor(key))}px`,
  );
  return { template: tracks.join(" "), minWidth: keys.reduce((sum, key) => sum + widthFor(key), 0) };
}

/**
 * The width a drag that began at `start` (the width the column is drawn with, which can be more than the largest
 * width a column keeps, as the page column's is when it fills a wide block) gives the column at `width`: never
 * narrower than the minimum, and never wider than the largest of the maximum and where it began.
 */
export function dragWidth(width: number, start: number): number {
  return Math.min(Math.max(MAX_COLUMN_WIDTH, start), Math.max(MIN_COLUMN_WIDTH, Math.round(width)));
}

/**
 * `properties` with `id` moved one place towards the start (`delta` -1) or the end (+1); `null` when
 * it is not in the list or already at that end. Ids the space no longer has stay where they are in
 * the list (`shown` is the subset that has columns): the column moves past its visible neighbour.
 */
export function moveProperty(
  properties: readonly PropertyId[],
  shown: readonly PropertyId[],
  id: PropertyId,
  delta: -1 | 1,
): PropertyId[] | null {
  const at = shown.indexOf(id);
  const neighbour = shown[at + delta];
  if (at < 0 || neighbour === undefined) {
    return null;
  }
  const next = [...properties];
  const from = next.indexOf(id);
  const to = next.indexOf(neighbour);
  if (from < 0 || to < 0) {
    return null;
  }
  next.splice(from, 1);
  next.splice(to, 0, id);
  return next;
}

/** The rows `[start, end)` to draw of `total` when the area is scrolled to `scrollTop`. */
export function visibleRows(total: number, scrollTop: number): { start: number; end: number } {
  const first = Math.floor(Math.max(0, scrollTop) / ROW_HEIGHT);
  const count = Math.ceil(WINDOW_HEIGHT / ROW_HEIGHT);
  return {
    start: Math.max(0, first - OVERSCAN),
    end: Math.min(total, first + count + OVERSCAN),
  };
}

/** Whether a table of `rows` rows draws only the ones in view. */
export function isWindowed(rows: number): boolean {
  return rows > VIRTUALIZE_AFTER;
}

/**
 * The scroll position that brings row `index` into view from `scrollTop` (the same when it already is).
 * `footer` is the height of the footer row that sticks to the bottom of the area (none by default).
 */
export function scrollToReveal(index: number, scrollTop: number, footer = 0): number {
  const top = index * ROW_HEIGHT;
  const bottom = top + ROW_HEIGHT;
  const visible = WINDOW_HEIGHT - ROW_HEIGHT - footer; // the header covers the first row of the area, the footer the last
  if (top < scrollTop) {
    return top;
  }
  if (bottom > scrollTop + visible) {
    return bottom - visible;
  }
  return scrollTop;
}
