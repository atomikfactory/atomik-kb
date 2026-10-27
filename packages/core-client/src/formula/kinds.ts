// The property kinds as formula types (the table of section 2.3).

import type { PropertyKind } from "../generated/PropertyKind";
import type { FormulaProperty } from "./types";
import { BOOLEAN, DATE, NUMBER, TEXT, listOf } from "./ty";
import type { Ty } from "./ty";

/** The base a rollup's `show_original` lists, by the kind of its target (`null` is the title: text). */
function originalBase(target: PropertyKind | null): "number" | "text" | "boolean" | "date" | "person" | "page" {
  switch (target) {
    case "person":
      return "person";
    case "relation":
      return "page";
    case "number":
      return "number";
    case "date":
      return "date";
    case "checkbox":
    case "checklist":
      return "boolean";
    default:
      return "text";
  }
}

/**
 * The type a property has in a formula: a `Ty`; `"formula"` for a formula property (its type is the result type of its
 * own expression); `"unsupported"` for `created_by` and `last_edited_by` (they are devices, not people) and for a rollup
 * without its configuration.
 */
export function kindType(prop: FormulaProperty): Ty | "formula" | "unsupported" {
  switch (prop.kind) {
    case "text":
    case "url":
    case "email":
    case "phone":
    case "select":
    case "status":
    case "place":
      return TEXT;
    case "number":
      return NUMBER;
    case "multi_select":
    case "files":
      return listOf("text");
    case "checklist":
      return listOf("boolean");
    case "date":
    case "created_time":
    case "last_edited_time":
      return DATE;
    case "checkbox":
      return BOOLEAN;
    case "person":
      return listOf("person");
    case "relation":
      return listOf("page");
    case "rollup": {
      if (prop.rollup === undefined) {
        return "unsupported";
      }
      switch (prop.rollup.function) {
        case "earliest":
        case "latest":
          return DATE;
        case "show_original":
          return listOf(originalBase(prop.rollup.targetKind));
        default:
          return NUMBER;
      }
    }
    case "formula":
      return "formula";
    case "created_by":
    case "last_edited_by":
      return "unsupported";
  }
}
