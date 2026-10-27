// The reference of the language (`FORMULA_FUNCTIONS`) against the functions the engine knows, and its examples against
// the grammar and the types.

import { describe, expect, it } from "vitest";

import { FORMULA_FUNCTIONS, checkDisplayFormula, formatFormulaValue, isFormulaError, parseFormula } from "./index";
import { makeFixture } from "./host.fixture";
import { STRICT_FUNCTIONS } from "./functions";
import { ARITY } from "./names";

/** The functions the checker and the evaluator handle themselves (they take expressions or need scopes). */
const SPECIAL = ["if", "ifs", "and", "or", "let", "lets", "prop", "id", "map", "filter", "find", "findIndex", "some", "every", "count"];

describe("FORMULA_FUNCTIONS", () => {
  it("lists every function once, and only functions", () => {
    const names = FORMULA_FUNCTIONS.map((f) => f.name);
    expect(new Set(names).size).toBe(names.length);
    expect([...names].sort()).toEqual([...ARITY.keys()].sort());
  });

  it("has an implementation for every function of the arity table", () => {
    for (const name of ARITY.keys()) {
      expect(STRICT_FUNCTIONS.has(name) || SPECIAL.includes(name), name).toBe(true);
    }
    for (const name of STRICT_FUNCTIONS.keys()) {
      expect(ARITY.has(name), name).toBe(true);
    }
  });

  it("fills every entry", () => {
    for (const f of FORMULA_FUNCTIONS) {
      expect(["logic", "number", "text", "date", "list", "page", "special"], f.name).toContain(f.category);
      expect(f.signatures.length, f.name).toBeGreaterThan(0);
      for (const signature of f.signatures) {
        expect(signature.startsWith(f.name) || signature.includes(`.${f.name}(`), signature).toBe(true);
        expect(signature, f.name).toContain("→");
      }
      expect(f.example.length, f.name).toBeGreaterThan(0);
    }
  });

  it("keeps the order of the reference: logic, number, text, date, list, page, special", () => {
    const order = ["logic", "number", "text", "date", "list", "page", "special"];
    const seen = FORMULA_FUNCTIONS.map((f) => order.indexOf(f.category));
    expect([...seen].sort((a, b) => a - b)).toEqual(seen);
  });

  it("has examples that parse, check against the editor's schema, and evaluate without throwing", () => {
    const fx = makeFixture();
    for (const f of FORMULA_FUNCTIONS) {
      expect(isFormulaError(parseFormula(f.example)), f.example).toBe(false);
      const checked = checkDisplayFormula(f.example, fx.host, "Name");
      expect(isFormulaError(checked) ? JSON.stringify(checked) : "ok", f.example).toBe("ok");
      const result = fx.run(f.example);
      expect(isFormulaError(result), f.example).toBe(false);
    }
  });

  it("uses each function in its own example (or a method of it)", () => {
    for (const f of FORMULA_FUNCTIONS) {
      expect(f.example.includes(`${f.name}(`), f.example).toBe(true);
    }
  });
});

describe("formatFormulaValue", () => {
  const host = {
    title: (id: string) => (id === "p" ? "Page" : undefined),
    person: (id: string) => (id === "a" ? { name: "Ada", email: "ada@example.com" } : undefined),
  };

  it("applies the format rules to a result", () => {
    expect(formatFormulaValue({ type: "number", data: 1.5 }, host)).toBe("1.5");
    expect(formatFormulaValue({ type: "number", data: 1e21 }, host)).toBe("1e+21");
    expect(formatFormulaValue({ type: "number", data: -0 }, host)).toBe("0");
    expect(formatFormulaValue({ type: "text", data: "x" }, host)).toBe("x");
    expect(formatFormulaValue({ type: "boolean", data: true }, host)).toBe("true");
    expect(formatFormulaValue({ type: "date", data: { date: "2026-10-18" } }, host)).toBe("October 18, 2026");
    expect(formatFormulaValue({ type: "date", data: { date: "2026-10-18", time: "15:05" } }, host)).toBe("October 18, 2026 3:05 PM");
    expect(formatFormulaValue({ type: "date", data: { date: "2026-10-18", time: "00:05" } }, host)).toBe("October 18, 2026 12:05 AM");
    expect(formatFormulaValue({ type: "date", data: { date: "2026-02-30" } }, host)).toBe("");
    expect(formatFormulaValue({ type: "page", data: "p" }, host)).toBe("Page");
    expect(formatFormulaValue({ type: "page", data: "zz" }, host)).toBe("");
    expect(formatFormulaValue({ type: "person", data: "a" }, host)).toBe("Ada");
    expect(formatFormulaValue({ type: "person", data: "zz" }, host)).toBe("");
    expect(
      formatFormulaValue(
        {
          type: "list",
          data: [
            { type: "text", data: "a" },
            { type: "text", data: "b" },
          ],
        },
        host,
      ),
    ).toBe("a, b");
  });
});
