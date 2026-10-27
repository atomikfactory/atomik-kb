// The functions of section 3 that take their arguments evaluated (strict): for each, its type rule (`check`) and its
// behaviour (`run`). The special forms (`if`, `ifs`, `and`, `or`, `let`, `lets`, `prop`, `id` and the list functions
// that take an expression) are in `check.ts` and `evaluate.ts`. Empty arguments: a number or date that is empty makes the
// result empty; an empty text reads `""`, an empty list `[]`, an empty boolean `false`.

import {
  addToDate,
  dateDifference,
  formatDate,
  hourOf,
  isoWeek,
  isoWeekday,
  localMs,
  makeDate,
  minuteOf,
  parseDateText,
  parseUnit,
  partsOf,
  DAY_MS,
} from "./dates";
import { formatValue } from "./format";
import { MAX_FORMULA_TEXT_CHARS } from "./limits";
import type { CallNode } from "./parser";
import type { Runtime } from "./runtime";
import { allMatches, firstMatch, replaceMatches } from "./text";
import { BOOLEAN, DATE, NUMBER, TEXT, describe, listOf, need, needList, needNumbers, typeFail, unify } from "./ty";
import type { Ty } from "./ty";
import {
  cmpText,
  cpLen,
  cpSlice,
  equalValues,
  finish,
  isDate,
  isPage,
  isPerson,
  list,
  num,
  text,
  trimWhite,
  valueKey,
} from "./value";
import type { DateVal, Value } from "./value";

/** The type rule and the behaviour of one function. */
export interface FnDef {
  /** The type of the call, given the types of its arguments; throws a `Fail` (type errors at the argument at fault). */
  check(types: readonly Ty[], call: CallNode): Ty;
  /** The value of the call, given its evaluated arguments. The result goes through `finish` in the caller. */
  run(args: readonly Value[], rt: Runtime): Value;
}

function nth<T>(items: readonly T[], i: number): T {
  const item = items[i];
  if (item === undefined) {
    throw new Error("formula: missing argument");
  }
  return item;
}

function at(args: readonly Value[], i: number): Value {
  return args[i] ?? null;
}

const FUNCTIONS = new Map<string, FnDef>();

function define(name: string, def: FnDef): void {
  FUNCTIONS.set(name, def);
}

// --- 3.1 logic ---

define("not", {
  check: (t, c) => {
    need(nth(t, 0), "boolean", nth(c.args, 0));
    return BOOLEAN;
  },
  run: (a) => at(a, 0) !== true,
});

define("empty", {
  check: () => BOOLEAN,
  run: (a) => {
    const v = at(a, 0);
    return v === null || v === "" || v === 0 || v === false || (Array.isArray(v) && v.length === 0);
  },
});

function sameType(t: readonly Ty[], c: CallNode): void {
  const left = nth(t, 0);
  const right = nth(t, 1);
  if (unify(left, right) === null) {
    typeFail(nth(c.args, 1), describe(left), right);
  }
}

define("equal", {
  check: (t, c) => {
    sameType(t, c);
    return BOOLEAN;
  },
  run: (a) => equalValues(at(a, 0), at(a, 1)),
});

define("unequal", {
  check: (t, c) => {
    sameType(t, c);
    return BOOLEAN;
  },
  run: (a) => !equalValues(at(a, 0), at(a, 1)),
});

// --- 3.3 numbers ---

function twoNumbers(f: (a: number, b: number) => number): FnDef {
  return {
    check: (t, c) => {
      need(nth(t, 0), "number", nth(c.args, 0));
      need(nth(t, 1), "number", nth(c.args, 1));
      return NUMBER;
    },
    run: (a) => {
      const x = num(at(a, 0));
      const y = num(at(a, 1));
      return x === null || y === null ? null : f(x, y);
    },
  };
}

function oneNumber(f: (x: number) => number): FnDef {
  return {
    check: (t, c) => {
      need(nth(t, 0), "number", nth(c.args, 0));
      return NUMBER;
    },
    run: (a) => {
      const x = num(at(a, 0));
      return x === null ? null : f(x);
    },
  };
}

