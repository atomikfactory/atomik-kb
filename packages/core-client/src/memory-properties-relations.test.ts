// Relations (one- and two-way) and rollups in `MemoryTransport` (ADR-0027): the value and its
// messages, the symmetric reads, the filters, the search text, purging and deleting, rollups and
// their configuration, backups and seeds. Ported from the Rust tests of `kb-model`
// (`tests/relations.rs`) and `kb-core` (`tests_relations.rs`).

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { ApiError } from "./generated/ApiError";
import type { PropertyConfig } from "./generated/PropertyConfig";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import type { RollupFunction } from "./generated/RollupFunction";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import {
  MAX_RELATED_PAGES,
  isComputedPropertyKind,
  normalisePropertyValue,
  propertyValueProblem,
  propertyValueText,
  propertyValuesEqual,
} from "./properties";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function must<T>(value: T | undefined): T {
  if (value === undefined) {
    throw new Error("a value the test needs is missing");
  }
  return value;
}

async function expectApiError(promise: Promise<unknown>, expected: ApiError): Promise<void> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  expect((error as CoreError).toApiError()).toEqual(expected);
}

const invalidInput = (field: string, message: string): ApiError => ({
  code: "InvalidInput",
  message,
  details: { field },
});

const relation = (...pages: string[]): PropertyValue => ({ kind: "relation", value: pages });
const number = (value: number): PropertyValue => ({ kind: "number", value });

const rollupConfig = (relationPropertyId: string, target: string, fn: RollupFunction): PropertyConfig => ({
  kind: "rollup",
  relationPropertyId,
  target,
  function: fn,
});

async function setup(titles = ["Alpha", "Beta", "Gamma", "Delta"]) {
  let counter = 0x1000;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => {
      clock.now += 1_000;
      return clock.now;
    },
    workspaceName: "Home",
    spaces: [{ name: "Personal", pages: titles.map((title) => ({ title })) }],
  });
  const client = new CoreClient(transport);
  const space = must((await client.spaceList())[0]);
  const roots = (await client.treeGet(space.id)).roots;
  const page = (title: string): string => must(roots.find((node) => node.page.title === title)).page.id;
  const pages = titles.map(page);
  const pair = async (name = "Related"): Promise<{ forward: PropertyDef; back: PropertyDef }> => {
    const forward = await client.propertyCreate(space.id, name, "relation", { kind: "relation", twoWay: true });
    const pairedId = (forward.config as { pairedPropertyId?: string } | undefined)?.pairedPropertyId;
    const back = must((await client.propertySchemaGet(space.id)).find((def) => def.id === pairedId));
    return { forward, back };
  };
  const related = async (pageId: string, propertyId: string): Promise<string[]> => {
    const found = (await client.pagePropertiesGet(pageId)).find(([property]) => property === propertyId);
    return found?.[1].kind === "relation" ? found[1].value : [];
  };
  const set = (pageId: string, propertyId: string, value: PropertyValue | null) =>
    client.pagePropertySet(pageId, propertyId, value);
  return { transport, client, spaceId: space.id, pages, pair, related, set, clock };
}

