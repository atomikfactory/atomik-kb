// The rules of the formula editor that need no React (`formula-editor-model.ts`): completion, the highlighted mirror, the
// schema and host the engine reads, the warnings and the translated errors. The engine is given hand-made tokens and
// functions here, so these tests do not depend on the engine's own lexer and function table.

import type { FormulaError, FormulaFunctionInfo, FormulaToken, PageInfo, PropertyDef, TreeView } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import {
  ambiguousNames,
  applyCompletion,
  buildFormulaHost,
  choosePreviewPage,
  completionAt,
  escapeFormulaString,
  formulaErrorMessage,
  formulaSchemaOf,
  formulaTypeLabel,
  functionGroups,
  highlightSegments,
  pagesOfTree,
  previewEnv,
  toCodePoints,
  toUtf16,
  translateTypeNames,
} from "./formula-editor-model";
import type { CompletionSources } from "./formula-editor-model";

const fn = (name: string, category: FormulaFunctionInfo["category"], signature: string, example = `${name}()`): FormulaFunctionInfo => ({
  name,
  category,
  signatures: [signature],
  example,
});

const FUNCTIONS: readonly FormulaFunctionInfo[] = [
  fn("sum", "number", "sum(numbers…) → number"),
  fn("sort", "list", "sort(list<T>) → list<T>"),
  fn("round", "number", "round(number, [places]) → number"),
  fn("now", "date", "now() → date"),
  fn("map", "list", "map(list<T>, expr) → list<U>"),
  fn("prop", "page", 'prop("Name") → T'),
];

const SOURCES: CompletionSources = {
  properties: [
    { name: "Price", kind: "number" },
    { name: "Qty", kind: "number" },
    { name: "Status", kind: "status" },
    { name: "Made by", kind: "created_by" },
    { name: 'Say "hi"', kind: "text" },
  ],
  titleLabel: "Title",
  functions: FUNCTIONS,
};

const labels = (text: string, caret = text.length): string[] => completionAt(text, caret, SOURCES)?.items.map((item) => item.label) ?? [];

describe("completing a word", () => {
  it("proposes the functions that start with the word, with their brackets and the caret inside", () => {
    const completion = completionAt("su", 2, SOURCES);
    expect(completion?.items.map((item) => item.label)).toEqual(["sum"]);
    const [item] = completion?.items ?? [];
    expect(item).toMatchObject({ insert: "sum()", caretBack: 1, kind: "function", detail: "sum(numbers…) → number" });
    expect(completion).toMatchObject({ from: 0, to: 2 });
    if (completion !== null && item !== undefined) {
      expect(applyCompletion("su", completion, item)).toEqual({ text: "sum()", caret: 4 });
    }
  });

  it("ignores case and lists the functions in the order of the reference, then the properties", () => {
    expect(labels("S")).toEqual(["sum", "sort", 'prop("Status")', 'prop("Say \\"hi\\"")']);
    expect(labels("so")).toEqual(["sort"]);
  });

  it("puts the caret after the brackets of a function that takes nothing", () => {
    const completion = completionAt("no", 2, SOURCES);
    const [item] = completion?.items ?? [];
    expect(item).toMatchObject({ insert: "now()", caretBack: 0 });
    if (completion !== null && item !== undefined) {
      expect(applyCompletion("no", completion, item)).toEqual({ text: "now()", caret: 5 });
    }
  });

  it("writes only the name when the brackets are already there, and replaces the rest of the word", () => {
    const completion = completionAt("su(1)", 2, SOURCES);
    const [item] = completion?.items ?? [];
    expect(item?.insert).toBe("sum");
    const longer = completionAt("suXYZ", 2, SOURCES);
    expect(longer).toMatchObject({ from: 0, to: 5 });
  });

  it("completes in the middle of a formula, after an operator and a bracket", () => {
    const text = "1 + ro";
    const completion = completionAt(text, text.length, SOURCES);
    expect(completion?.items.map((item) => item.label)).toEqual(["round"]);
    expect(completion).toMatchObject({ from: 4, to: 6 });
  });

  it("proposes only functions after a dot (a method call), not properties or variables", () => {
    expect(labels('prop("Price").s')).toEqual(["sum", "sort"]);
    expect(labels("x.cur")).toEqual([]);
  });

  it("proposes current and index", () => {
    expect(labels("cur")).toEqual(["current"]);
    expect(labels("ind")).toEqual(["index"]);
  });

  it("proposes a property as prop(\"Name\"), the ones that start with the word first", () => {
    expect(labels("Pri")).toEqual(['prop("Price")']);
    // `prop` itself is a function that starts with the word too.
    expect(labels("P")).toEqual(["prop", 'prop("Price")']);
    expect(labels("tat")).toEqual(['prop("Status")']);
    const completion = completionAt("Pri", 3, SOURCES);
    const [item] = completion?.items ?? [];
    if (completion !== null && item !== undefined) {
      expect(applyCompletion("Pri", completion, item)).toEqual({ text: 'prop("Price")', caret: 13 });
    }
  });

  it("proposes the title column by the label the UI gives it, and not the devices", () => {
    expect(labels("Ti")).toEqual(['prop("Title")']);
    expect(labels("Made")).toEqual([]);
  });

  it("escapes the quotes and backslashes of a name", () => {
    expect(escapeFormulaString('a"b\\c')).toBe('a\\"b\\\\c');
    const completion = completionAt("Say", 3, SOURCES);
    expect(completion?.items[0]?.insert).toBe('prop("Say \\"hi\\"")');
  });

  it("proposes nothing for a number, an empty word, a comment, or a string that is not a property's name", () => {
    expect(labels("12")).toEqual([]);
    expect(labels("1e")).toEqual([]);
    expect(labels("")).toEqual([]);
    expect(labels("1 + ")).toEqual([]);
    expect(labels("/* su")).toEqual([]);
    expect(labels('"su')).toEqual([]);
    expect(labels('format("su')).toEqual([]);
    // After the comment closed, completion is back.
    expect(labels("/* a */ su")).toEqual(["sum"]);
  });

  it("proposes nothing for a word no function, variable or property matches", () => {
    expect(labels("zzz")).toEqual([]);
  });

  it("is keyed by what is completed, so that Esc closes one list and not the next", () => {
    const first = completionAt("su", 2, SOURCES);
    const same = completionAt("su", 2, SOURCES);
    const other = completionAt("sum", 3, SOURCES);
    expect(first?.key).toBe(same?.key);
    expect(first?.key).not.toBe(other?.key);
  });
});