define("add", twoNumbers((a, b) => a + b));
define("subtract", twoNumbers((a, b) => a - b));
define("multiply", twoNumbers((a, b) => a * b));
define("divide", twoNumbers((a, b) => a / b));
define("mod", twoNumbers((a, b) => a % b));
define("pow", twoNumbers((a, b) => Math.pow(a, b)));
define("abs", oneNumber(Math.abs));
define("ceil", oneNumber(Math.ceil));
define("floor", oneNumber(Math.floor));
define("sqrt", oneNumber(Math.sqrt));
define("cbrt", oneNumber(Math.cbrt));
define("exp", oneNumber(Math.exp));
define("ln", oneNumber(Math.log));
define("log10", oneNumber(Math.log10));
define("log2", oneNumber(Math.log2));
define("sign", oneNumber((x) => (x > 0 ? 1 : x < 0 ? -1 : 0)));
define("pi", { check: () => NUMBER, run: () => Math.PI });
define("e", { check: () => NUMBER, run: () => Math.E });

const POWERS_OF_TEN = [1, 10, 100, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10];

/** `10^p` for p in -10..10: exact for p >= 0, `1 / 10^-p` below (a correctly rounded division, as in Rust). */
function powerOfTen(p: number): number {
  return p >= 0 ? (POWERS_OF_TEN[p] ?? 1) : 1 / (POWERS_OF_TEN[-p] ?? 1);
}

/** `round(x) = floor(x + 0.5)` (JavaScript's `Math.round` for the numbers that matter). */
function roundHalfUp(x: number): number {
  return Math.floor(x + 0.5);
}

define("round", {
  check: (t, c) => {
    need(nth(t, 0), "number", nth(c.args, 0));
    if (t.length > 1) {
      need(nth(t, 1), "number", nth(c.args, 1));
    }
    return NUMBER;
  },
  run: (a) => {
    const x = num(at(a, 0));
    if (x === null) {
      return null;
    }
    if (a.length < 2) {
      return roundHalfUp(x);
    }
    const places = num(at(a, 1));
    if (places === null) {
      return null;
    }
    const factor = powerOfTen(Math.max(-10, Math.min(10, Math.trunc(places))));
    return roundHalfUp(x * factor) / factor;
  },
});

const NUMBER_TEXT = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/u;

define("toNumber", {
  check: (t, c) => {
    const ty = nth(t, 0);
    if (ty.list || (ty.base !== null && ty.base !== "text" && ty.base !== "number" && ty.base !== "boolean" && ty.base !== "date")) {
      typeFail(nth(c.args, 0), "text", ty);
    }
    return NUMBER;
  },
  run: (a, rt) => {
    const v = at(a, 0);
    if (typeof v === "number") {
      return v;
    }
    if (typeof v === "boolean") {
      return v ? 1 : 0;
    }
    if (isDate(v)) {
      return v.ms - rt.env.offsetSeconds * 1000;
    }
    if (typeof v === "string") {
      const trimmed = trimWhite(v);
      return NUMBER_TEXT.test(trimmed) ? Number(trimmed) : null;
    }
    return null;
  },
});

/** Every number of the arguments, lists flattened, empty items skipped. */
function flattenNumbers(args: readonly Value[]): number[] {
  const found: number[] = [];
  for (const arg of args) {
    if (typeof arg === "number") {
      found.push(arg);
    } else if (Array.isArray(arg)) {
      for (const item of arg) {
        if (typeof item === "number") {
          found.push(item);
        }
      }
    }
  }
  return found;
}

/** Every date of the arguments, lists flattened, empty items skipped. */
function flattenDates(args: readonly Value[]): DateVal[] {
  const found: DateVal[] = [];
  for (const arg of args) {
    if (isDate(arg)) {
      found.push(arg);
    } else if (Array.isArray(arg)) {
      for (const item of arg) {
        if (isDate(item)) {
          found.push(item);
        }
      }
    }
  }
  return found;
}

/** The numbers of a call that takes `numbers…`: each argument a number or a list of numbers. */
function numbersCheck(t: readonly Ty[], c: CallNode): Ty {
  t.forEach((ty, i) => {
    needNumbers(ty, nth(c.args, i));
  });
  return NUMBER;
}