describe("the relation value", () => {
  it("follows the rules of the core, with its messages", () => {
    const problem = (value: PropertyValue) => propertyValueProblem(value);
    expect(problem(relation(id(1), id(2)))).toBeUndefined();
    expect(problem(relation())).toBeUndefined();
    expect(problem(relation(id(1), id(2), id(1)))).toBe("page ids must be distinct");
    expect(problem(relation(id(1), id(1).toUpperCase()))).toBe("page ids must be distinct");
    const many = Array.from({ length: MAX_RELATED_PAGES + 1 }, (_, at) => id(at + 1));
    expect(problem(relation(...many))).toBe("must list at most 100 pages");
    expect(problem(relation(...many.slice(0, MAX_RELATED_PAGES)))).toBeUndefined();
    expect(problem({ kind: "relation", value: ["not an id"] })).toBe("the value is malformed");
    expect(problem({ kind: "rollup", value: { function: "count_all", result: { type: "number", data: 1 } } })).toBe(
      "is computed from the page and cannot be set",
    );
    expect(normalisePropertyValue(relation())).toBeNull();
    expect(normalisePropertyValue(relation(id(1).toUpperCase()))).toEqual(relation(id(1)));
    expect(isComputedPropertyKind("rollup")).toBe(true);
    expect(isComputedPropertyKind("relation")).toBe(false);
    expect(propertyValuesEqual(relation(id(1), id(2)), relation(id(1), id(2)))).toBe(true);
    expect(propertyValuesEqual(relation(id(1), id(2)), relation(id(2), id(1)))).toBe(false);
  });

  it("searches as the titles of its pages", () => {
    const title = (page: string): string | undefined => ({ [id(1)]: "Roadmap", [id(2)]: "  ", [id(3)]: "Budget" })[page];
    const none = () => undefined;
    expect(propertyValueText(relation(id(3), id(9), id(2), id(1)), none, none, title)).toBe("Budget, Roadmap");
    expect(propertyValueText(relation(id(9)), none, none, title)).toBeUndefined();
  });
});

describe("a one-way relation", () => {
  it("stores an ordered list, reads it back, keeps unknown pages unread and clears with an empty list", async () => {
    const { client, pages, related, set, spaceId } = await setup();
    const [alpha, beta, gamma, delta] = pages.map((page) => page);
    const links = await client.propertyCreate(spaceId, "Links", "relation");
    expect(links.config).toEqual({ kind: "relation", twoWay: false });
    await set(must(alpha), links.id, relation(must(gamma), must(beta)));
    expect(await related(must(alpha), links.id)).toEqual([gamma, beta]);
    expect(await related(must(gamma), links.id)).toEqual([]);
    await set(must(alpha), links.id, relation(must(beta), must(gamma), must(delta)));
    expect(await related(must(alpha), links.id)).toEqual([beta, gamma, delta]);
    // A page the space does not have is kept but reads as nothing.
    await set(must(alpha), links.id, relation(must(beta), id(0x999)));
    expect(await related(must(alpha), links.id)).toEqual([beta]);
    await set(must(alpha), links.id, relation(id(0x999)));
    expect(await related(must(alpha), links.id)).toEqual([]);
    await set(must(alpha), links.id, relation());
    expect((await client.pageGet(must(alpha))).properties ?? []).toEqual([]);
    await expectApiError(set(must(alpha), links.id, relation(must(beta), must(beta))), invalidInput("value", "page ids must be distinct"));
    await expectApiError(
      set(must(alpha), links.id, { kind: "text", value: "x" }),
      invalidInput("value", "the value does not match the kind of the property"),
    );
  });

  it("lists a trashed page and drops a purged one", async () => {
    const { client, pages, related, set, spaceId } = await setup();
    const [alpha, beta, gamma] = pages.map((page) => must(page));
    const links = await client.propertyCreate(spaceId, "Links", "relation");
    await set(alpha as string, links.id, relation(beta as string, gamma as string));
    await client.pageTrash(beta as string);
    expect(await related(alpha as string, links.id)).toEqual([beta, gamma]);
    await client.pagePurge(beta as string);
    expect(await related(alpha as string, links.id)).toEqual([gamma]);
  });

  it("finds the pages through HasValue and RelatesTo and refuses the other filters", async () => {
    const { client, pages, set, spaceId } = await setup();
    const [alpha, beta] = pages.map((page) => must(page));
    const links = await client.propertyCreate(spaceId, "Links", "relation");
    await set(alpha as string, links.id, relation(beta as string));
    expect(await client.pagesByProperty(spaceId, links.id, { type: "RelatesTo", data: beta as string })).toEqual([alpha]);
    expect(await client.pagesByProperty(spaceId, links.id, { type: "RelatesTo", data: alpha as string })).toEqual([]);
    expect(await client.pagesByProperty(spaceId, links.id, { type: "HasValue" })).toEqual([alpha]);
    await expectApiError(
      client.pagesByProperty(spaceId, links.id, { type: "Equals", data: relation(beta as string) }),
      invalidInput("filter", "a list of pages cannot be compared; use HasValue or RelatesTo"),
    );
    await expectApiError(
      client.pagesByProperty(spaceId, links.id, { type: "Checked", data: true }),
      invalidInput("filter", "a checked state can only be searched in a checkbox property"),
    );
    const notes = await client.propertyCreate(spaceId, "Notes", "text");
    await expectApiError(
      client.pagesByProperty(spaceId, notes.id, { type: "RelatesTo", data: alpha as string }),
      invalidInput("filter", "related pages can only be searched in a relation property"),
    );
  });

  it("is found by the titles of its pages, also after a rename", async () => {
    const { client, pages, set, spaceId } = await setup(["Alpha", "Quarterly roadmap"]);
    const [alpha, roadmap] = pages.map((page) => must(page));
    const links = await client.propertyCreate(spaceId, "Links", "relation");
    await set(alpha as string, links.id, relation(roadmap as string));
    expect((await client.search("roadmap")).some((hit) => hit.pageId === alpha)).toBe(true);
    await client.pageRename(roadmap as string, "Yearly horizon");
    expect((await client.search("roadmap")).some((hit) => hit.pageId === alpha)).toBe(false);
    expect((await client.search("horizon")).some((hit) => hit.pageId === alpha)).toBe(true);
  });
});

