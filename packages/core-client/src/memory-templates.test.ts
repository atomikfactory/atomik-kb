// Templates in `MemoryTransport` (ADR-0056, `docs/architecture/templates.md`; the parts of the core this mirror owns): the flag
// (`PageSetTemplate`), the placeholders (`PageSetTemplatePlaceholder`, their interplay with `PagePropertySet`), the hiding of a
// template and its sub-pages from search, backlinks, reminders and the AI index, and how the flag and the placeholders travel
// through export, import, a move to another space and a backup.

import { describe, expect, it } from "vitest";

import { AI_SETTINGS_SETTING } from "./ai";
import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import type { PageInfo } from "./generated/PageInfo";
import type { PageNode } from "./generated/PageNode";
import type { TemplatePlaceholder } from "./generated/TemplatePlaceholder";
import type { TreeView } from "./generated/TreeView";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { readMemoryArchive } from "./memory-export";
import type { MemoryImportSource } from "./memory-import";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const HOUR = 3600;
const OCT_7_1430 = Date.UTC(2026, 9, 7, 14, 30) / 1000;

function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/** The wall clock reading `seconds` (a local time) as an instant on this machine: what `now` returns. */
function atLocal(seconds: number): number {
  const wall = new Date(seconds * 1000);
  return new Date(wall.getUTCFullYear(), wall.getUTCMonth(), wall.getUTCDate(), wall.getUTCHours(), wall.getUTCMinutes(), wall.getUTCSeconds()).getTime();
}

// Ids in creation order: the device (1), the workspace (2), "Home" (3), then its pages depth first: Tasks (4), Sub (5), Plain (6);
// then "Work" (7) with Other (8) and "Copy" (9).
const PLAIN = id(6);

const text = (value: string): JsonValue => ({ type: "text", text: value });
const paragraph = (n: number, ...content: JsonValue[]): JsonValue => ({ type: "paragraph", attrs: { id: blockId(n) }, content });
const doc = (...content: JsonValue[]): JsonValue => ({ type: "doc", content });
const MENTION: JsonValue = { type: "dateMention", attrs: { id: id(900), date: "2026-10-07", time: "14:30", reminder: 15 } };

const PROPERTIES = [
  { name: "Due", kind: "date" },
  { name: "Created", kind: "date" },
  { name: "Owner", kind: "person" },
  { name: "Notes", kind: "text" },
] as const;

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    properties: PROPERTIES,
    pages: [
      {
        title: "Tomato tasks",
        icon: "📋",
        content: doc(
          paragraph(1, text("A tomato plan with "), { type: "pageRef", attrs: { id: blockId(2), pageId: PLAIN } }, text(" and a date "), MENTION),
        ),
        properties: { Notes: "template notes" },
        children: [
          {
            title: "Tomato sub",
            content: doc(paragraph(3, text("A tomato sub-page with "), { type: "pageRef", attrs: { id: blockId(4), pageId: PLAIN } })),
          },
        ],
      },
      { title: "Plain tomato", content: doc(paragraph(5, text("A plain tomato page."))), properties: { Notes: "plain notes" } },
    ],
  },
  { name: "Work", properties: [{ name: "Due", kind: "date" }, { name: "Owner", kind: "text" }], pages: [{ title: "Other" }] },
  { name: "Copy", properties: PROPERTIES },
];

interface Options extends MemoryTransportOptions {
  /** The local wall-clock time of `now`, in seconds (default: noon of 2026-10-07). */
  localNow?: number;
}

async function setup({ localNow, ...options }: Options = {}) {
  let counter = 0;
  const sources: Record<string, MemoryImportSource> = {};
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: localNow === undefined ? () => T0 : () => atLocal(localNow),
    workspaceName: "Workspace",
    spaces: SPACES,
    importSources: sources,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  const spaces = await client.spaceList();
  const spaceId = (name: string): string => spaces.find((space) => space.name === name)?.id ?? "";
  const home = spaceId("Home");
  const schema = await client.propertySchemaGet(home);
  const propertyId = (name: string): string => schema.find((def) => def.name === name)?.id ?? "";
  const tree = await client.treeGet(home);
  const pageId = (title: string): string => pagesByTitle(tree).get(title) ?? "";
  return {
    transport,
    client,
    events,
    sources,
    home,
    work: spaceId("Work"),
    copy: spaceId("Copy"),
    tasks: pageId("Tomato tasks"),
    sub: pageId("Tomato sub"),
    plain: pageId("Plain tomato"),
    other: spaceId("Work") === "" ? "" : (pagesByTitle(await client.treeGet(spaceId("Work"))).get("Other") ?? ""),
    due: propertyId("Due"),
    created: propertyId("Created"),
    owner: propertyId("Owner"),
    notes: propertyId("Notes"),
  };
}

type Fixture = Awaited<ReturnType<typeof setup>>;