describe("completing the name inside prop(\"…\")", () => {
  it("proposes every property, the title first, as soon as the quote is open", () => {
    const completion = completionAt('prop("', 6, SOURCES);
    expect(completion?.items.map((item) => item.label)).toEqual(['prop("Title")', 'prop("Price")', 'prop("Qty")', 'prop("Status")', 'prop("Say \\"hi\\"")']);
  });

  it("filters by what is typed, anywhere in the name, and writes the name and the closing quote", () => {
    const text = 'prop("ric';
    const completion = completionAt(text, text.length, SOURCES);
    expect(completion?.items.map((item) => item.label)).toEqual(['prop("Price")']);
    const [item] = completion?.items ?? [];
    expect(item?.insert).toBe('Price"');
    if (completion !== null && item !== undefined) {
      expect(applyCompletion(text, completion, item)).toEqual({ text: 'prop("Price"', caret: 12 });
    }
  });

  it("replaces the name inside a string that is already closed, and moves past the closing quote", () => {
    const text = 'prop("Pr") * 2';
    const completion = completionAt(text, 8, SOURCES);
    expect(completion).toMatchObject({ from: 6, to: 8, skip: 1 });
    const [item] = completion?.items ?? [];
    expect(item?.insert).toBe("Price");
    if (completion !== null && item !== undefined) {
      expect(applyCompletion(text, completion, item)).toEqual({ text: 'prop("Price") * 2', caret: 12 });
    }
  });

  it("works for a method call and with white space inside the brackets", () => {
    expect(labels('x.prop("Pr')).toEqual(['prop("Price")']);
    expect(labels('prop( "Qt')).toEqual(['prop("Qty")']);
  });

  it("proposes the properties, quotes included, right after prop(", () => {
    const completion = completionAt("prop(", 5, SOURCES);
    expect(completion?.items[0]?.insert).toBe('"Title"');
    const [price] = completion?.items.filter((item) => item.label === 'prop("Price")') ?? [];
    if (completion !== null && price !== undefined) {
      expect(applyCompletion("prop(", completion, price)).toEqual({ text: 'prop("Price"', caret: 12 });
    }
    // Not when a string follows already.
    expect(labels('prop("Price")', 5)).toEqual([]);
  });

  it("is not for a string that is not the first thing after prop(", () => {
    expect(labels('prop(1, "Pr')).toEqual([]);
    expect(labels('myprop("Pr')).toEqual([]);
  });
});

