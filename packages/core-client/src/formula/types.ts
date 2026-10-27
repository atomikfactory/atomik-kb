// The public types of the formula engine (ADR-0055, `docs/architecture/formulas.md` sections 4.5 and 7.2). They live in
// their own file so that the engine's modules can import them without a cycle through `index.ts`, which re-exports them.

import type { AccountId } from "../generated/AccountId";
import type { AttachmentId } from "../generated/AttachmentId";
import type { PageId } from "../generated/PageId";
import type { PropertyKind } from "../generated/PropertyKind";
import type { PropertyValue } from "../generated/PropertyValue";
import type { RollupFunction } from "../generated/RollupFunction";

/** The codes of section 4.5. */
export type FormulaErrorCode =
  | "syntax"
  | "too_long"
  | "unknown_property"
  | "unsupported_property"
  | "unknown_function"
  | "unknown_variable"
  | "arity"
  | "type"
  | "cycle";

/** A parse or check error: positions in code points of the text given, `end` exclusive. */
export interface FormulaError {
  readonly code: FormulaErrorCode;
  /** English, for logs. The UI translates `code` and `detail`. */
  readonly message: string;
  readonly start: number;
  readonly end: number;
  /** A name, `"f:min:max"`, `"expected:found"`… (section 4.5). */
  readonly detail: string;
}

/** The time of a read. */
export interface FormulaEnv {
  readonly nowMs: number;
  /** The device's offset from UTC in seconds, east positive (`-new Date().getTimezoneOffset() * 60`). */
  readonly offsetSeconds: number;
}

/** A property as a formula sees it (`FormulaProperty` in Rust). */
export interface FormulaProperty {
  readonly id: string;
  readonly name: string;
  readonly kind: PropertyKind;
  readonly options: readonly { readonly id: string; readonly name: string }[];
  /** A formula's stored expression. */
  readonly expression?: string;
  /** A rollup's function and the kind of its target (`null` = the title). */
  readonly rollup?: { readonly function: RollupFunction; readonly targetKind: PropertyKind | null };
}

/** The schema a formula is checked against, by stored id (never `"title"`). */
export interface FormulaSchema {
  property(id: string): FormulaProperty | undefined;
  /** Every property in schema order (for name lookups and completion). */
  properties(): readonly FormulaProperty[];
}

/** The pages a formula reads. */
export interface FormulaHost extends FormulaSchema {
  title(page: PageId): string | undefined;
  /** A stored value or a rollup's value (never a formula's). */
  value(page: PageId, property: string): PropertyValue | undefined;
  /** The live related pages of a relation, in order. */
  related(page: PageId, relation: string): readonly PageId[];
  createdMs(page: PageId): number | undefined;
  editedMs(page: PageId): number | undefined;
  person(account: AccountId): { readonly name: string; readonly email: string } | undefined;
  fileName(attachment: AttachmentId): string | undefined;
}

/** A parsed expression (opaque outside this folder). */
export interface ParsedFormula {
  readonly text: string;
}

/** A token for the editor's highlighting and completion. */
export interface FormulaToken {
  readonly kind:
    | "number"
    | "string"
    | "ident"
    | "function"
    | "property"
    | "keyword"
    | "operator"
    | "punct"
    | "comment"
    | "error";
  readonly start: number;
  readonly end: number;
  readonly text: string;
}

/** One function of the language, for the editor's reference and completion. */
export interface FormulaFunctionInfo {
  readonly name: string;
  readonly category: "logic" | "number" | "text" | "date" | "list" | "page" | "special";
  /** Signatures as text, e.g. `"round(number, [places]) → number"`. */
  readonly signatures: readonly string[];
  /** An example in display form, e.g. `round(prop("Score"), 1)`. */
  readonly example: string;
}
