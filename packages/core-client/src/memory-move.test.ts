// The move of a page and its subpages to another space in `MemoryTransport` (ADR-0038,
// `docs/architecture/space-move.md`, 6): the plan request and the move itself, their checks, the ids
// that stay, the values mapped by property name, relations, saved views, attachments, favourites,
// history, search, and the events.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { PageNode } from "./generated/PageNode";
import type { PropertyDef } from "./generated/PropertyDef";
import type { PropertyValue } from "./generated/PropertyValue";
import type { TreeView } from "./generated/TreeView";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;

function uuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Deterministic ids and clock. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0x1000;
  let clock = T0;
  const transport = new MemoryTransport({
    newId: () => uuid(++counter),
    now: () => clock++,
    workspaceName: "Workspace",
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** Live page ids by title across a tree. */
function idsByTitle(view: TreeView): (title: string) => string {
  const found = new Map<string, string>();
  const walk = (nodes: PageNode[]): void => {
    for (const node of nodes) {
      found.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk(view.roots);
  return (title) => {
    const id = found.get(title);
    if (id === undefined) {
      throw new Error(`no live page titled ${title}`);
    }
    return id;
  };
}

/** The titles of a tree, indented by depth. */
function outline(view: TreeView): string[] {
  const lines: string[] = [];
  const walk = (nodes: PageNode[], depth: number): void => {
    for (const node of nodes) {
      lines.push(`${"  ".repeat(depth)}${node.page.title}`);
      walk(node.children, depth + 1);
    }
  };
  walk(view.roots, 0);
  return lines;
}

const OPTION = {
  happy: uuid(0x201),
  sad: uuid(0x202),
  urgent: uuid(0x203),
  later: uuid(0x204),
  workHappy: uuid(0x205),
  workUrgent: uuid(0x206),
};
const PROP = {
  status: uuid(0x101),
  estimate: uuid(0x102),
  mood: uuid(0x103),
  notes: uuid(0x104),
  related: uuid(0x105),
  links: uuid(0x107),
  done: uuid(0x108),
  workEstimate: uuid(0x111),
  workStatus: uuid(0x112),
  workMood: uuid(0x113),
};

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    tagOptions: [
      { id: OPTION.urgent, name: "urgent", color: "red" },
      { id: OPTION.later, name: "later", color: "gray" },
    ],
    properties: [
      { id: PROP.status, name: "Status", kind: "status" },
      { id: PROP.estimate, name: "Estimate", kind: "number" },
      {
        id: PROP.mood,
        name: "Mood",
        kind: "select",
        options: [
          { id: OPTION.happy, name: "Happy", color: "green" },
          { id: OPTION.sad, name: "Sad", color: "red" },
        ],
      },
      { id: PROP.notes, name: "Notes", kind: "text" },
      { id: PROP.related, name: "Related", kind: "relation", twoWay: true, backName: "Related by" },
      { id: PROP.links, name: "Links", kind: "relation" },
      { id: PROP.done, name: "Done", kind: "checkbox" },
    ],
    pages: [
      {
        title: "Alpha",
        icon: "🅰",
        favorite: true,
        tags: ["urgent"],
        properties: {
          Status: "In progress",
          Estimate: 3,
          Mood: "Happy",
          Notes: "hello",
          Done: true,
          Related: ["Alpha child"],
          Links: ["Stay"],
        },
        children: [
          { title: "Alpha child", favorite: true, properties: { Status: "Done", Estimate: 5, Mood: "Sad" } },
          { title: "Alpha deep", children: [{ title: "Alpha deeper" }] },
        ],
      },
      { title: "Stay", properties: { Related: ["Alpha"], Links: ["Alpha"], Estimate: 8 } },
      { title: "Sibling" },
    ],
  },
  {
    name: "Work",
    tagOptions: [{ id: OPTION.workUrgent, name: "Urgent", color: "orange" }],
    properties: [
      { id: PROP.workEstimate, name: "estimate", kind: "number" },
      { id: PROP.workStatus, name: "Status", kind: "text" },
      { id: PROP.workMood, name: "Mood", kind: "select", options: [{ id: OPTION.workHappy, name: "happy", color: "blue" }] },
    ],
    pages: [{ title: "Root", children: [{ title: "Nested" }, { title: "Last" }] }, { title: "Other" }],
  },
];

async function fixture(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: SPACES, ...options });
  const [home, work] = await ctx.client.spaceList();
  if (home === undefined || work === undefined) {
    throw new Error("the spaces are missing");
  }
  const homeTree = idsByTitle(await ctx.client.treeGet(home.id));
  const workTree = idsByTitle(await ctx.client.treeGet(work.id));
  ctx.events.length = 0;
  const names = async (spaceId: string): Promise<Map<string, PropertyDef>> =>
    new Map((await ctx.client.propertySchemaGet(spaceId)).map((def) => [def.name, def]));
  /** A page's readable values by property name; option and page ids shown by name and title. */
  const valuesOf = async (pageId: string): Promise<Record<string, unknown>> => {
    const page = await ctx.client.pageGet(pageId);
    const schema = new Map((await ctx.client.propertySchemaGet(page.spaceId)).map((def) => [def.id, def]));
    const out: Record<string, unknown> = {};
    for (const [id, value] of await ctx.client.pagePropertiesGet(pageId)) {
      const def = schema.get(id);
      out[def?.name ?? id] = shown(value, def);
    }
    return out;
  };
  const titles = new Map<string, string>();
  for (const space of [home, work]) {
    const tree = await ctx.client.treeGet(space.id);
    const walk = (nodes: PageNode[]): void => {
      for (const node of nodes) {
        titles.set(node.page.id, node.page.title);
        walk(node.children);
      }
    };
    walk(tree.roots);
  }
  function shown(value: PropertyValue, def: PropertyDef | undefined): unknown {
    switch (value.kind) {
      case "select":
      case "status":
        return def?.options.find((option) => option.id === value.value)?.name;
      case "multi_select":
        return value.value.map((id) => def?.options.find((option) => option.id === id)?.name);
      case "relation":
        return value.value.map((id) => titles.get(id));
      default:
        return value.value;
    }
  }
  return { ...ctx, home, work, homeTree, workTree, names, valuesOf };
}