function pagesByTitle(view: TreeView): Map<string, string> {
  const found = new Map<string, string>();
  const walk = (nodes: PageNode[]): void => {
    for (const node of nodes) {
      found.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk(view.roots);
  return found;
}

function find(nodes: PageNode[], pageId: string): PageInfo | undefined {
  for (const node of nodes) {
    if (node.page.id === pageId) {
      return node.page;
    }
    const inside = find(node.children, pageId);
    if (inside !== undefined) {
      return inside;
    }
  }
  return undefined;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** The titles of the pages hit, each once (a page has a hit per row that holds the words), sorted. */
const titles = (hits: { title: string }[]): string[] => [...new Set(hits.map((hit) => hit.title))].sort();

describe("PageSetTemplate", () => {
  it("makes a page a template and a page again; the sub-pages are inside it without the flag", async () => {
    const { client, tasks, sub, home } = await setup();
    expect((await client.pageGet(tasks)).template).toBeUndefined();

    const made = await client.pageSetTemplate(tasks, true);
    expect(made.template).toBe(true);
    expect((await client.pageGet(tasks)).template).toBe(true);
    expect((await client.pageGet(sub)).template).toBeUndefined();
    const tree = await client.treeGet(home);
    expect(find(tree.roots, tasks)?.template).toBe(true);
    expect(find(tree.roots, sub)?.template).toBeUndefined();

    const cleared = await client.pageSetTemplate(tasks, false);
    // Absent, not `false`.
    expect("template" in cleared).toBe(false);
    expect("template" in (await client.pageGet(tasks))).toBe(false);
  });

  it("stamps the page, announces StructureChanged, and changes nothing when the page already is what is asked", async () => {
    const { client, events, tasks, home } = await setup();
    events.length = 0;
    await client.pageSetTemplate(tasks, false);
    expect(events).toEqual([]);

    await client.pageSetTemplate(tasks, true);
    expect(events).toEqual([{ type: "StructureChanged", data: { spaceId: home } }]);
    events.length = 0;
    const before = await client.pageGet(tasks);
    await client.pageSetTemplate(tasks, true);
    expect(events).toEqual([]);
    expect(await client.pageGet(tasks)).toEqual(before);
    // One undo step only: the repeated request left no entry of its own.
    expect((await client.treeGet(home)).undo).toEqual({ canUndo: true, canRedo: false });
    await client.structureUndo(home);
    expect((await client.treeGet(home)).undo.canUndo).toBe(false);
  });

  it("refuses a page in the trash (InvalidInput for pageId) and an unknown page (NotFound)", async () => {
    const { client, tasks, sub } = await setup();
    await client.pageTrash(tasks);
    expect(await failure(client.pageSetTemplate(tasks, true))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    // The sub-page is in the trash with it.
    expect(await failure(client.pageSetTemplate(sub, true))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    expect(await failure(client.pageSetTemplate(id(999), true))).toMatchObject({ code: "NotFound" });
  });

  it("is one undo step that undo and redo revert", async () => {
    const { client, tasks, home } = await setup();
    await client.pageSetTemplate(tasks, true);
    const undone = await client.structureUndo(home);
    expect(find(undone.roots, tasks)?.template).toBeUndefined();
    expect((await client.pageGet(tasks)).template).toBeUndefined();
    const redone = await client.structureRedo(home);
    expect(find(redone.roots, tasks)?.template).toBe(true);

    // Clearing is a step too.
    await client.pageSetTemplate(tasks, false);
    expect((await client.pageGet(tasks)).template).toBeUndefined();
    await client.structureUndo(home);
    expect((await client.pageGet(tasks)).template).toBe(true);
  });

  it("needs the Editor role", async () => {
    for (const role of ["viewer", "commenter"] as const) {
      const { client, work, other } = await setup({ sharing: { profileName: "Maya", spaces: [{ space: "Work", role }] } });
      expect(work).not.toBe("");
      const refused = await failure(client.pageSetTemplate(other, true));
      expect({ role, code: refused.code, details: refused.details }).toEqual({
        role,
        code: "Forbidden",
        details: { reason: "role", required: "editor" },
      });
    }
    const { client, other } = await setup({ sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "editor" }] } });
    expect((await client.pageSetTemplate(other, true)).template).toBe(true);
  });

  it("can be seeded, with its placeholders by property name", async () => {
    const seeded = {
      name: "Home",
      properties: [
        { name: "Due", kind: "date" },
        { name: "Owner", kind: "person" },
      ],
      pages: [
        {
          title: "Seeded",
          template: true,
          templatePlaceholders: { Owner: "me", Due: "today" },
          children: [{ title: "Inside" }],
        },
        { title: "Plain" },
      ],
    } as const satisfies SeedSpace;
    const { client, home, due, owner } = await setup({ spaces: [seeded] });
    const roots = (await client.treeGet(home)).roots;
    expect(roots.map((node) => [node.page.title, node.page.template])).toEqual([["Seeded", true], ["Plain", undefined]]);
    expect(roots[0]?.page.templatePlaceholders).toEqual([[due, "today"], [owner, "me"]]);
    expect(roots[0]?.children[0]?.page.template).toBeUndefined();
    expect(roots[0]?.children[0]?.page.templatePlaceholders).toBeUndefined();
    // A name that is no property, or a kind that takes no such placeholder, is a seed error.
    const refused: Record<string, TemplatePlaceholder>[] = [{ Nothing: "today" }, { Due: "me" }, { Owner: "now" }];
    for (const placeholders of refused) {
      expect(() => new MemoryTransport({ spaces: [{ ...seeded, pages: [{ title: "Bad", template: true, templatePlaceholders: placeholders }] }] })).toThrow(CoreError);
    }
  });
});

describe("PageSetTemplatePlaceholder", () => {
  async function templated() {
    const fixture = await setup();
    await fixture.client.pageSetTemplate(fixture.tasks, true);
    fixture.events.length = 0;
    return fixture;
  }

  it("sets today and now on date properties and me on a person property, listed in schema order", async () => {
    const { client, tasks, due, created, owner, events, home } = await templated();
    // Set in the reverse of the schema order (Due, Created, Owner).
    expect((await client.pageSetTemplatePlaceholder(tasks, owner, "me")).templatePlaceholders).toEqual([[owner, "me"]]);
    expect((await client.pageSetTemplatePlaceholder(tasks, created, "now")).templatePlaceholders).toEqual([[created, "now"], [owner, "me"]]);
    const page = await client.pageSetTemplatePlaceholder(tasks, due, "today");
    expect(page.templatePlaceholders).toEqual([[due, "today"], [created, "now"], [owner, "me"]]);
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual(page.templatePlaceholders);
    expect(find((await client.treeGet(home)).roots, tasks)?.templatePlaceholders).toEqual(page.templatePlaceholders);
    expect(events.filter((event) => event.type === "StructureChanged")).toHaveLength(3);

    // Replacing one changes only that entry.
    expect((await client.pageSetTemplatePlaceholder(tasks, due, "now")).templatePlaceholders).toEqual([[due, "now"], [created, "now"], [owner, "me"]]);
  });

  it("clears a placeholder (the value is not touched) and leaves the key out when none is left", async () => {
    const { client, tasks, due, owner } = await templated();
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");
    expect((await client.pageSetTemplatePlaceholder(tasks, due, undefined)).templatePlaceholders).toEqual([[owner, "me"]]);
    const none = await client.pageSetTemplatePlaceholder(tasks, owner, undefined);
    expect("templatePlaceholders" in none).toBe(false);
    // Clearing what is not set is fine and changes nothing.
    expect("templatePlaceholders" in (await client.pageSetTemplatePlaceholder(tasks, due, undefined))).toBe(false);
  });

  it("works on a page inside a template, which is not flagged itself", async () => {
    const { client, sub, created } = await templated();
    const page = await client.pageSetTemplatePlaceholder(sub, created, "now");
    expect(page.templatePlaceholders).toEqual([[created, "now"]]);
    expect(page.template).toBeUndefined();
  });

  it("refuses what is not allowed, with the core's codes and fields", async () => {
    const { client, tasks, plain, due, owner, notes } = await templated();
    // A kind that does not take the placeholder.
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, due, "me"))).toMatchObject({ code: "InvalidInput", details: { field: "placeholder" } });
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, owner, "today"))).toMatchObject({ code: "InvalidInput", details: { field: "placeholder" } });
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, owner, "now"))).toMatchObject({ code: "InvalidInput", details: { field: "placeholder" } });
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, notes, "today"))).toMatchObject({ code: "InvalidInput", details: { field: "placeholder" } });
    // Unknown page, unknown property.
    expect(await failure(client.pageSetTemplatePlaceholder(id(999), due, "today"))).toMatchObject({ code: "NotFound" });
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, id(998), "today"))).toMatchObject({ code: "NotFound" });
    // A page that is neither a template nor inside one.
    expect(await failure(client.pageSetTemplatePlaceholder(plain, due, "today"))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    expect(await failure(client.pageSetTemplatePlaceholder(plain, due, undefined))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    // A page in the trash.
    await client.pageTrash(tasks);
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, due, "today"))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    // Nothing was written by any of them.
    await client.pageRestore(tasks);
    expect("templatePlaceholders" in (await client.pageGet(tasks))).toBe(false);
  });

  it("needs the Editor role", async () => {
    const { client, other, work } = await setup({ sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "viewer" }] } });
    const due = (await client.propertySchemaGet(work)).find((def) => def.name === "Due")?.id ?? "";
    expect(await failure(client.pageSetTemplatePlaceholder(other, due, "today"))).toMatchObject({ code: "Forbidden" });
  });

  it("removes the page's stored value of the property, in the same step", async () => {
    const { client, tasks, due, notes } = await templated();
    await client.pagePropertySet(tasks, due, { kind: "date", value: { date: "2026-10-01" } });
    expect((await client.pageGet(tasks)).properties?.map(([propertyId]) => propertyId)).toEqual([due, notes]);

    const page = await client.pageSetTemplatePlaceholder(tasks, due, "today");
    expect(page.properties?.map(([propertyId]) => propertyId)).toEqual([notes]);
    expect((await client.pagePropertiesGet(tasks)).map(([propertyId]) => propertyId)).toEqual([notes]);

    // Clearing the placeholder does not bring a value back, and leaves the other values alone.
    const cleared = await client.pageSetTemplatePlaceholder(tasks, due, undefined);
    expect(cleared.properties?.map(([propertyId]) => propertyId)).toEqual([notes]);
  });

  it("is reverted by undo and redone by redo, the value with it", async () => {
    const { client, tasks, due, home } = await templated();
    await client.pagePropertySet(tasks, due, { kind: "date", value: { date: "2026-10-01" } });
    await client.pageSetTemplatePlaceholder(tasks, due, "today");

    await client.structureUndo(home);
    let page = await client.pageGet(tasks);
    expect("templatePlaceholders" in page).toBe(false);
    expect(page.template).toBe(true);
    expect((await client.pagePropertiesGet(tasks)).find(([propertyId]) => propertyId === due)?.[1]).toEqual({ kind: "date", value: { date: "2026-10-01" } });

    await client.structureRedo(home);
    page = await client.pageGet(tasks);
    expect(page.templatePlaceholders).toEqual([[due, "today"]]);
    expect((await client.pagePropertiesGet(tasks)).some(([propertyId]) => propertyId === due)).toBe(false);

    // Clearing is a step of its own.
    await client.pageSetTemplatePlaceholder(tasks, due, undefined);
    await client.structureUndo(home);
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[due, "today"]]);
  });

  it("changes nothing when the same placeholder is set again (no event, no undo entry)", async () => {
    const { client, tasks, due, home, events } = await templated();
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    events.length = 0;
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    expect(events).toEqual([]);
    // The flag, then the placeholder: two steps, not three.
    await client.structureUndo(home);
    expect("templatePlaceholders" in (await client.pageGet(tasks))).toBe(false);
    expect((await client.pageGet(tasks)).template).toBe(true);
    await client.structureUndo(home);
    expect((await client.pageGet(tasks)).template).toBeUndefined();
    expect((await client.treeGet(home)).undo.canUndo).toBe(false);
  });

  it("is removed by PagePropertySet of its property, whether the value is set or cleared, but not by another property's", async () => {
    const { client, tasks, due, owner, created, notes } = await templated();
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(tasks, created, "now");
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");

    // Another property's value: the placeholders stay.
    await client.pagePropertySet(tasks, notes, { kind: "text", value: "x" });
    expect((await client.pageGet(tasks)).templatePlaceholders).toHaveLength(3);

    await client.pagePropertySet(tasks, due, { kind: "date", value: { date: "2026-11-02" } });
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[created, "now"], [owner, "me"]]);

    // Clearing a value that is already empty removes the placeholder too.
    expect((await client.pagePropertiesGet(tasks)).some(([propertyId]) => propertyId === created)).toBe(false);
    await client.pagePropertySet(tasks, created, null);
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[owner, "me"]]);

    // A value refused by the property leaves its placeholder.
    await failure(client.pagePropertySet(tasks, owner, { kind: "date", value: { date: "2026-11-02" } }));
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[owner, "me"]]);
  });

  it("is not brought back by an undo next to the value that replaced it", async () => {
    const { client, tasks, due, home } = await templated();
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(tasks, due, undefined);
    await client.pageSetTemplatePlaceholder(tasks, due, "now");
    await client.pagePropertySet(tasks, due, { kind: "date", value: { date: "2026-11-02" } });
    expect("templatePlaceholders" in (await client.pageGet(tasks))).toBe(false);
    // Undoing "now" would restore the state before it: the cleared one, with no placeholder either.
    await client.structureUndo(home);
    expect("templatePlaceholders" in (await client.pageGet(tasks))).toBe(false);
    // Undoing the clearing would restore "today" next to the value that was written since: it is dropped from the history.
    await client.structureUndo(home);
    expect("templatePlaceholders" in (await client.pageGet(tasks))).toBe(false);
  });

  it("is kept when the page leaves the template, and ignored until it is in one again", async () => {
    const { client, tasks, sub, created, home } = await templated();
    await client.pageSetTemplatePlaceholder(sub, created, "now");

    await client.pageMove(sub, null, 0);
    // Out of the template: no placeholder reported, and none can be set.
    expect("templatePlaceholders" in (await client.pageGet(sub))).toBe(false);
    expect(await failure(client.pageSetTemplatePlaceholder(sub, created, "today"))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });

    await client.pageMove(sub, tasks, 0);
    expect((await client.pageGet(sub)).templatePlaceholders).toEqual([[created, "now"]]);

    // The flag cleared on the template takes its sub-pages out too.
    await client.pageSetTemplate(tasks, false);
    expect("templatePlaceholders" in (await client.pageGet(sub))).toBe(false);
    await client.pageSetTemplate(tasks, true);
    expect((await client.pageGet(sub)).templatePlaceholders).toEqual([[created, "now"]]);
    expect((await client.treeGet(home)).roots.length).toBeGreaterThan(0);
  });

  it("lists only the properties that still exist", async () => {
    const { client, tasks, due, created, home } = await templated();
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(tasks, created, "now");
    await client.propertyDelete(home, due);
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[created, "now"]]);
    expect(await failure(client.pageSetTemplatePlaceholder(tasks, due, undefined))).toMatchObject({ code: "NotFound" });
  });
});