/** `min` and `max`: numbers, or dates (all of one group), the first argument deciding. */
function extremumCheck(t: readonly Ty[], c: CallNode): Ty {
  const decided = t.find((ty) => ty.base !== null);
  if (decided?.base === "date") {
    t.forEach((ty, i) => {
      if (ty.base !== null && ty.base !== "date") {
        typeFail(nth(c.args, i), "date", ty);
      }
    });
    return DATE;
  }
  return numbersCheck(t, c);
}

function extremum(name: "min" | "max"): FnDef {
  return {
    check: extremumCheck,
    run: (a) => {
      const dates = flattenDates(a);
      if (dates.length > 0) {
        return dates.reduce((best, d) => ((name === "min" ? d.ms < best.ms : d.ms > best.ms) ? d : best));
      }
      const numbers = flattenNumbers(a);
      if (numbers.length === 0) {
        return null;
      }
      return numbers.reduce((best, n) => ((name === "min" ? n < best : n > best) ? n : best));
    },
  };
}

define("min", extremum("min"));
define("max", extremum("max"));

define("sum", {
  check: numbersCheck,
  run: (a) => {
    let total = 0;
    for (const n of flattenNumbers(a)) {
      total += n;
    }
    return total;
  },
});

define("mean", {
  check: numbersCheck,
  run: (a) => {
    const numbers = flattenNumbers(a);
    if (numbers.length === 0) {
      return null;
    }
    let total = 0;
    for (const n of numbers) {
      total += n;
    }
    return total / numbers.length;
  },
});

define("median", {
  check: numbersCheck,
  run: (a) => {
    const numbers = flattenNumbers(a).sort((x, y) => x - y);
    if (numbers.length === 0) {
      return null;
    }
    const mid = Math.floor(numbers.length / 2);
    const high = numbers[mid] ?? 0;
    if (numbers.length % 2 === 1) {
      return high;
    }
    return ((numbers[mid - 1] ?? 0) + high) / 2;
  },
});

// --- 3.4 text ---

function textArgs(t: readonly Ty[], c: CallNode, shape: readonly ("text" | "number")[]): void {
  shape.forEach((base, i) => {
    if (i < t.length) {
      need(nth(t, i), base, nth(c.args, i));
    }
  });
}

define("length", {
  check: (t, c) => {
    const ty = nth(t, 0);
    if (!ty.list) {
      need(ty, "text", nth(c.args, 0));
    }
    return NUMBER;
  },
  run: (a) => {
    const v = at(a, 0);
    return Array.isArray(v) ? v.length : cpLen(text(v));
  },
});

define("substring", {
  check: (t, c) => {
    textArgs(t, c, ["text", "number", "number"]);
    return TEXT;
  },
  run: (a) => {
    const s = text(at(a, 0));
    const start = num(at(a, 1));
    if (start === null) {
      return null;
    }
    let end: number | undefined;
    if (a.length > 2) {
      const e = num(at(a, 2));
      if (e === null) {
        return null;
      }
      end = Math.max(0, Math.trunc(e));
    }
    const from = Math.max(0, Math.trunc(start));
    return end !== undefined && end < from ? "" : cpSlice(s, from, end);
  },
});

define("contains", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text"]);
    return BOOLEAN;
  },
  run: (a) => text(at(a, 0)).includes(text(at(a, 1))),
});

define("test", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text"]);
    return BOOLEAN;
  },
  run: (a, rt) => {
    const re = rt.pattern(text(at(a, 1)));
    return re === null ? null : firstMatch(re, text(at(a, 0))) !== null;
  },
});

define("match", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text"]);
    return listOf("text");
  },
  run: (a, rt) => {
    const re = rt.pattern(text(at(a, 1)));
    return re === null ? null : allMatches(re, text(at(a, 0))).map((m) => m[0]);
  },
});

define("replace", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text", "text"]);
    return TEXT;
  },
  run: (a, rt) => {
    const re = rt.pattern(text(at(a, 1)));
    if (re === null) {
      return null;
    }
    const s = text(at(a, 0));
    const m = firstMatch(re, s);
    return m === null ? s : replaceMatches(s, [m], text(at(a, 2)));
  },
});

