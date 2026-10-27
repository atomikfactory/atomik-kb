// "Export CSV" of a view (ADR-0036, `docs/architecture/import-export.md` section 2.8): the UI says which
// rows and which columns, the core formats them. The rows are exactly the ones the view shows, after its
// filters and sort (a board's cards column by column, the hidden columns left out); the columns are the
// title first, then the properties the view shows, in the order it shows them. Pure functions, shared by the
// view (`PageListView.tsx`) and its tests.

import type { CsvColumn, PageId, PageInfo, PropertyDef } from "@atomik-kb/core-client";

import { t } from "../i18n";
import { boardColumns, groupRows } from "./page-board-model";
import type { GroupState } from "./page-board-model";

/** What the core is asked to format. */
export interface CsvSelection {
  pageIds: PageId[];
  columns: CsvColumn[];
}

/** The title column, then one per property shown. */
export function csvColumns(shown: readonly PropertyDef[]): CsvColumn[] {
  return [{ type: "Title" }, ...shown.map((def): CsvColumn => ({ type: "Property", data: { propertyId: def.id } }))];
}

/**
 * The rows of a list or a table view: `rows` as the view draws them (filtered and sorted), with the
 * properties it shows.
 */
export function listSelection(rows: readonly PageInfo[], shown: readonly PropertyDef[]): CsvSelection {
  return { pageIds: rows.map((page) => page.id), columns: csvColumns(shown) };
}

/**
 * The cards of a board, column by column in the order the board draws them (the pages without a value
 * first), each column in the view's sort, the columns the block hides left out. `null` while the board has
 * no property to group by: it draws no cards then.
 */
export function boardSelection(
  rows: readonly PageInfo[],
  group: GroupState,
  hiddenGroups: readonly string[],
  shown: readonly PropertyDef[],
): CsvSelection | null {
  if (group.kind !== "ok") {
    return null;
  }
  const groups = groupRows(rows, group.def);
  const pageIds = boardColumns(group.def)
    .filter((column) => !hiddenGroups.includes(column.key))
    .flatMap((column) => (groups.get(column.key) ?? []).map((page) => page.id));
  return { pageIds, columns: csvColumns(shown) };
}

/** The characters a file name cannot hold on the three platforms (the control characters too). */
const UNSAFE_NAME_CHARS = '<>:"/\\|?*';

/** `name` with every character a file name cannot hold turned into a space. */
function withoutUnsafeChars(name: string): string {
  return Array.from(name, (char) =>
    (char.codePointAt(0) ?? 0) < 0x20 || UNSAFE_NAME_CHARS.includes(char) ? " " : char,
  ).join("");
}

/** The longest name stem offered, in characters. */
const MAX_STEM = 100;

/**
 * The file name offered for a view's CSV: `<page title>.csv`, with the characters a file name cannot hold
 * turned into spaces ("Untitled" for a page with no title).
 */
export function csvFileName(pageTitle: string): string {
  const stem = withoutUnsafeChars(pageTitle)
    .replace(/\s+/g, " ")
    .trim()
    .replace(/\.+$/, "")
    .trim()
    .slice(0, MAX_STEM)
    .trim();
  return `${stem === "" ? t("page.untitled") : stem}.csv`;
}