describe("hiding templates", () => {
  const flagged = async (options: Options = {}) => {
    const fixture = await setup(options);
    return { ...fixture, hide: () => fixture.client.pageSetTemplate(fixture.tasks, true), show: () => fixture.client.pageSetTemplate(fixture.tasks, false) };
  };

  it("leaves a template and its sub-pages out of search, titles, body, spread words and fuzzy hits", async () => {
    const { client, hide, show } = await flagged();
    expect(titles(await client.search("tomato"))).toEqual(["Plain tomato", "Tomato sub", "Tomato tasks"]);
    expect(titles(await client.search("tomato", { scope: "Titles" }))).toEqual(["Plain tomato", "Tomato sub", "Tomato tasks"]);
    // The words are spread over the rows of the page (title and a block, or a block and a property).
    expect(titles(await client.search("tasks plan"))).toEqual(["Tomato tasks"]);
    // A typo: only the fuzzy title pass finds it.
    expect(titles(await client.search("tomatp"))).toEqual(["Plain tomato", "Tomato sub", "Tomato tasks"]);
    // A property's text.
    expect(titles(await client.search("template notes"))).toEqual(["Tomato tasks"]);

    await hide();
    for (const options of [{}, { scope: "Titles" as const }]) {
      expect(titles(await client.search("tomato", options))).toEqual(["Plain tomato"]);
      expect(titles(await client.search("tomatp", options))).toEqual(["Plain tomato"]);
    }
    expect(await client.search("tasks plan")).toEqual([]);
    expect(await client.search("template notes")).toEqual([]);
    expect(await client.search("sub-page")).toEqual([]);

    await show();
    expect(titles(await client.search("tomato"))).toEqual(["Plain tomato", "Tomato sub", "Tomato tasks"]);
  });

  it("leaves a template's links out of the backlinks", async () => {
    const { client, plain, tasks, sub, hide, show } = await flagged();
    expect((await client.backlinksGet(plain)).map((link) => link.pageId).sort()).toEqual([tasks, sub].sort());
    await hide();
    expect(await client.backlinksGet(plain)).toEqual([]);
    // A page that is a template can still be asked about: its own backlinks are not hidden.
    expect(await client.backlinksGet(tasks)).toEqual([]);
    await show();
    expect(await client.backlinksGet(plain)).toHaveLength(2);
  });

  it("never reminds of a date mention inside a template", async () => {
    const until = OCT_7_1430 + HOUR;
    const { client, tasks, hide, show } = await flagged();
    expect((await client.remindersDue(until)).map((reminder) => reminder.pageId)).toEqual([tasks]);
    await hide();
    expect(await client.remindersDue(until)).toEqual([]);
    await show();
    expect(await client.remindersDue(until)).toHaveLength(1);

    // The scheduler: a template's mention does not fire, the same page as a page does.
    const hidden = await flagged({ localNow: OCT_7_1430 + HOUR });
    await hidden.hide();
    expect(await hidden.transport.remindersTick()).toBe(0);
    expect(hidden.events.some((event) => event.type === "ReminderDue")).toBe(false);
    const shown = await flagged({ localNow: OCT_7_1430 + HOUR });
    expect(await shown.transport.remindersTick()).toBe(1);
  });

  it("does not index a template, its sub-pages, or count them; a page that becomes one drops its chunks", async () => {
    const ai = { mode: "fake" } as const;
    const settings = { enabled: true, chatModel: "qwen3:8b", embeddingModel: "nomic-embed-text:latest", transcriptionModel: "r2t2:latest", semanticIndex: true, think: false };
    const { client, transport, tasks, hide, show } = await flagged({ ai, settings: { [AI_SETTINGS_SETTING]: settings } });
    const index = transport.ai;
    if (index === undefined) {
      throw new Error("the AI option gives the transport an ai");
    }
    // Home: Tasks, Sub, Plain; Work: Other; Copy: nothing.
    expect(await client.aiIndexStatus()).toMatchObject({ pagesIndexed: 0, pagesTotal: 4, dirty: 4 });
    await index.advanceIndex();
    const full = await client.aiIndexStatus();
    expect(full).toMatchObject({ pagesIndexed: 4, pagesTotal: 4, dirty: 0 });
    expect((await client.aiRetrieve("tomato", { type: "all" })).map((source) => source.title).sort()).toEqual(["Plain tomato", "Tomato sub", "Tomato tasks"]);

    await hide();
    const hidden = await client.aiIndexStatus();
    expect(hidden).toMatchObject({ pagesIndexed: 2, pagesTotal: 2, dirty: 0 });
    expect(hidden.chunks).toBeLessThan(full.chunks);
    expect((await client.aiRetrieve("tomato", { type: "all" })).map((source) => source.title)).toEqual(["Plain tomato"]);
    expect(await client.aiRetrieve("tomato", { type: "page", pageId: tasks })).toEqual([]);
    expect((await client.aiRetrieve("tomato", { type: "space", spaceId: (await client.pageGet(tasks)).spaceId })).map((source) => source.title)).toEqual(["Plain tomato"]);
    expect(await index.advanceIndex()).toBe(0);

    await show();
    expect(await client.aiIndexStatus()).toMatchObject({ pagesIndexed: 2, pagesTotal: 4, dirty: 2 });
    expect(await index.advanceIndex()).toBe(2);
  });

  it("does not hide the template from the tree, the counts, a property's pages, or a read by id", async () => {
    const { client, tasks, sub, home, notes, plain, hide } = await flagged();
    const before = (await client.spaceList()).find((space) => space.id === home);
    await hide();
    expect((await client.spaceList()).find((space) => space.id === home)).toEqual(before);
    expect(before?.pageCount).toBe(3);
    const tree = await client.treeGet(home);
    expect(find(tree.roots, tasks)?.template).toBe(true);
    expect(find(tree.roots, sub)).toBeDefined();
    expect((await client.pagesByProperty(home, notes, { type: "HasValue" })).sort()).toEqual([tasks, plain].sort());
    expect((await client.pageGet(sub)).title).toBe("Tomato sub");
    expect((await client.pageContentGet(sub)).pageId).toBe(sub);
    // Trashing it hides nothing more and nothing less than for any page.
    await client.pageTrash(tasks);
    expect((await client.treeGet(home)).trashed.map((entry) => entry.page.id)).toEqual([tasks]);
  });
});

