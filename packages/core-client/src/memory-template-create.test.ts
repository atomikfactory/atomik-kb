// `PageCreateFromTemplate` in `MemoryTransport` (ADR-0056, `docs/architecture/templates.md` section 5): the twin of the core's
// `template_create.rs`. The copy of a template's live subtree under a parent at an index, with no flag, the tokens
// `{{date}}` and `{{time}}` of the titles resolved with the local day and time of the device, the placeholders of the
// properties and of the mentions resolved (today, now, me), the request's values on the top copy, the checks that write
// nothing, relations and links into the subtree, the default template of views, one undo step, the events, and a
// duplicate that keeps the flag.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { PropertyValue } from "./generated/PropertyValue";
import type { TreeView } from "./generated/TreeView";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { fillTemplateContent, localMoment, portableBlockIds, remapPortable, resolveTitleTokens } from "./page-copy";
import type { SeedPage, SeedSpace } from "./structure";

/** 2026-10-18 23:40 UTC: with the offset of the tests (+2 hours) it is 2026-10-19 01:40 on the device's wall clock. */
const NOW_MS = Date.UTC(2026, 9, 18, 23, 40);
const OFFSET_SECONDS = 2 * 3600;
const LOCAL_DAY = "2026-10-19";
const LOCAL_TIME = "01:40";

