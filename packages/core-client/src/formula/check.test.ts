// The grammar, the positions of syntax errors (in code points) and the static checker: arity, types, names, scopes,
// cycles (sections 2.1 to 2.3, 4.3, 4.5).

import { describe, expect, it } from "vitest";

import { checkFormula, evaluateFormula, isFormulaError, parseFormula } from "./index";
import type { FormulaError } from "./index";
import { makeFixture } from "./host.fixture";
import type { PropertySpec } from "./host.fixture";

const fx = makeFixture();

function error(expression: string): FormulaError {
  const result = fx.run(expression);
  if (!isFormulaError(result)) {
    throw new Error(`expected an error for ${expression}, got ${JSON.stringify(result)}`);
  }
  return result;
}

function summary(expression: string): [string, number, number, string] {
  const e = error(expression);
  return [e.code, e.start, e.end, e.detail];
}

describe("syntax errors", () => {
  it.each([
    ["1 +", ["syntax", 3, 3, "an expression"]],
    ["(1", ["syntax", 2, 2, '")"']],
    ["1 2", ["syntax", 2, 3, "end of text"]],
    [")", ["syntax", 0, 1, "an expression"]],
    ["[1,]", ["syntax", 3, 4, "an expression"]],
    ["max(1, 2,)", ["syntax", 9, 10, "an expression"]],
    ["max(", ["syntax", 4, 4, "an expression"]],
    ["1 ? 2", ["syntax", 5, 5, '":"']],
    ["1 < 2 < 3", ["syntax", 6, 7, "end of text"]],
    ["1 == 2 == 3", ["syntax", 7, 9, "end of text"]],
    ["x.", ["syntax", 2, 2, "a function name"]],
    ["x.f", ["syntax", 3, 3, '"("']],
    ["1 $ 2", ["syntax", 2, 3, "character"]],
    ["1 = 2", ["syntax", 2, 3, "character"]],
    ["1 & 2", ["syntax", 2, 3, "character"]],
    ["1 | 2", ["syntax", 2, 3, "character"]],
    ["1 + not 2", ["syntax", 4, 7, "an expression"]],
  ] as const)("%s", (expression, expected) => {
    expect(summary(expression)).toEqual(expected);
  });

  it("reports the text errors where they are", () => {
    expect(summary('"abc')).toEqual(["syntax", 0, 4, '"']);
    expect(summary('"a\\qb"')).toEqual(["syntax", 2, 4, "\\"]);
    expect(summary('1 + "x\\')).toEqual(["syntax", 4, 7, '"']);
    expect(summary("1 /* x")).toEqual(["syntax", 2, 6, "*/"]);
    expect(summary("1e999")).toEqual(["syntax", 0, 5, "number"]);
  });

  it("raises the earliest error", () => {
    expect(summary('1 + ) "abc')[0]).toBe("syntax");
    expect(error('1 + ) "abc').start).toBe(4);
    expect(error('"abc 1 +').start).toBe(0);
  });

  it("refuses an empty expression at 0", () => {
    expect(summary("")).toEqual(["syntax", 0, 0, "expression"]);
    expect(summary("   \n ")[1]).toBe(0);
    expect(summary("/* only */")[1]).toBe(0);
  });

  it("counts positions in code points, not UTF-16 units", () => {
    expect(summary('"😀" + x')).toEqual(["unknown_variable", 6, 7, "x"]);
    expect(summary('"😀😀" 1')).toEqual(["syntax", 5, 6, "end of text"]);
    expect(summary("😀")).toEqual(["syntax", 0, 1, "character"]);
    expect(summary('prop("Nope😀")')).toEqual(["unknown_property", 5, 12, "Nope😀"]);
  });

  it("limits the length to 4,000 characters and the depth to 64 levels", () => {
    const fine = `1${" ".repeat(3999)}`;
    expect(fx.run(fine)).toEqual({ type: "number", data: 1 });
    expect(summary(`${fine} `)[0]).toBe("too_long");
    expect(summary(`${"😀".repeat(4001)}`)[0]).toBe("too_long");
    expect(fx.run(`${"(".repeat(64)}1${")".repeat(64)}`)).toEqual({ type: "number", data: 1 });
    expect(summary(`${"(".repeat(65)}1${")".repeat(65)}`)[0]).toBe("too_long");
    expect(summary(`${"[".repeat(70)}${"]".repeat(70)}`)[0]).toBe("too_long");
    expect(summary(`${"abs(".repeat(70)}1${")".repeat(70)}`)[0]).toBe("too_long");
  });

  it("runs chains of thousands of operators, even with a stack already in use", () => {
    /** Calls `f` with `depth` frames already on the stack. */
    const deep = <T,>(depth: number, f: () => T): T => (depth === 0 ? f() : deep(depth - 1, f));
    const number = (n: number): unknown => ({ type: "number", data: n });
    const boolean = (b: boolean): unknown => ({ type: "boolean", data: b });
    const chains: [string, unknown][] = [
      [`1${"+1".repeat(1999)}`, number(2000)],
      [`1${"*1".repeat(1999)}`, number(1)],
      [`1${"-1".repeat(1999)}`, number(-1998)],
      [`${"-".repeat(3998)}1`, number(1)],
      [`${"-".repeat(3999)}1`, number(-1)],
      [`${"not ".repeat(990)}true`, boolean(true)],
      [`true${" and true".repeat(390)}`, boolean(true)],
      [`false${" or false".repeat(390)}`, boolean(false)],
      [`"a"${' + "a"'.repeat(600)}`, { type: "text", data: "a".repeat(601) }],
      [`1${".abs()".repeat(600)}`, number(1)],
    ];
    for (const [text, expected] of chains) {
      expect(deep(300, () => fx.run(text)), text.slice(0, 20)).toEqual(expected);
    }
  });

  it("answers an error, never an exception, when a right-leaning chain is deeper than the stack", () => {
    const power = `${"1^".repeat(1900)}1`;
    expect(() => fx.run(power)).not.toThrow();
    const ternary = `${"true ? 1 : ".repeat(300)}2`;
    expect(fx.run(ternary)).toEqual({ type: "number", data: 1 });
  });
});