describe("a two-way relation", () => {
  it("makes two properties that name each other, in one change", async () => {
    const { client, spaceId, pair, transport } = await setup();
    const seen: string[] = [];
    transport.subscribe((event) => seen.push(event.type));
    const { forward, back } = await pair("Project");
    expect(forward.name).toBe("Project");
    expect(back.name).toBe("Project (back)");
    expect(forward.config).toEqual({ kind: "relation", twoWay: true, pairedPropertyId: back.id });
    expect(back.config).toEqual({ kind: "relation", twoWay: true, pairedPropertyId: forward.id });
    expect(seen.filter((type) => type === "PropertySchemaChanged")).toHaveLength(1);
    const long = await pair("x".repeat(100));
    expect(Array.from(long.back.name)).toHaveLength(100);
    expect(long.back.name.endsWith(" (back)")).toBe(true);
    await expectApiError(
      client.propertyCreate(spaceId, "x", "text", { kind: "relation", twoWay: false }),
      invalidInput("config", "only a relation, a rollup or a formula takes a configuration"),
    );
    await expectApiError(
      client.propertyCreate(spaceId, "x", "rollup"),
      invalidInput("config", "a rollup needs a configuration"),
    );
    await expectApiError(
      client.propertyCreate(spaceId, "x", "relation", { kind: "relation", twoWay: true, pairedPropertyId: forward.id }),
      invalidInput("config", "a new two-way relation makes its own paired property"),
    );
    await expectApiError(
      client.propertyConfigSet(spaceId, forward.id, { kind: "relation", twoWay: false }),
      invalidInput("config", "the configuration of a relation cannot be changed"),
    );
  });

  it("needs room for both properties", async () => {
    const { client, spaceId } = await setup();
    for (let at = 1; at <= 98; at += 1) {
      await client.propertyCreate(spaceId, `P${String(at)}`, "text");
    }
    await expectApiError(
      client.propertyCreate(spaceId, "A", "relation", { kind: "relation", twoWay: true }),
      invalidInput("spaceId", "a space holds at most 100 properties"),
    );
    expect(await client.propertySchemaGet(spaceId)).toHaveLength(99);
  });

  it("writes the other side in the same change and reads symmetrically", async () => {
    const { client, pages, pair, related, set, spaceId } = await setup();
    const [alpha, beta, gamma, delta] = pages.map((page) => must(page));
    const { forward, back } = await pair();
    await set(alpha as string, forward.id, relation(beta as string, gamma as string));
    for (const other of [beta, gamma]) {
      expect(await related(other as string, back.id)).toEqual([alpha]);
      expect((await client.pageGet(other as string)).properties).toContainEqual([back.id, relation(alpha as string)]);
    }
    expect(await client.pagesByProperty(spaceId, forward.id, { type: "RelatesTo", data: beta as string })).toEqual([alpha]);
    expect(await client.pagesByProperty(spaceId, back.id, { type: "RelatesTo", data: alpha as string })).toEqual([beta, gamma]);
    expect(await client.pagesByProperty(spaceId, back.id, { type: "HasValue" })).toEqual([beta, gamma]);

    // The back side writes the forward side of the other page.
    await set(delta as string, back.id, relation(alpha as string));
    expect(await related(alpha as string, forward.id)).toEqual([beta, gamma, delta]);
    // Removing is symmetric.
    await set(alpha as string, forward.id, relation(gamma as string, delta as string));
    expect(await related(beta as string, back.id)).toEqual([]);
    expect(await related(gamma as string, back.id)).toEqual([alpha]);
    await set(alpha as string, forward.id, null);
    expect(await related(gamma as string, back.id)).toEqual([]);
    expect(await related(delta as string, back.id)).toEqual([]);
  });

  it("lets a page relate to itself", async () => {
    const { pages, pair, related, set } = await setup();
    const [alpha, beta] = pages.map((page) => must(page));
    const { forward, back } = await pair();
    await set(alpha as string, forward.id, relation(alpha as string, beta as string));
    expect(await related(alpha as string, forward.id)).toEqual([alpha, beta]);
    expect(await related(alpha as string, back.id)).toEqual([alpha]);
    await set(alpha as string, forward.id, relation(beta as string));
    expect(await related(alpha as string, back.id)).toEqual([]);
  });

  it("turns the other property into a one-way relation that keeps its links when one is deleted", async () => {
    const { client, pages, pair, related, set, spaceId } = await setup();
    const [alpha, beta] = pages.map((page) => must(page));
    const { forward, back } = await pair();
    await set(alpha as string, forward.id, relation(beta as string));
    await client.propertyDelete(spaceId, back.id);
    const remaining = must((await client.propertySchemaGet(spaceId)).find((def) => def.id === forward.id));
    expect(remaining.config).toEqual({ kind: "relation", twoWay: false });
    expect(await related(alpha as string, forward.id)).toEqual([beta]);
    expect(await client.pagesByProperty(spaceId, forward.id, { type: "RelatesTo", data: beta as string })).toEqual([alpha]);

    const again = await setup();
    const second = await again.pair();
    await again.set(must(again.pages[0]), second.forward.id, relation(must(again.pages[1])));
    await again.client.propertyDelete(again.spaceId, second.forward.id);
    expect(await again.related(must(again.pages[1]), second.back.id)).toEqual([must(again.pages[0])]);
    expect(
      must((await again.client.propertySchemaGet(again.spaceId)).find((def) => def.id === second.back.id)).config,
    ).toEqual({ kind: "relation", twoWay: false });
  });

  it("is carried by a backup: the pairing, the links and a rollup", async () => {
    const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
    let counter = 0;
    const make = (files?: Map<string, Uint8Array>, prefix = "00000000") => {
      const transport = new MemoryTransport({
        newId: () => `${prefix}-0000-4000-8000-${String(++counter).padStart(12, "0")}`,
        now: () => T0,
        // Another installation starts empty: the backup brings the workspace.
        ...(files === undefined
          ? {
              workspaceName: "Research",
              spaces: [{ name: "Notes", pages: [{ title: "Alpha" }, { title: "Beta" }] }],
              security: { kitCreatedAt: T0 - 10_000, kitVerifiedAt: T0 - 9_000, kitKey: KEY },
            }
          : { files }),
      });
      return { transport, client: new CoreClient(transport) };
    };
    const origin = make();
    const space = must((await origin.client.spaceList())[0]);
    const roots = (await origin.client.treeGet(space.id)).roots;
    const alpha = must(roots[0]).page.id;
    const beta = must(roots[1]).page.id;
    const forward = await origin.client.propertyCreate(space.id, "Related", "relation", {
      kind: "relation",
      twoWay: true,
    });
    const pairedId = (forward.config as { pairedPropertyId?: string } | undefined)?.pairedPropertyId ?? "";
    const count = await origin.client.propertyCreate(
      space.id,
      "Count",
      "rollup",
      rollupConfig(forward.id, "title", "count_all"),
    );
    await origin.client.pagePropertySet(alpha, forward.id, relation(beta));
    await origin.client.backupExport("C:/backups/research.atomikkb");

    const words = (await kitWordsFromEntropy(KEY)).join(" ");
    const restored = make(origin.transport.files, "11111111");
    await restored.client.backupImport(undefined, words);
    const schema = await restored.client.propertySchemaGet(space.id);
    expect(must(schema.find((def) => def.id === forward.id)).config).toEqual({
      kind: "relation",
      twoWay: true,
      pairedPropertyId: pairedId,
    });
    expect(must(schema.find((def) => def.id === count.id)).config).toEqual(
      rollupConfig(forward.id, "title", "count_all"),
    );
    expect(await restored.client.pagePropertiesGet(beta)).toContainEqual([pairedId, relation(alpha)]);
    expect(await restored.client.pagePropertiesGet(alpha)).toContainEqual([
      count.id,
      { kind: "rollup", value: { function: "count_all", result: { type: "number", data: 1 } } },
    ]);
  });
});