define("replaceAll", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text", "text"]);
    return TEXT;
  },
  run: (a, rt) => {
    const re = rt.pattern(text(at(a, 1)));
    if (re === null) {
      return null;
    }
    const s = text(at(a, 0));
    return replaceMatches(s, allMatches(re, s), text(at(a, 2)));
  },
});

define("split", {
  check: (t, c) => {
    textArgs(t, c, ["text", "text"]);
    return listOf("text");
  },
  run: (a) => {
    const s = text(at(a, 0));
    const sep = text(at(a, 1));
    return sep === "" ? Array.from(s) : s.split(sep);
  },
});

define("join", {
  check: (t, c) => {
    needList(nth(t, 0), nth(c.args, 0));
    need(nth(t, 1), "text", nth(c.args, 1));
    return TEXT;
  },
  run: (a, rt) => {
    const sep = text(at(a, 1));
    return list(at(a, 0))
      .map((v) => formatValue(v, rt.host))
      .join(sep);
  },
});

define("lower", {
  check: (t, c) => {
    textArgs(t, c, ["text"]);
    return TEXT;
  },
  run: (a) => text(at(a, 0)).toLowerCase(),
});

define("upper", {
  check: (t, c) => {
    textArgs(t, c, ["text"]);
    return TEXT;
  },
  run: (a) => text(at(a, 0)).toUpperCase(),
});

define("trim", {
  check: (t, c) => {
    textArgs(t, c, ["text"]);
    return TEXT;
  },
  run: (a) => trimWhite(text(at(a, 0))),
});

define("repeat", {
  check: (t, c) => {
    textArgs(t, c, ["text", "number"]);
    return TEXT;
  },
  run: (a) => {
    const count = num(at(a, 1));
    if (count === null) {
      return null;
    }
    const s = text(at(a, 0));
    const times = Math.max(0, Math.min(MAX_FORMULA_TEXT_CHARS, Math.trunc(count)));
    if (s === "" || times === 0) {
      return "";
    }
    return cpLen(s) * times > MAX_FORMULA_TEXT_CHARS ? null : s.repeat(times);
  },
});

function pad(name: "padStart" | "padEnd"): FnDef {
  return {
    check: (t, c) => {
      textArgs(t, c, ["text", "number", "text"]);
      return TEXT;
    },
    run: (a) => {
      const s = text(at(a, 0));
      const target = num(at(a, 1));
      if (target === null) {
        return null;
      }
      const filler = Array.from(text(at(a, 2)));
      const missing = Math.trunc(target) - cpLen(s);
      if (missing <= 0 || filler.length === 0) {
        return s;
      }
      if (missing > MAX_FORMULA_TEXT_CHARS) {
        return null;
      }
      let fill = "";
      for (let i = 0; i < missing; i += 1) {
        fill += filler[i % filler.length] ?? "";
      }
      return name === "padStart" ? fill + s : s + fill;
    },
  };
}

define("padStart", pad("padStart"));
define("padEnd", pad("padEnd"));

define("format", {
  check: () => TEXT,
  run: (a, rt) => formatValue(at(a, 0), rt.host),
});

// --- 3.5 dates ---

function dateParts(f: (d: DateVal) => number): FnDef {
  return {
    check: (t, c) => {
      need(nth(t, 0), "date", nth(c.args, 0));
      return NUMBER;
    },
    run: (a) => {
      const d = at(a, 0);
      return isDate(d) ? f(d) : null;
    },
  };
}

define("now", {
  check: () => DATE,
  run: (_a, rt) => makeDate(localMs(rt.env.nowMs, rt.env.offsetSeconds), true),
});

define("today", {
  check: () => DATE,
  run: (_a, rt) => makeDate(Math.floor(localMs(rt.env.nowMs, rt.env.offsetSeconds) / DAY_MS) * DAY_MS, false),
});