function eventTypes(events: readonly Event[]): string[] {
  return events.map((event) => event.type);
}

describe("PageMoveToSpace checks", () => {
  it("refuses what the core refuses, with the fields it names", async () => {
    const { client, home, work, homeTree, workTree } = await fixture();
    await client.pageTrash(homeTree("Sibling"));
    const trashed = homeTree("Sibling");
    const unknown = uuid(0xffff);

    expect((await failure(client.pageMoveToSpace(unknown, work.id, null, null))).toApiError()).toMatchObject({ code: "NotFound" });
    const inTrash = await failure(client.pageMoveToSpace(trashed, work.id, null, null));
    expect(inTrash.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    expect((await failure(client.pageMoveToSpace(homeTree("Alpha"), unknown, null, null))).toApiError()).toMatchObject({ code: "NotFound" });
    const same = await failure(client.pageMoveToSpace(homeTree("Alpha"), home.id, null, null));
    expect(same.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "spaceId" } });
    for (const parent of [homeTree("Stay"), unknown]) {
      const refused = await failure(client.pageMoveToSpace(homeTree("Alpha"), work.id, parent, null));
      expect(refused.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "parentId" } });
    }
    const index = await failure(client.pageMoveToSpace(homeTree("Alpha"), work.id, null, -1));
    expect(index.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "index" } });
    // The plan checks the same things.
    expect((await failure(client.pageMoveToSpacePlan(trashed, work.id, null))).toApiError()).toMatchObject({ details: { field: "pageId" } });
    expect((await failure(client.pageMoveToSpacePlan(homeTree("Alpha"), home.id, null))).toApiError()).toMatchObject({ details: { field: "spaceId" } });
    expect((await failure(client.pageMoveToSpacePlan(homeTree("Alpha"), work.id, homeTree("Stay")))).toApiError()).toMatchObject({
      details: { field: "parentId" },
    });
    // A trashed parent of the target is no parent.
    await client.pageTrash(workTree("Last"));
    const parent = await failure(client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Last"), null));
    expect(parent.toApiError()).toMatchObject({ details: { field: "parentId" } });
    // Nothing moved.
    expect(outline(await client.treeGet(home.id))).toContain("Alpha");
  });

  it("keeps PageMove's refusal of a parent in another space", async () => {
    const { client, homeTree, workTree } = await fixture();
    const refused = await failure(client.pageMove(homeTree("Alpha"), workTree("Root"), 0));
    expect(refused.toApiError()).toMatchObject({ code: "InvalidInput", details: { field: "parentId" } });
  });

  it("is refused while the core is locked", async () => {
    const { client, transport, work, homeTree } = await fixture();
    transport.setCoreState("locked");
    expect((await failure(client.pageMoveToSpacePlan(homeTree("Alpha"), work.id, null))).code).toBe("Locked");
    expect((await failure(client.pageMoveToSpace(homeTree("Alpha"), work.id, null, null))).code).toBe("Locked");
  });
});

describe("the plan", () => {
  it("counts what the move would do and writes nothing", async () => {
    const { client, events, home, work, homeTree, workTree } = await fixture();
    const homeBefore = outline(await client.treeGet(home.id));
    const schemaBefore = await client.propertySchemaGet(work.id);
    const plan = await client.pageMoveToSpacePlan(homeTree("Alpha"), work.id, workTree("Nested"));
    expect(plan).toEqual({
      pageIds: ["Alpha", "Alpha child", "Alpha deep", "Alpha deeper"].map(homeTree),
      attachmentsMoved: 0,
      attachmentsLeft: 0,
      propertiesCreated: 4,
      valuesDropped: 0,
      relationLinksRemoved: 4,
    });
    expect(events).toEqual([]);
    expect(outline(await client.treeGet(home.id))).toEqual(homeBefore);
    expect(await client.propertySchemaGet(work.id)).toEqual(schemaBefore);
    // The move answers the same counts.
    const moved = await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Nested"), null);
    expect(moved).toEqual(plan);
  });
});

