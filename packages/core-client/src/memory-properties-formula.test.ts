// Formulas (ADR-0055) in `MemoryTransport`: the configuration and its refusals, the values computed on read, the derived
// result type, loops, the reach over relations (one hop, live pages), the time of a read, the refused filter, backups,
// seeds and the rules of the value. The mirror of `kb-model`'s `tests/formulas.rs`; the engine has its own corpus.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { FormulaResult } from "./generated/FormulaResult";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import {
  copyPropertyValue,
  isComputedPropertyKind,
  isEmptyPropertyValue,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";

const T0 = 1_700_000_000_000;
/** 2026-10-07T14:30:00Z. */
const OCT_7_1430 = 1_791_383_400_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

const number = (value: number): PropertyValue => ({ kind: "number", value });
const text = (data: string): FormulaResult => ({ type: "text", data });
const num = (data: number): FormulaResult => ({ type: "number", data });
const formula = (expression: string): PropertyConfig => ({ kind: "formula", expression });

async function setup(options: MemoryTransportOptions = {}, titles = ["Alpha", "Beta", "Gamma"]) {
  let counter = 0x1000;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0 + counter,
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: titles.map((title) => ({ title })) }],
    ...options,
  });
  const client = new CoreClient(transport);
  const space = must((await client.spaceList())[0]);
  const roots = (await client.treeGet(space.id)).roots;
  const pages = titles.map((title) => must(roots.find((node) => node.page.title === title)).page.id);
  const create = (name: string, kind: PropertyDef["kind"], config?: PropertyConfig): Promise<PropertyDef> =>
    client.propertyCreate(space.id, name, kind, config);
  /** The stored expression: `{name}` stands for the id of the property of that name. */
  const stored = async (template: string): Promise<string> => {
    const schema = await client.propertySchemaGet(space.id);
    return template.replace(/\{([^}]+)\}/gu, (_, name: string) => must(schema.find((def) => def.name === name)).id);
  };
  const make = async (name: string, template: string): Promise<PropertyDef> =>
    create(name, "formula", formula(await stored(template)));
  /** A page's formula value, `undefined` when it has none. */
  const result = async (pageId: string, property: PropertyDef): Promise<FormulaResult | undefined> => {
    const found = (await client.pagePropertiesGet(pageId)).find(([propertyId]) => propertyId === property.id);
    if (found === undefined) {
      return undefined;
    }
    expect(found[1].kind).toBe("formula");
    return found[1].kind === "formula" ? found[1].value : undefined;
  };
  const resultType = async (property: PropertyDef) =>
    (must((await client.propertySchemaGet(space.id)).find((def) => def.id === property.id)).config as {
      resultType?: unknown;
    }).resultType;
  return { transport, client, spaceId: space.id, pages, create, stored, make, result, resultType };
}