describe("the grammar", () => {
  it("parses every form", () => {
    for (const text of [
      "1",
      ".5",
      "1.5e-3",
      "2.5E+3",
      '"a\\"b\\\\c\\nd\\te"',
      "true",
      "[]",
      "[1, 2, 3]",
      "pi()",
      'prop("Score").abs()',
      "1 /* c */ + /* d */ 2",
      "1.round()",
      "1.5.round()",
      "true.not()",
      "(1)",
      "-(-1)",
      "!(!true)",
      "true ? 1 : 2",
    ]) {
      expect(isFormulaError(parseFormula(text)), text).toBe(false);
    }
  });

  it("evaluates with the precedence of 2.2", () => {
    const n = (expression: string): unknown => {
      const r = fx.run(expression);
      return isFormulaError(r) ? r.code : r === null ? null : (r as { data: unknown }).data;
    };
    expect(n("2 * 3 + 4")).toBe(10);
    expect(n("2 + 3 * 4")).toBe(14);
    expect(n("2 ^ 3 ^ 2")).toBe(512);
    expect(n("-2 ^ 2")).toBe(-4);
    expect(n("2 ^ -2")).toBe(0.25);
    expect(n("-2 * -3")).toBe(6);
    expect(n("10 - 2 - 3")).toBe(5);
    expect(n("100 / 10 / 5")).toBe(2);
    expect(n("7 % 4 * 2")).toBe(6);
    expect(n("1 + 2 == 3")).toBe(true);
    expect(n("not 1 == 2")).toBe(true);
    expect(n("true or false and false")).toBe(true);
    expect(n("(true or false) and false")).toBe(false);
    expect(n("false ? 1 : true ? 2 : 3")).toBe(2);
    expect(n("1 < 2 ? 10 : 20")).toBe(10);
    expect(n("true ? 1 + 1 : 3")).toBe(2);
    expect(n("2 * (3 + 4)")).toBe(14);
    expect(n("1.5.round()")).toBe(2);
    expect(n("1.round()")).toBe(1);
    expect(n(".5 + 1e3")).toBe(1000.5);
    expect(n("2.5E-1")).toBe(0.25);
    expect(n("-.5")).toBe(-0.5);
  });

  it("reads strings with the escapes of 2.1, a raw newline included", () => {
    const text = (expression: string): unknown => {
      const r = fx.run(expression);
      return isFormulaError(r) ? r.code : r === null ? null : (r as { data: unknown }).data;
    };
    expect(text('"a\\"b"')).toBe('a"b');
    expect(text('"a\\\\b"')).toBe("a\\b");
    expect(text('"a\\nb"')).toBe("a\nb");
    expect(text('"a\\tb"')).toBe("a\tb");
    expect(text('"a\nb"')).toBe("a\nb");
    expect(text("'a'")).toBe("syntax");
    expect(text('"\\u0041"')).toBe("syntax");
  });

  it("takes keywords as method names after a dot", () => {
    expect(fx.run("true.and(true)")).toEqual({ type: "boolean", data: true });
    expect(fx.run("false.or(true)")).toEqual({ type: "boolean", data: true });
    expect(fx.run("and(true, true)")).toEqual({ type: "boolean", data: true });
    expect(fx.run("or(false, true)")).toEqual({ type: "boolean", data: true });
  });
});

