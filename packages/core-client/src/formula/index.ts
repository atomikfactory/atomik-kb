// Formulas (ADR-0055, `docs/architecture/formulas.md`): the TypeScript twin of `kb-model`'s `formula` module. The
// in-memory core (`MemoryProperties`) computes formula values with it, and the UI's formula editor tokenizes, converts,
// checks and previews with it. Both engines run the shared corpus of `crates/kb-model/tests/formula-cases/`.
//
// The engine is split over the files of this folder: `lexer`, `parser`, `check` (types, names, cycles), `evaluate`,
// `functions`, `dates`, `format`, `convert` (names and ids), `tokens` (the editor's tokenizer), `catalog` (the reference).
// Positions are code points.

import type { FormulaResult } from "../generated/FormulaResult";
import type { FormulaType } from "../generated/FormulaType";
import type { PageId } from "../generated/PageId";
import { Checker } from "./check";
import { referencedIds, remapIds, toDisplay, toStored, toStoredMapped } from "./convert";
import { Fail, stackError } from "./errors";
import { evaluateStored } from "./evaluate";
import { formatResult } from "./format";
import { parseCached } from "./parser";
import { tokenize } from "./tokens";
import { formulaType } from "./ty";
import type {
  FormulaEnv,
  FormulaError,
  FormulaHost,
  FormulaSchema,
  FormulaToken,
  ParsedFormula,
} from "./types";

export {
  FORMULA_TITLE_ID,
  MAX_FORMULA_CHARS,
  MAX_FORMULA_DEPTH,
  MAX_FORMULA_LIST_ITEMS,
  MAX_FORMULA_STEPS,
} from "./limits";
export { FORMULA_FUNCTIONS } from "./catalog";
export type {
  FormulaEnv,
  FormulaError,
  FormulaErrorCode,
  FormulaFunctionInfo,
  FormulaHost,
  FormulaProperty,
  FormulaSchema,
  FormulaToken,
  ParsedFormula,
} from "./types";

/** Parses an expression (stored or display: same grammar). */
export function parseFormula(text: string): ParsedFormula | FormulaError {
  return parseCached(text);
}

/** Whether a parse or check answered an error. */
export function isFormulaError(value: unknown): value is FormulaError {
  return typeof value === "object" && value !== null && "code" in value && "start" in value && "end" in value;
}

/** Checks a stored expression against the schema; `selfId` is the formula's own property id. */
export function checkFormula(stored: string, schema: FormulaSchema, selfId?: string): FormulaType | FormulaError {
  try {
    return formulaType(new Checker(schema, selfId).checkText(stored).ty);
  } catch (error) {
    if (error instanceof Fail) {
      return error.error;
    }
    if (error instanceof RangeError) {
      return stackError(Array.from(stored).length);
    }
    throw error;
  }
}

/**
 * Checks an expression in display form (property names): converts it, checks it, and answers an error with its range in
 * the display text (the range of `checkFormula` is in the stored text, where ids replace the names). For the editor.
 */
export function checkDisplayFormula(
  display: string,
  schema: FormulaSchema,
  titleLabel: string,
  selfId?: string,
): FormulaType | FormulaError {
  const converted = toStoredMapped(display, schema, titleLabel);
  if ("code" in converted) {
    return converted;
  }
  const checked = checkFormula(converted.stored, schema, selfId);
  if (!isFormulaError(checked)) {
    return checked;
  }
  return { ...checked, start: converted.toDisplay(checked.start), end: converted.toDisplay(checked.end) };
}

/** Evaluates a stored expression on a page; `null` when the result is empty (section 4.4). */
export function evaluateFormula(stored: string, page: PageId, host: FormulaHost, env: FormulaEnv): FormulaResult | null {
  return evaluateStored(stored, page, host, env);
}

/** Display to stored: names of `prop("…")` and `.prop("…")` to ids; `titleLabel` is the UI's name of the title column. */
export function formulaToStored(display: string, schema: FormulaSchema, titleLabel: string): string | FormulaError {
  return toStored(display, schema, titleLabel);
}

/**
 * Stored to display: ids to names (an unknown id stays). With `titleLabel`, `prop("title")` shows as the label of the
 * title column (unless a property of the schema is called so: the text would not convert back).
 */
export function formulaToDisplay(stored: string, schema: FormulaSchema, titleLabel?: string): string {
  return toDisplay(stored, schema, titleLabel);
}

/** Rewrites the ids of `prop` calls through `map` (an id it does not map stays). */
export function remapFormulaIds(stored: string, map: (id: string) => string | undefined): string {
  return remapIds(stored, map);
}

/** The ids the `prop` calls name, in order, without repeats. */
export function formulaReferencedIds(stored: string): string[] {
  return referencedIds(stored);
}

/** Splits any text into tokens covering it entirely, white space excepted; never throws. */
export function tokenizeFormula(text: string): FormulaToken[] {
  return tokenize(text);
}

/** Formats a value as `format()` does (section 3.4.2), with the host for page titles and people. */
export function formatFormulaValue(result: FormulaResult, host: Pick<FormulaHost, "title" | "person">): string {
  return formatResult(result, host);
}