// ----- export, import, moves and backups ---------------------------------------------------------

/** Every file of an export of `spaceId` as a map path to text. */
async function exportedFiles({ client, transport }: Fixture, spaceId: string): Promise<Record<string, string>> {
  const result = await client.exportRun({ spaceId, includeChildren: true, format: "markdown" });
  const archive = readMemoryArchive(transport.files.get(result.path) ?? new Uint8Array());
  if (archive === undefined) {
    throw new Error("not an archive");
  }
  return Object.fromEntries(Object.entries(archive).flatMap(([path, content]) => (typeof content === "string" ? [[path, content]] : [])));
}

/** The text of the file of the page called `title`. */
function fileOf(files: Record<string, string>, title: string): string {
  const path = Object.keys(files).find((candidate) => candidate.endsWith(`/${title}.md`) || candidate === `${title}.md`);
  if (path === undefined) {
    throw new Error(`no file for ${title} in ${Object.keys(files).join(", ")}`);
  }
  return files[path] ?? "";
}

/** The lines of a file's front matter. */
const frontMatterOf = (file: string): string[] => file.split("\n").slice(1, file.split("\n").indexOf("---", 1));

describe("export", () => {
  async function templated() {
    const fixture = await setup();
    const { client, tasks, sub, due, created, owner } = fixture;
    await client.pageSetTemplate(tasks, true);
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(sub, created, "now");
    return fixture;
  }

  it("writes template: true and the placeholders by property name between the cover and the dates", async () => {
    const fixture = await templated();
    const front = frontMatterOf(fileOf(await exportedFiles(fixture, fixture.home), "Tomato tasks"));
    expect(front.slice(0, 6)).toEqual([
      'title: "Tomato tasks"',
      'icon: "📋"',
      "template: true",
      "templatePlaceholders:",
      '  "Due": "today"',
      '  "Owner": "me"',
    ]);
    expect(front[6]).toMatch(/^created: /u);
    expect(front[7]).toMatch(/^updated: /u);
    // The page's own properties still follow.
    expect(front).toContain('"Notes": "template notes"');
  });

  it("writes the placeholders of a page inside a template without the flag, and nothing for a page that is not in one", async () => {
    const fixture = await templated();
    const files = await exportedFiles(fixture, fixture.home);
    const sub = frontMatterOf(fileOf(files, "Tomato sub")).join("\n");
    expect(sub).toContain('templatePlaceholders:\n  "Created": "now"\n');
    expect(sub).not.toContain("template: true");
    const plain = frontMatterOf(fileOf(files, "Plain tomato")).join("\n");
    expect(plain).not.toContain("template");

    // Out of the template, a page keeps its map and exports none.
    await fixture.client.pageMove(fixture.sub, null, 0);
    const moved = await exportedFiles(fixture, fixture.home);
    expect(frontMatterOf(fileOf(moved, "Tomato sub")).join("\n")).not.toContain("template");
  });

  it("leaves out a placeholder whose property is gone, and the key when none is left", async () => {
    const fixture = await templated();
    await fixture.client.propertyDelete(fixture.home, fixture.created);
    const files = await exportedFiles(fixture, fixture.home);
    expect(frontMatterOf(fileOf(files, "Tomato sub")).join("\n")).not.toContain("templatePlaceholders");
    expect(frontMatterOf(fileOf(files, "Tomato tasks")).join("\n")).toContain("templatePlaceholders:");
  });

  it("writes a property named like a key of the front matter as `name (2)`, in any case", async () => {
    const fixture = await setup();
    const { client, home, plain } = fixture;
    for (const [name, value] of [
      ["Template", "v1"],
      ["TemplatePlaceholders", "v2"],
      ["Title", "v3"],
      ["Cover_Y", "v4"],
    ] as const) {
      const property = await client.propertyCreate(home, name, "text");
      await client.pagePropertySet(plain, property.id, { kind: "text", value });
    }
    const front = frontMatterOf(fileOf(await exportedFiles(fixture, home), "Plain tomato"));
    expect(front).toEqual(
      expect.arrayContaining(['"Template (2)": "v1"', '"TemplatePlaceholders (2)": "v2"', '"Title (2)": "v3"', '"Cover_Y (2)": "v4"']),
    );
    expect(front.filter((line) => /^"?(template|templateplaceholders|title)"?:/iu.test(line))).toEqual(['title: "Plain tomato"']);
  });
});

