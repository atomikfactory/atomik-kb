// `PageDuplicate` in `MemoryTransport` (ADR-0040, "Duplicate a page with its sub-pages"): the twin of the core's
// `duplicate.rs`. The copy of a page and its live sub-pages after the page, with new ids, the same titles (the
// top copy's with " (copy)"), icons, values and content; links into the copied pages pointing at the copies;
// drawings under the new block ids; relations; attachments shared by id; one undo step; the refusals; the events.

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
import { portableBlockIds } from "./page-copy";
import { TAGS_PROPERTY_ID } from "./properties";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const DAY = 86_400_000;

/** The ids `setup` hands out, in order: the device is 1, the workspace 2, then the space and its pages as seeded. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Block ids written by the seeds. */
function bid(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Deterministic ids and a clock the test can move. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now++,
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

/** The live pages of a tree by title, all of them (a title repeats after a duplication). */
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

/** The text of the paragraphs of portable content, in order. */
function textsOf(content: JsonValue): string[] {
  const out: string[] = [];
  const visit = (node: JsonValue): void => {
    if (typeof node !== "object" || node === null || Array.isArray(node)) {
      return;
    }
    const children = Array.isArray(node.content) ? node.content : [];
    if (node.type === "paragraph" || node.type === "heading") {
      const text = children.map((child) => (typeof child === "object" && child !== null && !Array.isArray(child) && typeof child.text === "string" ? child.text : "")).join("");
      if (text !== "") {
        out.push(text);
      }
    }
    children.forEach(visit);
  };
  visit(content);
  return out;
}

interface Link {
  kind: string;
  pageId: string;
  blockId: string | undefined;
}

/** Every `pageRef` and `pageLink` of portable content. */
function linksOf(content: JsonValue): Link[] {
  const out: Link[] = [];
  const visit = (node: JsonValue): void => {
    if (typeof node !== "object" || node === null || Array.isArray(node)) {
      return;
    }
    const attrs = node.attrs;
    if ((node.type === "pageRef" || node.type === "pageLink") && typeof attrs === "object" && attrs !== null && !Array.isArray(attrs)) {
      out.push({
        kind: node.type,
        pageId: typeof attrs.pageId === "string" ? attrs.pageId : "",
        blockId: typeof attrs.blockId === "string" ? attrs.blockId : undefined,
      });
    }
    (Array.isArray(node.content) ? node.content : []).forEach(visit);
  };
  visit(content);
  return out;
}

const PROP = { estimate: id(0x101), mood: id(0x102), related: id(0x103), files: id(0x104) };
const OPTION = { happy: id(0x201), sad: id(0x202), urgent: id(0x203) };
const PICTURE = id(0x401);
const LOGO = id(0x402);
const FILED = id(0x403);
const SPARE = id(0x404);

const ELEMENT = { id: "r1", type: "rectangle", index: "a0", version: 1, isDeleted: false };
const TEXT_ELEMENT = { id: "t1", type: "text", index: "a1", version: 1, isDeleted: false, text: "drawn zebra", originalText: "drawn zebra" };

// Device 1, workspace 2, space 3, then Before 4, Alpha 5, Child 6, Deep 7, Deeper 8, Gone 9, Outside 10, After 11.
const ALPHA: JsonValue = {
  type: "doc",
  content: [
    {
      type: "paragraph",
      attrs: { id: bid(1) },
      content: [
        { type: "text", text: "alpha zebra " },
        { type: "pageRef", attrs: { id: bid(2), pageId: id(6) } },
        { type: "pageRef", attrs: { id: bid(3), pageId: id(10) } },
      ],
    },
    { type: "pageLink", attrs: { id: bid(4), pageId: id(6), blockId: bid(21) } },
    { type: "pageLink", attrs: { id: bid(5), pageId: id(5) } },
    { type: "whiteboard", attrs: { id: bid(6), height: 400 } },
    { type: "image", attrs: { id: bid(7), attachmentId: PICTURE, alt: "a cat" } },
  ],
  whiteboards: { [bid(6)]: { r1: ELEMENT, t1: TEXT_ELEMENT } },
};
const CHILD: JsonValue = {
  type: "doc",
  content: [
    {
      type: "paragraph",
      attrs: { id: bid(21) },
      content: [
        { type: "text", text: "child giraffe " },
        { type: "pageRef", attrs: { id: bid(22), pageId: id(5), blockId: bid(1) } },
      ],
    },
    { type: "image", attrs: { id: bid(23), attachmentId: PICTURE, alt: "" } },
  ],
};
const DEEPER: JsonValue = { type: "doc", content: [{ type: "paragraph", attrs: { id: bid(31) }, content: [{ type: "text", text: "deepest tiny" }] }] };

const SPACES: readonly SeedSpace[] = [
  {
    name: "Home",
    tagOptions: [{ id: OPTION.urgent, name: "urgent", color: "red" }],
    properties: [
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
      { id: PROP.related, name: "Related", kind: "relation", twoWay: true, backName: "Related by" },
      { id: PROP.files, name: "Files", kind: "files" },
    ],
    pages: [
      { title: "Before" },
      {
        title: "Alpha",
        icon: `attachment:${LOGO}`,
        favorite: true,
        tags: ["urgent"],
        content: ALPHA,
        properties: { Estimate: 3, Mood: "Happy", Related: ["Outside"], Files: [FILED] },
        children: [
          { title: "Child", content: CHILD, properties: { Estimate: 5, Related: ["Alpha"] } },
          { title: "Deep", children: [{ title: "Deeper", content: DEEPER }] },
          { title: "Gone" },
        ],
      },
      { title: "Outside" },
      { title: "After" },
    ],
  },
];

const ATTACHMENTS = [
  { attachmentId: PICTURE, name: "cat.png", mime: "image/png", bytes: new Uint8Array([1, 2, 3]) },
  { attachmentId: LOGO, name: "logo.png", mime: "image/png", bytes: new Uint8Array([4, 5]) },
  { attachmentId: FILED, name: "report.pdf", mime: "application/pdf", bytes: new Uint8Array([6]) },
  { attachmentId: SPARE, name: "spare.png", mime: "image/png", bytes: new Uint8Array([7]) },
];

async function fixture(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: SPACES, attachments: ATTACHMENTS, ...options });
  const [home] = await ctx.client.spaceList();
  if (home === undefined) {
    throw new Error("the space is missing");
  }
  const view = await ctx.client.treeGet(home.id);
  const first = (title: string): string => {
    const found = titled(view, title)[0];
    if (found === undefined) {
      throw new Error(`no live page titled ${title}`);
    }
    return found;
  };
  // The page "Gone" goes to the trash before anything is duplicated.
  await ctx.client.pageTrash(first("Gone"));
  ctx.events.length = 0;
  const tree = (): Promise<TreeView> => ctx.client.treeGet(home.id);
  // A page has one hit per block that matches: the pages are listed once.
  const search = async (query: string): Promise<string[]> => [...new Set((await ctx.client.search(query)).map((hit) => hit.pageId))].sort();
  return { ...ctx, home, first, tree, search };
}