describe("names", () => {
  it("refuses an unknown function at its name, an unsupported one too", () => {
    expect(summary("foo(1)")).toEqual(["unknown_function", 0, 3, "foo"]);
    expect(summary("1 + foo(1)")).toEqual(["unknown_function", 4, 7, "foo"]);
    expect(summary("(1).foo()")).toEqual(["unknown_function", 4, 7, "foo"]);
    expect(summary('style("a", "b")')[0]).toBe("unknown_function");
    for (const name of ["unstyle", "link", "dateRange", "dateStart", "dateEnd", "formatNumber"]) {
      expect(summary(`${name}(1)`)[0], name).toBe("unknown_function");
    }
    expect(summary("Round(1)")[0]).toBe("unknown_function");
    expect(summary("constructor(1)")[0]).toBe("unknown_function");
    expect(summary("toString()")[0]).toBe("unknown_function");
    expect(summary("__proto__(1)")[0]).toBe("unknown_function");
  });

  it("refuses an unknown variable, `current` and `index` outside a list function", () => {
    expect(summary("x + 1")).toEqual(["unknown_variable", 0, 1, "x"]);
    expect(summary("current")).toEqual(["unknown_variable", 0, 7, "current"]);
    expect(summary("index + 1")[0]).toBe("unknown_variable");
    expect(summary("[1].map(1) + current")[0]).toBe("unknown_variable");
    expect(summary("[1].sum(current)")[0]).toBe("unknown_variable");
    expect(summary("[1, 2].map(current).sum() + index")[0]).toBe("unknown_variable");
    expect(summary("sort([current])")[0]).toBe("unknown_variable");
    expect(summary("[1].at(current)")[0]).toBe("unknown_variable");
  });

  it("binds `current` and `index` to the innermost list function", () => {
    expect(fx.run("[1, 2].map([10, 20].map(current).sum() + index)")).toEqual({
      type: "list",
      data: [
        { type: "number", data: 30 },
        { type: "number", data: 31 },
      ],
    });
    expect(fx.run('[1].map(["a"].map(current).join(","))')).toEqual({ type: "list", data: [{ type: "text", data: "a" }] });
    // The inner list function's `index` and `current` hide the outer ones inside it.
    expect(fx.run("[5, 6].map([7].map(current + index).sum())")).toEqual({
      type: "list",
      data: [
        { type: "number", data: 7 },
        { type: "number", data: 7 },
      ],
    });
  });

  it("scopes let and lets", () => {
    expect(summary("let(x, 1, 2) + x")[0]).toBe("unknown_variable");
    expect(summary("lets(a, b, b, 1, a)")).toEqual(["unknown_variable", 8, 9, "b"]);
    expect(summary("let(x, x, 1)")[0]).toBe("unknown_variable");
    expect(summary("let(x, 1)")).toEqual(["arity", 0, 9, "let:3:3"]);
    expect(summary("lets(a, 1, b, 2)")).toEqual(["arity", 0, 16, "lets:3:-1"]);
    expect(summary("lets(a, 1)")[0]).toBe("arity");
    expect(fx.run("let(x, 1, [10, 20].map(current + x).sum())")).toEqual({ type: "number", data: 32 });
  });

  it("refuses let names that are not plain names", () => {
    expect(summary('let("x", 1, 2)')).toEqual(["syntax", 4, 7, "identifier"]);
    expect(summary("let(1, 1, 2)")[0]).toBe("syntax");
    expect(summary("let(sum, 1, sum)")).toEqual(["syntax", 4, 7, "sum"]);
    expect(summary("let(current, 1, 1)")[0]).toBe("syntax");
    expect(summary("let(index, 1, 1)")[0]).toBe("syntax");
    expect(summary("let(if, 1, 1)")[0]).toBe("syntax");
    expect(summary("let(true, 1, 1)")[0]).toBe("syntax");
    expect(summary("let(not, 1, 1)")[0]).toBe("syntax");
    expect(summary("lets(a, 1, sum, 2, 3)")[0]).toBe("syntax");
    expect(summary("5.let(x, x)")[0]).toBe("syntax");
    expect(fx.run("let(style, 1, style)")).toEqual({ type: "number", data: 1 });
  });

  it("refuses a `prop` whose argument is not a text in quotes", () => {
    expect(summary("prop(1)")).toEqual(["syntax", 5, 6, "text literal"]);
    expect(summary('prop("a" + "b")')[0]).toBe("syntax");
    expect(summary("prop(x)")[0]).toBe("syntax");
    expect(summary('prop("Tasks").first().prop(1)')[0]).toBe("syntax");
    expect(summary("prop()")).toEqual(["syntax", 0, 6, "text literal"]);
    expect(summary('prop("a", "b", "c")')[0]).toBe("syntax");
  });
});

