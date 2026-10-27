// `PageBulk` in `MemoryTransport` (ADR-0058): the twin of the core's `bulk.rs`. The limits and the checks that write nothing,
// `SetValue` and `SetValues` (one event, all or nothing, not in structural undo), `Move`, `Duplicate` and `Trash` (the pages
// whose ancestor is listed go with it, the order, the answers, ONE undo step each), the events and the role gate.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { BulkAction } from "./generated/BulkAction";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { PropertyValue } from "./generated/PropertyValue";
import type { TreeView } from "./generated/TreeView";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { MAX_BULK_PAGES } from "./memory-bulk";
import type { SeedPage, SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

const text = (value: string): JsonValue => ({ type: "text", text: value });
const doc = (n: number, value: string): JsonValue => ({
  type: "doc",
  content: [{ type: "paragraph", attrs: { id: blockId(n) }, content: [text(value)] }],
});

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    properties: [
      { name: "Amount", kind: "number" },
      { name: "Stage", kind: "select", options: [{ name: "Todo" }, { name: "Done" }] },
      { name: "Blocks", kind: "relation", twoWay: true, backName: "Blocked by" },
      { name: "Owner", kind: "person" },
      { name: "Created", kind: "created_time" },
    ],
    pages: [
      {
        title: "A",
        properties: { Amount: 1 },
        content: doc(1, "hello from a"),
        children: [{ title: "A1", properties: { Amount: 2 }, content: doc(2, "hello from a1") }],
      },
      { title: "B" },
      { title: "C" },
      { title: "D" },
      { title: "P", children: [{ title: "X", children: [{ title: "X1" }] }] },
    ],
  },
  { name: "Other", pages: [{ title: "Foreign" }] },
  { name: "Work", pages: [{ title: "Plan" }] },
];

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** The titles of a tree, indented by depth, one string. */
function outline(view: TreeView): string {
  const lines: string[] = [];
  const walk = (nodes: PageNode[], depth: number): void => {
    for (const node of nodes) {
      lines.push(`${"  ".repeat(depth)}${node.page.title}`);
      walk(node.children, depth + 1);
    }
  };
  walk(view.roots, 0);
  return lines.map((line) => `${line}\n`).join("");
}

/** The first live page of that title. */
function find(view: TreeView, title: string): PageNode | undefined {
  const walk = (nodes: PageNode[]): PageNode | undefined => {
    for (const node of nodes) {
      if (node.page.title === title) {
        return node;
      }
      const found = walk(node.children);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  };
  return walk(view.roots);
}

async function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => T0,
    workspaceName: "Workspace",
    spaces: SPACES,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const spaceId = (name: string): string => spaces.find((space) => space.name === name)?.id ?? "";
  const home = spaceId("Home");
  const schema = await client.propertySchemaGet(home);
  const property = (name: string): string => schema.find((def) => def.name === name)?.id ?? "";
  const tree = (space = home): Promise<TreeView> => client.treeGet(space);
  const page = async (title: string, space = home): Promise<string> => find(await tree(space), title)?.page.id ?? "";
  /** The ids of live pages by title, as a tuple of the same length. */
  const titled = async <T extends string[]>(...titles: [...T]): Promise<{ [K in keyof T]: string }> =>
    (await Promise.all(titles.map((title) => page(title)))) as { [K in keyof T]: string };
  const outlined = async (space = home): Promise<string> => outline(await tree(space));
  const valueOf = async (pageId: string, name: string): Promise<PropertyValue | undefined> =>
    (await client.pagePropertiesGet(pageId)).find(([propertyId]) => propertyId === property(name))?.[1];
  events.length = 0;
  return { transport, client, events, spaces, spaceId, home, property, tree, page, titled, outlined, valueOf };
}

const number = (value: number): PropertyValue => ({ kind: "number", value });

/** How many of `events` are of `type`. */
function count(events: readonly Event[], type: Event["type"]): number {
  return events.filter((event) => event.type === type).length;
}