describe("import", () => {
  async function importFiles(fixture: Fixture, spaceId: string, files: Record<string, string>): Promise<void> {
    fixture.sources["notes"] = { kind: "folder", files };
    await fixture.client.importRun({ spaceId, source: "folder" });
  }

  const front = (...lines: string[]): string => `---\n${lines.join("\n")}\n---\n`;

  it("restores the flag and the placeholders (names and placeholders in any case; the entries to skip are skipped)", async () => {
    const fixture = await setup();
    const { client, home, due, owner } = fixture;
    await importFiles(fixture, home, {
      "Weekly.md": `${front(
        'title: "Weekly"',
        'template: " True "',
        "templatePlaceholders:",
        '  "owner": "Me"',
        '  "DUE": "TODAY"',
        '  "due": "now"',
        '  "Nowhere": "today"',
        '  "Notes": "today"',
        '  "Created": "tomorrow"',
        '  " ": "today"',
      )}Body\n`,
      "Flag.md": `${front("Template: TRUE")}Text\n`,
      "Bool.md": `${front("template: true")}Text\n`,
    });
    const tree = await client.treeGet(home);
    const page = (title: string): PageInfo | undefined => find(tree.roots, pagesByTitle(tree).get(title) ?? "");
    // In the schema's order, not the file's. Skipped: a name repeating an earlier one in another case ("due"), an unknown name, a
    // property that does not take it (Notes), an unknown placeholder, an empty name.
    expect(page("Weekly")?.template).toBe(true);
    expect(page("Weekly")?.templatePlaceholders).toEqual([[due, "today"], [owner, "me"]]);
    expect(page("Flag")?.template).toBe(true);
    expect(page("Bool")?.template).toBe(true);
    // Read as such, they are not properties.
    const names = (await client.propertySchemaGet(home)).map((def) => def.name.toLowerCase());
    expect([...names].sort()).toEqual(["created", "due", "notes", "owner", "tags"]);
    // The content is the page's own, and the flag hid the template from search.
    expect((await client.pageContentGet(pagesByTitle(tree).get("Weekly") ?? "")).content).toMatchObject({ content: [{ type: "paragraph" }] });
    expect(await client.search("Weekly", { spaceId: home })).toEqual([]);
  });

  it("reads `template` as the flag only when it is true, and `templatePlaceholders` only when it is a mapping: otherwise they are properties", async () => {
    const fixture = await setup();
    const { client, home } = fixture;
    await importFiles(fixture, home, {
      "Daily.md": `${front('template: "daily"')}Text\n`,
      "Off.md": `${front("template: false")}Text\n`,
      "Yes.md": `${front("template: yes")}Text\n`,
      "Scalar.md": `${front("templatePlaceholders: today")}Text\n`,
      "List.md": `${front("templatePlaceholders:", "  - today", "  - now")}Text\n`,
    });
    const tree = await client.treeGet(home);
    const ids = pagesByTitle(tree);
    for (const title of ["Daily", "Off", "Yes", "Scalar", "List"]) {
      const info = find(tree.roots, ids.get(title) ?? "");
      expect({ title, template: info?.template, placeholders: info?.templatePlaceholders }).toEqual({ title, template: undefined, placeholders: undefined });
    }
    const schema = await client.propertySchemaGet(home);
    const template = schema.find((def) => def.name === "template");
    const placeholders = schema.find((def) => def.name === "templatePlaceholders");
    expect(template).toBeDefined();
    expect(placeholders).toBeDefined();
    const valueOf = async (title: string, propertyId: string | undefined) =>
      (await client.pagePropertiesGet(ids.get(title) ?? "")).find(([id]) => id === propertyId)?.[1];
    expect(await valueOf("Daily", template?.id)).toMatchObject({ kind: "text", value: "daily" });
    expect(await valueOf("Yes", template?.id)).toMatchObject({ kind: "text", value: "yes" });
    expect(await valueOf("Scalar", placeholders?.id)).toMatchObject({ kind: "text", value: "today" });
    expect(await valueOf("List", placeholders?.id)).toBeDefined();
  });

  it("keeps the map of a page inside a template, which carries no flag itself", async () => {
    const fixture = await setup();
    const { client, home, created } = fixture;
    await importFiles(fixture, home, {
      "Parent.md": `${front("template: true")}Parent\n`,
      "Parent/Child.md": `${front("templatePlaceholders:", '  "created": "NOW"')}Child\n`,
    });
    const tree = await client.treeGet(home);
    const ids = pagesByTitle(tree);
    const parent = find(tree.roots, ids.get("Parent") ?? "");
    const child = find(tree.roots, ids.get("Child") ?? "");
    expect(parent?.template).toBe(true);
    expect(child?.template).toBeUndefined();
    expect(child?.templatePlaceholders).toEqual([[created, "now"]]);
  });

  it("matches the properties of the space it imports into: a kind that does not take the placeholder, or no such property, is ignored", async () => {
    const fixture = await setup();
    const { client, work } = fixture;
    await importFiles(fixture, work, {
      "Weekly.md": `${front("template: true", "templatePlaceholders:", '  "Due": "today"', '  "Owner": "me"', '  "Created": "now"')}Body\n`,
    });
    const tree = await client.treeGet(work);
    const weekly = find(tree.roots, pagesByTitle(tree).get("Weekly") ?? "");
    const workDue = (await client.propertySchemaGet(work)).find((def) => def.name === "Due")?.id;
    // Work has Due (a date) and Owner (text): only Due takes a placeholder; it has no Created.
    expect(weekly?.template).toBe(true);
    expect(weekly?.templatePlaceholders).toEqual([[workDue, "today"]]);
  });

  it("brings back, from an export, the template, its placeholders and its sub-pages' (the round trip)", async () => {
    const fixture = await setup();
    const { client, tasks, sub, home, copy, due, created, owner, sources, transport } = fixture;
    await client.pageSetTemplate(tasks, true);
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(sub, created, "now");
    const exported = await client.exportRun({ spaceId: home, includeChildren: true, format: "markdown" });
    const archive = readMemoryArchive(transport.files.get(exported.path) ?? new Uint8Array());
    if (archive === undefined) {
      throw new Error("not an archive");
    }
    sources["Home.zip"] = { kind: "zip", files: archive };
    await client.importRun({ spaceId: copy, source: "zip" });

    const schema = await client.propertySchemaGet(copy);
    const copied = (name: string): string => schema.find((def) => def.name === name)?.id ?? "";
    const tree = await client.treeGet(copy);
    const ids = pagesByTitle(tree);
    const page = (title: string): PageInfo | undefined => find(tree.roots, ids.get(title) ?? "");
    expect(page("Tomato tasks")?.template).toBe(true);
    expect(page("Tomato tasks")?.templatePlaceholders).toEqual([[copied("Due"), "today"], [copied("Owner"), "me"]]);
    expect(page("Tomato sub")?.template).toBeUndefined();
    expect(page("Tomato sub")?.templatePlaceholders).toEqual([[copied("Created"), "now"]]);
    expect(page("Plain tomato")?.template).toBeUndefined();
    expect(page("Plain tomato")?.templatePlaceholders).toBeUndefined();
    // Hidden like any template of the space.
    expect(titles(await client.search("tomato", { spaceId: copy }))).toEqual(["Plain tomato"]);
    // The flag came after the content: the text is what it was.
    expect(JSON.stringify((await client.pageContentGet(ids.get("Tomato tasks") ?? "")).content)).toContain("A tomato plan with");
  });
});

