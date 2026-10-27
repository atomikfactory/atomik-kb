// The names of the language: every function with its arity (section 3), the keywords and the variables bound inside
// list functions. A `Map`, never an object literal, so that `constructor` or `toString` are not functions.

/** `[min, max]` arguments, `max` -1 for no limit. */
export type Arity = readonly [min: number, max: number];

const ENTRIES: readonly (readonly [string, Arity])[] = [
  // 3.1 logic
  ["if", [3, 3]],
  ["ifs", [3, -1]],
  ["and", [2, -1]],
  ["or", [2, -1]],
  ["not", [1, 1]],
  ["empty", [1, 1]],
  ["equal", [2, 2]],
  ["unequal", [2, 2]],
  // 3.3 numbers
  ["add", [2, 2]],
  ["subtract", [2, 2]],
  ["multiply", [2, 2]],
  ["divide", [2, 2]],
  ["mod", [2, 2]],
  ["pow", [2, 2]],
  ["abs", [1, 1]],
  ["ceil", [1, 1]],
  ["floor", [1, 1]],
  ["sqrt", [1, 1]],
  ["cbrt", [1, 1]],
  ["exp", [1, 1]],
  ["ln", [1, 1]],
  ["log10", [1, 1]],
  ["log2", [1, 1]],
  ["sign", [1, 1]],
  ["pi", [0, 0]],
  ["e", [0, 0]],
  ["round", [1, 2]],
  ["toNumber", [1, 1]],
  ["min", [1, -1]],
  ["max", [1, -1]],
  ["sum", [1, -1]],
  ["mean", [1, -1]],
  ["median", [1, -1]],
  // 3.4 text
  ["length", [1, 1]],
  ["substring", [2, 3]],
  ["contains", [2, 2]],
  ["test", [2, 2]],
  ["match", [2, 2]],
  ["replace", [3, 3]],
  ["replaceAll", [3, 3]],
  ["split", [2, 2]],
  ["join", [2, 2]],
  ["lower", [1, 1]],
  ["upper", [1, 1]],
  ["trim", [1, 1]],
  ["repeat", [2, 2]],
  ["padStart", [3, 3]],
  ["padEnd", [3, 3]],
  ["format", [1, 1]],
  // 3.5 dates
  ["now", [0, 0]],
  ["today", [0, 0]],
  ["minute", [1, 1]],
  ["hour", [1, 1]],
  ["day", [1, 1]],
  ["date", [1, 1]],
  ["week", [1, 1]],
  ["month", [1, 1]],
  ["year", [1, 1]],
  ["dateAdd", [3, 3]],
  ["dateSubtract", [3, 3]],
  ["dateBetween", [3, 3]],
  ["formatDate", [2, 2]],
  ["parseDate", [1, 1]],
  ["timestamp", [1, 1]],
  ["fromTimestamp", [1, 1]],
  // 3.6 lists
  ["at", [2, 2]],
  ["first", [1, 1]],
  ["last", [1, 1]],
  ["slice", [2, 3]],
  ["concat", [1, -1]],
  ["sort", [1, 1]],
  ["reverse", [1, 1]],
  ["unique", [1, 1]],
  ["includes", [2, 2]],
  ["flat", [1, 1]],
  ["map", [2, 2]],
  ["filter", [2, 2]],
  ["find", [2, 2]],
  ["findIndex", [2, 2]],
  ["some", [2, 2]],
  ["every", [2, 2]],
  ["count", [1, 2]],
  // 3.7 pages, people and the rest
  ["prop", [1, 2]],
  ["id", [0, 1]],
  ["name", [1, 1]],
  ["email", [1, 1]],
  ["let", [3, 3]],
  ["lets", [3, -1]],
];

/** The arity of every function of the language, by name. */
export const ARITY: ReadonlyMap<string, Arity> = new Map(ENTRIES);

/** The functions that take an odd number of arguments (`ifs`: pairs and an else; `lets`: pairs and a body). */
export const ODD_ARGUMENTS: ReadonlySet<string> = new Set(["ifs", "lets"]);

/** The keywords of the lexical grammar. */
export const KEYWORDS: ReadonlySet<string> = new Set(["true", "false", "not", "and", "or"]);

/** The variables bound inside the expression argument of a list function. */
export const LIST_VARIABLES: ReadonlySet<string> = new Set(["current", "index"]);

/** Whether `name` is a function of the language. */
export function isFunctionName(name: string): boolean {
  return ARITY.has(name);
}

/** Whether a `let` may not use the name: a function, `current` or `index` (keywords never parse as names). */
export function isReservedName(name: string): boolean {
  return ARITY.has(name) || LIST_VARIABLES.has(name);
}