describe("the move", () => {
  it("moves the subtree with the same ids, under the parent at the index, and keeps what the pages are", async () => {
    const { client, events, home, work, homeTree, workTree } = await fixture();
    const before = await client.pageGet(homeTree("Alpha"));
    const summary = await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Root"), 1);
    expect(summary.pageIds).toEqual(["Alpha", "Alpha child", "Alpha deep", "Alpha deeper"].map(homeTree));

    expect(outline(await client.treeGet(home.id))).toEqual(["Stay", "Sibling"]);
    expect(outline(await client.treeGet(work.id))).toEqual([
      "Root",
      "  Nested",
      "  Alpha",
      "    Alpha child",
      "    Alpha deep",
      "      Alpha deeper",
      "  Last",
      "Other",
    ]);
    const after = await client.pageGet(homeTree("Alpha"));
    expect(after).toMatchObject({
      id: before.id,
      spaceId: work.id,
      parentId: workTree("Root"),
      title: "Alpha",
      icon: "🅰",
      createdAt: before.createdAt,
      updatedAt: before.updatedAt,
    });
    expect((await client.pageGet(homeTree("Alpha child"))).parentId).toBe(homeTree("Alpha"));
    expect((await client.spaceList()).map((space) => [space.name, space.pageCount])).toEqual([
      ["Home", 2],
      ["Work", 8],
    ]);

    expect(events[0]).toEqual({
      type: "PagesMovedSpace",
      data: { fromSpaceId: home.id, toSpaceId: work.id, pageIds: summary.pageIds },
    });
    expect(eventTypes(events).slice(0, 4)).toEqual(["PagesMovedSpace", "StructureChanged", "StructureChanged", "SpacesChanged"]);
    expect(events.slice(1, 3)).toEqual([
      { type: "StructureChanged", data: { spaceId: home.id } },
      { type: "StructureChanged", data: { spaceId: work.id } },
    ]);
    expect(eventTypes(events)).toContain("PropertySchemaChanged");
    expect(events.filter((event) => event.type === "StructureChanged")).toHaveLength(2);
  });

  it("puts the page at the root, last by default, and clamps the index", async () => {
    const { client, work, homeTree } = await fixture();
    await client.pageMoveToSpace(homeTree("Stay"), work.id, null, null);
    await client.pageMoveToSpace(homeTree("Sibling"), work.id, null, 99);
    expect(outline(await client.treeGet(work.id))).toEqual(["Root", "  Nested", "  Last", "Other", "Stay", "Sibling"]);
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, null, 0);
    expect(outline(await client.treeGet(work.id)).slice(0, 4)).toEqual(["Alpha", "  Alpha child", "  Alpha deep", "    Alpha deeper"]);
  });

  it("leaves the trashed pages of the subtree behind and never moves a trashed page", async () => {
    const { client, home, work, homeTree } = await fixture();
    await client.pageTrash(homeTree("Alpha deep"));
    const summary = await client.pageMoveToSpace(homeTree("Alpha"), work.id, null, null);
    expect(summary.pageIds).toEqual([homeTree("Alpha"), homeTree("Alpha child")]);
    const tree = await client.treeGet(home.id);
    expect(tree.trashed.map((entry) => entry.page.title)).toEqual(["Alpha deep"]);
    expect(outline(await client.treeGet(work.id))).toContain("  Alpha child");
  });

  it("moves back and forth keeping the ids", async () => {
    const { client, home, work, homeTree } = await fixture();
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, null, null);
    await client.pageMoveToSpace(homeTree("Alpha"), home.id, null, 0);
    expect(outline(await client.treeGet(home.id))).toEqual(["Alpha", "  Alpha child", "  Alpha deep", "    Alpha deeper", "Stay", "Sibling"]);
    expect((await client.pageGet(homeTree("Alpha deeper"))).spaceId).toBe(home.id);
  });

  it("makes favourites follow, last in the target's list", async () => {
    const { client, home, work, homeTree, workTree } = await fixture();
    await client.pageFavoriteSet(workTree("Other"), true);
    await client.pageFavoriteSet(homeTree("Stay"), true);
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, null, null);
    // Alpha and Alpha child were favourites from the start (the seed): they leave Home's list and join Work's.
    const favorites = (view: TreeView): string[] => view.favorites ?? [];
    expect(favorites(await client.treeGet(home.id))).toEqual([homeTree("Stay")]);
    expect(favorites(await client.treeGet(work.id))).toEqual([workTree("Other"), homeTree("Alpha"), homeTree("Alpha child")]);
  });

  it("is not an undo step and never lets an undo bring the pages back or take them away", async () => {
    const { client, home, work, homeTree, workTree } = await fixture();
    await client.pageRename(workTree("Other"), "Other, renamed");
    await client.pageRename(homeTree("Sibling"), "Sibling, renamed");
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Root"), null);
    // Undoing the rename in the target keeps the moved pages.
    await client.structureUndo(work.id);
    expect(outline(await client.treeGet(work.id))).toEqual([
      "Root",
      "  Nested",
      "  Last",
      "  Alpha",
      "    Alpha child",
      "    Alpha deep",
      "      Alpha deeper",
      "Other",
    ]);
    // Undoing in the source never brings them back there.
    await client.structureUndo(home.id);
    expect(outline(await client.treeGet(home.id))).toEqual(["Stay", "Sibling"]);
    await client.structureRedo(home.id);
    await client.structureRedo(work.id);
    expect(outline(await client.treeGet(home.id))).toEqual(["Stay", "Sibling, renamed"]);
    expect((await client.treeGet(work.id)).roots.map((node) => node.page.title)).toEqual(["Root", "Other, renamed"]);
    expect((await client.pageGet(homeTree("Alpha"))).spaceId).toBe(work.id);
  });

  it("finds the page by search in the target only", async () => {
    const { client, home, work, homeTree } = await fixture();
    expect((await client.search("Alpha", { spaceId: home.id })).map((hit) => hit.title)).toContain("Alpha");
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, null, null);
    expect((await client.search("Alpha", { spaceId: home.id })).map((hit) => hit.title)).toEqual([]);
    expect((await client.search("Alpha", { spaceId: work.id })).map((hit) => hit.title).slice(0, 2)).toEqual(["Alpha", "Alpha child"]);
  });

  it("keeps the links between pages: backlinks stay, and the pages they name are told", async () => {
    // Ids in creation order: the device, the workspace, "Home", then its pages.
    const alpha = uuid(0x1004);
    const mention: JsonValue = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: uuid(0x601) },
          content: [{ type: "text", text: "see " }, { type: "pageRef", attrs: { id: uuid(0x602), pageId: alpha } }],
        },
      ],
    };
    const { client, events } = setup({
      spaces: [{ name: "Home", pages: [{ title: "Alpha" }, { title: "Linker", content: mention }] }, { name: "Work" }],
    });
    const [home, work] = await client.spaceList();
    const linker = (await client.treeGet(home?.id ?? "")).roots[1]?.page.id ?? "";
    expect((await client.backlinksGet(alpha)).map((link) => [link.title, link.spaceId])).toEqual([["Linker", home?.id]]);
    events.length = 0;
    // The page that links moves: the page it names is told (its backlink now comes from another space).
    await client.pageMoveToSpace(linker, work?.id ?? "", null, null);
    expect(events).toContainEqual({ type: "BacklinksChanged", data: { pageId: alpha } });
    expect((await client.backlinksGet(alpha)).map((link) => [link.title, link.spaceId])).toEqual([["Linker", work?.id]]);
    // The page that is linked moves: its backlinks stay.
    await client.pageMoveToSpace(alpha, work?.id ?? "", null, null);
    expect((await client.backlinksGet(alpha)).map((link) => link.title)).toEqual(["Linker"]);
  });
});