describe("a rollup", () => {
  async function board() {
    const context = await setup(["Hub", "One", "Two", "Three", "Alone"]);
    const { client, spaceId, set, pages } = context;
    const [hub, one, two, three] = pages.map((page) => must(page));
    const links = await client.propertyCreate(spaceId, "Links", "relation");
    const cost = await client.propertyCreate(spaceId, "Cost", "number");
    const due = await client.propertyCreate(spaceId, "Due", "date");
    const done = await client.propertyCreate(spaceId, "Done", "checkbox");
    await set(hub as string, links.id, relation(one as string, two as string, three as string));
    await set(one as string, cost.id, number(10));
    await set(two as string, cost.id, number(5));
    await set(one as string, due.id, { kind: "date", value: { date: "2026-10-09" } });
    await set(two as string, due.id, { kind: "date", value: { date: "2026-10-07", time: "09:00" } });
    await set(one as string, done.id, { kind: "checkbox", value: true });
    await set(two as string, done.id, { kind: "checkbox", value: false });
    return { ...context, hub: hub as string, one: one as string, two: two as string, three: three as string, links, cost, due, done };
  }

  const rolled = async (client: CoreClient, pageId: string, property: string) =>
    (await client.pagePropertiesGet(pageId)).find(([id]) => id === property)?.[1];

  it("summarises a property of the related pages and is part of PageInfo", async () => {
    const { client, spaceId, hub, links, cost, due, done, pages } = await board();
    const create = (name: string, target: string, fn: RollupFunction) =>
      client.propertyCreate(spaceId, name, "rollup", rollupConfig(links.id, target, fn));
    const sum = await create("Sum", cost.id, "sum");
    const average = await create("Average", cost.id, "average");
    const earliest = await create("Earliest", due.id, "earliest");
    const percent = await create("Percent", done.id, "percent_checked");
    const count = await create("Count", "title", "count_all");
    const titles = await create("Titles", "title", "show_original");
    expect(await rolled(client, hub, sum.id)).toEqual({ kind: "rollup", value: { function: "sum", result: { type: "number", data: 15 } } });
    expect(await rolled(client, hub, average.id)).toEqual({
      kind: "rollup",
      value: { function: "average", result: { type: "number", data: 7.5 } },
    });
    expect(await rolled(client, hub, earliest.id)).toEqual({
      kind: "rollup",
      value: { function: "earliest", result: { type: "date", data: { date: "2026-10-07", time: "09:00" } } },
    });
    expect(await rolled(client, hub, percent.id)).toEqual({
      kind: "rollup",
      value: { function: "percent_checked", result: { type: "number", data: 100 / 3 } },
    });
    expect(await rolled(client, hub, count.id)).toEqual({
      kind: "rollup",
      value: { function: "count_all", result: { type: "number", data: 3 } },
    });
    expect(await rolled(client, hub, titles.id)).toEqual({
      kind: "rollup",
      value: {
        function: "show_original",
        result: { type: "values", data: ["One", "Two", "Three"].map((value) => ({ kind: "text", value })) },
      },
    });
    // A page without relations: counts are zero, an average has no result.
    const alone = must(pages[4]);
    expect(await rolled(client, alone, count.id)).toEqual({
      kind: "rollup",
      value: { function: "count_all", result: { type: "number", data: 0 } },
    });
    expect(await rolled(client, alone, average.id)).toBeUndefined();
    // It is in PageInfo.properties like the others.
    expect((await client.pageGet(hub)).properties).toContainEqual([sum.id, await rolled(client, hub, sum.id)]);
  });

  it("follows the related pages' values and ignores the trashed", async () => {
    const { client, spaceId, hub, three, links, cost, set } = await board();
    const sum = await client.propertyCreate(spaceId, "Sum", "rollup", rollupConfig(links.id, cost.id, "sum"));
    await set(three, cost.id, number(100));
    expect(await rolled(client, hub, sum.id)).toMatchObject({ value: { result: { data: 115 } } });
    await client.pageTrash(three);
    expect(await rolled(client, hub, sum.id)).toMatchObject({ value: { result: { data: 15 } } });
    await client.pageRestore(three);
    expect(await rolled(client, hub, sum.id)).toMatchObject({ value: { result: { data: 115 } } });
    await set(hub, links.id, relation(three));
    expect(await rolled(client, hub, sum.id)).toMatchObject({ value: { result: { data: 100 } } });
  });

  it("rolls up through the back side of a two-way relation", async () => {
    const { client, pages, pair, set, spaceId } = await setup(["Project", "A", "B"]);
    const [project, a, b] = pages.map((page) => must(page));
    const { forward, back } = await pair();
    const cost = await client.propertyCreate(spaceId, "Cost", "number");
    const sum = await client.propertyCreate(spaceId, "Sum", "rollup", rollupConfig(back.id, cost.id, "sum"));
    await set(a as string, cost.id, number(4));
    await set(b as string, cost.id, number(6));
    await set(a as string, forward.id, relation(project as string));
    await set(b as string, forward.id, relation(project as string));
    expect(await rolled(client, project as string, sum.id)).toMatchObject({ value: { result: { data: 10 } } });
  });

  it("is read-only and filtered through the computed value", async () => {
    const { client, spaceId, hub, one, two, three, links, cost } = await board();
    const sum = await client.propertyCreate(spaceId, "Sum", "rollup", rollupConfig(links.id, cost.id, "sum"));
    await expectApiError(
      client.pagePropertySet(hub, sum.id, number(1)),
      invalidInput("propertyId", "this property is computed from the page and cannot be set"),
    );
    await expectApiError(
      client.pagePropertySet(hub, sum.id, null),
      invalidInput("propertyId", "this property is computed from the page and cannot be set"),
    );
    // A sum of nothing is 0: every live page has a value.
    expect(await client.pagesByProperty(spaceId, sum.id, { type: "HasValue" })).toHaveLength(5);
    expect(
      await client.pagesByProperty(spaceId, sum.id, {
        type: "Equals",
        data: { kind: "rollup", value: { function: "sum", result: { type: "number", data: 15 } } },
      }),
    ).toEqual([hub]);
    await expectApiError(
      client.pagesByProperty(spaceId, sum.id, { type: "RelatesTo", data: hub }),
      invalidInput("filter", "related pages can only be searched in a relation property"),
    );
    void [one, two, three];
  });

  it("checks its configuration when it is written and can change it", async () => {
    const { client, spaceId, hub, links, cost, due } = await board();
    const refuse = (relationId: string, target: string, fn: RollupFunction, message: string) =>
      expectApiError(
        client.propertyCreate(spaceId, "Bad", "rollup", rollupConfig(relationId, target, fn)),
        invalidInput("config", message),
      );
    await refuse(cost.id, cost.id, "sum", "the rollup needs a relation property");
    await refuse(id(0x777), cost.id, "sum", "the rollup needs a relation property");
    await refuse(links.id, id(0x777), "count_all", "the rollup target does not exist");
    const files = await client.propertyCreate(spaceId, "Files", "files");
    await refuse(links.id, files.id, "count_all", "this kind of property cannot be rolled up");
    await refuse(links.id, due.id, "sum", "this function does not apply to the target");
    await refuse(links.id, "title", "average", "this function does not apply to the target");
    await refuse(links.id, cost.id, "earliest", "this function does not apply to the target");
    await refuse(links.id, cost.id, "percent_checked", "this function does not apply to the target");

    const sum = await client.propertyCreate(spaceId, "Sum", "rollup", rollupConfig(links.id, cost.id, "sum"));
    const changed = await client.propertyConfigSet(spaceId, sum.id, rollupConfig(links.id, cost.id, "max"));
    expect(changed.config).toEqual(rollupConfig(links.id, cost.id, "max"));
    expect(await rolled(client, hub, sum.id)).toMatchObject({ value: { function: "max", result: { data: 10 } } });
    await expectApiError(
      client.propertyConfigSet(spaceId, sum.id, rollupConfig(links.id, due.id, "max")),
      invalidInput("config", "this function does not apply to the target"),
    );
    await expectApiError(
      client.propertyConfigSet(spaceId, cost.id, rollupConfig(links.id, "title", "count_all")),
      invalidInput("config", "only a relation, a rollup or a formula takes a configuration"),
    );
  });

  it("reads as nothing once its target or relation is deleted", async () => {
    const { client, spaceId, hub, links, cost } = await board();
    const sum = await client.propertyCreate(spaceId, "Sum", "rollup", rollupConfig(links.id, cost.id, "sum"));
    expect(await rolled(client, hub, sum.id)).toBeDefined();
    await client.propertyDelete(spaceId, cost.id);
    expect(await rolled(client, hub, sum.id)).toBeUndefined();
    const count = await client.propertyCreate(spaceId, "Count", "rollup", rollupConfig(links.id, "title", "count_all"));
    expect(await rolled(client, hub, count.id)).toBeDefined();
    await client.propertyDelete(spaceId, links.id);
    expect(await rolled(client, hub, count.id)).toBeUndefined();
  });
});