describe("a move to another space", () => {
  async function templated() {
    const fixture = await setup();
    const { client, tasks, sub, due, created, owner } = fixture;
    await client.pageSetTemplate(tasks, true);
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(tasks, created, "now");
    await client.pageSetTemplatePlaceholder(sub, due, "now");
    await client.pageSetTemplatePlaceholder(sub, created, "now");
    return fixture;
  }

  it("carries the flag and the placeholders, mapped to the target's properties; one whose counterpart takes no such placeholder is dropped", async () => {
    const { client, tasks, sub, work, due: homeDue } = await templated();
    const summary = await client.pageMoveToSpace(tasks, work, null, null);
    const schema = await client.propertySchemaGet(work);
    const workId = (name: string): string | undefined => schema.find((def) => def.name === name)?.id;
    expect(workId("Due")).not.toBe(homeDue);
    // Work has a Due date and an Owner that is text (it takes no `me`, and is not replaced); it had no Created, which the
    // placeholders need, and no Notes, which a value needs: both were created.
    expect(summary.propertiesCreated).toBe(2);
    expect(schema.find((def) => def.name === "Created")?.kind).toBe("date");
    const moved = await client.pageGet(tasks);
    expect(moved.spaceId).toBe(work);
    expect(moved.template).toBe(true);
    expect(moved.templatePlaceholders).toEqual([[workId("Due"), "today"], [workId("Created"), "now"]]);
    const inside = await client.pageGet(sub);
    expect(inside.template).toBeUndefined();
    expect(inside.templatePlaceholders).toEqual([[workId("Due"), "now"], [workId("Created"), "now"]]);
    // Still hidden in its new space.
    expect(await client.search("tomato", { spaceId: work })).toEqual([]);
  });

  it("keeps the map of a sub-page that moves out of its template, ready for when it is in one again", async () => {
    const { client, sub, work } = await templated();
    await client.pageMoveToSpace(sub, work, null, null);
    const schema = await client.propertySchemaGet(work);
    const workId = (name: string): string | undefined => schema.find((def) => def.name === name)?.id;
    const out = await client.pageGet(sub);
    expect(out.spaceId).toBe(work);
    expect(out.template).toBeUndefined();
    expect("templatePlaceholders" in out).toBe(false);
    expect((await client.pageSetTemplate(sub, true)).templatePlaceholders).toEqual([[workId("Due"), "now"], [workId("Created"), "now"]]);
  });

  it("follows the move in the target's history: an undo there keeps the mapped placeholders", async () => {
    const { client, tasks, work } = await templated();
    await client.pageMoveToSpace(tasks, work, null, null);
    await client.pageSetTemplate(tasks, false);
    await client.structureUndo(work);
    const schema = await client.propertySchemaGet(work);
    const workId = (name: string): string | undefined => schema.find((def) => def.name === name)?.id;
    expect((await client.pageGet(tasks)).template).toBe(true);
    expect((await client.pageGet(tasks)).templatePlaceholders).toEqual([[workId("Due"), "today"], [workId("Created"), "now"]]);
  });

  describe("the property a placeholder sits on", () => {
    /** A template with placeholders, and spaces to move it to: one without properties, one with a Due date, one with a text Due. */
    async function moving(placeholders: Record<string, TemplatePlaceholder>) {
      let counter = 0;
      const client = new CoreClient(
        new MemoryTransport({
          newId: () => id(++counter),
          now: () => T0,
          workspaceName: "Workspace",
          spaces: [
            {
              name: "Source",
              properties: [
                { name: "Due", kind: "date" },
                { name: "Owner", kind: "person" },
              ],
              pages: [{ title: "Template", template: true, templatePlaceholders: placeholders }],
            },
            { name: "Empty" },
            { name: "Has", properties: [{ name: "Due", kind: "date" }] },
            { name: "Text", properties: [{ name: "Due", kind: "text" }] },
          ],
        }),
      );
      const spaces = await client.spaceList();
      const space = (name: string): string => spaces.find((candidate) => candidate.name === name)?.id ?? "";
      const template = (await client.treeGet(space("Source"))).roots[0]?.page.id ?? "";
      const schema = async (name: string) => client.propertySchemaGet(space(name));
      return { client, space, template, schema };
    }

    it("is created in a target that has none, counted by the plan and the move, and the placeholder is kept", async () => {
      const { client, space, template, schema } = await moving({ Due: "today" });
      expect((await client.pageMoveToSpacePlan(template, space("Empty"), null)).propertiesCreated).toBe(1);
      // The plan wrote nothing: Tags only.
      expect(await schema("Empty")).toHaveLength(1);
      const summary = await client.pageMoveToSpace(template, space("Empty"), null, null);
      expect(summary.propertiesCreated).toBe(1);
      const created = (await schema("Empty")).find((def) => def.name === "Due");
      expect(created).toMatchObject({ kind: "date" });
      expect((await client.pageGet(template)).templatePlaceholders).toEqual([[created?.id, "today"]]);
    });

    it("is created with its kind for a person placeholder too", async () => {
      const { client, space, template, schema } = await moving({ Owner: "me" });
      const summary = await client.pageMoveToSpace(template, space("Empty"), null, null);
      expect(summary.propertiesCreated).toBe(1);
      const created = (await schema("Empty")).find((def) => def.name === "Owner");
      expect(created).toMatchObject({ kind: "person" });
      expect((await client.pageGet(template)).templatePlaceholders).toEqual([[created?.id, "me"]]);
    });

    it("is the target's own when it has a property of that name and kind: nothing is created", async () => {
      const { client, space, template, schema } = await moving({ Due: "now" });
      expect((await client.pageMoveToSpacePlan(template, space("Has"), null)).propertiesCreated).toBe(0);
      const summary = await client.pageMoveToSpace(template, space("Has"), null, null);
      expect(summary.propertiesCreated).toBe(0);
      const has = (await schema("Has")).filter((def) => def.name === "Due");
      expect(has).toHaveLength(1);
      expect((await client.pageGet(template)).templatePlaceholders).toEqual([[has[0]?.id, "now"]]);
    });

    it("is dropped when the target's property of that name is of a kind that takes no such placeholder", async () => {
      const { client, space, template, schema } = await moving({ Due: "today" });
      const summary = await client.pageMoveToSpace(template, space("Text"), null, null);
      expect(summary.propertiesCreated).toBe(0);
      expect((await schema("Text")).filter((def) => def.name === "Due")).toHaveLength(1);
      expect("templatePlaceholders" in (await client.pageGet(template))).toBe(false);
      expect((await client.pageGet(template)).template).toBe(true);
    });
  });
});