describe("property values", () => {
  it("follow by name: reused when the kind matches, coerced through the text otherwise, created when missing", async () => {
    const { client, work, homeTree, workTree, names, valuesOf } = await fixture();
    const summary = await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Nested"), null);
    expect(summary).toMatchObject({ propertiesCreated: 4, valuesDropped: 0 });

    // Same name and kind (any case): the target's property is reused.
    const schema = await names(work.id);
    expect(schema.get("estimate")?.id).toBe(PROP.workEstimate);
    expect(schema.has("Estimate")).toBe(false);
    // Missing: created with the same kind, in the order of the source's schema.
    expect([...schema.keys()]).toEqual(["Tags", "estimate", "Status", "Mood", "Notes", "Related", "Related by", "Done"]);
    expect(schema.get("Notes")?.kind).toBe("text");
    expect(schema.get("Done")?.kind).toBe("checkbox");
    expect(schema.get("Related")?.config).toMatchObject({ kind: "relation", twoWay: true });

    expect(await valuesOf(homeTree("Alpha"))).toEqual({
      Tags: ["Urgent"],
      estimate: 3,
      // A status into a text property: its text.
      Status: "In progress",
      // A select into a select by option name (case-folded), the missing option added.
      Mood: "happy",
      Notes: "hello",
      Done: true,
      Related: ["Alpha child"],
    });
    expect(await valuesOf(homeTree("Alpha child"))).toEqual({
      estimate: 5,
      Status: "Done",
      Mood: "Sad",
      "Related by": ["Alpha"],
    });
    // The select gained the option it lacked, coloured as in the source.
    expect(schema.get("Mood")?.options.map((option) => [option.name, option.color])).toEqual([
      ["happy", "blue"],
      ["Sad", "red"],
    ]);
    // Tags options map by name; the target's option and colour stay, nothing was added.
    expect(schema.get("Tags")?.options.map((option) => [option.name, option.color])).toEqual([["Urgent", "orange"]]);
  });

  it("leaves the source with no value of the moved pages and the pages that stay untouched", async () => {
    const { client, home, work, homeTree, workTree, valuesOf } = await fixture();
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Nested"), null);
    expect(await valuesOf(homeTree("Stay"))).toEqual({ Estimate: 8 });
    expect((await client.propertySchemaGet(home.id)).map((def) => def.name)).toEqual([
      "Tags",
      "Status",
      "Estimate",
      "Mood",
      "Notes",
      "Related",
      "Related by",
      "Links",
      "Done",
    ]);
    // Searching the properties of the target finds them there.
    expect((await client.search("hello", { spaceId: work.id })).map((hit) => hit.title)).toEqual(["Alpha"]);
    expect(await client.search("hello", { spaceId: home.id })).toEqual([]);
    expect(await client.pagesByProperty(work.id, PROP.workEstimate, { type: "Equals", data: { kind: "number", value: 5 } })).toEqual([
      homeTree("Alpha child"),
    ]);
  });

  it("creates a created property with the source's kind, options and colours, a status without its defaults", async () => {
    const { client, work, homeTree, names } = await fixture({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Phase", kind: "status", options: [{ name: "Draft", group: "todo", color: "gray" }, { name: "Live", group: "done", color: "green" }] },
            { name: "Area", kind: "multi_select", options: [{ name: "North", color: "blue" }, { name: "South", color: "pink" }] },
          ],
          pages: [{ title: "Doc", properties: { Phase: "Live", Area: ["South", "North"] } }],
        },
        { name: "Work" },
      ],
    });
    const summary = await client.pageMoveToSpace(homeTree("Doc"), (await client.spaceList())[1]?.id ?? work.id, null, null);
    expect(summary).toMatchObject({ propertiesCreated: 2, valuesDropped: 0 });
    const target = (await client.spaceList())[1]?.id ?? "";
    const schema = await names(target);
    expect(schema.get("Phase")?.options.map((option) => [option.name, option.color, option.group])).toEqual([
      ["Draft", "gray", "todo"],
      ["Live", "green", "done"],
    ]);
    expect(schema.get("Area")?.options.map((option) => [option.name, option.color])).toEqual([
      ["North", "blue"],
      ["South", "pink"],
    ]);
    const values = await client.pagePropertiesGet(homeTree("Doc"));
    expect(values).toHaveLength(2);
  });

  it("drops the values that cannot follow: a kind that does not fit, and the 100-property limit", async () => {
    const filler = Array.from({ length: 97 }, (_, at) => ({ name: `Filler ${String(at)}`, kind: "text" as const }));
    const { client, homeTree } = await fixture({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Size", kind: "text" },
            { name: "Count", kind: "number" },
            { name: "Extra one", kind: "text" },
            { name: "Extra two", kind: "checkbox" },
            { name: "Extra three", kind: "text" },
          ],
          pages: [
            { title: "Doc", properties: { Size: "big", Count: 4, "Extra one": "a", "Extra two": true, "Extra three": "c" } },
            { title: "Doc two", properties: { Size: "12", Count: 7, "Extra one": "b" } },
          ],
        },
        // Tags and 97 fillers: one slot left (Tags counts) besides the 'Count' that exists by name.
        { name: "Work", properties: [{ name: "Count", kind: "date" }, ...filler.slice(0, 96)] },
      ],
    });
    const [, work] = await client.spaceList();
    const target = work?.id ?? "";
    expect((await client.propertySchemaGet(target)).length).toBe(98);
    const plan = await client.pageMoveToSpacePlan(homeTree("Doc"), target, null);
    // Size, Extra one, Extra two and Extra three are missing, 2 slots are free: Size and Extra one are created,
    // Extra two and Extra three drop their values. Count (a number) into a date drops 1: "4" is no date.
    expect(plan).toMatchObject({ propertiesCreated: 2, valuesDropped: 3 });
    const summary = await client.pageMoveToSpace(homeTree("Doc"), target, null, null);
    expect(summary).toEqual(plan);
    expect((await client.propertySchemaGet(target)).length).toBe(100);
    const second = await client.pageMoveToSpace(homeTree("Doc two"), target, null, null);
    // "12" into a text property is the text, 7 into a date is no date.
    expect(second).toMatchObject({ propertiesCreated: 0, valuesDropped: 1 });
    expect((await client.pagePropertiesGet(homeTree("Doc two"))).map(([, value]) => value.kind).sort()).toEqual(["text", "text"]);
  });

  it("coerces through the text form: number into text, text into number, select into multi-select", async () => {
    const { client, homeTree } = await fixture({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Points", kind: "number" },
            { name: "Code", kind: "text" },
            { name: "Kind", kind: "select", options: [{ name: "A" }, { name: "B" }] },
            { name: "When", kind: "text" },
          ],
          pages: [{ title: "Doc", properties: { Points: 2.5, Code: "42", Kind: "B", When: "2026-10-07" } }],
        },
        {
          name: "Work",
          properties: [
            { name: "Points", kind: "text" },
            { name: "Code", kind: "number" },
            { name: "Kind", kind: "multi_select", options: [{ name: "A" }] },
            { name: "When", kind: "date" },
          ],
        },
      ],
    });
    const [, work] = await client.spaceList();
    const target = work?.id ?? "";
    const summary = await client.pageMoveToSpace(homeTree("Doc"), target, null, null);
    expect(summary).toMatchObject({ propertiesCreated: 0, valuesDropped: 0 });
    const schema = new Map((await client.propertySchemaGet(target)).map((def) => [def.name, def]));
    const nameOf = new Map([...schema.values()].map((def) => [def.id, def.name]));
    const values = new Map((await client.pagePropertiesGet(homeTree("Doc"))).map(([id, value]) => [nameOf.get(id), value]));
    expect(values.get("Points")).toEqual({ kind: "text", value: "2.5" });
    expect(values.get("Code")).toEqual({ kind: "number", value: 42 });
    expect(values.get("When")).toEqual({ kind: "date", value: { date: "2026-10-07" } });
    const kind = values.get("Kind");
    expect(kind?.kind).toBe("multi_select");
    expect(schema.get("Kind")?.options.map((option) => option.name)).toEqual(["A", "B"]);
  });
});

