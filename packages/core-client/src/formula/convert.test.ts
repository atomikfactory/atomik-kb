// Names and ids (section 6): display to stored, stored to display, remap, referenced ids, and the display positions of
// check errors.

import { describe, expect, it } from "vitest";

import {
  checkDisplayFormula,
  formulaReferencedIds,
  formulaToDisplay,
  formulaToStored,
  isFormulaError,
  remapFormulaIds,
} from "./index";
import { makeFixture } from "./host.fixture";
import type { PropertySpec } from "./host.fixture";

const schemaSpec: PropertySpec[] = [
  { id: "id-price", name: "Price", kind: "number" },
  { id: "id-qty", name: "Qty", kind: "number" },
  { id: "id-rel", name: "Rel", kind: "relation" },
  { id: "id-hours", name: "Hours", kind: "number" },
  { id: "id-quote", name: 'He said "hi" \\ bye', kind: "text" },
  { id: "id-dup-1", name: "Dup", kind: "number" },
  { id: "id-dup-2", name: "Dup", kind: "text" },
  { id: "id-emoji", name: "Prix 💶", kind: "number" },
  { id: "id-cb", name: "Creator", kind: "created_by" },
];
const schema = makeFixture(schemaSpec, []).host;

function stored(display: string, titleLabel = "Name"): string {
  const result = formulaToStored(display, schema, titleLabel);
  if (isFormulaError(result)) {
    throw new Error(result.message);
  }
  return result;
}

describe("formulaToStored", () => {
  it("rewrites names to ids and keeps everything else byte for byte", () => {
    expect(stored('prop("Price") * prop("Qty")')).toBe('prop("id-price") * prop("id-qty")');
    expect(stored('  prop( /* a */ "Price" )\n\t+ "prop(\\"Qty\\")" /* prop("Qty") */')).toBe(
      '  prop( /* a */ "id-price" )\n\t+ "prop(\\"Qty\\")" /* prop("Qty") */',
    );
    expect(stored('prop("Rel").map(current.prop("Hours")).sum()')).toBe('prop("id-rel").map(current.prop("id-hours")).sum()');
    expect(stored("1 + 2")).toBe("1 + 2");
    expect(stored("")).toBe("");
  });

  it("reads the title through `title` and the label the UI gives", () => {
    expect(stored('prop("Name")')).toBe('prop("title")');
    expect(stored('prop("title")')).toBe('prop("title")');
    expect(stored('prop("Titre")', "Titre")).toBe('prop("title")');
    expect(stored('prop("Rel").first().prop("Name")')).toBe('prop("id-rel").first().prop("title")');
  });

  it("lets a property win over the title's label, and the first of two with one name win", () => {
    expect(stored('prop("Price")', "Price")).toBe('prop("id-price")');
    expect(stored('prop("Dup")')).toBe('prop("id-dup-1")');
  });

  it("writes and reads names with quotes, backslashes and emoji", () => {
    expect(stored('prop("He said \\"hi\\" \\\\ bye")')).toBe('prop("id-quote")');
    expect(formulaToDisplay('prop("id-quote")', schema)).toBe('prop("He said \\"hi\\" \\\\ bye")');
    expect(stored('prop("Prix 💶") + 1')).toBe('prop("id-emoji") + 1');
    expect(formulaToDisplay('1 + prop("id-emoji")', schema)).toBe('1 + prop("Prix 💶")');
  });

  it("reports an unknown name at its argument, in code points", () => {
    const result = formulaToStored('1 + prop("Nope")', schema, "Name");
    expect(result).toMatchObject({ code: "unknown_property", start: 9, end: 15, detail: "Nope" });
    const emoji = formulaToStored('"💶" + prop("Nope")', schema, "Name");
    expect(emoji).toMatchObject({ code: "unknown_property", start: 11, end: 17 });
    const first = formulaToStored('prop("A") + prop("B")', schema, "Name");
    expect(first).toMatchObject({ detail: "A", start: 5 });
  });

  it("applies to the calls it can see in a text that does not parse", () => {
    expect(stored('prop("Price") + (')).toBe('prop("id-price") + (');
    expect(stored('prop("Price") prop("Qty") ]')).toBe('prop("id-price") prop("id-qty") ]');
    expect(stored('prop("Pri')).toBe('prop("Pri');
    expect(stored('prop("Price"')).toBe('prop("Price"');
    expect(stored('prop("Price\\q")')).toBe('prop("Price\\q")');
    expect(stored('prop(Price)')).toBe("prop(Price)");
    expect(stored('prop("Price", "Qty")')).toBe('prop("Price", "Qty")');
    expect(stored('foo("Price")')).toBe('foo("Price")');
    expect(stored('"prop(\\"Price\\")"')).toBe('"prop(\\"Price\\")"');
  });

  it("does not rewrite what is not a prop call", () => {
    expect(stored('"Price" + format(prop("Price"))')).toBe('"Price" + format(prop("id-price"))');
    expect(stored('props("Price")')).toBe('props("Price")');
  });
});