define("minute", dateParts((d) => minuteOf(partsOf(d.ms))));
define("hour", dateParts((d) => hourOf(partsOf(d.ms))));
define("day", dateParts((d) => isoWeekday(partsOf(d.ms).days)));
define("date", dateParts((d) => partsOf(d.ms).d));
define("week", dateParts((d) => isoWeek(partsOf(d.ms).days)));
define("month", dateParts((d) => partsOf(d.ms).m));
define("year", dateParts((d) => partsOf(d.ms).y));

function dateShift(sign: 1 | -1): FnDef {
  return {
    check: (t, c) => {
      need(nth(t, 0), "date", nth(c.args, 0));
      need(nth(t, 1), "number", nth(c.args, 1));
      need(nth(t, 2), "text", nth(c.args, 2));
      return DATE;
    },
    run: (a) => {
      const d = at(a, 0);
      const n = num(at(a, 1));
      const unit = parseUnit(text(at(a, 2)));
      if (!isDate(d) || n === null || unit === null) {
        return null;
      }
      return addToDate(d, sign * Math.trunc(n), unit);
    },
  };
}

define("dateAdd", dateShift(1));
define("dateSubtract", dateShift(-1));

define("dateBetween", {
  check: (t, c) => {
    need(nth(t, 0), "date", nth(c.args, 0));
    need(nth(t, 1), "date", nth(c.args, 1));
    need(nth(t, 2), "text", nth(c.args, 2));
    return NUMBER;
  },
  run: (a) => {
    const x = at(a, 0);
    const y = at(a, 1);
    const unit = parseUnit(text(at(a, 2)));
    return isDate(x) && isDate(y) && unit !== null ? dateDifference(x, y, unit) : null;
  },
});

define("formatDate", {
  check: (t, c) => {
    need(nth(t, 0), "date", nth(c.args, 0));
    need(nth(t, 1), "text", nth(c.args, 1));
    return TEXT;
  },
  run: (a) => {
    const d = at(a, 0);
    return isDate(d) ? formatDate(d, text(at(a, 1))) : null;
  },
});

define("parseDate", {
  check: (t, c) => {
    need(nth(t, 0), "text", nth(c.args, 0));
    return DATE;
  },
  run: (a) => parseDateText(text(at(a, 0))),
});

define("timestamp", {
  check: (t, c) => {
    need(nth(t, 0), "date", nth(c.args, 0));
    return NUMBER;
  },
  run: (a, rt) => {
    const d = at(a, 0);
    return isDate(d) ? d.ms - rt.env.offsetSeconds * 1000 : null;
  },
});

define("fromTimestamp", {
  check: (t, c) => {
    need(nth(t, 0), "number", nth(c.args, 0));
    return DATE;
  },
  run: (a, rt) => {
    const n = num(at(a, 0));
    return n === null ? null : makeDate(Math.trunc(n) + rt.env.offsetSeconds * 1000, true);
  },
});

// --- 3.6 lists ---

/** The type of an item of the list the first argument is. */
function itemOf(t: readonly Ty[], c: CallNode): Ty {
  const ty = needList(nth(t, 0), nth(c.args, 0));
  return { base: ty.base, list: false };
}

define("at", {
  check: (t, c) => {
    const item = itemOf(t, c);
    need(nth(t, 1), "number", nth(c.args, 1));
    return item;
  },
  run: (a) => {
    const items = list(at(a, 0));
    const n = num(at(a, 1));
    if (n === null) {
      return null;
    }
    const i = Math.trunc(n);
    return items[i < 0 ? items.length + i : i] ?? null;
  },
});

define("first", {
  check: (t, c) => itemOf(t, c),
  run: (a) => list(at(a, 0))[0] ?? null,
});

define("last", {
  check: (t, c) => itemOf(t, c),
  run: (a) => list(at(a, 0)).at(-1) ?? null,
});

define("slice", {
  check: (t, c) => {
    const ty = needList(nth(t, 0), nth(c.args, 0));
    need(nth(t, 1), "number", nth(c.args, 1));
    if (t.length > 2) {
      need(nth(t, 2), "number", nth(c.args, 2));
    }
    return ty;
  },
  run: (a) => {
    const start = num(at(a, 1));
    if (start === null) {
      return null;
    }
    if (a.length > 2) {
      const end = num(at(a, 2));
      return end === null ? null : list(at(a, 0)).slice(Math.trunc(start), Math.trunc(end));
    }
    return list(at(a, 0)).slice(Math.trunc(start));
  },
});

