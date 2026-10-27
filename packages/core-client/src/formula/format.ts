// `format()` (section 3.4.2): the text a value becomes in `+`, `join` and `format`, over run-time values and over the
// `FormulaResult` the editor shows.

import type { FormulaResult } from "../generated/FormulaResult";
import { dateFromValue, formatDateDefault } from "./dates";
import type { FormulaHost } from "./types";
import { isDate, isPage, isPerson } from "./value";
import type { Value } from "./value";

/** What the formatting needs to know about pages and people. */
export type FormatHost = Pick<FormulaHost, "title" | "person">;

/** The shortest text that reads back as the same number, as JavaScript's `String(n)` (`-0` is `0`). */
export function formatNumber(n: number): string {
  return n === 0 ? "0" : String(n);
}

/** `format(value)`: empty is `""`, a list is its items formatted and joined with `", "`. */
export function formatValue(v: Value, host: FormatHost): string {
  if (v === null) {
    return "";
  }
  if (typeof v === "number") {
    return formatNumber(v);
  }
  if (typeof v === "string") {
    return v;
  }
  if (typeof v === "boolean") {
    return v ? "true" : "false";
  }
  if (Array.isArray(v)) {
    return v.map((item) => formatValue(item, host)).join(", ");
  }
  if (isDate(v)) {
    return formatDateDefault(v);
  }
  if (isPage(v)) {
    return host.title(v.id) ?? "";
  }
  if (isPerson(v)) {
    return host.person(v.id)?.name ?? "";
  }
  return "";
}

/** `format` applied to a formula's result (what the editor and the exports show). */
export function formatResult(result: FormulaResult, host: FormatHost): string {
  switch (result.type) {
    case "number":
      return formatNumber(result.data);
    case "text":
      return result.data;
    case "boolean":
      return result.data ? "true" : "false";
    case "date": {
      const date = dateFromValue(result.data);
      return date === null ? "" : formatDateDefault(date);
    }
    case "page":
      return host.title(result.data) ?? "";
    case "person":
      return host.person(result.data)?.name ?? "";
    case "list":
      return result.data.map((item) => formatResult(item, host)).join(", ");
  }
}