describe("relations", () => {
  it("keep the pages that move along, drop the links to pages that stay (both sides), and create a pair when both halves are used", async () => {
    const { client, home, work, homeTree, workTree, valuesOf } = await fixture();
    const stayBefore = (await client.pageGet(homeTree("Stay"))).updatedAt;
    const plan = await client.pageMoveToSpacePlan(homeTree("Alpha"), work.id, workTree("Nested"));
    // Alpha's Related by [Stay] and Links [Stay], Stay's Related [Alpha] and Links [Alpha].
    expect(plan.relationLinksRemoved).toBe(4);
    await client.pageMoveToSpace(homeTree("Alpha"), work.id, workTree("Nested"), null);
    expect(await valuesOf(homeTree("Stay"))).toEqual({ Estimate: 8 });
    // The back property shows the link on the other side.
    expect((await valuesOf(homeTree("Alpha child")))["Related by"]).toEqual(["Alpha"]);
    expect((await valuesOf(homeTree("Alpha"))).Related).toEqual(["Alpha child"]);
    // The page that stays was written (its last edit is later).
    expect((await client.pageGet(homeTree("Stay"))).updatedAt).toBeGreaterThan(stayBefore);
    // Links had no value left to carry: not created.
    expect((await client.propertySchemaGet(work.id)).some((def) => def.name === "Links")).toBe(false);
    expect((await client.propertySchemaGet(home.id)).some((def) => def.name === "Links")).toBe(true);
  });

  it("creates a one-way relation when only one half is used, and a rollup with its relation", async () => {
    const { client, homeTree } = await fixture({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Subtasks", kind: "relation", twoWay: true, backName: "Parent" },
            { name: "Points", kind: "number" },
            { name: "Total", kind: "rollup", rollup: { relation: "Subtasks", target: "Points", function: "sum" } },
          ],
          pages: [
            { title: "Epic", properties: { Subtasks: ["Task one", "Task two"] } },
            { title: "Task one", properties: { Points: 2 } },
            { title: "Task two", properties: { Points: 3 } },
          ],
        },
        { name: "Work" },
      ],
    });
    const [, work] = await client.spaceList();
    const target = work?.id ?? "";
    // The three pages move one after the other: the epic first, with its subtasks gone behind it.
    const epic = await client.pageMoveToSpacePlan(homeTree("Epic"), target, null);
    // Epic's two subtasks stay (2) and each of them lists the epic as its parent (2 more).
    expect(epic.relationLinksRemoved).toBe(4);
    // A task alone: its parent link goes (1), the epic's list of subtasks loses it (1); Points is created.
    const task = await client.pageMoveToSpacePlan(homeTree("Task one"), target, null);
    expect(task).toMatchObject({ propertiesCreated: 1, relationLinksRemoved: 2 });
    await client.pageMoveToSpace(homeTree("Task one"), target, null, null);
    const schema = new Map((await client.propertySchemaGet(target)).map((def) => [def.name, def]));
    expect([...schema.keys()]).toEqual(["Tags", "Points"]);
  });

  it("moves a connected group together: a two-way pair, its rollup and the computed value", async () => {
    const { client, homeTree } = await fixture({
      spaces: [
        {
          name: "Home",
          properties: [
            { name: "Subtasks", kind: "relation", twoWay: true, backName: "Parent" },
            { name: "Points", kind: "number" },
            { name: "Total", kind: "rollup", rollup: { relation: "Subtasks", target: "Points", function: "sum" } },
          ],
          pages: [
            {
              title: "Epic",
              children: [{ title: "Task one", properties: { Points: 2 } }, { title: "Task two", properties: { Points: 3 } }],
              properties: { Subtasks: ["Task one", "Task two"] },
            },
          ],
        },
        { name: "Work" },
      ],
    });
    const [, work] = await client.spaceList();
    const target = work?.id ?? "";
    const summary = await client.pageMoveToSpace(homeTree("Epic"), target, null, null);
    expect(summary).toMatchObject({ propertiesCreated: 4, valuesDropped: 0, relationLinksRemoved: 0 });
    const schema = new Map((await client.propertySchemaGet(target)).map((def) => [def.name, def]));
    expect([...schema.keys()]).toEqual(["Tags", "Subtasks", "Parent", "Points", "Total"]);
    expect(schema.get("Total")?.config).toMatchObject({ kind: "rollup", relationPropertyId: schema.get("Subtasks")?.id, function: "sum" });
    const total = (await client.pagePropertiesGet(homeTree("Epic"))).find(([id]) => id === schema.get("Total")?.id);
    expect(total?.[1]).toMatchObject({ kind: "rollup", value: { function: "sum", result: { type: "number", data: 5 } } });
  });
});