define("concat", {
  check: (t, c) => {
    let joined = needList(nth(t, 0), nth(c.args, 0));
    for (let i = 1; i < t.length; i += 1) {
      const ty = needList(nth(t, i), nth(c.args, i));
      const merged = unify(joined, ty);
      if (merged === null) {
        return typeFail(nth(c.args, i), describe(joined), ty);
      }
      joined = merged;
    }
    return joined;
  },
  run: (a) => a.flatMap((v) => list(v)),
});

function compareValues(x: Value, y: Value, rt: Runtime): number {
  if (typeof x === "number" && typeof y === "number") {
    return x < y ? -1 : x > y ? 1 : 0;
  }
  if (typeof x === "string" && typeof y === "string") {
    return cmpText(x, y);
  }
  if (typeof x === "boolean" && typeof y === "boolean") {
    return Number(x) - Number(y);
  }
  if (isDate(x) && isDate(y)) {
    return x.ms < y.ms ? -1 : x.ms > y.ms ? 1 : 0;
  }
  if (isPage(x) && isPage(y)) {
    return cmpText(rt.host.title(x.id) ?? "", rt.host.title(y.id) ?? "") || cmpText(x.id, y.id);
  }
  if (isPerson(x) && isPerson(y)) {
    return cmpText(rt.host.person(x.id)?.name ?? "", rt.host.person(y.id)?.name ?? "") || cmpText(x.id, y.id);
  }
  return 0;
}

define("sort", {
  check: (t, c) => needList(nth(t, 0), nth(c.args, 0)),
  run: (a, rt) => {
    const items = list(at(a, 0));
    const present = items.filter((v) => v !== null);
    present.sort((x, y) => compareValues(x, y, rt));
    return present.concat(items.filter((v) => v === null));
  },
});

define("reverse", {
  check: (t, c) => needList(nth(t, 0), nth(c.args, 0)),
  run: (a) => [...list(at(a, 0))].reverse(),
});

define("unique", {
  check: (t, c) => needList(nth(t, 0), nth(c.args, 0)),
  run: (a) => {
    const seen = new Set<string>();
    const kept: Value[] = [];
    for (const item of list(at(a, 0))) {
      const key = valueKey(item);
      if (!seen.has(key)) {
        seen.add(key);
        kept.push(item);
      }
    }
    return kept;
  },
});

define("includes", {
  check: (t, c) => {
    const item = itemOf(t, c);
    const found = nth(t, 1);
    if (unify(item, found) === null) {
      typeFail(nth(c.args, 1), describe(item), found);
    }
    return BOOLEAN;
  },
  run: (a) => {
    const wanted = at(a, 1);
    return list(at(a, 0)).some((v) => equalValues(v, wanted));
  },
});

define("flat", {
  check: (t, c) => needList(nth(t, 0), nth(c.args, 0)),
  run: (a) => list(at(a, 0)),
});

// --- 3.7 people ---

define("name", {
  check: (t, c) => {
    need(nth(t, 0), "person", nth(c.args, 0));
    return TEXT;
  },
  run: (a, rt) => {
    const p = at(a, 0);
    return isPerson(p) ? (rt.host.person(p.id)?.name ?? "") : "";
  },
});

define("email", {
  check: (t, c) => {
    need(nth(t, 0), "person", nth(c.args, 0));
    return TEXT;
  },
  run: (a, rt) => {
    const p = at(a, 0);
    return isPerson(p) ? (rt.host.person(p.id)?.email ?? "") : "";
  },
});

/** The functions that take their arguments evaluated, by name. */
export const STRICT_FUNCTIONS: ReadonlyMap<string, FnDef> = FUNCTIONS;

/** Runs a strict function and applies the limits of an operation's result. */
export function runStrict(def: FnDef, args: readonly Value[], rt: Runtime): Value {
  return finish(def.run(args, rt));
}