describe("formulaToDisplay", () => {
  it("is the reverse; an id with no property stays", () => {
    expect(formulaToDisplay('prop("id-price") * prop("id-qty")', schema)).toBe('prop("Price") * prop("Qty")');
    expect(formulaToDisplay('prop("zzz") + prop("id-price")', schema)).toBe('prop("zzz") + prop("Price")');
    expect(formulaToDisplay('prop("title")', schema)).toBe('prop("title")');
    expect(formulaToDisplay('prop("title")', schema, "Name")).toBe('prop("Name")');
    expect(formulaToDisplay('prop("title")', schema, "Price")).toBe('prop("title")');
    expect(formulaToDisplay('prop("id-rel").map(current.prop("id-hours"))', schema)).toBe(
      'prop("Rel").map(current.prop("Hours"))',
    );
  });

  it("round trips", () => {
    for (const display of [
      'prop("Price") * prop("Qty")',
      'prop("Rel").filter(current.prop("Hours") > 2).map(current.prop("Name"))',
      '/* c */ prop( "Prix 💶" )',
      'prop("He said \\"hi\\" \\\\ bye") + "x"',
    ]) {
      const converted = stored(display);
      expect(formulaToDisplay(converted, schema, "Name")).toBe(display);
    }
  });
});

describe("remapFormulaIds", () => {
  it("rewrites the ids through the map and leaves the others", () => {
    const map = (id: string): string | undefined => ({ a: "x", b: "y" })[id as "a" | "b"];
    expect(remapFormulaIds('prop("a") + prop("b") + prop("c")', map)).toBe('prop("x") + prop("y") + prop("c")');
    expect(remapFormulaIds('prop("a").first().prop("b") /* prop("a") */', map)).toBe(
      'prop("x").first().prop("y") /* prop("a") */',
    );
    expect(remapFormulaIds("1 + (", map)).toBe("1 + (");
    expect(remapFormulaIds('prop("a") + (', map)).toBe('prop("x") + (');
    expect(remapFormulaIds('prop("title")', () => undefined)).toBe('prop("title")');
  });
});

describe("formulaReferencedIds", () => {
  it("lists the ids in order without repeats, `.prop` included", () => {
    expect(formulaReferencedIds('prop("a") + prop("b") + prop("a")')).toEqual(["a", "b"]);
    expect(formulaReferencedIds('prop("r").map(current.prop("h")).sum() + prop("title")')).toEqual(["r", "h", "title"]);
    expect(formulaReferencedIds("1 + 2")).toEqual([]);
    expect(formulaReferencedIds('prop("a") + (')).toEqual(["a"]);
    expect(formulaReferencedIds('"prop(\\"a\\")" /* prop("b") */')).toEqual([]);
    expect(formulaReferencedIds("")).toEqual([]);
  });
});

describe("checkDisplayFormula", () => {
  it("checks the display text and reports where in it", () => {
    expect(checkDisplayFormula('prop("Price") * 2', schema, "Name")).toEqual({ base: "number", list: false });
    // `prop("Name")` is `prop("title")` once stored: the ranges after it move.
    expect(checkDisplayFormula('prop("Name") + 1', schema, "Name")).toEqual({ base: "text", list: false });
    expect(checkDisplayFormula('prop("Name") == 1', schema, "Name")).toMatchObject({ code: "type", start: 16, end: 17 });
    expect(checkDisplayFormula('prop("Price") + prop("Name") + foo(1)', schema, "Name")).toMatchObject({
      code: "unknown_function",
      start: 31,
      end: 34,
    });
    expect(checkDisplayFormula('prop("Price") +', schema, "Name")).toMatchObject({ code: "syntax", start: 15, end: 15 });
    expect(checkDisplayFormula('prop("Price" + 1)', schema, "Name")).toMatchObject({ code: "syntax" });
    expect(checkDisplayFormula('prop("Nope") + 1', schema, "Name")).toMatchObject({ code: "unknown_property", start: 5 });
    expect(checkDisplayFormula('prop("Creator")', schema, "Name")).toMatchObject({
      code: "unsupported_property",
      start: 5,
      end: 14,
    });
  });

  it("maps a range that covers a rewritten argument to the argument as written", () => {
    // ids are longer than the names here ("id-price" against "Price"), the shift goes both ways with "Prix 💶".
    expect(checkDisplayFormula('prop("Price") + prop("Qty") + true', schema, "Name")).toMatchObject({
      code: "type",
      start: 30,
      end: 34,
    });
    expect(checkDisplayFormula('prop("Prix 💶") + true', schema, "Name")).toMatchObject({ start: 17, end: 21 });
  });
});