function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Block ids written by the seeds. */
function bid(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

const text = (value: string): JsonValue => ({ type: "text", text: value });
const paragraph = (n: number, ...content: JsonValue[]): JsonValue => ({ type: "paragraph", attrs: { id: bid(n) }, content });
const doc = (...content: JsonValue[]): JsonValue => ({ type: "doc", content });

/** Deterministic ids and a clock the test can move. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: NOW_MS, offset: OFFSET_SECONDS };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    localOffsetSeconds: () => clock.offset,
    workspaceName: "Workspace",
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events, clock };
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
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

/** The ids of the live pages with this title, in tree order. */
function titled(view: TreeView, title: string): string[] {
  const found: string[] = [];
  const walk = (nodes: PageNode[]): void => {
    for (const node of nodes) {
      if (node.page.title === title) {
        found.push(node.page.id);
      }
      walk(node.children);
    }
  };
  walk(view.roots);
  return found;
}

/** The page and its live subtree from a tree, pre-order. */
function subtreeOf(view: TreeView, pageId: string): string[] {
  const found: string[] = [];
  const walk = (node: PageNode, inside: boolean): void => {
    const here = inside || node.page.id === pageId;
    if (here) {
      found.push(node.page.id);
    }
    node.children.forEach((child) => {
      walk(child, here);
    });
  };
  view.roots.forEach((root) => {
    walk(root, false);
  });
  return found;
}

/** The child ids of the page `pageId`, in order. */
function childrenOf(view: TreeView, pageId: string): string[] {
  const find = (nodes: PageNode[]): PageNode | undefined => {
    for (const node of nodes) {
      const found = node.page.id === pageId ? node : find(node.children);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  };
  return find(view.roots)?.children.map((child) => child.page.id) ?? [];
}

type JsonObject = { [key: string]: JsonValue | undefined };

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Every node of portable content with this type, in document order. */
function nodesOf(content: JsonValue, type: string): JsonObject[] {
  const out: JsonObject[] = [];
  const visit = (node: JsonValue | undefined): void => {
    if (!isObject(node)) {
      return;
    }
    if (node.type === type) {
      out.push(node);
    }
    (Array.isArray(node.content) ? node.content : []).forEach(visit);
  };
  visit(content);
  return out;
}

/** The attributes of a node. */
function attrsOf(node: JsonObject | undefined): JsonObject {
  const attrs = node?.attrs;
  return isObject(attrs) ? attrs : {};
}

/** The views of the first `pageList` block of portable content. */
function viewsOf(content: JsonValue): JsonObject[] {
  const views = attrsOf(nodesOf(content, "pageList")[0]).views;
  const out: JsonObject[] = [];
  for (const view of Array.isArray(views) ? views : []) {
    if (typeof view === "object" && view !== null && !Array.isArray(view)) {
      out.push(view);
    }
  }
  return out;
}

/** The text of the paragraphs of portable content, in order. */
function textsOf(content: JsonValue): string[] {
  const out: string[] = [];
  const visit = (node: JsonValue): void => {
    if (!isObject(node)) {
      return;
    }
    const children = Array.isArray(node.content) ? node.content : [];
    if (node.type === "paragraph" || node.type === "heading") {
      const joined = children.map((child) => (isObject(child) && typeof child.text === "string" ? child.text : "")).join("");
      if (joined !== "") {
        out.push(joined);
      }
    }
    children.forEach(visit);
  };
  visit(content);
  return out;
}

const OTHER_PERSON = id(0x9001);
// The ids the seeds hand out, in order: the device (1), the workspace (2), "Home" (3), then its pages depth first: Tasks (4),
// Other (5), Standup (6), Notes (7), Deep (8), Gone (9), Plain (10), Outside (11).
const OTHER = id(5);
const NOTES = id(7);

const TEMPLATE_CONTENT: JsonValue = doc(
  paragraph(
    1,
    text("agenda zebra "),
    { type: "dateMention", attrs: { id: bid(2), date: "2020-01-01", relative: "today", reminder: 15 } },
    { type: "dateMention", attrs: { id: bid(3), date: "2020-01-01", time: "09:00", relative: "now" } },
    { type: "dateMention", attrs: { id: bid(4), date: "2021-05-06" } },
    { type: "personMention", attrs: { accountId: OTHER_PERSON, label: "Ada", relative: "me" } },
    { type: "personMention", attrs: { accountId: OTHER_PERSON, label: "Ada" } },
    { type: "pageRef", attrs: { id: bid(5), pageId: NOTES } },
  ),
  {
    type: "templateButton",
    attrs: { id: bid(6), label: "Add a note" },
    content: [
      paragraph(
        7,
        { type: "dateMention", attrs: { id: bid(8), date: "2020-01-01", relative: "today" } },
        { type: "personMention", attrs: { accountId: OTHER_PERSON, label: "Ada", relative: "me" } },
      ),
    ],
  },
  {
    type: "pageList",
    attrs: {
      id: bid(9),
      views: [
        { id: "default", layout: "list", defaultTemplate: NOTES },
        { id: "other", layout: "table", defaultTemplate: OTHER },
      ],
    },
  },
);

// The pages are found by title below; the template's content names Notes and Other by the ids above.
const PAGES: readonly SeedPage[] = [
  {
    title: "Tasks",
    children: [
      { title: "Other" },
      {
        title: "Standup {{date}} {{time}}",
        icon: "📋",
        template: true,
        content: TEMPLATE_CONTENT,
        properties: { Status: "Doing" },
        templatePlaceholders: { Due: "today" },
        children: [
          {
            title: "Notes of {{date}}",
            content: doc(paragraph(10, text("notes giraffe"))),
            templatePlaceholders: { Seen: "now", Owner: "me" },
            children: [{ title: "Deep", properties: { Cost: 7 } }],
          },
          { title: "Gone" },
        ],
      },
    ],
  },
  { title: "Plain" },
  { title: "Outside" },
];

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    properties: [
      { name: "Status", kind: "select", options: [{ name: "Doing", color: "blue" }] },
      { name: "Due", kind: "date" },
      { name: "Seen", kind: "date" },
      { name: "Owner", kind: "person" },
      { name: "Cost", kind: "number" },
      { name: "Blocks", kind: "relation", twoWay: true, backName: "Blocked by" },
    ],
    pages: PAGES,
  },
  { name: "Work", pages: [{ title: "Elsewhere" }] },
];

async function fixture(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: SPACES, ...options });
  const spaces = await ctx.client.spaceList();
  const home = spaces.find((space) => space.name === "Home");
  const work = spaces.find((space) => space.name === "Work");
  if (home === undefined || work === undefined) {
    throw new Error("the spaces are missing");
  }
  const schema = await ctx.client.propertySchemaGet(home.id);
  const property = (name: string): string => {
    const found = schema.find((def) => def.name === name);
    if (found === undefined) {
      throw new Error(`no property ${name}`);
    }
    return found.id;
  };
  const blocks = schema.find((def) => def.name === "Blocks");
  const blockedBy = blocks?.config?.kind === "relation" ? blocks.config.pairedPropertyId : undefined;
  const view = await ctx.client.treeGet(home.id);
  const first = (title: string): string => {
    const found = titled(view, title)[0];
    if (found === undefined) {
      throw new Error(`no live page titled ${title}`);
    }
    return found;
  };
  // The page "Gone" goes to the trash before anything is made.
  await ctx.client.pageTrash(first("Gone"));
  const tree = (): Promise<TreeView> => ctx.client.treeGet(home.id);
  const search = async (query: string): Promise<string[]> => [...new Set((await ctx.client.search(query)).map((hit) => hit.pageId))].sort();
  const status = (await ctx.client.propertySchemaGet(home.id)).find((def) => def.name === "Status")?.options[0]?.id ?? "";
  const me = (await ctx.client.peopleMentionable(home.id)).find((person) => person.isMe)?.accountId ?? "";
  ctx.events.length = 0;
  return { ...ctx, home, work, first, tree, search, property, blockedBy: blockedBy ?? "", doing: status, me };
}