describe("a formula", () => {
  it("is computed when the values are read, never stored, and cannot be set", async () => {
    const { client, pages, create, make, result, spaceId } = await setup();
    const [alpha, beta] = pages;
    const price = await create("Price", "number");
    const double = await make("Double", 'prop("{Price}") * 2');
    expect(double.config).toEqual({
      kind: "formula",
      expression: `prop("${price.id}") * 2`,
      resultType: { base: "number", list: false },
    });
    expect(await result(must(alpha), double)).toBeUndefined();
    await client.pagePropertySet(must(alpha), price.id, number(3));
    expect(await result(must(alpha), double)).toEqual(num(6));
    expect(await result(must(beta), double)).toBeUndefined();
    await client.pagePropertySet(must(alpha), price.id, number(4.5));
    expect(await result(must(alpha), double)).toEqual(num(9));

    // The tree carries it like any value.
    const info = must((await client.treeGet(spaceId)).roots.find((node) => node.page.id === alpha)).page;
    expect(info.properties).toContainEqual([double.id, { kind: "formula", value: num(9) }]);

    // Nothing writes it.
    const refusal = {
      code: "InvalidInput",
      message: "this property is computed from the page and cannot be set",
      details: { field: "propertyId" },
    };
    expect((await failure(client.pagePropertySet(must(alpha), double.id, { kind: "formula", value: text("x") }))).toApiError()).toEqual(
      refusal,
    );
    expect((await failure(client.pagePropertySet(must(alpha), double.id, null))).toApiError()).toEqual(refusal);
    expect(isComputedPropertyKind("formula")).toBe(true);
  });

  it("reads the title, other formulas and rollups", async () => {
    const { client, pages, create, make, result } = await setup();
    const [alpha, beta] = pages;
    const price = await create("Price", "number");
    await client.pagePropertySet(must(alpha), price.id, number(1));
    await client.pagePropertySet(must(beta), price.id, number(2));
    const shout = await make("Shout", 'prop("title") + "!"');
    const plus = await make("Plus", 'prop("{Price}") + 1');
    const times = await make("Times", 'prop("{Plus}") * 10');
    expect(await result(must(alpha), shout)).toEqual(text("Alpha!"));
    expect(await result(must(alpha), times)).toEqual(num(20));
    expect(await result(must(beta), times)).toEqual(num(30));
    expect(plus.config).toMatchObject({ resultType: { base: "number", list: false } });

    const tasks = await create("Tasks", "relation");
    const total = await create("Total", "rollup", {
      kind: "rollup",
      relationPropertyId: tasks.id,
      target: price.id,
      function: "sum",
    });
    await client.pagePropertySet(must(alpha), tasks.id, { kind: "relation", value: [must(alpha), must(beta)] });
    const plusTotal = await make("Plus total", 'prop("{Total}") + 1');
    expect(await result(must(alpha), plusTotal)).toEqual(num(4));
    expect(total.kind).toBe("rollup");
  });

  it("is checked when created or changed, and a refusal says the code and the position", async () => {
    const { client, spaceId, create, make, stored } = await setup();
    await create("Price", "number");
    await create("Notes", "text");
    const refusal = async (promise: Promise<unknown>): Promise<string> => {
      const error = await failure(promise);
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toEqual({ field: "config" });
      return error.message;
    };
    expect(await refusal(client.propertyCreate(spaceId, "A", "formula", formula("1 +")))).toMatch(
      /^the formula is refused: syntax at \d+\.\.\d+: /u,
    );
    expect(await refusal(client.propertyCreate(spaceId, "A", "formula", formula('prop("0000") + 1')))).toContain(
      "unknown_property",
    );
    expect(await refusal(make("A", 'prop("{Notes}") * 2'))).toContain("type");
    expect(await refusal(client.propertyCreate(spaceId, "A", "formula"))).toBe("a formula needs a configuration");
    expect(await refusal(client.propertyCreate(spaceId, "A", "text", formula("1")))).toBe(
      "only a relation, a rollup or a formula takes a configuration",
    );
    expect((await client.propertySchemaGet(spaceId)).map((def) => def.name)).toEqual(["Tags", "Price", "Notes"]);

    const made = await make("Double", 'prop("{Price}") * 2');
    const before = await client.propertySchemaGet(spaceId);
    expect(await refusal(client.propertyConfigSet(spaceId, made.id, formula(await stored('prop("{Notes}") * 2'))))).toContain(
      "type",
    );
    expect(await client.propertySchemaGet(spaceId)).toEqual(before);
    const changed = await client.propertyConfigSet(spaceId, made.id, formula(await stored('prop("{Notes}") + "!"')));
    expect(changed.config).toMatchObject({ kind: "formula", resultType: { base: "text", list: false } });
    // A result type sent by a client is ignored; another kind's property takes no configuration.
    const same = await client.propertyConfigSet(spaceId, made.id, {
      kind: "formula",
      expression: await stored('prop("{Notes}") + "!"'),
      resultType: { base: "boolean", list: true },
    });
    expect(same.config).toMatchObject({ resultType: { base: "text", list: false } });
    expect(await refusal(client.propertyConfigSet(spaceId, must(before[1]).id, formula("1")))).toBe(
      "only a relation, a rollup or a formula takes a configuration",
    );
  });

  it("is refused when it depends on itself, directly or through another formula", async () => {
    const { client, spaceId, create, make, stored } = await setup();
    await create("Price", "number");
    const first = await make("First", 'prop("{Price}") + 1');
    await make("Second", 'prop("{First}") + 1');
    const refusal = async (expression: string): Promise<string> => {
      const error = await failure(client.propertyConfigSet(spaceId, first.id, formula(await stored(expression))));
      expect(error.details).toEqual({ field: "config" });
      return error.message;
    };
    expect(await refusal('prop("{First}")')).toContain("cycle");
    expect(await refusal('prop("{Second}") + 1')).toContain("cycle");
    const schema = await client.propertySchemaGet(spaceId);
    expect(must(schema.find((def) => def.id === first.id)).config).toMatchObject({
      expression: `prop("${must(schema.find((def) => def.name === "Price")).id}") + 1`,
    });
  });

  it("reports the result type, and loses it and its values when a property it reads is deleted", async () => {
    const { client, pages, spaceId, create, make, result, resultType } = await setup();
    const [alpha] = pages;
    const price = await create("Price", "number");
    await create("Due", "date");
    await create("Related", "relation");
    const double = await make("Double", 'prop("{Price}") * 2');
    const plus = await make("Plus", 'prop("{Double}") + 1');
    expect(await resultType(await make("Label", 'prop("title") + "x"'))).toEqual({ base: "text", list: false });
    expect(await resultType(await make("When", 'prop("{Due}")'))).toEqual({ base: "date", list: false });
    expect(await resultType(await make("Linked", 'prop("{Related}")'))).toEqual({ base: "page", list: true });
    expect(await resultType(await make("Tags too", `prop("${must((await client.propertySchemaGet(spaceId))[0]).id}")`))).toEqual({
      base: "text",
      list: true,
    });
    await client.pagePropertySet(must(alpha), price.id, number(2));
    expect(await result(must(alpha), plus)).toEqual(num(5));

    await client.propertyDelete(spaceId, price.id);
    expect(await resultType(double)).toBeUndefined();
    expect(await resultType(plus)).toBeUndefined();
    expect(await result(must(alpha), double)).toBeUndefined();
    expect(await result(must(alpha), plus)).toBeUndefined();
    // The expression is still stored: the settings show the error, and a fix brings the values back.
    const cost = await create("Cost", "number");
    await client.propertyConfigSet(spaceId, double.id, formula(`prop("${cost.id}") * 2`));
    expect(await resultType(plus)).toEqual({ base: "number", list: false });
  });

  it("reads the pages of a relation one hop, the live ones only", async () => {
    const { client, pages, create, make, result } = await setup({}, ["Project", "Task A", "Task B", "Task C"]);
    const [project, a, b, c] = pages;
    const tasks = await create("Tasks", "relation");
    const hours = await create("Hours", "number");
    const sum = await make("Total hours", 'prop("{Tasks}").map(current.prop("{Hours}")).sum()');
    const names = await make("Names", 'prop("{Tasks}").map(current.prop("title")).join(", ")');
    await client.pagePropertySet(must(a), hours.id, number(5));
    await client.pagePropertySet(must(b), hours.id, number(7));
    await client.pagePropertySet(must(c), hours.id, number(11));
    await client.pagePropertySet(must(project), tasks.id, { kind: "relation", value: [must(a), must(b), must(c)] });
    expect(await result(must(project), sum)).toEqual(num(23));
    expect(await result(must(project), names)).toEqual(text("Task A, Task B, Task C"));
    expect(sum.config).toMatchObject({ resultType: { base: "number", list: false } });

    // A trashed page is not related any more.
    await client.pageTrash(must(b));
    expect(await result(must(project), sum)).toEqual(num(16));
    expect(await result(must(project), names)).toEqual(text("Task A, Task C"));
  });

  it("reads a two-way relation from both sides", async () => {
    const { client, pages, make, result, spaceId } = await setup({}, ["Project", "Task"]);
    const [project, task] = pages;
    const forward = await client.propertyCreate(spaceId, "Tasks", "relation", { kind: "relation", twoWay: true });
    const back = must((await client.propertySchemaGet(spaceId)).find((def) => def.name === "Tasks (back)"));
    const owner = await make("Owner", `prop("${back.id}").map(current.prop("title")).join("")`);
    await client.pagePropertySet(must(project), forward.id, { kind: "relation", value: [must(task)] });
    expect(await result(must(task), owner)).toEqual(text("Project"));
  });

  it("reads now() and today() with the time of the read and the offset of the device", async () => {
    let now = OCT_7_1430;
    let offset = 0;
    const { client, pages, make, result } = await setup({ now: () => now, localOffsetSeconds: () => offset });
    const [alpha] = pages;
    const stamp = await make("Stamp", 'formatDate(now(), "YYYY-MM-DD HH:mm")');
    const day = await make("Day", 'formatDate(today(), "YYYY-MM-DD")');
    expect(await result(must(alpha), stamp)).toEqual(text("2026-10-07 14:30"));
    expect(await result(must(alpha), day)).toEqual(text("2026-10-07"));
    // Ten hours east of Greenwich it is already the next day; time moves, the value follows at the next read.
    offset = 10 * 3600;
    expect(await result(must(alpha), stamp)).toEqual(text("2026-10-08 00:30"));
    expect(await result(must(alpha), day)).toEqual(text("2026-10-08"));
    offset = 0;
    now += 86_400_000;
    expect(await result(must(alpha), day)).toEqual(text("2026-10-08"));
    expect(await client.pagePropertiesGet(must(alpha))).toHaveLength(2);
  });

  it("reads the creation time in the local day", async () => {
    const { client, make, result, create, spaceId } = await setup({ now: () => OCT_7_1430, localOffsetSeconds: () => 2 * 3600 });
    const created = await create("Created", "created_time");
    const when = await make("When", 'formatDate(prop("{Created}"), "YYYY-MM-DD HH:mm")');
    const page = await client.pageCreate({ spaceId, title: "Fresh" });
    expect(await result(page.id, when)).toEqual(text("2026-10-07 16:30"));
    expect(created.kind).toBe("created_time");
  });

  it("has no filter in PagesByProperty", async () => {
    const { client, spaceId, create, make } = await setup();
    await create("Price", "number");
    const double = await make("Double", 'prop("{Price}") * 2');
    for (const filter of [{ type: "HasValue" }, { type: "Checked", data: true }] as const) {
      const error = await failure(client.pagesByProperty(spaceId, double.id, filter));
      expect(error.toApiError()).toEqual({
        code: "InvalidInput",
        message: "a formula cannot be searched; its pages are filtered in the views",
        details: { field: "filter" },
      });
    }
  });
});