describe("positions", () => {
  it("convert between code points and UTF-16 units across an astral character", () => {
    const text = "a😀b";
    expect(text.length).toBe(4);
    expect(toUtf16(text, 0)).toBe(0);
    expect(toUtf16(text, 2)).toBe(3);
    expect(toUtf16(text, 3)).toBe(4);
    expect(toUtf16(text, 99)).toBe(4);
    expect(toCodePoints(text, 3)).toBe(2);
    expect(toCodePoints(text, 4)).toBe(3);
    expect(toCodePoints(text, 2)).toBe(1);
  });
});

describe("the mirror", () => {
  const token = (kind: FormulaToken["kind"], start: number, end: number, text: string): FormulaToken => ({ kind, start, end, text });

  it("is the text cut into runs that join back to it, white space included", () => {
    const text = '1 + prop("A")';
    const tokens = [
      token("number", 0, 1, "1"),
      token("operator", 2, 3, "+"),
      token("function", 4, 8, "prop"),
      token("punct", 8, 9, "("),
      token("property", 9, 12, '"A"'),
      token("punct", 12, 13, ")"),
    ];
    const segments = highlightSegments(text, tokens, null);
    expect(segments.map((segment) => segment.text).join("")).toBe(text);
    expect(segments.map((segment) => [segment.text, segment.kind])).toEqual([
      ["1", "number"],
      [" ", null],
      ["+", "operator"],
      [" ", null],
      ["prop", "function"],
      ["(", "punct"],
      ['"A"', "property"],
      [")", "punct"],
    ]);
    expect(segments.some((segment) => segment.error)).toBe(false);
  });

  it("cuts the runs where the error range starts and ends", () => {
    const text = "1 + foo";
    const tokens = [token("number", 0, 1, "1"), token("operator", 2, 3, "+"), token("ident", 4, 7, "foo")];
    const segments = highlightSegments(text, tokens, { start: 5, end: 7 });
    expect(segments.map((segment) => segment.text).join("")).toBe(text);
    expect(segments.filter((segment) => segment.error).map((segment) => segment.text).join("")).toBe("oo");
    expect(segments.filter((segment) => segment.error).every((segment) => segment.kind === "ident")).toBe(true);
  });

  it("counts positions in code points, so that an emoji before a token does not shift it", () => {
    const text = '"😀" + x';
    const tokens = [token("string", 0, 3, '"😀"'), token("operator", 4, 5, "+"), token("ident", 6, 7, "x")];
    const segments = highlightSegments(text, tokens, { start: 6, end: 7 });
    expect(segments.map((segment) => segment.text).join("")).toBe(text);
    expect(segments.find((segment) => segment.error)?.text).toBe("x");
    expect(segments.find((segment) => segment.kind === "string")?.text).toBe('"😀"');
  });

  it("is the text in one plain run when the lexer gave no tokens", () => {
    expect(highlightSegments("abc", [], null)).toEqual([{ text: "abc", kind: null, error: false }]);
    expect(highlightSegments("", [], null)).toEqual([]);
  });
});

describe("the warning about names", () => {
  it("lists the names that prop reads and more than one property goes by, once each", () => {
    const names = ["Price", "Qty", "Price", "Other"];
    expect(ambiguousNames('prop("Price") * prop("Qty") + prop("Price")', names, "Title")).toEqual(["Price"]);
    expect(ambiguousNames('prop("Qty")', names, "Title")).toEqual([]);
  });

  it("counts the title column when a property takes its name", () => {
    expect(ambiguousNames('prop("Title")', ["Title", "Qty"], "Title")).toEqual(["Title"]);
    expect(ambiguousNames('prop("Title")', ["Qty"], "Title")).toEqual([]);
  });

  it("reads escaped names and white space in the brackets", () => {
    expect(ambiguousNames('prop( "A\\"B" )', ['A"B', 'A"B'], "Title")).toEqual(['A"B']);
  });
});