function value(values: [string, PropertyValue][], propertyId: string): PropertyValue | undefined {
  return values.find(([id]) => id === propertyId)?.[1];
}

function sorted(ids: string[]): string[] {
  return [...ids].sort();
}

const day = (date: string, time?: string): PropertyValue => ({ kind: "date", value: time === undefined ? { date } : { date, time } });

describe("PageCreateFromTemplate", () => {
  it("makes a page under a parent at an index with resolved titles, in new blocks, as pages and not templates", async () => {
    const { client, events, first, tree, home, search } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const tasks = first("Tasks");
    const liveBefore = (await client.spaceList()).find((space) => space.id === home.id)?.pageCount ?? 0;
    expect(await search("zebra")).toEqual([]);

    const made = await client.pageCreateFromTemplate(template, { parentId: tasks, index: 0 });

    expect(made.id).not.toBe(template);
    expect(made).toMatchObject({ title: `Standup ${LOCAL_DAY} ${LOCAL_TIME}`, icon: "📋", spaceId: home.id, parentId: tasks });
    expect(made.template).toBeUndefined();
    expect(made.templatePlaceholders).toBeUndefined();
    expect(await client.pageGet(made.id)).toEqual(made);
    const view = await tree();
    expect(outline(view)).toEqual([
      "Tasks",
      `  Standup ${LOCAL_DAY} ${LOCAL_TIME}`,
      `    Notes of ${LOCAL_DAY}`,
      "      Deep",
      "  Other",
      "  Standup {{date}} {{time}}",
      "    Notes of {{date}}",
      "      Deep",
      "Plain",
      "Outside",
    ]);
    expect(view.trashed.map((entry) => entry.page.title)).toEqual(["Gone"]);
    const copies = subtreeOf(view, made.id);
    expect(copies).toHaveLength(3);
    expect((await client.spaceList()).find((space) => space.id === home.id)?.pageCount).toBe(liveBefore + 3);
    expect((await client.pageGet(copies[1] ?? "")).parentId).toBe(copies[0]);
    expect((await client.pageGet(copies[2] ?? "")).parentId).toBe(copies[1]);

    // The template is untouched.
    expect(await client.pageGet(template)).toMatchObject({ template: true, title: "Standup {{date}} {{time}}" });

    // The content: the template's, in new blocks; the copies are found by search, the template is not.
    const original = (await client.pageContentGet(template)).content;
    const content = (await client.pageContentGet(copies[0] ?? "")).content;
    expect(textsOf(content)[0]).toBe("agenda zebra ");
    expect(portableBlockIds(content).filter((block) => portableBlockIds(original).includes(block))).toEqual([]);
    expect(await search("zebra")).toEqual([copies[0] ?? ""]);
    expect(await search("giraffe")).toEqual([copies[1] ?? ""]);

    // The events of a duplicate: the structure, the space counts, each new document.
    expect(events.slice(0, 2)).toEqual([{ type: "StructureChanged", data: { spaceId: home.id } }, { type: "SpacesChanged" }]);
    for (const page of copies) {
      expect(events).toContainEqual({ type: "PageContentChanged", data: { pageId: page, source: "Core" } });
    }
  });

  it("puts the top copy at the top level, last, or clamped, and under any live page outside the template", async () => {
    const { client, first, tree } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const tasks = first("Tasks");
    const other = first("Other");

    const top = await client.pageCreateFromTemplate(template);
    expect(top.parentId).toBeUndefined();
    const last = await client.pageCreateFromTemplate(template, { parentId: tasks });
    const clamped = await client.pageCreateFromTemplate(template, { parentId: tasks, index: 99 });
    const view = await tree();
    expect(view.roots.map((node) => node.page.id)).toEqual([tasks, first("Plain"), first("Outside"), top.id]);
    expect(childrenOf(view, tasks)).toEqual([other, template, last.id, clamped.id]);

    const middle = await client.pageCreateFromTemplate(template, { parentId: tasks, index: 1 });
    expect(childrenOf(await tree(), tasks)).toEqual([other, middle.id, template, last.id, clamped.id]);
    const under = await client.pageCreateFromTemplate(template, { parentId: middle.id });
    expect(under.parentId).toBe(middle.id);
  });

  it("resolves the placeholders with the local day, the local time and the account of the request", async () => {
    const { client, first, tree, property, doing, me, clock, home } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const tasks = first("Tasks");

    const made = await client.pageCreateFromTemplate(template, { parentId: tasks });
    const copies = subtreeOf(await tree(), made.id);

    const own = await client.pagePropertiesGet(copies[0] ?? "");
    expect(value(own, property("Status"))).toEqual({ kind: "select", value: doing });
    expect(value(own, property("Due"))).toEqual(day(LOCAL_DAY));
    expect(made.properties?.find(([id]) => id === property("Due"))?.[1]).toEqual(day(LOCAL_DAY));
    const notes = await client.pagePropertiesGet(copies[1] ?? "");
    expect(value(notes, property("Seen"))).toEqual(day(LOCAL_DAY, LOCAL_TIME));
    expect(value(notes, property("Owner"))).toEqual({ kind: "person", value: [me] });
    expect(value(await client.pagePropertiesGet(copies[2] ?? ""), property("Cost"))).toEqual({ kind: "number", value: 7 });
    // The values are found by the property queries (the copies', not the template's).
    const holders = async (name: string): Promise<string[]> => sorted(await client.pagesByProperty(home.id, property(name), { type: "HasValue" }));
    expect(await holders("Due")).toEqual([copies[0] ?? ""]);
    expect(await holders("Seen")).toEqual([copies[1] ?? ""]);
    expect(await holders("Owner")).toEqual([copies[1] ?? ""]);
    // The copies carry no placeholders; the template keeps its own, and its values are unchanged.
    expect((await client.pageGet(copies[1] ?? "")).templatePlaceholders).toBeUndefined();
    expect((await client.pageGet(first("Notes of {{date}}"))).templatePlaceholders).toHaveLength(2);
    expect(value(await client.pagePropertiesGet(template), property("Due"))).toBeUndefined();

    // The moment is the request's, and the offset is the device's.
    clock.now += 3_600_000;
    const later = await client.pageCreateFromTemplate(template, { parentId: tasks });
    expect(later.title).toBe(`Standup ${LOCAL_DAY} 02:40`);
    clock.offset = -5 * 3600;
    const west = await client.pageCreateFromTemplate(template, { parentId: tasks });
    expect(west.title).toBe("Standup 2026-10-18 19:40");
    const westCopies = subtreeOf(await tree(), west.id);
    expect(value(await client.pagePropertiesGet(westCopies[0] ?? ""), property("Due"))).toEqual(day("2026-10-18"));
    expect(value(await client.pagePropertiesGet(westCopies[1] ?? ""), property("Seen"))).toEqual(day("2026-10-18", "19:40"));
  });

  it("writes the request's values on the top copy, over a placeholder of the same property", async () => {
    const { client, first, tree, property, doing, me } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const made = await client.pageCreateFromTemplate(template, {
      parentId: first("Tasks"),
      values: [
        [property("Status"), { kind: "select", value: doing }],
        [property("Due"), day("2031-02-03")],
        [property("Cost"), { kind: "number", value: 12 }],
      ],
    });
    const copies = subtreeOf(await tree(), made.id);
    const own = await client.pagePropertiesGet(copies[0] ?? "");
    expect(value(own, property("Due"))).toEqual(day("2031-02-03"));
    expect(value(own, property("Cost"))).toEqual({ kind: "number", value: 12 });
    expect(value(await client.pagePropertiesGet(copies[1] ?? ""), property("Cost"))).toBeUndefined();
    expect(value(await client.pagePropertiesGet(copies[1] ?? ""), property("Owner"))).toEqual({ kind: "person", value: [me] });
  });

  it("makes relations inside the subtree follow the copies and keeps the others paired", async () => {
    const { client, first, tree, property, blockedBy, home } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const notes = first("Notes of {{date}}");
    const deep = first("Deep");
    const outside = first("Outside");
    const blocks = property("Blocks");
    await client.pagePropertySet(template, blocks, { kind: "relation", value: [outside] });
    await client.pagePropertySet(notes, blocks, { kind: "relation", value: [deep] });

    const made = await client.pageCreateFromTemplate(template, { parentId: first("Tasks") });
    const copies = subtreeOf(await tree(), made.id);

    expect(value(await client.pagePropertiesGet(copies[0] ?? ""), blocks)).toEqual({ kind: "relation", value: [outside] });
    expect(value(await client.pagePropertiesGet(outside), blockedBy)).toEqual({ kind: "relation", value: [template, copies[0] ?? ""] });
    expect(value(await client.pagePropertiesGet(copies[1] ?? ""), blocks)).toEqual({ kind: "relation", value: [copies[2] ?? ""] });
    expect(value(await client.pagePropertiesGet(copies[2] ?? ""), blockedBy)).toEqual({ kind: "relation", value: [copies[1] ?? ""] });
    expect(value(await client.pagePropertiesGet(deep), blockedBy)).toEqual({ kind: "relation", value: [notes] });
    // The paired side of the outside page is found by the property queries too.
    const holders = async (propertyId: string): Promise<string[]> => sorted(await client.pagesByProperty(home.id, propertyId, { type: "HasValue" }));
    expect(await holders(blockedBy)).toEqual(sorted([outside, deep, copies[2] ?? ""]));
    expect(await holders(blocks)).toEqual(sorted([template, notes, copies[0] ?? "", copies[1] ?? ""]));

    // A relation among the request's values pairs the other side too.
    const other = first("Other");
    const again = await client.pageCreateFromTemplate(template, {
      parentId: first("Tasks"),
      values: [[blocks, { kind: "relation", value: [other] }]],
    });
    expect(value(await client.pagePropertiesGet(again.id), blocks)).toEqual({ kind: "relation", value: [other] });
    expect(value(await client.pagePropertiesGet(other), blockedBy)).toEqual({ kind: "relation", value: [again.id] });
  });

  it("resolves relative mentions, follows links and default templates, and leaves a template button's content alone", async () => {
    const { client, first, tree, me } = await fixture();
    await client.profileSet("Grace");
    const template = first("Standup {{date}} {{time}}");
    const other = first("Other");
    expect(other).toBe(OTHER);
    expect(first("Notes of {{date}}")).toBe(NOTES);
    const made = await client.pageCreateFromTemplate(template, { parentId: first("Tasks") });
    const copies = subtreeOf(await tree(), made.id);
    const content = (await client.pageContentGet(made.id)).content;

    const dates = nodesOf(content, "dateMention");
    expect(dates).toHaveLength(4);
    expect(attrsOf(dates[0])).toMatchObject({ date: LOCAL_DAY, reminder: 15 });
    expect(attrsOf(dates[0])).not.toHaveProperty("relative");
    expect(attrsOf(dates[1])).toMatchObject({ date: LOCAL_DAY, time: LOCAL_TIME });
    expect(attrsOf(dates[1])).not.toHaveProperty("relative");
    expect(attrsOf(dates[2])).toMatchObject({ date: "2021-05-06" });
    const people = nodesOf(content, "personMention");
    expect(people).toHaveLength(3);
    expect(attrsOf(people[0])).toMatchObject({ accountId: me, label: "Grace" });
    expect(attrsOf(people[0])).not.toHaveProperty("relative");
    expect(attrsOf(people[1])).toMatchObject({ accountId: OTHER_PERSON, label: "Ada" });

    // The button and its content are copied as they are: relative mentions stay relative.
    const buttons = nodesOf(content, "templateButton");
    expect(buttons).toHaveLength(1);
    expect(attrsOf(buttons[0]).label).toBe("Add a note");
    expect(attrsOf(buttons[0]).id).not.toBe(bid(6));
    expect(attrsOf(dates[3])).toMatchObject({ relative: "today", date: "2020-01-01" });
    expect(attrsOf(people[2])).toMatchObject({ relative: "me", accountId: OTHER_PERSON });

    // The link to Notes and the default template of the first view point to Notes' copy; the second view's, outside the
    // subtree, is kept.
    expect(attrsOf(nodesOf(content, "pageRef")[0]).pageId).toBe(copies[1]);
    const named = viewsOf(content);
    expect(named.map((view) => view.defaultTemplate)).toEqual([copies[1], other]);
    expect(named[1]?.layout).toBe("table");

    // The template is exactly as it was.
    const original = nodesOf((await client.pageContentGet(template)).content, "dateMention");
    expect(attrsOf(original[0]).relative).toBe("today");
    expect(attrsOf(original[1]).relative).toBe("now");
  });

  it("is refused with nothing written for an unknown page, a page that is not a template, a trashed template, a bad parent or a bad value", async () => {
    const { client, events, first, tree, property, work, doing, search } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const tasks = first("Tasks");
    const plain = first("Plain");
    const notes = first("Notes of {{date}}");
    const elsewhere = (await client.treeGet(work.id)).roots[0]?.page.id ?? "";
    await client.pageSetTemplate(plain, true);
    await client.pageTrash(plain);
    const trashedParent = (await tree()).trashed[0]?.page.id ?? "";
    events.length = 0;
    const before = outline(await tree());
    const version = await search("zebra");
    const errorOf = async (promise: Promise<unknown>) => (await failure(promise)).toApiError();
    const invalidField = (field: string) => ({ code: "InvalidInput", details: { field } });

    expect(await errorOf(client.pageCreateFromTemplate(id(0xffff)))).toMatchObject({ code: "NotFound" });
    expect(await errorOf(client.pageCreateFromTemplate(tasks))).toMatchObject(invalidField("templateId"));
    expect(await errorOf(client.pageCreateFromTemplate(notes))).toMatchObject(invalidField("templateId"));
    expect(await errorOf(client.pageCreateFromTemplate(plain))).toMatchObject(invalidField("templateId"));
    for (const parentId of [id(0xffff), trashedParent, elsewhere]) {
      expect(await errorOf(client.pageCreateFromTemplate(template, { parentId }))).toMatchObject(invalidField("parentId"));
    }
    for (const inside of [template, notes, first("Deep")]) {
      expect(await errorOf(client.pageCreateFromTemplate(template, { parentId: inside }))).toMatchObject(invalidField("parentId"));
    }
    const good: [string, PropertyValue] = [property("Cost"), { kind: "number", value: 1 }];
    const bad: [string, PropertyValue][] = [
      [property("Cost"), { kind: "text", value: "x" }],
      [property("Status"), { kind: "select", value: id(0xeeee) }],
      [id(0xdddd), { kind: "number", value: 1 }],
      [property("Owner"), { kind: "person", value: [id(0xcccc)] }],
      [property("Cost"), { kind: "number", value: Number.NaN }],
    ];
    for (const refusal of bad) {
      expect(await errorOf(client.pageCreateFromTemplate(template, { parentId: tasks, values: [good, refusal] }))).toMatchObject(
        invalidField("values"),
      );
    }
    expect(doing).not.toBe("");

    expect(outline(await tree())).toEqual(before);
    expect(await search("zebra")).toEqual(version);
    expect(events).toEqual([]);
  });

  it("refuses a template of more than 1,000 pages and makes one of exactly 1,000", async () => {
    const big = setup({
      spaces: [{ name: "Big", pages: [{ title: "Top", template: true, children: Array.from({ length: 1000 }, (_, n) => ({ title: `p${String(n)}` })) }] }],
    });
    const [space] = await big.client.spaceList();
    const view = await big.client.treeGet(space?.id ?? "");
    const top = view.roots[0]?.page.id ?? "";
    big.events.length = 0;
    expect((await failure(big.client.pageCreateFromTemplate(top))).toApiError()).toMatchObject({
      code: "InvalidInput",
      details: { field: "templateId" },
    });
    expect(outline(await big.client.treeGet(space?.id ?? ""))).toHaveLength(1001);
    expect(big.events).toEqual([]);
    await big.client.pageTrash(view.roots[0]?.children[0]?.page.id ?? "");
    const made = await big.client.pageCreateFromTemplate(top);
    expect(subtreeOf(await big.client.treeGet(space?.id ?? ""), made.id)).toHaveLength(1000);
  });

  it("is one undo step, and a redo brings the pages, their values and their content back", async () => {
    const { client, first, tree, home, property } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const original = outline(await tree());
    const made = await client.pageCreateFromTemplate(template, { parentId: first("Tasks") });
    const copies = subtreeOf(await tree(), made.id);
    const created = outline(await tree());
    expect(created).not.toEqual(original);

    const undone = await client.structureUndo(home.id);
    expect(outline(undone)).toEqual(original);
    for (const page of copies) {
      expect((await failure(client.pageGet(page))).toApiError()).toMatchObject({ code: "NotFound" });
    }
    expect((await client.pageGet(template)).template).toBe(true);

    const redone = await client.structureRedo(home.id);
    expect(outline(redone)).toEqual(created);
    expect(value(await client.pagePropertiesGet(copies[0] ?? ""), property("Due"))).toEqual(day(LOCAL_DAY));
    expect(textsOf((await client.pageContentGet(copies[0] ?? "")).content)[0]).toBe("agenda zebra ");
    expect((await client.pageGet(made.id)).template).toBeUndefined();
  });

  it("is made by the duplicate of a template too, which keeps the flag, the placeholders and the default templates inside it", async () => {
    const { client, first, tree, property } = await fixture();
    const template = first("Standup {{date}} {{time}}");
    const copy = await client.pageDuplicate(template);

    expect(copy).toMatchObject({ title: "Standup {{date}} {{time}} (copy)", template: true });
    expect(copy.templatePlaceholders).toEqual([[property("Due"), "today"]]);
    const copies = subtreeOf(await tree(), copy.id);
    const sub = await client.pageGet(copies[1] ?? "");
    expect(sub.template).toBeUndefined();
    expect(sub.title).toBe("Notes of {{date}}");
    expect(sub.templatePlaceholders).toHaveLength(2);
    expect(value(await client.pagePropertiesGet(copy.id), property("Due"))).toBeUndefined();
    expect(copies).toContain(viewsOf((await client.pageContentGet(copy.id)).content)[0]?.defaultTemplate);

    // The duplicate of a page that is not a template is not one.
    expect((await client.pageDuplicate(first("Tasks"))).template).toBeUndefined();
    // A page made from the duplicate works like one from the template.
    const made = await client.pageCreateFromTemplate(copy.id, { parentId: first("Tasks") });
    expect(made.template).toBeUndefined();
    expect(made.title).toBe(`Standup ${LOCAL_DAY} ${LOCAL_TIME} (copy)`);
  });
});