describe("arity", () => {
  it.each([
    ["round()", "round:1:2"],
    ["round(1, 2, 3)", "round:1:2"],
    ["pi(1)", "pi:0:0"],
    ["max()", "max:1:-1"],
    ["sum()", "sum:1:-1"],
    ["if(true, 1)", "if:3:3"],
    ["ifs(true, 1)", "ifs:3:-1"],
    ["ifs(true, 1, 2, 3)", "ifs:3:-1"],
    ["and(true)", "and:2:-1"],
    ["(true).not(false)", "not:1:1"],
    ["substring(\"a\")", "substring:2:3"],
    ["slice([1])", "slice:2:3"],
    ["concat()", "concat:1:-1"],
    ["count()", "count:1:2"],
    ["[1].map()", "map:2:2"],
    ["[1].map(1, 2)", "map:2:2"],
    ["now(1)", "now:0:0"],
    ["id(1, 2)", "id:0:1"],
    ["email()", "email:1:1"],
  ] as const)("%s", (expression, detail) => {
    const e = error(expression);
    expect(e.code).toBe("arity");
    expect(e.detail).toBe(detail);
  });

  it("locates the whole call", () => {
    expect(summary("1 + round()")).toEqual(["arity", 4, 11, "round:1:2"]);
    expect(summary("[1].map()")).toEqual(["arity", 0, 9, "map:2:2"]);
  });
});

