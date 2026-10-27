// Static types of the checker (section 2.3). `base: null` is the open type of what an empty list `[]` holds (or of an
// item taken from one): it unifies with every base.

import type { FormulaBase } from "../generated/FormulaBase";
import type { FormulaType } from "../generated/FormulaType";
import { failAt } from "./errors";
import type { Node } from "./parser";

export interface Ty {
  readonly base: FormulaBase | null;
  readonly list: boolean;
}

export const NUMBER: Ty = { base: "number", list: false };
export const TEXT: Ty = { base: "text", list: false };
export const BOOLEAN: Ty = { base: "boolean", list: false };
export const DATE: Ty = { base: "date", list: false };
export const PAGE: Ty = { base: "page", list: false };
export const PERSON: Ty = { base: "person", list: false };

export function scalar(base: FormulaBase | null): Ty {
  return { base, list: false };
}

export function listOf(base: FormulaBase | null): Ty {
  return { base, list: true };
}

/** `number`, `list<text>`, `any` for the open type. */
export function describe(t: Ty): string {
  const base = t.base ?? "any";
  return t.list ? `list<${base}>` : base;
}

/** The type both sides agree on, or `null` when they do not. */
export function unify(a: Ty, b: Ty): Ty | null {
  if (a.list !== b.list) {
    return null;
  }
  if (a.base === null) {
    return b;
  }
  if (b.base === null || a.base === b.base) {
    return a;
  }
  return null;
}

/** The result type a formula reports: an open type is text. */
export function formulaType(t: Ty): FormulaType {
  return { base: t.base ?? "text", list: t.list };
}

/** A `type` error at `node`: `expected` is a type name (or a description), `found` the type met. */
export function typeFail(node: Node, expected: string, found: Ty): never {
  return failAt("type", `expected ${expected}, found ${describe(found)}`, node.s, node.e, `${expected}:${describe(found)}`);
}

/** The node's type must be `base` (a scalar). */
export function need(ty: Ty, base: FormulaBase, node: Node): void {
  if (ty.list || (ty.base !== null && ty.base !== base)) {
    typeFail(node, base, ty);
  }
}

/** The node's type must be a list; answers it. */
export function needList(ty: Ty, node: Node): Ty {
  if (!ty.list) {
    return typeFail(node, "list<any>", ty);
  }
  return ty;
}

/** The node's type must be a number or a list of numbers (the `numbers…` of section 3.3). */
export function needNumbers(ty: Ty, node: Node): void {
  if (ty.base !== null && ty.base !== "number") {
    typeFail(node, "number", ty);
  }
}