describe("the pure steps of a creation from a template", () => {
  const moment = { date: "2026-10-18", time: "13:05" };

  it("reads the local day and time of an instant with the offset, across midnight and leap days", () => {
    const at = (seconds: number, offset: number) => localMoment(seconds * 1000, offset);
    // 2026-10-07 14:30 UTC.
    const utc = Date.UTC(2026, 9, 7, 14, 30) / 1000;
    expect(at(utc, 0)).toEqual({ date: "2026-10-07", time: "14:30" });
    expect(at(utc, 14 * 3600)).toEqual({ date: "2026-10-08", time: "04:30" });
    expect(at(utc, -15 * 3600)).toEqual({ date: "2026-10-06", time: "23:30" });
    expect(at(utc, 30 * 60)).toEqual({ date: "2026-10-07", time: "15:00" });
    expect(at(0, 0)).toEqual({ date: "1970-01-01", time: "00:00" });
    expect(at(0, -1)).toEqual({ date: "1969-12-31", time: "23:59" });
    expect(localMoment(-1, 0)).toEqual({ date: "1969-12-31", time: "23:59" });
    const leap = Date.UTC(2024, 1, 29, 12) / 1000;
    expect(at(leap, 0)).toEqual({ date: "2024-02-29", time: "12:00" });
    expect(at(leap + 86_400, 0)).toEqual({ date: "2024-03-01", time: "12:00" });
    expect(at(Date.UTC(2025, 11, 31, 23, 59) / 1000, 0)).toEqual({ date: "2025-12-31", time: "23:59" });
    expect(at(Date.UTC(2026, 0, 1) / 1000, 0)).toEqual({ date: "2026-01-01", time: "00:00" });
  });

  it("resolves every title token and nothing else, and never grows a title past its limit", () => {
    expect(resolveTitleTokens("Plain", moment, 500)).toBe("Plain");
    expect(resolveTitleTokens("", moment, 500)).toBe("");
    expect(resolveTitleTokens("{{date}} at {{time}} ({{date}})", moment, 500)).toBe("2026-10-18 at 13:05 (2026-10-18)");
    expect(resolveTitleTokens("{{DATE}} {{date }} {date}", moment, 500)).toBe("{{DATE}} {{date }} {date}");
    expect(resolveTitleTokens("{{date}}", moment, 4)).toBe("2026");
    expect(resolveTitleTokens("ab {{date}}", moment, 3)).toBe("ab");
  });

  it("fills relative mentions, leaves a template button's content and other values of relative alone, and does not modify its input", () => {
    const mention = (n: number, attrs: JsonObject): JsonValue => ({ type: "dateMention", attrs: { id: bid(n), ...attrs } as JsonValue });
    const person = (attrs: JsonObject): JsonValue => ({ type: "personMention", attrs: attrs as JsonValue });
    const me = { account: id(0xa1), label: "Grace" };
    const content = doc(
      paragraph(
        1,
        mention(2, { date: "2020-01-01", relative: "today" }),
        mention(3, { date: "2020-01-01", time: "09:00", relative: "today" }),
        mention(4, { date: "2020-01-01", time: "09:00", relative: "now" }),
        mention(5, { date: "2020-01-01", relative: "tomorrow" }),
        mention(6, { date: "2020-01-01" }),
        person({ accountId: id(0xb2), label: "Ada", relative: "me" }),
        person({ accountId: id(0xb2), label: "Ada" }),
      ),
      { type: "templateButton", attrs: { id: bid(7) }, content: [paragraph(8, mention(9, { date: "2020-01-01", relative: "today" }))] },
    );
    const snapshot = JSON.stringify(content);
    const filled = fillTemplateContent(content, { moment, me });
    expect(JSON.stringify(content)).toBe(snapshot);

    const dates = nodesOf(filled, "dateMention");
    expect(attrsOf(dates[0])).toEqual({ id: bid(2), date: "2026-10-18" });
    expect(attrsOf(dates[1])).toEqual({ id: bid(3), date: "2026-10-18", time: "09:00" });
    expect(attrsOf(dates[2])).toEqual({ id: bid(4), date: "2026-10-18", time: "13:05" });
    expect(attrsOf(dates[3])).toEqual({ id: bid(5), date: "2020-01-01", relative: "tomorrow" });
    expect(attrsOf(dates[4])).toEqual({ id: bid(6), date: "2020-01-01" });
    expect(attrsOf(dates[5])).toEqual({ id: bid(9), date: "2020-01-01", relative: "today" });
    const people = nodesOf(filled, "personMention");
    expect(attrsOf(people[0])).toEqual({ accountId: id(0xa1), label: "Grace" });
    expect(attrsOf(people[1])).toEqual({ accountId: id(0xb2), label: "Ada" });

    // No name: the label that was written stays; no account: `relative` goes and the mention stays as written.
    expect(attrsOf(nodesOf(fillTemplateContent(content, { moment, me: { account: id(0xa1), label: undefined } }), "personMention")[0])).toEqual({
      accountId: id(0xa1),
      label: "Ada",
    });
    expect(attrsOf(nodesOf(fillTemplateContent(content, { moment, me: undefined }), "personMention")[0])).toEqual({ accountId: id(0xb2), label: "Ada" });
    expect(Array.from(attrsOf(nodesOf(fillTemplateContent(content, { moment, me: { account: id(0xa1), label: "é".repeat(150) } }), "personMention")[0]).label as string)).toHaveLength(
      100,
    );
  });

  it("makes a view's default template inside the subtree name its copy", () => {
    const pageA = id(0x71);
    const pageB = id(0x72);
    const copyA = id(0x81);
    const content = doc(
      {
        type: "pageList",
        attrs: {
          id: bid(1),
          views: [
            { id: "default", layout: "list", defaultTemplate: pageA },
            { id: "v2", layout: "table", defaultTemplate: pageB, name: "Outside" },
            { id: "v3", layout: "board" },
            { id: "v4", layout: "board", defaultTemplate: "not an id" },
          ],
        },
      },
      { type: "callout", attrs: { id: bid(2), views: [{ defaultTemplate: pageA }] } },
    );
    const remapped = remapPortable(content, { pages: new Map([[pageA, copyA]]), blocks: new Map() });
    const views = viewsOf(remapped);
    expect(views[0]).toEqual({ id: "default", layout: "list", defaultTemplate: copyA });
    expect(views[1]).toEqual({ id: "v2", layout: "table", defaultTemplate: pageB, name: "Outside" });
    expect(views[2]).toEqual({ id: "v3", layout: "board" });
    expect(views[3]?.defaultTemplate).toBe("not an id");
    expect(attrsOf(nodesOf(remapped, "callout")[0]).views).toEqual([{ defaultTemplate: pageA }]);
  });
});
