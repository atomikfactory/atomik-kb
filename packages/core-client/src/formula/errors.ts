// How the engine's modules report a formula error: `failAt` throws a `Fail`, and the public functions catch it and
// answer the plain `FormulaError`.

import type { FormulaError, FormulaErrorCode } from "./types";

/** Thrown inside the engine; never escapes `index.ts`. */
export class Fail extends Error {
  readonly error: FormulaError;

  constructor(error: FormulaError) {
    super(error.message);
    this.name = "Fail";
    this.error = error;
  }
}

/** A `FormulaError` value. */
export function makeError(
  code: FormulaErrorCode,
  message: string,
  start: number,
  end: number,
  detail = "",
): FormulaError {
  return { code, message, start, end, detail };
}

/** Throws a `Fail` for the range `start..end` (code points, `end` exclusive). */
export function failAt(code: FormulaErrorCode, message: string, start: number, end: number, detail = ""): never {
  throw new Fail(makeError(code, message, start, end, detail));
}

/** The error of a text that nests deeper than the engine's stack can follow. */
export function stackError(end: number): FormulaError {
  return makeError("too_long", "the formula is nested too deeply", 0, end, "depth");
}