describe("the value of a formula", () => {
  const value = (data: FormulaResult): PropertyValue => ({ kind: "formula", value: data });

  it("is computed, copies deeply, compares by content and has no search text of its own", () => {
    expect(propertyValueProblem(value(num(1)))).toBe("is computed from the page and cannot be set");
    expect(isEmptyPropertyValue(value(num(0)))).toBe(false);
    const list: FormulaResult = {
      type: "list",
      data: [{ type: "date", data: { date: "2026-10-07", time: "14:30" } }, { type: "date", data: { date: "2026-10-08" } }],
    };
    const copy = copyPropertyValue(value(list));
    expect(copy).toEqual(value(list));
    expect(copy).not.toBe(value(list));
    if (copy.kind === "formula" && copy.value.type === "list") {
      expect(copy.value.data).not.toBe(list.data);
    }
    expect(propertyValuesEqual(value(list), copy)).toBe(true);
    expect(propertyValuesEqual(value(num(1)), value(num(2)))).toBe(false);
    expect(propertyValuesEqual(value(num(1)), value(text("1")))).toBe(false);
    expect(propertyValuesEqual(value(list), value({ type: "list", data: [{ type: "date", data: { date: "2026-10-07" } }] }))).toBe(false);
  });

  it("reads as the text format() makes of it", () => {
    const none = () => undefined;
    const title = (page: string): string | undefined => ({ [id(1)]: "Roadmap" })[page];
    const person = (account: string): string | undefined => ({ [id(2)]: "Ada" })[account];
    expect(propertyValueText(value(num(1.5)), none)).toBe("1.5");
    expect(propertyValueText(value({ type: "boolean", data: true }), none)).toBe("true");
    expect(propertyValueText(value({ type: "date", data: { date: "2026-10-07" } }), none)).toBe("October 7, 2026");
    expect(
      propertyValueText(
        value({ type: "list", data: [{ type: "page", data: id(1) }, { type: "page", data: id(9) }, { type: "person", data: id(2) }] }),
        none,
        none,
        title,
        person,
      ),
    ).toBe("Roadmap, , Ada");
  });
});