describe("the errors", () => {
  const error = (code: FormulaError["code"], detail: string): FormulaError => ({ code, message: `english ${code}`, start: 0, end: 1, detail });

  it("are sentences made from the code and the detail", () => {
    expect(formulaErrorMessage(error("syntax", ""))).toBe("The formula is not valid here.");
    expect(formulaErrorMessage(error("syntax", '")"'))).toBe('Syntax error: expected ")".');
    expect(formulaErrorMessage(error("too_long", ""))).toBe("The formula is too long or nested too deeply.");
    expect(formulaErrorMessage(error("unknown_property", "Nope"))).toBe("There is no property called “Nope”.");
    expect(formulaErrorMessage(error("unsupported_property", "Made by"))).toBe("“Made by” cannot be used in a formula.");
    expect(formulaErrorMessage(error("unknown_function", "sumx"))).toBe("There is no function called “sumx”.");
    expect(formulaErrorMessage(error("unknown_variable", "foo"))).toBe("There is no name “foo” here.");
    expect(formulaErrorMessage(error("cycle", "Total"))).toBe("This formula depends on itself through “Total”.");
  });

  it("say how many arguments a function takes", () => {
    expect(formulaErrorMessage(error("arity", "abs:1:1"))).toBe("abs takes exactly 1 argument.");
    expect(formulaErrorMessage(error("arity", "round:1:2"))).toBe("round takes 1 to 2 arguments.");
    expect(formulaErrorMessage(error("arity", "min:1:-1"))).toBe("min takes at least 1 argument.");
    expect(formulaErrorMessage(error("arity", "pow:2:2"))).toBe("pow takes exactly 2 arguments.");
  });

  it("name the types in the person's words", () => {
    expect(formulaErrorMessage(error("type", "number:text"))).toBe("Expected number, but found text.");
    expect(formulaErrorMessage(error("type", "list<number>:list<boolean>"))).toBe("Expected list (number), but found list (true or false).");
  });

  it("fall back to the engine's message for a code this version does not know", () => {
    expect(formulaErrorMessage({ code: "new_code" as FormulaError["code"], message: "From the engine", start: 0, end: 0, detail: "" })).toBe("From the engine");
  });

  it("translate type names wherever they appear", () => {
    expect(translateTypeNames("page")).toBe("page");
    expect(translateTypeNames("list<person>")).toBe("list (person)");
    expect(translateTypeNames("boolean")).toBe("true or false");
    expect(translateTypeNames("an unknown")).toBe("an unknown");
  });

  it("write the type of a result", () => {
    expect(formulaTypeLabel({ base: "number", list: false })).toBe("number");
    expect(formulaTypeLabel({ base: "text", list: true })).toBe("list (text)");
    expect(formulaTypeLabel({ base: "date", list: false })).toBe("date");
  });
});

// ----- the schema and the host ----------------------------------------------------------------------------------------------

const def = (id: string, name: string, kind: PropertyDef["kind"], extra: Partial<PropertyDef> = {}): PropertyDef => ({ id, name, kind, options: [], createdAt: 0, ...extra });

const DEFS: readonly PropertyDef[] = [
  def("p-price", "Price", "number"),
  def("p-status", "Status", "status", { options: [{ id: "o-todo", name: "To do", color: "gray", group: "todo" }] }),
  def("p-tasks", "Tasks", "relation", { config: { kind: "relation", twoWay: false } }),
  def("p-hours", "Hours", "rollup", { config: { kind: "rollup", relationPropertyId: "p-tasks", target: "p-price", function: "sum" } }),
  def("p-names", "Names", "rollup", { config: { kind: "rollup", relationPropertyId: "p-tasks", target: "title", function: "show_original" } }),
  def("p-lost", "Lost", "rollup", { config: { kind: "rollup", relationPropertyId: "p-tasks", target: "p-gone", function: "sum" } }),
  def("p-total", "Total", "formula", { config: { kind: "formula", expression: 'prop("p-price") * 2' } }),
];

describe("the schema of a space as the engine reads it", () => {
  const schema = formulaSchemaOf(DEFS);

  it("lists the properties in order and finds one by id", () => {
    expect(schema.properties().map((property) => property.id)).toEqual(DEFS.map((entry) => entry.id));
    expect(schema.property("p-price")).toMatchObject({ name: "Price", kind: "number", options: [] });
    expect(schema.property("nope")).toBeUndefined();
  });

  it("carries the options by id and name", () => {
    expect(schema.property("p-status")?.options).toEqual([{ id: "o-todo", name: "To do" }]);
  });

  it("carries a formula's stored expression", () => {
    expect(schema.property("p-total")?.expression).toBe('prop("p-price") * 2');
  });

  it("carries a rollup's function and the kind of its target (none for the title, nothing when the target is gone)", () => {
    expect(schema.property("p-hours")?.rollup).toEqual({ function: "sum", targetKind: "number" });
    expect(schema.property("p-names")?.rollup).toEqual({ function: "show_original", targetKind: null });
    expect(schema.property("p-lost")?.rollup).toBeUndefined();
  });
});

function page(id: string, extra: Partial<PageInfo> = {}): PageInfo {
  return { id, spaceId: "s", title: id.toUpperCase(), createdAt: 1000, updatedAt: 2000, ...extra };
}

