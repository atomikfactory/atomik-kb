// What the UI makes of the value of a formula property (ADR-0055, `docs/architecture/formulas.md` section 9): the
// result a page holds, the shape of a property's result (its derived `resultType`), the neutral text of a result
// (sorting and the "contains" filters read it; the core writes the CSV itself) and the conversions the views and the
// footer's calculations need. Pure: no React, no store. How a result is drawn is `FormulaValue` in
// `editor/PageListValue.tsx`.

import type { FormulaResult, FormulaType, PageId, PropertyDef, PropertyValue } from "@atomik-kb/core-client";

/** What a formula's text lookups need: the title of a page and the name of a person (neither known reads as nothing). */
export interface FormulaLookups {
  /** The title of a page, `null` when it is not known. */
  readonly pageTitle?: (id: PageId) => string | null;
  /** The name of a person of the space, empty when it is not known. */
  readonly personName?: (id: string) => string;
}

/**
 * What a formula property gives, as far as the views go: a scalar by its base, `list` for any list, `none` for a
 * formula that does not check (no `resultType`: no page has a value).
 */
export type FormulaShape = "number" | "text" | "boolean" | "date" | "page" | "person" | "list" | "none";

/** The type of the result of a formula property; `undefined` when it does not check, and for a property of another kind. */
export function formulaTypeOf(def: PropertyDef): FormulaType | undefined {
  return def.config?.kind === "formula" ? def.config.resultType : undefined;
}

/** The stored expression of a formula property, `undefined` for any other property. */
export function formulaExpressionOf(def: PropertyDef): string | undefined {
  return def.config?.kind === "formula" ? def.config.expression : undefined;
}

/** The shape of the result of a property; `none` for a property that is not a formula or one that does not check. */
export function formulaShapeOf(def: PropertyDef): FormulaShape {
  const type = formulaTypeOf(def);
  if (type === undefined) {
    return "none";
  }
  return type.list ? "list" : type.base;
}

/** The result a page's value holds; `undefined` for no value, and for a value of another kind. */
export function formulaResultOf(value: PropertyValue | undefined): FormulaResult | undefined {
  return value?.kind === "formula" ? value.value : undefined;
}

/** The items of a result: those of a list, the result itself otherwise. */
export function formulaItems(result: FormulaResult): readonly FormulaResult[] {
  return result.type === "list" ? result.data : [result];
}

/**
 * A result as plain text, in no language: a number as JavaScript writes it, a day as `YYYY-MM-DD` (and ` HH:mm` with a
 * time), a boolean as `true` or `false`, a page by its title, a person by the name, a list as its items joined with `, `.
 * What sorting and the text filters read.
 */
export function formulaPlainText(result: FormulaResult, lookups: FormulaLookups = {}): string {
  switch (result.type) {
    case "number":
      return String(result.data);
    case "text":
      return result.data;
    case "boolean":
      return result.data ? "true" : "false";
    case "date":
      return result.data.time === undefined ? result.data.date : `${result.data.date} ${result.data.time}`;
    case "page":
      return lookups.pageTitle?.(result.data) ?? "";
    case "person":
      return lookups.personName?.(result.data) ?? "";
    case "list":
      return result.data
        .map((item) => formulaPlainText(item, lookups))
        .filter((text) => text !== "")
        .join(", ");
  }
}

/**
 * A result as the value of the kind the footer's calculations know: a number, a text, a day or a checkbox. A list, a
 * page or a person stays a formula value (the calculations count it).
 */
export function formulaCellValue(result: FormulaResult): PropertyValue {
  switch (result.type) {
    case "number":
      return { kind: "number", value: result.data };
    case "text":
      return { kind: "text", value: result.data };
    case "boolean":
      return { kind: "checkbox", value: result.data };
    case "date":
      return { kind: "date", value: result.data };
    default:
      return { kind: "formula", value: result };
  }
}
