// The reference of the language for the editor (sections 3.1 to 3.7): every function with its category, its signatures and
// an example in display form (property names, as a person types them). The descriptions are the UI's own strings.

import type { FormulaFunctionInfo } from "./types";

type Category = FormulaFunctionInfo["category"];

function fn(name: string, category: Category, signatures: readonly string[], example: string): FormulaFunctionInfo {
  return { name, category, signatures, example };
}

/** Every function of section 3, in the order of the reference. */
export const FORMULA_FUNCTIONS: readonly FormulaFunctionInfo[] = [
  // 3.1 logic
  fn("if", "logic", ["if(boolean, T, T) → T"], 'if(prop("Done"), "yes", "no")'),
  fn(
    "ifs",
    "logic",
    ["ifs(boolean, T, boolean, T, …, T) → T"],
    'ifs(prop("Score") > 8, "high", prop("Score") > 4, "mid", "low")',
  ),
  fn("and", "logic", ["and(boolean, boolean, …) → boolean"], 'and(prop("Done"), prop("Score") > 5)'),
  fn("or", "logic", ["or(boolean, boolean, …) → boolean"], 'or(prop("Done"), prop("Score") > 5)'),
  fn("not", "logic", ["not(boolean) → boolean"], 'not(prop("Done"))'),
  fn("empty", "logic", ["empty(T) → boolean", "empty(list<T>) → boolean"], 'empty(prop("Due"))'),
  fn("equal", "logic", ["equal(T, T) → boolean"], 'equal(prop("Status"), "Done")'),
  fn("unequal", "logic", ["unequal(T, T) → boolean"], 'unequal(prop("Status"), "Done")'),

  // 3.3 numbers
  fn("add", "number", ["add(number, number) → number"], 'add(prop("Price"), prop("Qty"))'),
  fn("subtract", "number", ["subtract(number, number) → number"], 'subtract(prop("Price"), 1)'),
  fn("multiply", "number", ["multiply(number, number) → number"], 'multiply(prop("Price"), prop("Qty"))'),
  fn("divide", "number", ["divide(number, number) → number"], 'divide(prop("Price"), prop("Qty"))'),
  fn("mod", "number", ["mod(number, number) → number"], 'mod(prop("Qty"), 2)'),
  fn("pow", "number", ["pow(number, number) → number"], 'pow(prop("Qty"), 2)'),
  fn("abs", "number", ["abs(number) → number"], 'abs(prop("Score") - 10)'),
  fn("ceil", "number", ["ceil(number) → number"], 'ceil(prop("Price"))'),
  fn("floor", "number", ["floor(number) → number"], 'floor(prop("Price"))'),
  fn("sqrt", "number", ["sqrt(number) → number"], 'sqrt(prop("Qty"))'),
  fn("cbrt", "number", ["cbrt(number) → number"], 'cbrt(prop("Qty"))'),
  fn("exp", "number", ["exp(number) → number"], "exp(1)"),
  fn("ln", "number", ["ln(number) → number"], 'ln(prop("Qty"))'),
  fn("log10", "number", ["log10(number) → number"], 'log10(prop("Qty"))'),
  fn("log2", "number", ["log2(number) → number"], 'log2(prop("Qty"))'),
  fn("sign", "number", ["sign(number) → number"], 'sign(prop("Score") - 5)'),
  fn("pi", "number", ["pi() → number"], 'pi() * prop("Qty") ^ 2'),
  fn("e", "number", ["e() → number"], "e() ^ 2"),
  fn("round", "number", ["round(number, [places]) → number"], 'round(prop("Price") * 1.2, 2)'),
  fn(
    "toNumber",
    "number",
    ["toNumber(text) → number", "toNumber(boolean) → number", "toNumber(date) → number"],
    'toNumber(prop("Note"))',
  ),
  fn(
    "min",
    "number",
    ["min(number, …) → number", "min(date, …) → date"],
    'min(prop("Price"), prop("Qty"))',
  ),
  fn(
    "max",
    "number",
    ["max(number, …) → number", "max(date, …) → date"],
    'max(prop("Tasks").map(current.prop("Hours")))',
  ),
  fn("sum", "number", ["sum(number, …) → number"], 'sum(prop("Tasks").map(current.prop("Hours")))'),
  fn("mean", "number", ["mean(number, …) → number"], 'mean(prop("Tasks").map(current.prop("Hours")))'),
  fn("median", "number", ["median(number, …) → number"], 'median(prop("Price"), prop("Qty"), 10)'),

  // 3.4 text
  fn("length", "text", ["length(text) → number", "length(list<T>) → number"], 'length(prop("Note"))'),
  fn("substring", "text", ["substring(text, start, [end]) → text"], 'substring(prop("Note"), 0, 3)'),
  fn("contains", "text", ["contains(text, text) → boolean"], 'contains(prop("Note"), "urgent")'),
  fn("test", "text", ["test(text, pattern) → boolean"], 'test(prop("Note"), "^[A-Z]")'),
  fn("match", "text", ["match(text, pattern) → list<text>"], 'match(prop("Note"), "[0-9]+")'),
  fn("replace", "text", ["replace(text, pattern, text) → text"], 'replace(prop("Note"), "o", "0")'),
  fn("replaceAll", "text", ["replaceAll(text, pattern, text) → text"], 'replaceAll(prop("Note"), "o", "0")'),
  fn("split", "text", ["split(text, text) → list<text>"], 'split(prop("Note"), " ")'),
  fn("join", "text", ["join(list<T>, text) → text"], 'join(prop("Tags"), ", ")'),
  fn("lower", "text", ["lower(text) → text"], 'lower(prop("Note"))'),
  fn("upper", "text", ["upper(text) → text"], 'upper(prop("Note"))'),
  fn("trim", "text", ["trim(text) → text"], 'trim(prop("Note"))'),
  fn("repeat", "text", ["repeat(text, count) → text"], 'repeat("*", round(prop("Score")))'),
  fn("padStart", "text", ["padStart(text, length, text) → text"], 'padStart(format(prop("Qty")), 3, "0")'),
  fn("padEnd", "text", ["padEnd(text, length, text) → text"], 'padEnd(prop("Note"), 12, ".")'),
  fn("format", "text", ["format(T) → text", "format(list<T>) → text"], 'format(prop("Due"))'),

  // 3.5 dates
  fn("now", "date", ["now() → date"], 'dateBetween(now(), prop("Due"), "hours")'),
  fn("today", "date", ["today() → date"], 'dateBetween(prop("Due"), today(), "days")'),
  fn("minute", "date", ["minute(date) → number"], "minute(now())"),
  fn("hour", "date", ["hour(date) → number"], "hour(now())"),
  fn("day", "date", ["day(date) → number"], 'day(prop("Due"))'),
  fn("date", "date", ["date(date) → number"], 'date(prop("Due"))'),
  fn("week", "date", ["week(date) → number"], 'week(prop("Due"))'),
  fn("month", "date", ["month(date) → number"], 'month(prop("Due"))'),
  fn("year", "date", ["year(date) → number"], 'year(prop("Due"))'),
  fn("dateAdd", "date", ["dateAdd(date, number, unit) → date"], 'dateAdd(prop("Due"), 1, "weeks")'),
  fn("dateSubtract", "date", ["dateSubtract(date, number, unit) → date"], 'dateSubtract(prop("Due"), 3, "days")'),
  fn("dateBetween", "date", ["dateBetween(date, date, unit) → number"], 'dateBetween(prop("Due"), today(), "days")'),
  fn("formatDate", "date", ["formatDate(date, text) → text"], 'formatDate(prop("Due"), "MMM D, YYYY")'),
  fn("parseDate", "date", ["parseDate(text) → date"], 'parseDate("2026-10-18")'),
  fn("timestamp", "date", ["timestamp(date) → number"], 'timestamp(prop("Due"))'),
  fn("fromTimestamp", "date", ["fromTimestamp(number) → date"], "fromTimestamp(1760000000000)"),

  // 3.6 lists
  fn("at", "list", ["at(list<T>, index) → T"], 'at(prop("Tags"), 0)'),
  fn("first", "list", ["first(list<T>) → T"], 'first(prop("Tags"))'),
  fn("last", "list", ["last(list<T>) → T"], 'last(prop("Tags"))'),
  fn("slice", "list", ["slice(list<T>, start, [end]) → list<T>"], 'slice(prop("Tags"), 1)'),
  fn("concat", "list", ["concat(list<T>, list<T>, …) → list<T>"], 'concat(prop("Tags"), ["extra"])'),
  fn("sort", "list", ["sort(list<T>) → list<T>"], 'sort(prop("Tags"))'),
  fn("reverse", "list", ["reverse(list<T>) → list<T>"], 'reverse(prop("Tags"))'),
  fn("unique", "list", ["unique(list<T>) → list<T>"], 'unique(prop("Tags"))'),
  fn("includes", "list", ["includes(list<T>, T) → boolean"], 'includes(prop("Tags"), "red")'),
  fn("flat", "list", ["flat(list<T>) → list<T>"], 'flat(prop("Tags"))'),
  fn("map", "list", ["map(list<T>, expression) → list<U>"], 'prop("Tasks").map(current.prop("Hours"))'),
  fn("filter", "list", ["filter(list<T>, boolean expression) → list<T>"], 'prop("Tasks").filter(current.prop("Hours") > 2)'),
  fn("find", "list", ["find(list<T>, boolean expression) → T"], 'prop("Tasks").find(current.prop("Hours") > 2)'),
  fn(
    "findIndex",
    "list",
    ["findIndex(list<T>, boolean expression) → number"],
    'prop("Tasks").findIndex(current.prop("Hours") > 2)',
  ),
  fn("some", "list", ["some(list<T>, boolean expression) → boolean"], 'prop("Tasks").some(current.prop("Hours") > 2)'),
  fn("every", "list", ["every(list<T>, boolean expression) → boolean"], 'prop("Tasks").every(current.prop("Hours") > 2)'),
  fn(
    "count",
    "list",
    ["count(list<T>) → number", "count(list<T>, boolean expression) → number"],
    'prop("Tasks").count(current.prop("Hours") > 2)',
  ),

  // 3.7 pages, people and the rest
  fn("prop", "page", ["prop(\"Name\") → T", 'page.prop("Name") → T'], 'prop("Tasks").first().prop("Hours")'),
  fn("id", "page", ["id() → text", "id(page) → text"], "id()"),
  fn("name", "page", ["name(person) → text"], 'prop("Owner").map(name(current)).join(", ")'),
  fn("email", "page", ["email(person) → text"], 'email(prop("Owner").first())'),
  fn("let", "special", ["let(name, value, expression) → U"], 'let(total, prop("Price") * prop("Qty"), total * 1.2)'),
  fn(
    "lets",
    "special",
    ["lets(name1, value1, name2, value2, …, expression) → U"],
    'lets(a, prop("Price"), b, a * 2, a + b)',
  ),
];