describe("the host of the preview", () => {
  const first = page("a", {
    properties: [
      ["p-price", { kind: "number", value: 3 }],
      ["p-tasks", { kind: "relation", value: ["b", "gone", "c"] }],
      ["p-total", { kind: "formula", value: { type: "number", data: 6 } }],
    ],
  });
  const pages = new Map([first, page("b"), page("c", { trashedAt: 5 })].map((entry) => [entry.id, entry] as const));
  // A page of the trash is not in the tree's roots: only `b` and `a` are live here.
  pages.delete("c");
  const host = buildFormulaHost(formulaSchemaOf(DEFS), {
    pages,
    people: [{ accountId: "acc-1", name: "Ada" }],
  });

  it("reads titles, times and stored values from the pages the UI holds", () => {
    expect(host.title("a")).toBe("A");
    expect(host.title("zzz")).toBeUndefined();
    expect(host.createdMs("a")).toBe(1000);
    expect(host.editedMs("a")).toBe(2000);
    expect(host.value("a", "p-price")).toEqual({ kind: "number", value: 3 });
    expect(host.value("a", "p-status")).toBeUndefined();
  });

  it("never gives a formula's value (the engine computes formulas itself)", () => {
    expect(host.value("a", "p-total")).toBeUndefined();
  });

  it("gives the related pages that are live, in order", () => {
    expect(host.related("a", "p-tasks")).toEqual(["b"]);
    expect(host.related("a", "p-price")).toEqual([]);
    expect(host.related("zzz", "p-tasks")).toEqual([]);
  });

  it("knows the people by name, and no files", () => {
    expect(host.person("acc-1")).toEqual({ name: "Ada", email: "" });
    expect(host.person("acc-2")).toBeUndefined();
    expect(host.fileName("att-1")).toBeUndefined();
  });

  it("is the schema's too", () => {
    expect(host.property("p-price")?.name).toBe("Price");
    expect(host.properties()).toHaveLength(DEFS.length);
  });
});

describe("the pages of a tree and the one the preview runs on", () => {
  const tree = {
    spaceId: "s",
    roots: [
      { page: page("a"), children: [{ page: page("b"), children: [] }] },
      { page: page("c"), children: [] },
    ],
    trashed: [],
  } as unknown as TreeView;
  const pages = pagesOfTree(tree);

  it("lists the live pages by id, depth first", () => {
    expect([...pages.keys()]).toEqual(["a", "b", "c"]);
    expect(pagesOfTree(null).size).toBe(0);
  });

  it("is the first candidate that is still in the tree", () => {
    expect(choosePreviewPage(pages, [page("gone"), page("b")], "c")?.id).toBe("b");
  });

  it("is the page in front when there is no candidate, and then the first page", () => {
    expect(choosePreviewPage(pages, [], "c")?.id).toBe("c");
    expect(choosePreviewPage(pages, [], "gone")?.id).toBe("a");
    expect(choosePreviewPage(pages, [], null)?.id).toBe("a");
    expect(choosePreviewPage(new Map(), [], null)).toBeNull();
  });

  it("gives the time of the read with this device's offset", () => {
    const env = previewEnv(new Date(Date.UTC(2026, 9, 18, 9, 0, 0)));
    expect(env.nowMs).toBe(Date.UTC(2026, 9, 18, 9, 0, 0));
    expect(env.offsetSeconds).toBe(-new Date(env.nowMs).getTimezoneOffset() * 60);
  });
});

describe("the reference", () => {
  const describe_ = (name: string): string => (name === "sum" ? "Adds the numbers" : "");

  it("groups the functions by category, in the order of the categories, leaving out the empty ones", () => {
    const groups = functionGroups(FUNCTIONS, "", describe_);
    expect(groups.map((group) => group.id)).toEqual(["number", "date", "list", "page"]);
    expect(groups.find((group) => group.id === "number")?.items.map((item) => item.name)).toEqual(["sum", "round"]);
  });

  it("filters by name, description or signature, ignoring case", () => {
    expect(functionGroups(FUNCTIONS, "SOR", describe_).flatMap((group) => group.items.map((item) => item.name))).toEqual(["sort"]);
    expect(functionGroups(FUNCTIONS, "adds", describe_).flatMap((group) => group.items.map((item) => item.name))).toEqual(["sum"]);
    expect(functionGroups(FUNCTIONS, "places", describe_).flatMap((group) => group.items.map((item) => item.name))).toEqual(["round"]);
    expect(functionGroups(FUNCTIONS, "zzzz", describe_)).toEqual([]);
  });
});
