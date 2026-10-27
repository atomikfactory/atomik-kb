// A small in-memory host for the formula tests: the schema of the editor's examples (Score, Price, Qty, Status, Tags, Due,
// Done, Tasks, Hours, Owner, Note, …), a few pages, and helpers that run a display-form expression the way the in-memory
// core does (convert, check, evaluate).

import type { FormulaResult } from "../generated/FormulaResult";
import type { PropertyKind } from "../generated/PropertyKind";
import type { PropertyValue } from "../generated/PropertyValue";
import type { RollupFunction } from "../generated/RollupFunction";
import { checkFormula, evaluateFormula, formulaToStored, isFormulaError } from "./index";
import type { FormulaEnv, FormulaError, FormulaHost, FormulaProperty } from "./index";
import type { FormulaType } from "../generated/FormulaType";

export interface PropertySpec {
  id: string;
  name: string;
  kind: PropertyKind;
  options?: { id: string; name: string }[];
  /** A formula's expression in display form. */
  expression?: string;
  /** A formula's expression as stored (ids), taken as is: for formulas that do not convert or check. */
  stored?: string;
  rollup?: { function: RollupFunction; targetKind: PropertyKind | null };
}

export interface PageSpec {
  id: string;
  title: string;
  created?: number;
  edited?: number;
  values?: Record<string, PropertyValue>;
}

export const DEFAULT_SCHEMA: PropertySpec[] = [
  { id: "score", name: "Score", kind: "number" },
  { id: "price", name: "Price", kind: "number" },
  { id: "qty", name: "Qty", kind: "number" },
  {
    id: "status",
    name: "Status",
    kind: "status",
    options: [
      { id: "todo", name: "To do" },
      { id: "done", name: "Done" },
    ],
  },
  {
    id: "tags",
    name: "Tags",
    kind: "multi_select",
    options: [
      { id: "t1", name: "red" },
      { id: "t2", name: "blue" },
    ],
  },
  { id: "due", name: "Due", kind: "date" },
  { id: "done", name: "Done", kind: "checkbox" },
  { id: "tasks", name: "Tasks", kind: "relation" },
  { id: "hours", name: "Hours", kind: "number" },
  { id: "owner", name: "Owner", kind: "person" },
  { id: "note", name: "Note", kind: "text" },
];

export const DEFAULT_PAGES: PageSpec[] = [
  {
    id: "p1",
    title: "Project",
    values: {
      score: { kind: "number", value: 7 },
      price: { kind: "number", value: 3 },
      qty: { kind: "number", value: 4 },
      status: { kind: "status", value: "todo" },
      tags: { kind: "multi_select", value: ["t1", "t2"] },
      due: { kind: "date", value: { date: "2026-10-25" } },
      done: { kind: "checkbox", value: true },
      tasks: { kind: "relation", value: ["p2", "p3"] },
      owner: { kind: "person", value: ["ada"] },
      note: { kind: "text", value: "Hello World 42" },
    },
  },
  { id: "p2", title: "Task A", values: { hours: { kind: "number", value: 5 } } },
  { id: "p3", title: "Task B", values: { hours: { kind: "number", value: 2.5 } } },
  { id: "p4", title: "", values: {} },
];

/** 2026-10-18T09:30 local at UTC+2, as Unix ms. */
export const DEFAULT_ENV: FormulaEnv = { nowMs: Date.UTC(2026, 9, 18, 9, 30) - 2 * 3_600_000, offsetSeconds: 7200 };

export interface Fixture {
  host: FormulaHost;
  env: FormulaEnv;
  /** The properties of the schema as the engine sees them (formula expressions in stored form). */
  properties: FormulaProperty[];
  /** Runs a display-form expression on a page: the result, or the error of the conversion or the check. */
  run(expression: string, page?: string): FormulaResult | null | FormulaError;
  /** Converts and checks a display-form expression. */
  type(expression: string, self?: string): FormulaType | FormulaError;
  /** The stored form of a display-form expression. */
  stored(expression: string): string;
}

export function makeFixture(
  schema: PropertySpec[] = DEFAULT_SCHEMA,
  pages: PageSpec[] = DEFAULT_PAGES,
  env: FormulaEnv = DEFAULT_ENV,
  people: Record<string, { name: string; email: string }> = { ada: { name: "Ada", email: "ada@example.com" } },
): Fixture {
  const names = {
    property: () => undefined,
    properties: () => schema.map((s) => ({ id: s.id, name: s.name, kind: s.kind, options: s.options ?? [] })),
  };
  const properties: FormulaProperty[] = schema.map((s) => {
    let expression: string | undefined = s.stored;
    if (s.expression !== undefined) {
      const converted = formulaToStored(s.expression, names, "Name");
      if (isFormulaError(converted)) {
        throw new Error(converted.message);
      }
      expression = converted;
    }
    return {
      id: s.id,
      name: s.name,
      kind: s.kind,
      options: s.options ?? [],
      ...(expression === undefined ? {} : { expression }),
      ...(s.rollup === undefined ? {} : { rollup: s.rollup }),
    };
  });
  const byId = new Map(properties.map((p) => [p.id, p]));
  const byPage = new Map(pages.map((p) => [p.id, p]));
  const host: FormulaHost = {
    property: (id) => byId.get(id),
    properties: () => properties,
    title: (page) => byPage.get(page)?.title,
    value: (page, property) => byPage.get(page)?.values?.[property],
    related: (page, relation) => {
      const value = byPage.get(page)?.values?.[relation];
      return value?.kind === "relation" ? value.value.filter((id) => byPage.has(id)) : [];
    },
    createdMs: (page) => byPage.get(page)?.created,
    editedMs: (page) => byPage.get(page)?.edited,
    person: (account) => people[account],
    fileName: () => undefined,
  };
  const stored = (expression: string): string => {
    const converted = formulaToStored(expression, host, "Name");
    if (isFormulaError(converted)) {
      throw new Error(`${converted.code}: ${converted.message}`);
    }
    return converted;
  };
  return {
    host,
    env,
    properties,
    stored,
    run(expression, page = "p1") {
      const converted = formulaToStored(expression, host, "Name");
      if (isFormulaError(converted)) {
        return converted;
      }
      const checked = checkFormula(converted, host);
      if (isFormulaError(checked)) {
        return checked;
      }
      return evaluateFormula(converted, page, host, env);
    },
    type(expression, self) {
      const converted = formulaToStored(expression, host, "Name");
      if (isFormulaError(converted)) {
        return converted;
      }
      return checkFormula(converted, host, self);
    },
  };
}

/** The value's data (`{ type: "number", data: 3 }` → 3) for compact assertions; errors and `null` pass through. */
export function plain(result: FormulaResult | null | FormulaError): unknown {
  if (result === null || isFormulaError(result)) {
    return result;
  }
  if (result.type === "list") {
    return result.data.map((item) => plain(item));
  }
  return result.data;
}
