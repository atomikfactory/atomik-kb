// The shared corpus of formula cases (`crates/kb-model/tests/formula-cases/*.json`, `docs/architecture/formulas.md` 7.3):
// the Rust engine and this twin run the same files. Each case is converted from display to stored form, checked, and
// evaluated; one `it` per case so a failure names it. Like the Rust harness, this one gives pages, people and properties
// ids of the length of a UUID (`id()` answers them; a property may be called `title`) and maps them back in the results.

import { readFileSync, readdirSync } from "node:fs";

import { describe, expect, it } from "vitest";

import type { FormulaResult } from "../generated/FormulaResult";
import type { FormulaType } from "../generated/FormulaType";
import type { PropertyKind } from "../generated/PropertyKind";
import type { PropertyValue } from "../generated/PropertyValue";
import type { RollupFunction } from "../generated/RollupFunction";
import { checkFormula, evaluateFormula, formulaToStored, isFormulaError, parseFormula } from "./index";
import type { FormulaEnv, FormulaError, FormulaHost, FormulaProperty, FormulaSchema } from "./index";

declare module "node:fs" {
  export function readdirSync(path: URL): string[];
}

interface SchemaEntry {
  id: string;
  name: string;
  kind: PropertyKind;
  options?: { id: string; name: string }[];
  expression?: string;
  rollup?: { function: RollupFunction; targetKind: PropertyKind | null };
}

interface PageEntry {
  id: string;
  title: string;
  created?: string;
  edited?: string;
  values?: Record<string, PropertyValue>;
}

interface CaseEntry {
  name: string;
  page?: string;
  expression: string;
  self?: string;
  type?: FormulaType;
  result?: FormulaResult | null;
  error?: string;
  at?: number;
  tolerance?: number;
}

interface CorpusFile {
  schema: SchemaEntry[];
  pages: PageEntry[];
  people?: { id: string; name: string; email: string }[];
  files?: { id: string; name: string }[];
  titleLabel?: string;
  env: { now: string; offsetMinutes: number };
  cases: CaseEntry[];
}

const CASES_DIR = new URL("../../../../crates/kb-model/tests/formula-cases/", import.meta.url);

/** A local time `YYYY-MM-DDTHH:mm` as local ms (calendar math in UTC, so the machine's time zone is never read). */
function localMs(text: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/u.exec(text);
  if (m === null) {
    throw new Error(`bad local time in a corpus file: ${text}`);
  }
  return Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
}

/** Ids of the length of a UUID, from the position of the short id in its list. */
function realIds(shorts: readonly string[], prefix: string): { real: Map<string, string>; short: Map<string, string> } {
  const real = new Map<string, string>();
  const short = new Map<string, string>();
  shorts.forEach((id, i) => {
    const uuid = `${prefix}0000000-0000-4000-8000-${(i + 1).toString(16).padStart(12, "0")}`;
    real.set(id, uuid);
    short.set(uuid, id);
  });
  return { real, short };
}

function build(file: CorpusFile): {
  host: FormulaHost;
  schema: FormulaSchema;
  env: FormulaEnv;
  titleLabel: string;
  realPage: (id: string) => string;
  realProperty: (id: string) => string;
  shorten: (result: FormulaResult | null) => FormulaResult | null;
} {
  const titleLabel = file.titleLabel ?? "Name";
  const pageIds = realIds(file.pages.map((p) => p.id), "0");
  const personIds = realIds((file.people ?? []).map((p) => p.id), "a");
  const propertyIds = realIds(file.schema.map((e) => e.id), "b");
  const property = (id: string): string => propertyIds.real.get(id) ?? id;
  const page = (id: string): string => pageIds.real.get(id) ?? id;
  const account = (id: string): string => personIds.real.get(id) ?? id;

  const names: FormulaSchema = {
    property: () => undefined,
    properties: () =>
      file.schema.map(
        (e) => ({ id: property(e.id), name: e.name, kind: e.kind, options: e.options ?? [] }) satisfies FormulaProperty,
      ),
  };
  const properties: FormulaProperty[] = file.schema.map((entry) => {
    let expression: string | undefined;
    if (entry.expression !== undefined) {
      const stored = formulaToStored(entry.expression, names, titleLabel);
      if (isFormulaError(stored)) {
        throw new Error(`the formula ${entry.name} of a corpus file does not convert: ${stored.message}`);
      }
      expression = stored;
    }
    return {
      id: property(entry.id),
      name: entry.name,
      kind: entry.kind,
      options: entry.options ?? [],
      ...(expression === undefined ? {} : { expression }),
      ...(entry.rollup === undefined ? {} : { rollup: entry.rollup }),
    };
  });
  const byId = new Map(properties.map((p) => [p.id, p]));
  const pages = new Map(file.pages.map((p) => [page(p.id), p]));
  const people = new Map((file.people ?? []).map((p) => [account(p.id), p]));
  const files = new Map((file.files ?? []).map((f) => [f.id, f.name]));
  const offsetMinutes = file.env.offsetMinutes;
  const unix = (local: string | undefined): number | undefined =>
    local === undefined ? undefined : localMs(local) - offsetMinutes * 60_000;

  const schema: FormulaSchema = {
    property: (id) => byId.get(id),
    properties: () => properties,
  };
  const host: FormulaHost = {
    ...schema,
    title: (id) => pages.get(id)?.title,
    value: (id, propertyId) => {
      const value = pages.get(id)?.values?.[propertyIds.short.get(propertyId) ?? propertyId];
      if (value?.kind === "relation") {
        return { kind: "relation", value: value.value.map(page) };
      }
      if (value?.kind === "person") {
        return { kind: "person", value: value.value.map(account) };
      }
      return value;
    },
    related: (id, relation) => {
      const value = pages.get(id)?.values?.[propertyIds.short.get(relation) ?? relation];
      return value?.kind === "relation" ? value.value.map(page).filter((related) => pages.has(related)) : [];
    },
    createdMs: (id) => unix(pages.get(id)?.created),
    editedMs: (id) => unix(pages.get(id)?.edited),
    person: (id) => people.get(id),
    fileName: (attachment) => files.get(attachment),
  };
  const env: FormulaEnv = { nowMs: unix(file.env.now) ?? 0, offsetSeconds: offsetMinutes * 60 };

  const shorten = (result: FormulaResult | null): FormulaResult | null => {
    if (result === null) {
      return null;
    }
    switch (result.type) {
      case "page":
        return { type: "page", data: pageIds.short.get(result.data) ?? result.data };
      case "person":
        return { type: "person", data: personIds.short.get(result.data) ?? result.data };
      case "list":
        return { type: "list", data: result.data.map((item) => shorten(item) ?? item) };
      default:
        return result;
    }
  };
  return { host, schema, env, titleLabel, realPage: page, realProperty: property, shorten };
}