describe("saved views", () => {
  const BOARD = uuid(0x301);
  const view = (attrs: Record<string, JsonValue>): JsonValue => ({
    type: "doc",
    content: [{ type: "pageList", attrs: { id: BOARD, ...attrs } }],
  });

  const BOARD_ATTRS: Record<string, JsonValue> = {
    layout: "board",
    source: { kind: "tag", optionId: OPTION.urgent },
    properties: [PROP.status, PROP.estimate, PROP.notes, uuid(0x999)],
    sort: { by: PROP.estimate, dir: "desc" },
    filters: [
      { propertyId: PROP.status, op: "hasOption", value: OPTION.sad },
      { propertyId: PROP.notes, op: "isSet" },
      { propertyId: PROP.estimate, op: "isSet" },
    ],
    groupBy: PROP.status,
    hiddenGroups: [OPTION.sad, "none"],
    columnWidths: { title: 300, [PROP.estimate]: 120, [PROP.notes]: 90 },
    calculations: { title: "count_all", [PROP.estimate]: "sum", [PROP.notes]: "count_empty", [uuid(0x999)]: "sum" },
  };

  async function boardFixture(attrs: Record<string, JsonValue> = BOARD_ATTRS) {
    return fixture({
      spaces: [
        {
          name: "Home",
          tagOptions: [{ id: OPTION.urgent, name: "urgent" }],
          properties: [
            { id: PROP.status, name: "Stage", kind: "select", options: [{ id: OPTION.happy, name: "Todo" }, { id: OPTION.sad, name: "Done" }] },
            { id: PROP.estimate, name: "Size", kind: "number" },
            { id: PROP.notes, name: "Empty", kind: "text" },
            { id: PROP.done, name: "Ghost", kind: "checkbox" },
          ],
          pages: [
            {
              title: "Board",
              content: view(attrs),
              tags: ["urgent"],
              children: [
                { title: "Card one", tags: ["urgent"], properties: { Stage: "Todo", Size: 3 } },
                { title: "Card two", properties: { Stage: "Done" } },
              ],
            },
          ],
        },
        { name: "Work", tagOptions: [{ id: OPTION.workUrgent, name: "URGENT" }] },
      ],
    });
  }

  it("remap their ids through the mapping of the values and drop the ones with no counterpart", async () => {
    const { client, homeTree } = await boardFixture();
    const [, work] = await client.spaceList();
    const target = work?.id ?? "";
    await client.pageMoveToSpace(homeTree("Board"), target, null, null);
    const schema = new Map((await client.propertySchemaGet(target)).map((def) => [def.name, def]));
    const stage = schema.get("Stage");
    const size = schema.get("Size");
    expect(stage?.id).not.toBe(PROP.status);
    // Only properties with values are created: Stage and Size, not Empty or Ghost.
    expect([...schema.keys()]).toEqual(["Tags", "Stage", "Size"]);
    const content = (await client.pageContentGet(homeTree("Board"))).content;
    const block = (content as { content: { attrs: Record<string, JsonValue> }[] }).content[0]?.attrs ?? {};
    const done = stage?.options.find((option) => option.name === "Done")?.id;
    expect(block).toEqual({
      id: BOARD,
      layout: "board",
      // The Tags option, by name, in the target's Tags.
      source: { kind: "tag", optionId: schema.get("Tags")?.options[0]?.id },
      // Empty and the unknown id are gone.
      properties: [stage?.id, size?.id],
      sort: { by: size?.id, dir: "desc" },
      filters: [
        { propertyId: stage?.id, op: "hasOption", value: done },
        { propertyId: size?.id, op: "isSet" },
      ],
      groupBy: stage?.id,
      hiddenGroups: [done, "none"],
      columnWidths: { title: 300, [size?.id ?? ""]: 120 },
      // The footer's functions follow their columns; the page column's stays, the ones of Empty and the unknown id go.
      calculations: { title: "count_all", [size?.id ?? ""]: "sum" },
    });
  });

  describe("of a block with several views (ADR-0054)", () => {
    const isSet = (propertyId: string): JsonValue => ({ propertyId, op: "isSet" });
    const hasOption = (propertyId: string, value: string): JsonValue => ({ propertyId, op: "hasOption", value });
    const GHOST = uuid(0x999);
    const stored = (): Record<string, JsonValue> => ({
      layout: "table",
      source: { kind: "children" },
      properties: [PROP.status, PROP.estimate, PROP.notes],
      sort: { by: PROP.estimate, dir: "asc" },
      filters: [hasOption(PROP.status, OPTION.sad), isSet(PROP.notes)],
      columnWidths: { title: 320, [PROP.estimate]: 120, [PROP.notes]: 90 },
      views: [
        {
          id: "k3f9a2bc",
          name: "",
          layout: "table",
          properties: [PROP.status, PROP.estimate, PROP.notes],
          sort: { by: PROP.estimate, dir: "asc" },
          filter: {
            op: "and",
            items: [
              hasOption(PROP.status, OPTION.sad),
              isSet(PROP.notes),
              { op: "or", items: [hasOption(PROP.status, OPTION.happy), isSet(PROP.notes)] },
              { op: "or", items: [isSet(PROP.notes), isSet(GHOST)] },
              { op: "or", items: [] },
            ],
          },
          columnWidths: { title: 320, [PROP.estimate]: 120, [PROP.notes]: 90 },
        },
        {
          id: "b81d00e4",
          name: "By stage",
          layout: "board",
          properties: [PROP.estimate],
          sort: { by: "title", dir: "asc" },
          groupBy: PROP.status,
          hiddenGroups: [OPTION.sad, OPTION.happy, "none"],
          openIn: "center",
          futureKey: { ids: [PROP.estimate] },
        },
        { id: "7c0a55d1", name: "Schedule", layout: "calendar", properties: [], sort: { by: "updated", dir: "asc" }, dateBy: PROP.estimate, endBy: PROP.notes },
        { id: "00000001", name: "Lost", layout: "calendar", properties: [], sort: { by: GHOST, dir: "asc" }, dateBy: GHOST, endBy: GHOST, filter: { op: "or", items: [isSet(GHOST)] } },
        { id: "e2a9b6f0", name: "", layout: "gallery", properties: [], sort: { by: "created", dir: "desc" }, dateBy: "created", cardSize: "l", filter: "not a group" },
        "not a view",
      ],
    });

    it("are remapped like the block's own attributes, and the mirror of the first view stays equal to it", async () => {
      const { client, homeTree } = await boardFixture(stored());
      const [, work] = await client.spaceList();
      const target = work?.id ?? "";
      await client.pageMoveToSpace(homeTree("Board"), target, null, null);
      const schema = new Map((await client.propertySchemaGet(target)).map((def) => [def.name, def]));
      const stage = schema.get("Stage");
      const size = schema.get("Size");
      const done = stage?.options.find((option) => option.name === "Done")?.id;
      const todo = stage?.options.find((option) => option.name === "Todo")?.id;
      const content = (await client.pageContentGet(homeTree("Board"))).content;
      const block = (content as { content: { attrs: Record<string, JsonValue> }[] }).content[0]?.attrs ?? {};
      const views = (block.views ?? []) as Record<string, JsonValue>[];

      // `Empty` has no values (no counterpart): it goes from the block's attributes and from every view.
      expect(block.properties).toEqual([stage?.id, size?.id]);
      expect(block.filters).toEqual([hasOption(stage?.id ?? "", done ?? "")]);
      expect(views).toHaveLength(6);
      const [first, board, calendar, lost, gallery, notAView] = views;
      expect(first).toMatchObject({ id: "k3f9a2bc", properties: block.properties, sort: block.sort, columnWidths: block.columnWidths });
      expect(first?.columnWidths).toEqual({ title: 320, [size?.id ?? ""]: 120 });
      // The plain filters of the first view's AND group are the mirror's `filters`.
      const tree = first?.filter as { op: string; items: JsonValue[] };
      expect(tree.items.filter((item) => !(typeof item === "object" && item !== null && "items" in item))).toEqual(block.filters);
      // The group that lost every filter is gone, the one that kept one and the one that was empty stay.
      expect(tree).toEqual({
        op: "and",
        items: [
          hasOption(stage?.id ?? "", done ?? ""),
          { op: "or", items: [hasOption(stage?.id ?? "", todo ?? "")] },
          { op: "or", items: [] },
        ],
      });

      expect(board).toEqual({
        id: "b81d00e4",
        name: "By stage",
        layout: "board",
        properties: [size?.id],
        sort: { by: "title", dir: "asc" },
        groupBy: stage?.id,
        hiddenGroups: [done, todo, "none"],
        openIn: "center",
        futureKey: { ids: [PROP.estimate] },
      });
      // `Size` follows as the calendar's date; the end property has no counterpart and goes.
      expect(calendar).toEqual({ id: "7c0a55d1", name: "Schedule", layout: "calendar", properties: [], sort: { by: "updated", dir: "asc" }, dateBy: size?.id });
      // Nothing is left to follow in this one, but the view stays.
      expect(lost).toEqual({ id: "00000001", name: "Lost", layout: "calendar", properties: [] });
      expect(gallery).toEqual({ id: "e2a9b6f0", name: "", layout: "gallery", properties: [], sort: { by: "created", dir: "desc" }, dateBy: "created", cardSize: "l", filter: "not a group" });
      expect(notAView).toBe("not a view");
      // The tag source of the block is only remapped for a tag (this one lists the children).
      expect(block.source).toEqual({ kind: "children" });
    });

    it("leave a block whose views name no property alone: no document is rewritten", async () => {
      const { client, events, homeTree } = await boardFixture({
        layout: "list",
        views: [{ id: "a", name: "", layout: "calendar", properties: [], sort: { by: "title", dir: "asc" }, dateBy: "created", cardSize: "s" }],
      });
      const [, work] = await client.spaceList();
      events.length = 0;
      await client.pageMoveToSpace(homeTree("Board"), work?.id ?? "", null, null);
      expect(events.some((event) => event.type === "PageContentChanged")).toBe(false);
    });
  });

  it("announce the documents they changed as the core's own change", async () => {
    const { client, events, homeTree } = await boardFixture();
    const [, work] = await client.spaceList();
    events.length = 0;
    await client.pageMoveToSpace(homeTree("Board"), work?.id ?? "", null, null);
    expect(events).toContainEqual({ type: "PageContentChanged", data: { pageId: homeTree("Board"), source: "Core" } });
  });

  it("leave a page without a board alone and a view with nothing to remap unchanged", async () => {
    const { client, events, work, homeTree } = await fixture();
    await client.pageMoveToSpace(homeTree("Stay"), work.id, null, null);
    expect(events.some((event) => event.type === "PageContentChanged")).toBe(false);
  });
});