describe("a backup and a seed", () => {
  it("carry the expression of a formula and nothing else, and a restored space reads the same", async () => {
    const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
    let counter = 0;
    const make = (files?: Map<string, Uint8Array>, prefix = "00000000") => {
      const transport = new MemoryTransport({
        newId: () => `${prefix}-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
        now: () => T0,
        ...(files === undefined
          ? {
              workspaceName: "Research",
              spaces: [{ name: "Notes", pages: [{ title: "Alpha" }] }],
              security: { kitCreatedAt: T0 - 10_000, kitVerifiedAt: T0 - 9_000, kitKey: KEY },
            }
          : { files }),
      });
      return { transport, client: new CoreClient(transport) };
    };
    const origin = make();
    const space = must((await origin.client.spaceList())[0]);
    const alpha = must((await origin.client.treeGet(space.id)).roots[0]).page.id;
    const price = await origin.client.propertyCreate(space.id, "Price", "number");
    const double = await origin.client.propertyCreate(space.id, "Double", "formula", formula(`prop("${price.id}") * 2`));
    await origin.client.pagePropertySet(alpha, price.id, number(4));
    await origin.client.backupExport("C:/backups/formula.atomikkb");

    const words = (await kitWordsFromEntropy(KEY)).join(" ");
    const restored = make(origin.transport.files, "11111111");
    await restored.client.backupImport(undefined, words);
    const schema = await restored.client.propertySchemaGet(space.id);
    expect(must(schema.find((def) => def.id === double.id)).config).toEqual({
      kind: "formula",
      expression: `prop("${price.id}") * 2`,
      resultType: { base: "number", list: false },
    });
    expect(await restored.client.pagePropertiesGet(alpha)).toContainEqual([double.id, { kind: "formula", value: num(8) }]);
  });

  it("seed formulas by the names of the properties they read, and refuse one that does not check", async () => {
    const seeded = (formulas: { name: string; formula: string }[]) =>
      new MemoryTransport({
        newId: (() => {
          let counter = 0x2000;
          return () => id(++counter);
        })(),
        now: () => T0,
        workspaceName: "Home",
        spaces: [
          {
            name: "Personal",
            properties: [
              { name: "Priority", kind: "number" },
              ...formulas.map(({ name, formula: expression }) => ({ name, kind: "formula" as const, formula: expression })),
            ],
            pages: [{ title: "Write", properties: { Priority: 3 } }, { title: "Ship" }],
          },
        ],
      });
    const transport = seeded([
      { name: "Score", formula: 'prop("Priority") * 10' },
      { name: "Label", formula: 'prop("title") + " (" + format(prop("Score")) + ")"' },
    ]);
    const client = new CoreClient(transport);
    const space = must((await client.spaceList())[0]);
    const schema = await client.propertySchemaGet(space.id);
    const priority = must(schema.find((def) => def.name === "Priority"));
    const score = must(schema.find((def) => def.name === "Score"));
    const label = must(schema.find((def) => def.name === "Label"));
    expect(score.config).toEqual({
      kind: "formula",
      expression: `prop("${priority.id}") * 10`,
      resultType: { base: "number", list: false },
    });
    expect(label.config).toMatchObject({ resultType: { base: "text", list: false } });
    const write = must((await client.treeGet(space.id)).roots.find((node) => node.page.title === "Write")).page;
    expect(write.properties).toContainEqual([score.id, { kind: "formula", value: num(30) }]);
    expect(write.properties).toContainEqual([label.id, { kind: "formula", value: text("Write (30)") }]);

    expect(() => seeded([{ name: "Bad", formula: 'prop("Nothing") * 2' }])).toThrow(/the formula "Bad" is refused/u);
    expect(() => seeded([{ name: "Bad", formula: "1 +" }])).toThrow(/the formula "Bad" is refused/u);
  });
});

describe("a rollup that shows the originals", () => {
  it("carries the options of its target, so a formula reads their names", async () => {
    const { client, pages, create, make, result, spaceId } = await setup({}, ["Project", "Task A", "Task B"]);
    const [project, a, b] = pages;
    const tasks = await create("Tasks", "relation");
    const stage = await create("Stage", "select");
    const todo = await client.propertyOptionAdd(spaceId, stage.id, "Todo", "gray");
    const done = await client.propertyOptionAdd(spaceId, stage.id, "Done", "green");
    await create("Originals", "rollup", {
      kind: "rollup",
      relationPropertyId: tasks.id,
      target: stage.id,
      function: "show_original",
    });
    const names = await make("Names", 'prop("{Originals}").join(", ")');
    await client.pagePropertySet(must(a), stage.id, { kind: "select", value: todo.id });
    await client.pagePropertySet(must(b), stage.id, { kind: "select", value: done.id });
    await client.pagePropertySet(must(project), tasks.id, { kind: "relation", value: [must(a), must(b)] });
    expect(await result(must(project), names)).toEqual(text("Todo, Done"));
  });
});
