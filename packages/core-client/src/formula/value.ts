// Values at run time (section 2.4) and the small helpers on them: empty is `null`; dates are local "floating" instants
// (`ms` since 1970-01-01T00:00 local time, plus whether a time of day was given); pages carry how far from the formula's
// page they were reached (`hop`, section 4.3).

import { MAX_FORMULA_LIST_ITEMS, MAX_FORMULA_TEXT_CHARS } from "./limits";

export interface DateVal {
  readonly k: "date";
  readonly ms: number;
  readonly time: boolean;
}
export interface PageVal {
  readonly k: "page";
  readonly id: string;
  /** 1 for a page of this page's relation (its `.prop` reads), 2 for one reached through a related page (it does not). */
  readonly hop: number;
}
export interface PersonVal {
  readonly k: "person";
  readonly id: string;
}
export type Value = null | number | string | boolean | DateVal | PageVal | PersonVal | Value[];

export function isDate(v: Value): v is DateVal {
  return typeof v === "object" && v !== null && !Array.isArray(v) && v.k === "date";
}
export function isPage(v: Value): v is PageVal {
  return typeof v === "object" && v !== null && !Array.isArray(v) && v.k === "page";
}
export function isPerson(v: Value): v is PersonVal {
  return typeof v === "object" && v !== null && !Array.isArray(v) && v.k === "person";
}

/** A number that is finite (`-0` becomes `0`), else empty: the rule of every arithmetic result. */
export function finite(x: number): number | null {
  if (!Number.isFinite(x)) {
    return null;
  }
  return x === 0 ? 0 : x;
}

/** The argument as a number, or `null` for empty (and for anything else, which the checker keeps out). */
export function num(v: Value): number | null {
  return typeof v === "number" ? v : null;
}

/** The argument as a text: empty reads `""`. */
export function text(v: Value): string {
  return typeof v === "string" ? v : "";
}

/** The argument as a list: empty reads `[]`. */
export function list(v: Value): Value[] {
  return Array.isArray(v) ? v : [];
}

/** The number of code points of a text. */
export function cpLen(s: string): number {
  let n = 0;
  for (let i = 0; i < s.length; i += 1) {
    const c = s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
      const d = s.charCodeAt(i + 1);
      if (d >= 0xdc00 && d <= 0xdfff) {
        i += 1;
      }
    }
    n += 1;
  }
  return n;
}

function hasSurrogates(s: string): boolean {
  for (let i = 0; i < s.length; i += 1) {
    const c = s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdfff) {
      return true;
    }
  }
  return false;
}

/** `s.slice(a, b)` counted in code points (both clamped to the text). */
export function cpSlice(s: string, a: number, b?: number): string {
  if (!hasSurrogates(s)) {
    return s.slice(a, b);
  }
  return Array.from(s).slice(a, b).join("");
}

/** Compares two texts by code point (not by UTF-16 unit): -1, 0 or 1. */
export function cmpText(a: string, b: string): number {
  if (a === b) {
    return 0;
  }
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const ca = a.codePointAt(i) ?? 0;
    const cb = b.codePointAt(j) ?? 0;
    if (ca !== cb) {
      return ca < cb ? -1 : 1;
    }
    i += ca > 0xffff ? 2 : 1;
    j += cb > 0xffff ? 2 : 1;
  }
  if (i >= a.length && j >= b.length) {
    return 0;
  }
  return i >= a.length ? -1 : 1;
}

function isWhite(c: number): boolean {
  return (
    (c >= 0x09 && c <= 0x0d) ||
    c === 0x20 ||
    c === 0x85 ||
    c === 0xa0 ||
    c === 0x1680 ||
    (c >= 0x2000 && c <= 0x200a) ||
    c === 0x2028 ||
    c === 0x2029 ||
    c === 0x202f ||
    c === 0x205f ||
    c === 0x3000
  );
}

/** Trims Unicode white space (the `White_Space` property, as Rust's `trim`; not JavaScript's, which adds U+FEFF). */
export function trimWhite(s: string): string {
  let a = 0;
  let b = s.length;
  while (a < b && isWhite(s.charCodeAt(a))) {
    a += 1;
  }
  while (b > a && isWhite(s.charCodeAt(b - 1))) {
    b -= 1;
  }
  return s.slice(a, b);
}

/** Whether a text is within the engine's limit (`MAX_TEXT_CHARS` characters). */
export function textFits(s: string): boolean {
  return s.length <= MAX_FORMULA_TEXT_CHARS || cpLen(s) <= MAX_FORMULA_TEXT_CHARS;
}

/** The limits of an operation's result: a number that is not finite, a text or a list that is too long, is empty. */
export function finish(v: Value): Value {
  if (typeof v === "number") {
    return finite(v);
  }
  if (typeof v === "string") {
    return textFits(v) ? v : null;
  }
  if (Array.isArray(v)) {
    return v.length > MAX_FORMULA_LIST_ITEMS ? null : v;
  }
  return v;
}

/** Whether two values are equal as `==` says: lists item by item, dates by instant, empty only to empty. */
export function equalValues(a: Value, b: Value): boolean {
  if (a === null || b === null) {
    return a === b;
  }
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) {
      return false;
    }
    return a.every((item, i) => equalValues(item, b[i] ?? null));
  }
  if (Array.isArray(b)) {
    return false;
  }
  if (typeof a === "object") {
    if (typeof b !== "object") {
      return false;
    }
    if (a.k === "date") {
      return b.k === "date" && a.ms === b.ms;
    }
    return a.k === b.k && a.id === (b as { id?: string }).id;
  }
  return a === b;
}

/** A key that is the same for two values that `equalValues` finds equal (for `unique` and `includes`). */
export function valueKey(v: Value): string {
  if (v === null) {
    return "e";
  }
  if (Array.isArray(v)) {
    return `l[${v.map(valueKey).join(",")}]`;
  }
  switch (typeof v) {
    case "number":
      return `n${String(v)}`;
    case "string":
      return `s${v}`;
    case "boolean":
      return v ? "b1" : "b0";
    default:
      return v.k === "date" ? `d${String(v.ms)}` : `${v.k === "page" ? "p" : "a"}${v.id}`;
  }
}

/**
 * Applies section 4.4 to a value: empty, `""` and `[]` become empty; a list loses its empty items; limits are enforced;
 * dates are kept inside years 1..9999 where they are made.
 */
export function normalize(v: Value): Value {
  if (v === null) {
    return null;
  }
  if (typeof v === "number") {
    return finite(v);
  }
  if (typeof v === "string") {
    return v === "" || !textFits(v) ? null : v;
  }
  if (Array.isArray(v)) {
    const out: Value[] = [];
    for (const item of v) {
      const kept = normalize(item);
      if (kept !== null) {
        out.push(kept);
      }
    }
    return out.length === 0 || out.length > MAX_FORMULA_LIST_ITEMS ? null : out;
  }
  return v;
}