describe("attachments", () => {
  const PICTURE = uuid(0x401);
  const SHARED = uuid(0x402);
  const FILED = uuid(0x403);
  const ICON = uuid(0x404);
  const image = (attachmentId: string, n: number): JsonValue => ({
    type: "image",
    attrs: { id: uuid(0x500 + n), attachmentId, alt: "pic" },
  });
  const seeds = [
    { attachmentId: PICTURE, name: "picture.png", mime: "image/png", bytes: new Uint8Array([1, 2, 3]) },
    { attachmentId: SHARED, name: "shared.png", mime: "image/png", bytes: new Uint8Array([4, 5]) },
    { attachmentId: FILED, name: "report.pdf", mime: "application/pdf", bytes: new Uint8Array([6]) },
    { attachmentId: ICON, name: "icon.png", mime: "image/png", bytes: new Uint8Array([7]) },
  ];
  const spaces: SeedSpace[] = [
    {
      name: "Home",
      properties: [{ name: "Files", kind: "files" }],
      pages: [
        {
          title: "Doc",
          icon: `attachment:${ICON}`,
          content: { type: "doc", content: [image(PICTURE, 1), image(SHARED, 2)] },
          properties: { Files: [FILED] },
        },
        { title: "Elsewhere", content: { type: "doc", content: [image(SHARED, 3)] } },
      ],
    },
    { name: "Work" },
  ];

  it("move with the pages when only moved pages use them, the others stay and are counted", async () => {
    const { client, events, home, work, homeTree } = await fixture({ spaces, attachments: seeds });
    // Seeded attachments go to the first space.
    expect((await client.attachmentList(home.id)).map((info) => info.name).sort()).toEqual(["icon.png", "picture.png", "report.pdf", "shared.png"]);
    const plan = await client.pageMoveToSpacePlan(homeTree("Doc"), work.id, null);
    expect(plan).toMatchObject({ attachmentsMoved: 3, attachmentsLeft: 1, propertiesCreated: 1 });
    events.length = 0;
    const summary = await client.pageMoveToSpace(homeTree("Doc"), work.id, null, null);
    expect(summary).toEqual(plan);
    expect((await client.attachmentList(work.id)).map((info) => info.name).sort()).toEqual(["icon.png", "picture.png", "report.pdf"]);
    expect((await client.attachmentList(home.id)).map((info) => info.name)).toEqual(["shared.png"]);
    // The references and the retention marks follow the pages.
    expect((await client.attachmentGet(PICTURE)).spaceId).toBe(work.id);
    expect((await client.attachmentGet(PICTURE)).references).toBe(1);
    expect((await client.attachmentGet(SHARED)).references).toBe(2);
    expect(events.filter((event) => event.type === "AttachmentsChanged")).toEqual([
      { type: "AttachmentsChanged", data: { spaceId: home.id } },
      { type: "AttachmentsChanged", data: { spaceId: work.id } },
    ]);
    // The page's icon is the moved attachment and still reads.
    expect((await client.pageGet(homeTree("Doc"))).icon).toBe(`attachment:${ICON}`);
  });

  it("keep an attachment where it is when a page that stays shows it, whichever page is moved", async () => {
    const { client, home, work, homeTree } = await fixture({ spaces, attachments: seeds });
    const summary = await client.pageMoveToSpace(homeTree("Elsewhere"), work.id, null, null);
    expect(summary).toMatchObject({ attachmentsMoved: 0, attachmentsLeft: 1 });
    expect((await client.attachmentGet(SHARED)).spaceId).toBe(home.id);
  });
});