describe("types", () => {
  it("names the expected and the found type at the argument at fault", () => {
    expect(summary("1 + true")).toEqual(["type", 4, 8, "number:boolean"]);
    expect(summary("true + true")).toEqual(["type", 0, 4, "number:boolean"]);
    expect(summary("length(1)")).toEqual(["type", 7, 8, "text:number"]);
    expect(summary('[1, "a"]')).toEqual(["type", 4, 7, "number:text"]);
    expect(summary("if(1, 2, 3)")).toEqual(["type", 3, 4, "boolean:number"]);
    expect(summary('if(true, 1, "a")')).toEqual(["type", 12, 15, "number:text"]);
    expect(summary('ifs(true, 1, true, "a", 3)')[0]).toBe("type");
    expect(summary('1 == "1"')).toEqual(["type", 5, 8, "number:text"]);
    expect(summary("[[1], [2]]")[0]).toBe("type");
    expect(summary("-true")).toEqual(["type", 1, 5, "number:boolean"]);
    expect(summary("not 1")).toEqual(["type", 4, 5, "boolean:number"]);
    expect(summary("1 and true")[0]).toBe("type");
    expect(summary('prop("Tags") + 1')[0]).toBe("type");
    expect(summary('prop("Tags") == "red"')).toEqual(["type", 16, 21, "list<text>:text"]);
    expect(summary('prop("Due") + 1')[0]).toBe("type");
    expect(summary("true < 1")[0]).toBe("type");
    expect(summary('prop("Tasks") < prop("Tasks")')[0]).toBe("type");
    expect(summary('prop("Tags") < prop("Tags")')[0]).toBe("type");
    expect(summary('sum(["a"])')[0]).toBe("type");
    expect(summary("min(1, parseDate(\"2026-01-01\"))")[0]).toBe("type");
    expect(summary('min(parseDate("2026-01-01"), 1)')[0]).toBe("type");
    expect(summary('round("a")')[0]).toBe("type");
    expect(summary('toNumber([1])')[0]).toBe("type");
    expect(summary('dateAdd(1, 1, "days")')[0]).toBe("type");
    expect(summary('dateAdd(today(), "1", "days")')[0]).toBe("type");
    expect(summary('formatDate(today(), 1)')[0]).toBe("type");
    expect(summary('join("a", ",")')[0]).toBe("type");
    expect(summary('at(1, 1)')[0]).toBe("type");
    expect(summary('at([1], "a")')[0]).toBe("type");
    expect(summary('includes([1], "a")')[0]).toBe("type");
    expect(summary('concat([1], ["a"])')[0]).toBe("type");
    expect(summary('name("a")')[0]).toBe("type");
    expect(summary('name(prop("Owner"))')[0]).toBe("type");
    expect(summary('id(1)')[0]).toBe("type");
    expect(summary('"a".map(current)')[0]).toBe("type");
    expect(summary("[1].filter(current)")[0]).toBe("type");
    expect(summary("[1].map([2])")[0]).toBe("type");
    expect(summary('prop("Tasks").map(current.prop("Tags"))')[0]).toBe("type");
    expect(summary('prop("Tasks").prop("Hours")')[0]).toBe("type");
    expect(summary("length(true)")[0]).toBe("type");
    expect(summary("equal(1, \"a\")")[0]).toBe("type");
  });

  it("answers the type of a formula", () => {
    const t = (expression: string): unknown => fx.type(expression);
    expect(t('prop("Score") * 2')).toEqual({ base: "number", list: false });
    expect(t('prop("Status")')).toEqual({ base: "text", list: false });
    expect(t('prop("Tags")')).toEqual({ base: "text", list: true });
    expect(t('prop("Done")')).toEqual({ base: "boolean", list: false });
    expect(t('prop("Due")')).toEqual({ base: "date", list: false });
    expect(t('prop("Tasks")')).toEqual({ base: "page", list: true });
    expect(t('prop("Owner")')).toEqual({ base: "person", list: true });
    expect(t('prop("Name")')).toEqual({ base: "text", list: false });
    expect(t("[]")).toEqual({ base: "text", list: true });
    expect(t("concat([], [1])")).toEqual({ base: "number", list: true });
    expect(t("concat([1], [])")).toEqual({ base: "number", list: true });
    expect(t('if(true, [], prop("Tags"))')).toEqual({ base: "text", list: true });
    expect(t("[].map(current)")).toEqual({ base: "text", list: true });
    expect(t("first([])")).toEqual({ base: "text", list: false });
    expect(t("first([]) + 1")).toEqual({ base: "number", list: false });
    expect(t("[1, 2].map(current > 1)")).toEqual({ base: "boolean", list: true });
    expect(t("[1].find(current > 0)")).toEqual({ base: "number", list: false });
    expect(t("[1].findIndex(current > 0)")).toEqual({ base: "number", list: false });
    expect(t("[1].some(current > 0)")).toEqual({ base: "boolean", list: false });
    expect(t("[1].count()")).toEqual({ base: "number", list: false });
    expect(t("[1].filter(current > 0)")).toEqual({ base: "number", list: true });
    expect(t('"a" + 1')).toEqual({ base: "text", list: false });
    expect(t("1 + 1")).toEqual({ base: "number", list: false });
    expect(t('prop("Due").dateAdd(1, "days")')).toEqual({ base: "date", list: false });
    expect(t('min(prop("Due"), today())')).toEqual({ base: "date", list: false });
    expect(t("max(1, [2, 3])")).toEqual({ base: "number", list: false });
    expect(t('prop("Tasks").first()')).toEqual({ base: "page", list: false });
    expect(t('prop("Tasks").first().prop("Name")')).toEqual({ base: "text", list: false });
    expect(t('prop("Tasks").map(current.prop("Hours"))')).toEqual({ base: "number", list: true });
    expect(t('[1, 2] == []')).toEqual({ base: "boolean", list: false });
    expect(t("ifs(false, [1], true, [], [2])")).toEqual({ base: "number", list: true });
    expect(t("let(x, [1], x)")).toEqual({ base: "number", list: true });
    expect(t("at([true], 0)")).toEqual({ base: "boolean", list: false });
  });
});