describe("PageBulk", () => {
  it("refuses a bad list of pages and writes nothing", async () => {
    const { client, events, home, spaceId, page, titled, outlined } = await setup();
    const [a, b] = await titled("A", "B");
    const foreign = await page("Foreign", spaceId("Other"));
    const gone = await page("D");
    await client.pageTrash(gone);
    const before = await outlined();
    events.length = 0;

    const tooMany = Array.from({ length: MAX_BULK_PAGES + 1 }, (_, n) => id(10_000 + n));
    const cases: [string, string[]][] = [
      ["no page", []],
      ["a page twice", [a, b, a]],
      ["a trashed page", [a, gone]],
      ["a page of another space", [a, foreign]],
      ["an unknown page", [a, id(999)]],
      ["more than 1,000 pages", tooMany],
    ];
    const actions: BulkAction[] = [
      { type: "Trash" },
      { type: "Duplicate" },
      { type: "Move", data: {} },
      { type: "SetValue", data: { propertyId: id(1), value: null } },
    ];
    for (const action of actions) {
      for (const [why, pages] of cases) {
        const refused = await failure(client.pageBulk(home, pages, action));
        expect({ why, code: refused.code, details: refused.details }).toEqual({
          why,
          code: "InvalidInput",
          details: { field: "pageIds" },
        });
      }
    }
    expect(await outlined()).toBe(before);
    expect(events).toEqual([]);
    expect(await failure(client.pageBulk(id(777), [a], { type: "Trash" }))).toMatchObject({ code: "NotFound" });
  });

  describe("SetValue and SetValues", () => {
    it("SetValue writes every page in one change and one event, and None clears", async () => {
      const { client, events, home, property, titled, valueOf } = await setup();
      const [b, c, d] = await titled("B", "C", "D");
      const amount = property("Amount");
      events.length = 0;

      const answer = await client.pageBulk(home, [b, c], { type: "SetValue", data: { propertyId: amount, value: number(5) } });
      expect(answer).toEqual([b, c]);
      expect(events.filter((event) => event.type === "StructureChanged")).toEqual([{ type: "StructureChanged", data: { spaceId: home } }]);
      expect(await valueOf(b, "Amount")).toEqual(number(5));
      expect(await valueOf(c, "Amount")).toEqual(number(5));
      expect(await valueOf(d, "Amount")).toBeUndefined();
      expect([...(await client.pagesByProperty(home, amount, { type: "Equals", data: number(5) }))].sort()).toEqual([b, c].sort());

      await client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: amount, value: null } });
      expect(await valueOf(b, "Amount")).toBeUndefined();
      expect(await valueOf(c, "Amount")).toEqual(number(5));
    });

    it("SetValues gives each page its own value, and a refused value writes none", async () => {
      const { client, events, home, property, titled, valueOf } = await setup();
      const [b, c, d] = await titled("B", "C", "D");
      const amount = property("Amount");
      const stage = property("Stage");
      const todo = (await client.propertySchemaGet(home)).find((def) => def.id === stage)?.options[0]?.id ?? "";
      await client.pagePropertySet(c, amount, number(9));
      events.length = 0;

      const answer = await client.pageBulk(home, [b, c, d], {
        type: "SetValues",
        data: { propertyId: amount, values: [number(1), null, number(3)] },
      });
      expect(answer).toEqual([b, c, d]);
      expect(await valueOf(b, "Amount")).toEqual(number(1));
      expect(await valueOf(c, "Amount")).toBeUndefined();
      expect(await valueOf(d, "Amount")).toEqual(number(3));
      expect(count(events, "StructureChanged")).toBe(1);

      const wrongLength = await failure(
        client.pageBulk(home, [b, c, d], { type: "SetValues", data: { propertyId: amount, values: [number(7)] } }),
      );
      expect(wrongLength).toMatchObject({ code: "InvalidInput", details: { field: "values" } });

      // The second value names an option the property does not have: the first is not written either.
      const unknownOption = await failure(
        client.pageBulk(home, [b, c], {
          type: "SetValues",
          data: { propertyId: stage, values: [{ kind: "select", value: todo }, { kind: "select", value: id(404) }] },
        }),
      );
      expect(unknownOption).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
      expect(await valueOf(b, "Stage")).toBeUndefined();
      // Another kind, a computed property, an unknown property, a person who is no member.
      expect(
        await failure(client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: amount, value: { kind: "text", value: "x" } } })),
      ).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
      expect(
        await failure(client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: property("Created"), value: null } })),
      ).toMatchObject({ code: "InvalidInput", details: { field: "propertyId" } });
      expect(
        await failure(client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: id(808), value: null } })),
      ).toMatchObject({ code: "NotFound" });
      expect(
        await failure(
          client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: property("Owner"), value: { kind: "person", value: [id(555)] } } }),
        ),
      ).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
      expect(await valueOf(b, "Amount")).toEqual(number(1));
      expect(await valueOf(b, "Owner")).toBeUndefined();
    });

    it("keeps a two-way relation paired like one write at a time", async () => {
      const { client, events, home, property, titled, valueOf } = await setup();
      const [b, c, d, p] = await titled("B", "C", "D", "P");
      events.length = 0;
      await client.pageBulk(home, [b, c], {
        type: "SetValues",
        data: {
          propertyId: property("Blocks"),
          values: [
            { kind: "relation", value: [p] },
            { kind: "relation", value: [p, d] },
          ],
        },
      });
      expect(await valueOf(p, "Blocked by")).toEqual({ kind: "relation", value: [b, c] });
      expect(await valueOf(d, "Blocked by")).toEqual({ kind: "relation", value: [c] });
      expect(count(events, "StructureChanged")).toBe(1);
      await client.pageBulk(home, [b, c], { type: "SetValue", data: { propertyId: property("Blocks"), value: null } });
      expect(await valueOf(p, "Blocked by")).toBeUndefined();
      expect(await valueOf(d, "Blocked by")).toBeUndefined();
    });

    it("is not a structural undo step", async () => {
      const { client, home, property, page, valueOf } = await setup();
      const b = await page("B");
      await client.pageBulk(home, [b], { type: "SetValue", data: { propertyId: property("Amount"), value: number(2) } });
      await client.pageCreate({ spaceId: home, title: "New" });
      await client.structureUndo(home);
      // The undo took the creation of "New"; the value stays.
      expect(await page("New")).toBe("");
      expect(await valueOf(b, "Amount")).toEqual(number(2));
    });
  });

  describe("Move", () => {
    it("puts the pages last under the parent in the order given, as one undo step", async () => {
      const { client, events, home, titled, outlined } = await setup();
      const [a, b, c, d, p, x] = await titled("A", "B", "C", "D", "P", "X");
      await client.pageCreate({ spaceId: home, title: "Last" });
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\nLast\n");
      events.length = 0;

      expect(await client.pageBulk(home, [c, a], { type: "Move", data: { parentId: p } })).toEqual([c, a]);
      expect(await outlined()).toBe("B\nD\nP\n  X\n    X1\n  C\n  A\n    A1\nLast\n");
      expect(count(events, "StructureChanged")).toBe(1);
      expect((await client.pageGet(c)).parentId).toBe(p);

      // One undo puts both back; the next one is the step before (the creation of "Last").
      await client.structureUndo(home);
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\nLast\n");
      await client.structureUndo(home);
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\n");
      await client.structureRedo(home);
      await client.structureRedo(home);
      expect(await outlined()).toBe("B\nD\nP\n  X\n    X1\n  C\n  A\n    A1\nLast\n");

      // To the top level, last, in the order given; a page already there is moved to its place.
      expect(await client.pageBulk(home, [x, c], { type: "Move", data: {} })).toEqual([x, c]);
      expect(await outlined()).toBe("B\nD\nP\n  A\n    A1\nLast\nX\n  X1\nC\n");
      expect(await client.pageBulk(home, [b], { type: "Move", data: {} })).toEqual([b]);
      expect(await outlined()).toBe("D\nP\n  A\n    A1\nLast\nX\n  X1\nC\nB\n");
      expect((await client.pageGet(d)).parentId).toBeUndefined();
    });

    it("moves a page whose ancestor is listed with that ancestor, not alone", async () => {
      const { client, home, titled, outlined } = await setup();
      const [a, a1, b, d] = await titled("A", "A1", "B", "D");
      expect(await client.pageBulk(home, [a1, a, b], { type: "Move", data: { parentId: d } })).toEqual([a, b]);
      expect(await outlined()).toBe("C\nD\n  A\n    A1\n  B\nP\n  X\n    X1\n");
      await client.structureUndo(home);
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\n");
    });

    it("writes nothing under a parent that cannot take the pages", async () => {
      const { client, events, home, spaceId, page, titled, outlined } = await setup();
      const [b, p, x1] = await titled("B", "P", "X1");
      const foreign = await page("Foreign", spaceId("Other"));
      const gone = await page("C");
      await client.pageTrash(gone);
      const before = await outlined();
      events.length = 0;
      const cases: [string, string[], string][] = [
        ["one of the pages", [b, p], p],
        ["below one of the pages", [b, p], x1],
        ["a trashed page", [b], gone],
        ["a page of another space", [b], foreign],
        ["an unknown page", [b], id(909)],
      ];
      for (const [why, pages, parentId] of cases) {
        const refused = await failure(client.pageBulk(home, pages, { type: "Move", data: { parentId } }));
        expect({ why, code: refused.code, details: refused.details }).toEqual({ why, code: "InvalidInput", details: { field: "parentId" } });
      }
      expect(await outlined()).toBe(before);
      expect(events).toEqual([]);
    });
  });

  describe("Duplicate", () => {
    it("copies each page with its subtree right after it, as one undo step, and answers the top copies in order", async () => {
      const { client, events, home, titled, outlined, tree, valueOf } = await setup();
      const [a, a1, b] = await titled("A", "A1", "B");
      const live = (await client.spaceList()).find((space) => space.id === home)?.pageCount ?? 0;
      events.length = 0;

      // `a1` goes with `a`: copied once, inside the copy of `a`.
      const answer = await client.pageBulk(home, [b, a1, a], { type: "Duplicate" });
      expect(answer).toHaveLength(2);
      const [bCopy, aCopy] = answer as [string, string];
      expect((await client.pageGet(bCopy)).title).toBe("B (copy)");
      expect((await client.pageGet(aCopy)).title).toBe("A (copy)");
      expect(await outlined()).toBe("A\n  A1\nA (copy)\n  A1\nB\nB (copy)\nC\nD\nP\n  X\n    X1\n");
      expect((await client.spaceList()).find((space) => space.id === home)?.pageCount).toBe(live + 3);

      // The values and the content of the copies, the subtree included; the originals are untouched.
      expect(await valueOf(aCopy, "Amount")).toEqual(number(1));
      const a1Copy = find(await tree(), "A (copy)")?.children[0]?.page.id ?? "";
      expect(a1Copy).not.toBe(a1);
      expect(await valueOf(a1Copy, "Amount")).toEqual(number(2));
      expect(JSON.stringify((await client.pageContentGet(aCopy)).content)).toContain("hello from a");
      expect(JSON.stringify((await client.pageContentGet(a1Copy)).content)).toContain("hello from a1");
      expect(await valueOf(a, "Amount")).toEqual(number(1));
      expect(count(events, "StructureChanged")).toBe(1);
      expect(count(events, "SpacesChanged")).toBe(1);

      // One undo removes every copy, one redo brings them all back.
      await client.structureUndo(home);
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\n");
      await client.structureRedo(home);
      expect(await outlined()).toBe("A\n  A1\nA (copy)\n  A1\nB\nB (copy)\nC\nD\nP\n  X\n    X1\n");
    });

    it("copies at most 1,000 pages in all", async () => {
      const children: SeedPage[] = Array.from({ length: MAX_BULK_PAGES }, (_, n) => ({ title: `Child ${String(n)}` }));
      const { client, events, home, titled, outlined } = await setup({
        spaces: [{ name: "Home", pages: [{ title: "Root", children }, { title: "Small" }] }],
      });
      const [root, small] = await titled("Root", "Small");
      const child = find(await client.treeGet(home), "Child 5")?.page.id ?? "";
      const before = await outlined();
      events.length = 0;
      // 1,001 pages in the subtree of the root; its child goes with it and changes nothing.
      for (const pages of [[root], [child, root], [small, root]]) {
        const refused = await failure(client.pageBulk(home, pages, { type: "Duplicate" }));
        expect(refused).toMatchObject({ code: "InvalidInput", details: { field: "pageIds" } });
      }
      expect(await outlined()).toBe(before);
      expect(events).toEqual([]);
      expect(await client.pageBulk(home, [small], { type: "Duplicate" })).toHaveLength(1);
    });
  });

  describe("Trash", () => {
    it("moves each page with its subtree to the trash as one undo step", async () => {
      const { client, events, home, titled, outlined, tree } = await setup();
      const [a, a1, c] = await titled("A", "A1", "C");
      events.length = 0;

      expect(await client.pageBulk(home, [a1, c, a], { type: "Trash" })).toEqual([c, a]);
      expect(await outlined()).toBe("B\nD\nP\n  X\n    X1\n");
      const view = await tree();
      expect(view.trashed.map((entry) => entry.page.id).sort()).toEqual([a, c].sort());
      expect(view.trashed.find((entry) => entry.page.id === a)?.descendantCount).toBe(1);
      expect((await client.pageGet(a1)).trashedAt).toBeDefined();
      expect(count(events, "StructureChanged")).toBe(1);
      expect(count(events, "SpacesChanged")).toBe(1);

      await client.structureUndo(home);
      expect(await outlined()).toBe("A\n  A1\nB\nC\nD\nP\n  X\n    X1\n");
      expect((await tree()).trashed).toEqual([]);
      await client.structureRedo(home);
      expect(await outlined()).toBe("B\nD\nP\n  X\n    X1\n");

      // A page already in the trash is no live page.
      const [b] = await titled("B");
      const refused = await failure(client.pageBulk(home, [b, a], { type: "Trash" }));
      expect(refused).toMatchObject({ code: "InvalidInput", details: { field: "pageIds" } });
      expect(await outlined()).toBe("B\nD\nP\n  X\n    X1\n");
    });
  });

  it("needs the Editor role", async () => {
    const { client, spaceId, page } = await setup({ sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "viewer" }] } });
    const work = spaceId("Work");
    const plan = await page("Plan", work);
    const actions: BulkAction[] = [
      { type: "Trash" },
      { type: "Duplicate" },
      { type: "Move", data: {} },
      { type: "SetValue", data: { propertyId: id(1), value: null } },
      { type: "SetValues", data: { propertyId: id(1), values: [null] } },
    ];
    for (const action of actions) {
      expect(await failure(client.pageBulk(work, [plan], action))).toMatchObject({ code: "Forbidden" });
    }
    expect(find(await client.treeGet(work), "Plan")).toBeDefined();
  });
});
