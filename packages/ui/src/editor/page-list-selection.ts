// The selection of rows of a database block (ADR-0058, `docs/architecture/database-power-ups.md` section 5.1): which rows
// the person ticked, and where a Shift+click range starts. Pure, so the rules are tested apart from the layouts. `order`
// is always the rows in display order (the board: column by column, then card order), so a range follows what the person
// sees and "all" is every row of the view, the ones beyond the row limit included.
//
// A selection never holds a page that left the view: `pruneSelection` drops it when the rows change, and
// `PageListView` calls it on every render.

import type { PageId } from "@atomik-kb/core-client";

/** The ticked rows, and the row a Shift+click range starts from (the one ticked or unticked last). */
export interface RowSelection {
  readonly ids: ReadonlySet<PageId>;
  readonly anchor: PageId | null;
}

/** Nothing selected. */
export const EMPTY_SELECTION: RowSelection = { ids: new Set(), anchor: null };

/** How much of a list of rows is selected. */
export type SelectionState = "none" | "some" | "all";

/** `selection` with the row `pageId` ticked, or unticked when it was ticked; it becomes the anchor of ranges. */
export function toggleRow(selection: RowSelection, pageId: PageId): RowSelection {
  const ids = new Set(selection.ids);
  if (!ids.delete(pageId)) {
    ids.add(pageId);
  }
  return { ids, anchor: pageId };
}

/**
 * `selection` with every row from the anchor to `pageId` ticked, in display order (whichever of the two comes first).
 * Without an anchor, or when the anchor is no longer a row, only `pageId` is ticked. `pageId` becomes the anchor.
 */
export function selectRange(selection: RowSelection, order: readonly PageId[], pageId: PageId): RowSelection {
  const to = order.indexOf(pageId);
  const from = selection.anchor === null ? -1 : order.indexOf(selection.anchor);
  const ids = new Set(selection.ids);
  if (to < 0) {
    return selection;
  }
  if (from < 0) {
    ids.add(pageId);
  } else {
    for (const id of order.slice(Math.min(from, to), Math.max(from, to) + 1)) {
      ids.add(id);
    }
  }
  return { ids, anchor: pageId };
}

/** Every row of `order` ticked. The anchor stays where it was, when it is still a row. */
export function selectAll(order: readonly PageId[], anchor: PageId | null = null): RowSelection {
  return { ids: new Set(order), anchor: anchor !== null && order.includes(anchor) ? anchor : null };
}

/** Exactly the rows `pageIds` ticked (the copies after a Duplicate). */
export function selectOnly(pageIds: readonly PageId[]): RowSelection {
  return { ids: new Set(pageIds), anchor: pageIds[pageIds.length - 1] ?? null };
}

/** Nothing selected. */
export function clearSelection(): RowSelection {
  return EMPTY_SELECTION;
}

/**
 * `selection` without the rows that are not in `rows` (a page trashed, moved, or filtered out): the same object when
 * every ticked row is still there, so that a render that changes nothing changes no state.
 */
export function pruneSelection(selection: RowSelection, rows: ReadonlySet<PageId>): RowSelection {
  let gone = false;
  for (const id of selection.ids) {
    if (!rows.has(id)) {
      gone = true;
      break;
    }
  }
  const anchorGone = selection.anchor !== null && !rows.has(selection.anchor);
  if (!gone && !anchorGone) {
    return selection;
  }
  return { ids: new Set([...selection.ids].filter((id) => rows.has(id))), anchor: anchorGone ? null : selection.anchor };
}

/** Whether none, some or all of the rows of `order` are ticked (nothing is "all" of no rows). */
export function selectionState(selection: RowSelection, order: readonly PageId[]): SelectionState {
  let ticked = 0;
  for (const id of order) {
    if (selection.ids.has(id)) {
      ticked += 1;
    }
  }
  if (ticked === 0) {
    return "none";
  }
  return ticked === order.length ? "all" : "some";
}

/** The ticked rows of `order`, in display order. */
export function selectedInOrder(selection: RowSelection, order: readonly PageId[]): PageId[] {
  return order.filter((id) => selection.ids.has(id));
}

/** Whether `target` is a field the person types in (the keys of the view must leave it alone): text inputs, text areas, selects, editable content. */
export function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) {
    return false;
  }
  if (target.isContentEditable || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) {
    return true;
  }
  return target instanceof HTMLInputElement && !["checkbox", "radio", "button", "submit", "reset"].includes(target.type);
}

/** What a key pressed in the view asks of the selection. */
export type SelectionShortcut = "selectAll" | "clear";

/** The part of a keyboard event the shortcuts read. */
export interface ShortcutKey {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly defaultPrevented: boolean;
  readonly target: EventTarget | null;
}

/**
 * Ctrl+A (Cmd+A) selects every row, Escape clears a selection that exists; neither when the key is meant for a field the person
 * types in, or was already taken. `null` for any other key.
 */
export function selectionShortcut(event: ShortcutKey, hasSelection: boolean): SelectionShortcut | null {
  if (event.defaultPrevented || isTextField(event.target)) {
    return null;
  }
  if (event.key === "Escape") {
    return hasSelection ? "clear" : null;
  }
  if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "a") {
    return "selectAll";
  }
  return null;
}