describe("kinds of property", () => {
  const schema: PropertySpec[] = [
    { id: "txt", name: "Txt", kind: "text" },
    { id: "url", name: "Url", kind: "url" },
    { id: "mail", name: "Mail", kind: "email" },
    { id: "tel", name: "Tel", kind: "phone" },
    { id: "sel", name: "Sel", kind: "select" },
    { id: "multi", name: "Multi", kind: "multi_select" },
    { id: "files", name: "Files", kind: "files" },
    { id: "check", name: "Check", kind: "checklist" },
    { id: "place", name: "Place", kind: "place" },
    { id: "who", name: "Who", kind: "person" },
    { id: "rel", name: "Rel", kind: "relation" },
    { id: "ct", name: "Ct", kind: "created_time" },
    { id: "et", name: "Et", kind: "last_edited_time" },
    { id: "cb", name: "Cb", kind: "created_by" },
    { id: "eb", name: "Eb", kind: "last_edited_by" },
    { id: "r1", name: "R1", kind: "rollup", rollup: { function: "count_all", targetKind: null } },
    { id: "r2", name: "R2", kind: "rollup", rollup: { function: "earliest", targetKind: "date" } },
    { id: "r3", name: "R3", kind: "rollup", rollup: { function: "show_original", targetKind: null } },
    { id: "r4", name: "R4", kind: "rollup", rollup: { function: "show_original", targetKind: "number" } },
    { id: "r5", name: "R5", kind: "rollup", rollup: { function: "show_original", targetKind: "checklist" } },
    { id: "r6", name: "R6", kind: "rollup", rollup: { function: "show_original", targetKind: "date" } },
    { id: "r7", name: "R7", kind: "rollup", rollup: { function: "checklist_progress", targetKind: "checklist" } },
    { id: "r8", name: "R8", kind: "rollup", rollup: { function: "percent_checked", targetKind: "checkbox" } },
    { id: "r9", name: "R9", kind: "rollup", rollup: { function: "show_original", targetKind: "select" } },
    { id: "bare", name: "Bare", kind: "rollup" },
  ];
  const kinds = makeFixture(schema, []);

  it.each([
    ["Txt", "text", false],
    ["Url", "text", false],
    ["Mail", "text", false],
    ["Tel", "text", false],
    ["Sel", "text", false],
    ["Multi", "text", true],
    ["Files", "text", true],
    ["Check", "boolean", true],
    ["Place", "text", false],
    ["Who", "person", true],
    ["Rel", "page", true],
    ["Ct", "date", false],
    ["Et", "date", false],
    ["R1", "number", false],
    ["R2", "date", false],
    ["R3", "text", true],
    ["R4", "number", true],
    ["R5", "boolean", true],
    ["R6", "date", true],
    ["R7", "number", false],
    ["R8", "number", false],
    ["R9", "text", true],
  ] as const)("%s is %s (list %s)", (name, base, list) => {
    expect(kinds.type(`prop("${name}")`)).toEqual({ base, list });
  });

  it("refuses the devices and a rollup without its configuration", () => {
    for (const name of ["Cb", "Eb", "Bare"]) {
      const result = kinds.type(`prop("${name}")`);
      expect(isFormulaError(result) && result.code, name).toBe("unsupported_property");
      expect(isFormulaError(result) && result.detail, name).toBe(name);
    }
    const at = kinds.type('1 + prop("Cb")');
    expect(isFormulaError(at) && at.start).toBe(9);
  });

  it("refuses an unknown property at its argument", () => {
    const result = kinds.type('1 + prop("Zzz")');
    expect(isFormulaError(result) && [result.code, result.start, result.end, result.detail]).toEqual([
      "unknown_property",
      9,
      14,
      "Zzz",
    ]);
  });
});

