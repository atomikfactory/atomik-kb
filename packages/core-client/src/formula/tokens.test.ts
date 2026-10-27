// The editor's tokenizer (section 7.2): kinds, positions in code points, coverage, and that it never throws.

import { describe, expect, it } from "vitest";

import { checkFormula, evaluateFormula, isFormulaError, parseFormula, tokenizeFormula } from "./index";
import { makeFixture } from "./host.fixture";

function kinds(text: string): [string, string][] {
  return tokenizeFormula(text).map((t) => [t.kind, t.text]);
}

describe("tokenizeFormula", () => {
  it("classifies the tokens of a formula", () => {
    expect(kinds('prop("Price") * 2')).toEqual([
      ["function", "prop"],
      ["punct", "("],
      ["property", '"Price"'],
      ["punct", ")"],
      ["operator", "*"],
      ["number", "2"],
    ]);
    expect(kinds('if(not x and true, "a", .5)')).toEqual([
      ["function", "if"],
      ["punct", "("],
      ["keyword", "not"],
      ["ident", "x"],
      ["keyword", "and"],
      ["keyword", "true"],
      ["punct", ","],
      ["string", '"a"'],
      ["punct", ","],
      ["number", ".5"],
      ["punct", ")"],
    ]);
  });

  it("calls a known function name followed by `(` a function, as a function or a method", () => {
    expect(kinds("x.sum()")).toEqual([
      ["ident", "x"],
      ["punct", "."],
      ["function", "sum"],
      ["punct", "("],
      ["punct", ")"],
    ]);
    expect(kinds("foo(1)")[0]).toEqual(["ident", "foo"]);
    expect(kinds("sum + 1")[0]).toEqual(["ident", "sum"]);
    expect(kinds("sum /* c */ (1)")[0]).toEqual(["function", "sum"]);
    expect(kinds("current")[0]).toEqual(["ident", "current"]);
    expect(kinds("constructor(1)")[0]).toEqual(["ident", "constructor"]);
    expect(kinds("not(x)")[0]).toEqual(["keyword", "not"]);
    expect(kinds("x.not()").map(([kind]) => kind)).toEqual(["ident", "punct", "keyword", "punct", "punct"]);
  });

  it("marks the text argument of `prop` and `.prop` as a property", () => {
    expect(kinds('x.prop("A")')[4]).toEqual(["property", '"A"']);
    expect(kinds('prop( "A" )')[2]).toEqual(["property", '"A"']);
    expect(kinds('prop("A')[2]).toEqual(["property", '"A']);
    expect(kinds('format("A")')[2]).toEqual(["string", '"A"']);
    expect(kinds('prop(1, "A")')[4]).toEqual(["string", '"A"']);
    expect(kinds('"prop(" + "A"')[2]).toEqual(["string", '"A"']);
  });

  it("covers comments, unclosed texts and strange characters", () => {
    expect(kinds("1 /* a */ + 2")).toEqual([
      ["number", "1"],
      ["comment", "/* a */"],
      ["operator", "+"],
      ["number", "2"],
    ]);
    expect(kinds("1 /* a")[1]).toEqual(["comment", "/* a"]);
    expect(kinds('"abc')).toEqual([["string", '"abc']]);
    expect(kinds("a @ b")).toEqual([
      ["ident", "a"],
      ["error", "@"],
      ["ident", "b"],
    ]);
    expect(kinds("1 = 2")[1]).toEqual(["error", "="]);
    expect(kinds("a && b || !c != d")).toEqual([
      ["ident", "a"],
      ["operator", "&&"],
      ["ident", "b"],
      ["operator", "||"],
      ["operator", "!"],
      ["ident", "c"],
      ["operator", "!="],
      ["ident", "d"],
    ]);
    expect(kinds("a ? b : [c]").map(([kind]) => kind)).toEqual(["ident", "punct", "ident", "punct", "punct", "ident", "punct"]);
    expect(kinds("1.5.round()")[0]).toEqual(["number", "1.5"]);
    expect(kinds("1e")).toEqual([
      ["number", "1"],
      ["ident", "e"],
    ]);
    expect(kinds("1e5 + 2E-3")).toEqual([
      ["number", "1e5"],
      ["operator", "+"],
      ["number", "2E-3"],
    ]);
  });

  it("counts positions in code points", () => {
    const tokens = tokenizeFormula('"😀" x 🙂');
    expect(tokens.map((t) => [t.kind, t.start, t.end])).toEqual([
      ["string", 0, 3],
      ["ident", 4, 5],
      ["error", 6, 7],
    ]);
  });

  it("answers nothing for white space and empty text", () => {
    expect(tokenizeFormula("")).toEqual([]);
    expect(tokenizeFormula(" \t\r\n ")).toEqual([]);
  });

  it("covers all the text but white space, whatever it is, and never throws", () => {
    const alphabet = ["a", "z", "_", "1", ".", "e", '"', "\\", "(", ")", "[", "]", ",", "/", "*", "😀", "é", " ", "\n", "+", "=", "&", "|", "!", "<", "?", ":", "prop", "not", "true", "x"];
    let seed = 12345;
    const next = (): number => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed;
    };
    for (let round = 0; round < 400; round += 1) {
      let text = "";
      const length = next() % 14;
      for (let i = 0; i < length; i += 1) {
        text += alphabet[next() % alphabet.length] ?? "";
      }
      const chars = Array.from(text);
      const tokens = tokenizeFormula(text);
      let at = 0;
      for (const token of tokens) {
        // The gap before a token is white space only; the token's text is the text at its range.
        expect(chars.slice(at, token.start).every((c) => c === " " || c === "\t" || c === "\r" || c === "\n"), text).toBe(true);
        expect(chars.slice(token.start, token.end).join(""), text).toBe(token.text);
        expect(token.end, text).toBeGreaterThan(token.start);
        at = token.end;
      }
      expect(chars.slice(at).every((c) => c === " " || c === "\t" || c === "\r" || c === "\n"), text).toBe(true);
    }
  });
});

describe("the engine never throws", () => {
  it("answers an error or a value for any text", () => {
    const fx = makeFixture();
    const alphabet = ["a", "1", ".", '"', "\\", "(", ")", "[", "]", ",", "/", "*", "😀", " ", "+", "-", "!", "<", "?", ":", "prop(", "not ", "true", "current", "index", "map(", "let(", "x", "sum(", "if(", '"score"', '"tasks"', "=="];
    let seed = 987654;
    const next = (): number => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed;
    };
    for (let round = 0; round < 1500; round += 1) {
      let text = "";
      const length = next() % 12;
      for (let i = 0; i < length; i += 1) {
        text += alphabet[next() % alphabet.length] ?? "";
      }
      const parsed = parseFormula(text);
      expect(parsed === null, text).toBe(false);
      const checked = checkFormula(text, fx.host);
      if (!isFormulaError(checked)) {
        // Whatever checks can be evaluated, to a value or to no value.
        const result = evaluateFormula(text, "p1", fx.host, fx.env);
        expect(result === undefined, text).toBe(false);
      }
    }
  });
});
