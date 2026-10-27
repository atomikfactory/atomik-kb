// The lexer (section 2.1). Works on an array of code points so that every position is a code point offset, and never
// throws: a lexical error is recorded on the token it belongs to (`err`), so that the parser can raise it when it reaches
// the token (the earliest error in the text wins) and the editor's tokenizer can still colour the text.

import { makeError } from "./errors";
import { KEYWORDS } from "./names";
import type { FormulaError } from "./types";

export type TokKind = "num" | "str" | "ident" | "kw" | "punct" | "op" | "comment" | "error" | "eof";

export interface Tok {
  readonly kind: TokKind;
  readonly start: number;
  readonly end: number;
  readonly text: string;
  /** A number token's value, or a string token's decoded content. */
  readonly value?: number | string;
  /** The lexical error found in this token (an unclosed comment or string, a bad escape, a number out of range). */
  readonly err?: FormulaError;
}

const PUNCT: ReadonlySet<string> = new Set(["(", ")", "[", "]", ",", ".", "?", ":"]);
const OPS_ONE: ReadonlySet<string> = new Set(["!", "<", ">", "+", "-", "*", "/", "%", "^"]);
const OPS_TWO: ReadonlySet<string> = new Set(["&&", "||", "==", "!=", "<=", ">="]);

function isDigit(c: string): boolean {
  return c >= "0" && c <= "9";
}

function isIdentStart(c: string): boolean {
  return (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c === "_";
}

function isIdentPart(c: string): boolean {
  return isIdentStart(c) || isDigit(c);
}

/** Splits the code points of an expression into tokens, white space excepted. */
export function scan(chars: readonly string[]): Tok[] {
  const toks: Tok[] = [];
  const n = chars.length;
  const at = (k: number): string => chars[k] ?? "";
  const slice = (a: number, b: number): string => chars.slice(a, b).join("");
  let i = 0;
  while (i < n) {
    const c = at(i);
    if (c === " " || c === "\t" || c === "\r" || c === "\n") {
      i += 1;
      continue;
    }
    const start = i;

    // comment
    if (c === "/" && at(i + 1) === "*") {
      let j = i + 2;
      let closed = false;
      while (j < n) {
        if (at(j) === "*" && at(j + 1) === "/") {
          j += 2;
          closed = true;
          break;
        }
        j += 1;
      }
      const err = closed ? undefined : makeError("syntax", "unclosed comment", start, j, "*/");
      toks.push({ kind: "comment", start, end: j, text: slice(start, j), ...(err ? { err } : {}) });
      i = j;
      continue;
    }

    // string
    if (c === '"') {
      let j = i + 1;
      let value = "";
      let closed = false;
      let escapeError: FormulaError | undefined;
      while (j < n) {
        const d = at(j);
        if (d === '"') {
          j += 1;
          closed = true;
          break;
        }
        if (d === "\\") {
          const e = at(j + 1);
          if (e === '"') {
            value += '"';
          } else if (e === "\\") {
            value += "\\";
          } else if (e === "n") {
            value += "\n";
          } else if (e === "t") {
            value += "\t";
          } else {
            escapeError ??= makeError("syntax", "unknown escape in a text", j, Math.min(j + 2, n), "\\");
          }
          j += j + 1 < n ? 2 : 1;
          continue;
        }
        value += d;
        j += 1;
      }
      const err = closed ? escapeError : makeError("syntax", "unclosed text", start, n, '"');
      toks.push({ kind: "str", start, end: j, text: slice(start, j), value, ...(err ? { err } : {}) });
      i = j;
      continue;
    }

    // number
    if (isDigit(c) || (c === "." && isDigit(at(i + 1)))) {
      let j = i;
      while (isDigit(at(j))) {
        j += 1;
      }
      if (at(j) === "." && isDigit(at(j + 1))) {
        j += 1;
        while (isDigit(at(j))) {
          j += 1;
        }
      }
      if (at(j) === "e" || at(j) === "E") {
        let k = j + 1;
        if (at(k) === "+" || at(k) === "-") {
          k += 1;
        }
        if (isDigit(at(k))) {
          while (isDigit(at(k))) {
            k += 1;
          }
          j = k;
        }
      }
      const text = slice(start, j);
      const value = Number(text);
      const err = Number.isFinite(value) ? undefined : makeError("syntax", "number out of range", start, j, "number");
      toks.push({ kind: "num", start, end: j, text, value, ...(err ? { err } : {}) });
      i = j;
      continue;
    }

    // identifier or keyword
    if (isIdentStart(c)) {
      let j = i + 1;
      while (isIdentPart(at(j))) {
        j += 1;
      }
      const text = slice(start, j);
      toks.push({ kind: KEYWORDS.has(text) ? "kw" : "ident", start, end: j, text });
      i = j;
      continue;
    }

    // operators and punctuation
    const two = c + at(i + 1);
    if (OPS_TWO.has(two)) {
      toks.push({ kind: "op", start, end: i + 2, text: two });
      i += 2;
      continue;
    }
    if (OPS_ONE.has(c)) {
      toks.push({ kind: "op", start, end: i + 1, text: c });
      i += 1;
      continue;
    }
    if (PUNCT.has(c)) {
      toks.push({ kind: "punct", start, end: i + 1, text: c });
      i += 1;
      continue;
    }

    toks.push({
      kind: "error",
      start,
      end: i + 1,
      text: c,
      err: makeError("syntax", `unexpected character "${c}"`, start, i + 1, "character"),
    });
    i += 1;
  }
  return toks;
}