describe("seeding", () => {
  it("creates relations by page title, a pair on request, and rollups by property name", async () => {
    let counter = 0x2000;
    const transport = new MemoryTransport({
      newId: () => id(++counter),
      now: () => T0,
      workspaceName: "Home",
      spaces: [
        {
          name: "Personal",
          properties: [
            { name: "Cost", kind: "number" },
            { name: "Tasks", kind: "relation", twoWay: true, backName: "Project" },
            { name: "Total", kind: "rollup", rollup: { relation: "Tasks", target: "Cost", function: "sum" } },
          ],
          pages: [
            { title: "Launch", properties: { Tasks: ["Write", "Ship"] } },
            { title: "Write", properties: { Cost: 3 } },
            { title: "Ship", properties: { Cost: 4 } },
          ],
        },
      ],
    });
    const client = new CoreClient(transport);
    const space = must((await client.spaceList())[0]);
    const schema = await client.propertySchemaGet(space.id);
    expect(schema.map((def) => def.name)).toEqual(["Tags", "Cost", "Tasks", "Project", "Total"]);
    const tasks = must(schema.find((def) => def.name === "Tasks"));
    const project = must(schema.find((def) => def.name === "Project"));
    const total = must(schema.find((def) => def.name === "Total"));
    expect(tasks.config).toEqual({ kind: "relation", twoWay: true, pairedPropertyId: project.id });
    const roots = (await client.treeGet(space.id)).roots;
    const page = (title: string): string => must(roots.find((node) => node.page.title === title)).page.id;
    expect(await client.pagePropertiesGet(page("Write"))).toContainEqual([project.id, relation(page("Launch"))]);
    expect(await client.pagePropertiesGet(page("Launch"))).toContainEqual([
      total.id,
      { kind: "rollup", value: { function: "sum", result: { type: "number", data: 7 } } },
    ]);
  });
});