describe("cycles and nested formulas", () => {
  const schema: PropertySpec[] = [
    { id: "n", name: "N", kind: "number" },
    { id: "rel", name: "Rel", kind: "relation" },
    { id: "a", name: "A", kind: "formula", expression: 'prop("B") + 1' },
    { id: "b", name: "B", kind: "formula", expression: 'prop("A") + 1' },
    { id: "c", name: "C", kind: "formula", expression: 'prop("N") * 2' },
    { id: "d", name: "D", kind: "formula", expression: 'prop("C") + prop("C")' },
    { id: "e", name: "E", kind: "formula", expression: 'prop("Rel").first().prop("E")' },
    { id: "f", name: "F", kind: "formula", stored: 'prop("zzz")' },
    { id: "g", name: "G", kind: "formula", stored: "1 +" },
    { id: "h", name: "H", kind: "formula", expression: 'prop("F") + 1' },
    { id: "i", name: "I", kind: "formula", expression: 'prop("Rel").first().prop("C")' },
  ];
  const cycles = makeFixture(schema, []);

  it("refuses a loop among the formulas `prop` reaches", () => {
    const result = cycles.type('prop("A")');
    expect(isFormulaError(result) && result.code).toBe("cycle");
    expect(["A", "B"]).toContain(isFormulaError(result) ? result.detail : "");
    expect(cycles.type('prop("B") * 2')).toMatchObject({ code: "cycle" });
  });

  it("refuses a loop through the formula being checked, even when it is not in the schema yet", () => {
    expect(cycles.type('prop("C") + 1', "c")).toMatchObject({ code: "cycle", detail: "C" });
    expect(cycles.type('prop("D")', "c")).toMatchObject({ code: "cycle" });
    expect(cycles.type('prop("C")', "d")).toEqual({ base: "number", list: false });
    expect(cycles.type('prop("C")', "brand-new")).toEqual({ base: "number", list: false });
    expect(checkFormula('prop("fresh") + 1', cycles.host, "fresh")).toMatchObject({ code: "unknown_property" });
    expect(cycles.type('prop("Rel").first().prop("E")', "e")).toMatchObject({ code: "cycle" });
  });

  it("follows `.prop` too, since its type is the type of the formula it reads", () => {
    expect(cycles.type('prop("Rel").first().prop("E")')).toMatchObject({ code: "cycle" });
    expect(cycles.type('prop("Rel").first().prop("C")')).toEqual({ base: "number", list: false });
  });

  it("types a formula that reads another", () => {
    expect(cycles.type('prop("D") + 1')).toEqual({ base: "number", list: false });
    expect(cycles.type('prop("I")')).toEqual({ base: "number", list: false });
  });

  it("cannot read a formula that does not check: unsupported_property at the reference, named after it", () => {
    const result = cycles.type('1 + prop("F")');
    expect(isFormulaError(result) && [result.code, result.start, result.end, result.detail]).toEqual([
      "unsupported_property",
      9,
      12,
      "F",
    ]);
    const syntax = cycles.type('prop("G")');
    expect(isFormulaError(syntax) && [syntax.code, syntax.start, syntax.end, syntax.detail]).toEqual([
      "unsupported_property",
      5,
      8,
      "G",
    ]);
    const further = cycles.type('prop("H")');
    expect(isFormulaError(further) && [further.code, further.detail]).toEqual(["unsupported_property", "H"]);
  });

  it("has no value for a page when its formula does not check", () => {
    const empty = makeFixture(schema, [{ id: "p", title: "P", values: { n: { kind: "number", value: 2 } } }]);
    expect(empty.run('prop("N") * 2', "p")).toEqual({ type: "number", data: 4 });
    expect(empty.run('prop("A")', "p")).toMatchObject({ code: "cycle" });
    // A formula that does not check has no value.
    const stored = empty.stored('prop("A")');
    expect(checkFormula(stored, empty.host)).toMatchObject({ code: "cycle" });
    expect(evaluateFormula(stored, "p", empty.host, empty.env)).toBeNull();
  });
});