describe("a backup", () => {
  const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
  const PATH = "C:/backups/templates.atomikkb";

  it("restores the flag and the placeholders of the pages, and writes them as `template` and `templateValues`", async () => {
    const fixture = await setup({ security: { kitCreatedAt: T0 - 10_000, kitVerifiedAt: T0 - 9_000, kitKey: KEY } });
    const { client, tasks, sub, plain, due, created, owner, transport } = fixture;
    await client.pageSetTemplate(tasks, true);
    await client.pageSetTemplatePlaceholder(tasks, owner, "me");
    await client.pageSetTemplatePlaceholder(tasks, due, "today");
    await client.pageSetTemplatePlaceholder(sub, created, "now");
    await client.backupExport(PATH);

    // The state of the file: additive keys, only where set.
    const body = new TextDecoder().decode(transport.files.get(PATH)).split("\n")[2] ?? "";
    const state = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(body), (char) => char.charCodeAt(0)))) as {
      spaces: { pages: { id: string; template?: boolean; templateValues?: Record<string, string> }[] }[];
    };
    const stored = new Map(state.spaces.flatMap((space) => space.pages).map((page) => [page.id, page]));
    expect(stored.get(tasks)).toMatchObject({ template: true, templateValues: { [due]: "today", [owner]: "me" } });
    expect(stored.get(sub)?.templateValues).toEqual({ [created]: "now" });
    expect(stored.get(sub)?.template).toBeUndefined();
    expect(stored.get(plain)?.template).toBeUndefined();
    expect(stored.get(plain)?.templateValues).toBeUndefined();

    let counter = 0;
    const restored = new CoreClient(
      new MemoryTransport({ files: transport.files, newId: () => `11111111-0000-4000-8000-${String(++counter).padStart(12, "0")}` }),
    );
    await restored.backupImport(PATH, (await kitWordsFromEntropy(KEY)).join(" "));
    const page = await restored.pageGet(tasks);
    expect(page.template).toBe(true);
    expect(page.templatePlaceholders).toEqual([[due, "today"], [owner, "me"]]);
    expect((await restored.pageGet(sub)).templatePlaceholders).toEqual([[created, "now"]]);
    expect((await restored.pageGet(sub)).template).toBeUndefined();
    expect((await restored.pageGet(plain)).template).toBeUndefined();
    expect(titles(await restored.search("tomato"))).toEqual(["Plain tomato"]);
  });
});