function expectError(error: FormulaError, expected: CaseEntry): void {
  expect(error.code, `${error.code} at ${String(error.start)}..${String(error.end)}: ${error.message}`).toBe(expected.error);
  if (expected.at !== undefined) {
    expect(error.start, error.message).toBe(expected.at);
  }
}

function expectResult(actual: FormulaResult | null, expected: FormulaResult | null, tolerance: number | undefined): void {
  if (tolerance === undefined || actual === null || expected === null) {
    expect(actual).toEqual(expected);
    return;
  }
  expect(actual.type).toBe(expected.type);
  if (actual.type === "number" && expected.type === "number") {
    const scale = Math.max(Math.abs(actual.data), Math.abs(expected.data));
    expect(Math.abs(actual.data - expected.data) <= tolerance * scale, `${String(actual.data)} vs ${String(expected.data)}`).toBe(true);
  } else if (actual.type === "list" && expected.type === "list") {
    expect(actual.data.length).toBe(expected.data.length);
    actual.data.forEach((item, i) => {
      expectResult(item, expected.data[i] ?? null, tolerance);
    });
  } else {
    expect(actual).toEqual(expected);
  }
}

const files = readdirSync(CASES_DIR)
  .filter((name) => name.endsWith(".json"))
  .sort();

describe("the formula corpus", () => {
  it("has files", () => {
    expect(files.length).toBeGreaterThan(0);
  });

  for (const name of files) {
    const file = JSON.parse(readFileSync(new URL(name, CASES_DIR), "utf8")) as CorpusFile;
    const { host, schema, env, titleLabel, realPage, realProperty, shorten } = build(file);
    const firstPage = file.pages[0]?.id ?? "";

    describe(name, () => {
      for (const c of file.cases) {
        it(c.name, () => {
          // The grammar is the same in both forms: a syntax error is reported on what the person wrote.
          const parsed = parseFormula(c.expression);
          if (isFormulaError(parsed)) {
            if (c.error === undefined) {
              expect.fail(`unexpected syntax error: ${JSON.stringify(parsed)}`);
            }
            expectError(parsed, c);
            return;
          }
          const stored = formulaToStored(c.expression, schema, titleLabel);
          if (isFormulaError(stored)) {
            if (c.error === undefined) {
              expect.fail(`unexpected conversion error: ${JSON.stringify(stored)}`);
            }
            expectError(stored, c);
            return;
          }
          const checked = checkFormula(stored, schema, c.self === undefined ? undefined : realProperty(c.self));
          if (isFormulaError(checked)) {
            if (c.error === undefined) {
              expect.fail(`unexpected check error: ${JSON.stringify(checked)}`);
            }
            expectError(checked, c);
            return;
          }
          if (c.error !== undefined) {
            expect.fail(`expected the error ${c.error}, the formula checks as ${JSON.stringify(checked)}`);
          }
          if (c.type !== undefined) {
            expect(checked).toEqual(c.type);
          }
          if (c.result !== undefined) {
            const actual = shorten(evaluateFormula(stored, realPage(c.page ?? firstPage), host, env));
            expectResult(actual, c.result, c.tolerance);
          }
        });
      }
    });
  }
});