function sorted(ids: string[]): string[] {
  return [...ids].sort();
}

describe("PageDuplicate", () => {
  it("copies a page and its live sub-pages right after it, with new ids, the same titles, icons and content", async () => {
    const { client, events, first, tree, home, search } = await fixture();
    const alpha = first("Alpha");
    const favoritesBefore = (await tree()).favorites;
    const liveBefore = (await client.spaceList())[0]?.pageCount ?? 0;

    const copy = await client.pageDuplicate(alpha);

    expect(copy.id).not.toBe(alpha);
    expect(copy).toMatchObject({ title: "Alpha (copy)", icon: `attachment:${LOGO}`, spaceId: home.id });
    expect(copy.parentId).toBeUndefined();
    expect(await client.pageGet(copy.id)).toEqual(copy);
    const view = await tree();
    expect(outline(view)).toEqual([
      "Before",
      "Alpha",
      "  Child",
      "  Deep",
      "    Deeper",
      "Alpha (copy)",
      "  Child",
      "  Deep",
      "    Deeper",
      "Outside",
      "After",
    ]);
    expect(view.trashed.map((entry) => entry.page.title)).toEqual(["Gone"]);
    expect(view.favorites).toEqual(favoritesBefore);
    expect((await client.spaceList())[0]?.pageCount).toBe(liveBefore + 4);

    const originals = subtreeOf(view, alpha);
    const copies = subtreeOf(view, copy.id);
    expect(copies).toHaveLength(4);
    expect(new Set([...originals, ...copies]).size).toBe(8);
    for (const [at, original] of originals.entries()) {
      const [o, c] = [await client.pageGet(original), await client.pageGet(copies[at] ?? "")];
      if (at > 0) {
        expect(c.title).toBe(o.title);
      }
    }
    expect((await client.pageGet(copies[1] ?? "")).parentId).toBe(copies[0]);
    expect((await client.pageGet(copies[3] ?? "")).parentId).toBe(copies[2]);

    // The content: the same text, new block ids, nothing shared.
    for (const [at, original] of originals.entries()) {
      const o = (await client.pageContentGet(original)).content;
      const c = (await client.pageContentGet(copies[at] ?? "")).content;
      expect(textsOf(c)).toEqual(textsOf(o));
      expect(portableBlockIds(c)).toHaveLength(portableBlockIds(o).length);
      const shared = portableBlockIds(c).filter((block) => portableBlockIds(o).includes(block));
      expect(shared).toEqual([]);
    }
    expect(await search("zebra")).toEqual(sorted([alpha, copies[0] ?? ""]));
    expect(await search("giraffe")).toEqual(sorted([originals[1] ?? "", copies[1] ?? ""]));
    expect(await search("tiny")).toEqual(sorted([originals[3] ?? "", copies[3] ?? ""]));
    const byTitle = (await client.search("Alpha", { scope: "Titles" })).map((hit) => hit.pageId).sort();
    expect(byTitle).toEqual(sorted([alpha, copy.id]));

    // The events: the structure, the space counts, each new document, in the core's order.
    expect(events.slice(0, 2)).toEqual([{ type: "StructureChanged", data: { spaceId: home.id } }, { type: "SpacesChanged" }]);
    for (const page of copies) {
      expect(events).toContainEqual({ type: "PageContentChanged", data: { pageId: page, source: "Core" } });
    }
    expect(events).toContainEqual({ type: "BacklinksChanged", data: { pageId: first("Outside") } });
  });

  it("places a copy after its page among the siblings of any parent, and copies a copy", async () => {
    const { client, first, tree, home } = await fixture();
    const child = first("Child");
    await client.pageDuplicate(child);
    expect(outline(await tree()).slice(1, 7)).toEqual(["Alpha", "  Child", "  Child (copy)", "  Deep", "    Deeper", "Outside"]);
    const again = await client.pageDuplicate(titled(await tree(), "Child (copy)")[0] ?? "");
    expect(again.title).toBe("Child (copy) (copy)");
    expect(again.parentId).toBe(first("Alpha"));
    expect(outline(await tree()).slice(1, 6)).toEqual(["Alpha", "  Child", "  Child (copy)", "  Child (copy) (copy)", "  Deep"]);
    // At the root: the last page.
    await client.pageDuplicate(first("After"));
    const lines = outline(await tree());
    expect(lines.slice(-2)).toEqual(["After", "After (copy)"]);
    expect((await client.spaceList())[0]?.id).toBe(home.id);
  });

  it("cuts a long title to fit and titles the copy of an untitled page", async () => {
    const { client, home } = await fixture();
    const long = await client.pageCreate({ spaceId: home.id, title: "x".repeat(497) });
    const copy = await client.pageDuplicate(long.id);
    expect(Array.from(copy.title)).toHaveLength(500);
    expect(copy.title.endsWith("xxx (copy)")).toBe(true);
    const untitled = await client.pageCreate({ spaceId: home.id, title: "" });
    expect((await client.pageDuplicate(untitled.id)).title).toBe("(copy)");
    const exact = await client.pageCreate({ spaceId: home.id, title: "y".repeat(493) });
    expect((await client.pageDuplicate(exact.id)).title).toBe(`${"y".repeat(493)} (copy)`);
    const spaced = await client.pageCreate({ spaceId: home.id, title: `${"b".repeat(491)}  tail` });
    expect((await client.pageDuplicate(spaced.id)).title).toBe(`${"b".repeat(491)} (copy)`);
  });

  it("points links into the copied subtree at the copies and keeps the others", async () => {
    const { client, first, tree, events } = await fixture();
    const [alpha, child, outside] = [first("Alpha"), first("Child"), first("Outside")];
    events.length = 0;
    const copyAlpha = (await client.pageDuplicate(alpha)).id;
    const copyChild = subtreeOf(await tree(), copyAlpha)[1] ?? "";

    const links = linksOf((await client.pageContentGet(copyAlpha)).content);
    const contentChild = (await client.pageContentGet(copyChild)).content;
    expect(links).toHaveLength(4);
    expect(links[0]).toEqual({ kind: "pageRef", pageId: copyChild, blockId: undefined });
    expect(links[1]).toEqual({ kind: "pageRef", pageId: outside, blockId: undefined });
    expect(links[2]?.kind).toBe("pageLink");
    expect(links[2]?.pageId).toBe(copyChild);
    expect(links[2]?.blockId).not.toBe(bid(21));
    expect(portableBlockIds(contentChild)).toContain(links[2]?.blockId);
    expect(links[3]).toEqual({ kind: "pageLink", pageId: copyAlpha, blockId: undefined });
    const back = linksOf(contentChild);
    expect(back).toHaveLength(1);
    expect(back[0]?.pageId).toBe(copyAlpha);
    expect(back[0]?.blockId).not.toBe(bid(1));
    expect(portableBlockIds((await client.pageContentGet(copyAlpha)).content)).toContain(back[0]?.blockId);

    // The originals are untouched.
    const original = linksOf((await client.pageContentGet(alpha)).content);
    expect(original[0]?.pageId).toBe(child);
    expect(original[2]?.blockId).toBe(bid(21));

    // The backlinks follow the copies.
    const sources = async (pageId: string): Promise<string[]> => [...new Set((await client.backlinksGet(pageId)).map((link) => link.pageId))].sort();
    expect(await sources(outside)).toEqual(sorted([alpha, copyAlpha]));
    expect(await sources(child)).toEqual([alpha]);
    expect(await sources(copyChild)).toEqual([copyAlpha]);
    expect(events).toContainEqual({ type: "BacklinksChanged", data: { pageId: outside } });
  });

  it("moves a drawing under the copy of its block and indexes its text for the copy", async () => {
    const { client, first, search } = await fixture();
    const alpha = first("Alpha");
    expect(await search("drawn")).toEqual([alpha]);
    const copy = await client.pageDuplicate(alpha);
    const original = (await client.pageContentGet(alpha)).content as { whiteboards: { [key: string]: JsonValue } };
    const copied = (await client.pageContentGet(copy.id)).content as { whiteboards: { [key: string]: JsonValue } };
    const keys = Object.keys(copied.whiteboards);
    expect(keys).toHaveLength(1);
    expect(keys[0]).not.toBe(bid(6));
    expect(portableBlockIds(copied as JsonValue)).toContain(keys[0]);
    expect(copied.whiteboards[keys[0] ?? ""]).toEqual(original.whiteboards[bid(6)]);
    expect(Object.keys(original.whiteboards)).toEqual([bid(6)]);
    expect(await search("drawn")).toEqual(sorted([alpha, copy.id]));
  });

  it("copies the values and keeps two-way relations paired, relating the copies among themselves", async () => {
    const { client, first, tree } = await fixture();
    const [alpha, child, outside] = [first("Alpha"), first("Child"), first("Outside")];
    const valueOf = async (pageId: string, property: string): Promise<PropertyValue | undefined> =>
      (await client.pagePropertiesGet(pageId)).find(([propertyId]) => propertyId === property)?.[1];
    const back = (await client.propertySchemaGet((await client.pageGet(alpha)).spaceId)).find((def) => def.name === "Related by")?.id ?? "";

    const copy = await client.pageDuplicate(alpha);
    const copyChild = subtreeOf(await tree(), copy.id)[1] ?? "";

    expect(await valueOf(copy.id, PROP.estimate)).toEqual({ kind: "number", value: 3 });
    expect(await valueOf(copy.id, PROP.mood)).toEqual({ kind: "select", value: OPTION.happy });
    expect(await valueOf(copy.id, TAGS_PROPERTY_ID)).toEqual({ kind: "multi_select", value: [OPTION.urgent] });
    expect(await valueOf(copyChild, PROP.estimate)).toEqual({ kind: "number", value: 5 });
    expect(await valueOf(copy.id, PROP.files)).toEqual({ kind: "files", value: [FILED] });
    expect(copy.properties?.length).toBeGreaterThanOrEqual(4);
    // A relation to a page outside is kept, and that page is related to both.
    expect(await valueOf(copy.id, PROP.related)).toEqual({ kind: "relation", value: [outside] });
    expect(await valueOf(outside, back)).toEqual({ kind: "relation", value: [alpha, copy.id] });
    // The relation inside relates the copies, from both sides; the originals are not related to them.
    expect(await valueOf(copyChild, PROP.related)).toEqual({ kind: "relation", value: [copy.id] });
    expect(await valueOf(copy.id, back)).toEqual({ kind: "relation", value: [copyChild] });
    expect(await valueOf(alpha, back)).toEqual({ kind: "relation", value: [child] });
    expect(await valueOf(child, PROP.related)).toEqual({ kind: "relation", value: [alpha] });
    expect(sorted(await client.pagesByProperty((await client.pageGet(alpha)).spaceId, PROP.estimate, { type: "HasValue" }))).toEqual(sorted([alpha, child, copy.id, copyChild]));
  });

  it("is refused for a page in the trash, an unknown page and a subtree of more than 1,000 pages, writing nothing", async () => {
    const { client, tree, events } = await fixture();
    const field = (error: CoreError) => error.toApiError();
    expect(field(await failure(client.pageDuplicate(id(0xffff))))).toMatchObject({ code: "NotFound" });
    const trashed = (await tree()).trashed[0]?.page.id ?? "";
    expect(field(await failure(client.pageDuplicate(trashed)))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    const before = outline(await tree());
    expect(events).toEqual([]);

    const big = setup({
      spaces: [{ name: "Big", pages: [{ title: "Top", children: Array.from({ length: 1000 }, (_, n) => ({ title: `p${String(n)}` })) }] }],
    });
    const [space] = await big.client.spaceList();
    const view = await big.client.treeGet(space?.id ?? "");
    const top = view.roots[0]?.page.id ?? "";
    big.events.length = 0;
    expect(field(await failure(big.client.pageDuplicate(top)))).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    expect(outline(await big.client.treeGet(space?.id ?? ""))).toHaveLength(1001);
    expect(big.events).toEqual([]);
    // One page fewer fits: 1,000 pages are copied.
    const first999 = view.roots[0]?.children[0]?.page.id ?? "";
    await big.client.pageTrash(first999);
    const copy = await big.client.pageDuplicate(top);
    expect(subtreeOf(await big.client.treeGet(space?.id ?? ""), copy.id)).toHaveLength(1000);

    expect(outline(await tree())).toEqual(before);
  });

  it("is one undo step, and a redo brings the pages, their content and their values back", async () => {
    const { client, first, tree, search, home } = await fixture();
    const alpha = first("Alpha");
    const original = outline(await tree());
    const copy = await client.pageDuplicate(alpha);
    const copies = subtreeOf(await tree(), copy.id);
    const copied = outline(await tree());
    expect((await tree()).undo.canUndo).toBe(true);

    const undone = await client.structureUndo(home.id);
    expect(outline(undone)).toEqual(original);
    expect(undone.undo.canRedo).toBe(true);
    for (const page of copies) {
      expect((await failure(client.pageGet(page))).toApiError()).toMatchObject({ code: "NotFound" });
    }
    expect(await search("zebra")).toEqual([alpha]);

    const redone = await client.structureRedo(home.id);
    expect(outline(redone)).toEqual(copied);
    expect(await search("zebra")).toEqual(sorted([alpha, copy.id]));
    expect(textsOf((await client.pageContentGet(copy.id)).content)[0]).toBe("alpha zebra ");
    expect((await client.pagePropertiesGet(copy.id)).length).toBeGreaterThanOrEqual(4);
  });

  it("shares the attachments: the copies reference them, and they stay while a copy does", async () => {
    const { client, first, tree, clock, home } = await fixture();
    const alpha = first("Alpha");
    const references = async (attachmentId: string): Promise<number> => (await client.attachmentGet(attachmentId)).references;
    expect(await references(PICTURE)).toBe(2);
    expect(await references(LOGO)).toBe(1);
    expect(await references(FILED)).toBe(1);

    const copy = await client.pageDuplicate(alpha);
    const copies = subtreeOf(await tree(), copy.id);
    // The copy's blocks, icon and files value reference the same attachments: no new ones.
    expect(await references(PICTURE)).toBe(4);
    expect(await references(LOGO)).toBe(2);
    expect(await references(FILED)).toBe(2);
    expect((await client.attachmentList(home.id)).map((info) => info.name).sort()).toEqual(["cat.png", "logo.png", "report.pdf", "spare.png"]);
    const content = (await client.pageContentGet(copies[1] ?? "")).content as { content: { [key: string]: JsonValue }[] };
    const image = content.content.find((block) => block.type === "image");
    expect(image?.attrs).toMatchObject({ attachmentId: PICTURE });
    expect(image?.attrs).not.toMatchObject({ id: bid(23) });

    // The original goes for good: the copy keeps the attachments alive past the retention.
    await client.pageTrash(alpha);
    await client.pagePurge(alpha);
    expect(await references(PICTURE)).toBe(2);
    expect(await references(LOGO)).toBe(1);
    expect(await references(FILED)).toBe(1);
    expect((await client.attachmentGet(LOGO)).unreferencedSince).toBeUndefined();
    clock.now += 40 * DAY;
    await client.attachmentGc();
    const names = (await client.attachmentList(home.id)).map((info) => info.name).sort();
    expect(names).toEqual(["cat.png", "logo.png", "report.pdf"]);
    expect((await client.attachmentRead(PICTURE)).length).toBe(3);
  });

  it("copies a page that has no document yet as an empty page", async () => {
    const { client, home } = await fixture();
    const page = await client.pageCreate({ spaceId: home.id, title: "Fresh" });
    const copy = await client.pageDuplicate(page.id);
    expect(textsOf((await client.pageContentGet(copy.id)).content)).toEqual([]);
    expect(portableBlockIds((await client.pageContentGet(copy.id)).content)).toHaveLength(1);
  });
});
