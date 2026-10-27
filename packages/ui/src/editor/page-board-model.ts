// The data of the board layout (ADR-0016, amendment of 2026-10-08, the board): the two attributes
// `groupBy` and `hiddenGroups` read leniently, which columns a group property gives, and which
// column a page is in. Pure functions, shared by the view (`PageBoardView.tsx`) and its tests.

import type { OptionId, PageInfo, PropertyDef, PropertyOption, StatusGroup } from "@atomik-kb/core-client";

import { groupedOptions } from "../property-format";
import { valueOfPage } from "./page-list-model";

/** The key of the column of the pages without a value (also what `hiddenGroups` lists for it). */
export const NO_GROUP = "none";

/** The property id an attribute holds; anything that is not a non-empty text reads as none. */
export function readGroupBy(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

/** The column keys an attribute lists (option ids, `"none"`), in order, without repeats. */
export function readHiddenGroups(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const keys: string[] = [];
  for (const item of value as unknown[]) {
    if (typeof item === "string" && item !== "" && !keys.includes(item)) {
      keys.push(item);
    }
  }
  return keys;
}

/** The kinds of property a board can group by. */
export function isGroupable(def: PropertyDef): boolean {
  return def.kind === "select" || def.kind === "status";
}

export type GroupState =
  | { kind: "none" }
  | { kind: "missing"; id: string }
  | { kind: "wrongKind"; def: PropertyDef }
  | { kind: "ok"; def: PropertyDef };

/** What `groupBy` points at in the schema. */
export function resolveGroup(groupBy: string | null, defs: readonly PropertyDef[]): GroupState {
  if (groupBy === null) {
    return { kind: "none" };
  }
  const def = defs.find((candidate) => candidate.id === groupBy);
  if (def === undefined) {
    return { kind: "missing", id: groupBy };
  }
  return isGroupable(def) ? { kind: "ok", def } : { kind: "wrongKind", def };
}

export interface BoardColumn {
  /** The option id, or `NO_GROUP`. */
  key: string;
  option: PropertyOption | null;
  /** The group of a status option. */
  group: StatusGroup | null;
}

/** The columns of a property: the pages without a value first, then one per option (a status in its groups). */
export function boardColumns(def: PropertyDef): BoardColumn[] {
  const columns: BoardColumn[] = [{ key: NO_GROUP, option: null, group: null }];
  if (def.kind === "status") {
    for (const entry of groupedOptions(def.options)) {
      for (const option of entry.options) {
        columns.push({ key: option.id, option, group: entry.group });
      }
    }
  } else {
    for (const option of def.options) {
      columns.push({ key: option.id, option, group: null });
    }
  }
  return columns;
}

/** The column key of a page: its option, or `NO_GROUP` when it has none (or one the property lacks). */
export function columnOf(page: PageInfo, def: PropertyDef): string {
  const value = valueOfPage(page, def.id);
  if ((value?.kind === "select" || value?.kind === "status") && def.options.some((option) => option.id === value.value)) {
    return value.value;
  }
  return NO_GROUP;
}

/** `rows` (already filtered and sorted) split by column, each keeping its order. */
export function groupRows(rows: readonly PageInfo[], def: PropertyDef): Map<string, PageInfo[]> {
  const groups = new Map<string, PageInfo[]>();
  for (const column of boardColumns(def)) {
    groups.set(column.key, []);
  }
  for (const page of rows) {
    groups.get(columnOf(page, def))?.push(page);
  }
  return groups;
}

/** The value that puts a page in a column (`null` clears it for `NO_GROUP`). */
export function valueForColumn(def: PropertyDef, key: string): { kind: "select" | "status"; value: OptionId } | null {
  return key === NO_GROUP || (def.kind !== "select" && def.kind !== "status") ? null : { kind: def.kind, value: key };
}
