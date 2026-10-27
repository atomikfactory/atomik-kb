import { LoroDoc, LoroList, LoroMap, LoroText, VersionVector } from "loro-crdt";
import { describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { TreeView } from "./generated/TreeView";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { configurePageDoc, loroToPortable } from "./page-doc";
import type { SeedSpace } from "./structure";

const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";

/** The ids `setup` hands out, in order: the device is 1, then spaces and pages as created. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/** Block ids written by the test "editor" (the core indexes only valid ids, like the editor's UUIDs). */
function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${String(n).padStart(12, "0")}`;
}

/** Deterministic ids and clock, a client and the list of emitted events. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  let clock = 1_700_000_000_000;
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock++,
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

function idsOf(view: TreeView): (title: string) => string {
  const found = new Map<string, string>();
  const walk = (nodes: PageNode[]): void => {
    for (const node of nodes) {
      found.set(node.page.title, node.page.id);
      walk(node.children);
    }
  };
  walk(view.roots);
  return (title) => {
    const pageId = found.get(title);
    if (pageId === undefined) {
      throw new Error(`no live page titled ${title}`);
    }
    return pageId;
  };
}

// Device 1, space 2, then Alpha 3, Beta 4 (under Alpha), Gamma 5 in a first space, Work 6
// and Delta 7 in a second one.
const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [{ title: "Alpha", icon: "🅰", children: [{ title: "Beta" }] }, { title: "Gamma" }],
  },
  { name: "Work", pages: [{ title: "Delta" }] },
];

async function demo(options: MemoryTransportOptions = {}) {
  const ctx = setup({ spaces: SPACES, ...options });
  const spaces = await ctx.client.spaceList();
  const [personal, work] = spaces;
  if (personal === undefined || work === undefined) {
    throw new Error("the demo spaces are missing");
  }
  const personalIds = idsOf(await ctx.client.treeGet(personal.id));
  const workIds = idsOf(await ctx.client.treeGet(work.id));
  return {
    ...ctx,
    personal: personal.id,
    work: work.id,
    alpha: personalIds("Alpha"),
    beta: personalIds("Beta"),
    gamma: personalIds("Gamma"),
    delta: workIds("Delta"),
  };
}

// ----- an editor replica, as the UI's sync loop drives it -------------------------------

function isList(value: unknown): value is LoroList {
  return value instanceof LoroList;
}

function isMap(value: unknown): value is LoroMap {
  return value instanceof LoroMap;
}

/** The `children` list of the top-level block at `index`. */
function blockChildren(doc: LoroDoc, index: number): LoroList {
  const blocks = doc.getMap("doc").get("children");
  const block = isList(blocks) ? blocks.get(index) : undefined;
  const children = isMap(block) ? block.get("children") : undefined;
  if (!isList(children)) {
    throw new Error(`no block at ${String(index)}`);
  }
  return children;
}

function appendText(doc: LoroDoc, index: number, value: string): void {
  const children = blockChildren(doc, index);
  const last: unknown = children.length > 0 ? children.get(children.length - 1) : undefined;
  const text = last instanceof LoroText ? last : children.insertContainer(children.length, new LoroText());
  text.insert(text.length, value);
  doc.commit();
}

/** Appends an inline node (`pageRef` with `attributes`, or `hardBreak`) to a block. */
function appendInline(doc: LoroDoc, index: number, nodeName: string, attributes: Record<string, string>): void {
  const children = blockChildren(doc, index);
  const node = children.insertContainer(children.length, new LoroMap());
  node.set("nodeName", nodeName);
  const map = node.setContainer("attributes", new LoroMap());
  for (const [key, value] of Object.entries(attributes)) {
    map.set(key, value);
  }
  node.setContainer("children", new LoroList());
  doc.commit();
}

function appendPageRef(doc: LoroDoc, index: number, pageId: string): void {
  appendInline(doc, index, "pageRef", { pageId });
}

function appendParagraph(doc: LoroDoc, id: string, value: string): void {
  const blocks = doc.getMap("doc").get("children");
  if (!isList(blocks)) {
    throw new Error("no blocks");
  }
  const block = blocks.insertContainer(blocks.length, new LoroMap());
  block.set("nodeName", "paragraph");
  block.setContainer("attributes", new LoroMap()).set("id", id);
  const children = block.setContainer("children", new LoroList());
  if (value !== "") {
    children.insertContainer(0, new LoroText()).insert(0, value);
  }
  doc.commit();
}

/** Removes the last inline child of a block (for example the reference just added). */
function removeLast(doc: LoroDoc, index: number): void {
  const children = blockChildren(doc, index);
  children.delete(children.length - 1, 1);
  doc.commit();
}

/** Opens a page like the editor: import the core's snapshot, then send updates since the last acknowledged version. */
async function openEditor(transport: MemoryTransport, pageId: string) {
  const doc = new LoroDoc();
  configurePageDoc(doc);
  doc.import(await transport.docs.open(pageId));
  let acked = doc.oplogVersion();
  return {
    doc,
    async flush(): Promise<void> {
      const update = doc.export({ mode: "update", from: acked });
      await transport.docs.import(pageId, update);
      acked = doc.oplogVersion();
    },
  };
}

/** Opens `pageId`, lets `edit` change it and sends the change. */
async function write(transport: MemoryTransport, pageId: string, edit: (doc: LoroDoc) => void): Promise<void> {
  const editor = await openEditor(transport, pageId);
  edit(editor.doc);
  await editor.flush();
}

function emptyPage(paragraphId: string): JsonValue {
  return { type: "doc", content: [{ type: "paragraph", attrs: { id: paragraphId } }] };
}

const backlinksChanged = (pageId: string): Event => ({ type: "BacklinksChanged", data: { pageId } });
const structureChanged = (spaceId: string): Event => ({ type: "StructureChanged", data: { spaceId } });

describe("page documents (Transport.docs)", () => {
  it("are created on first open like the core: root plus one empty paragraph with a fresh id", async () => {
    const { transport, client, alpha, events } = await demo();
    const snapshot = await transport.docs.open(alpha);
    const doc = new LoroDoc();
    doc.import(snapshot);
    // Ids 1 to 7 went to the device, the spaces and the pages; the paragraph gets the next one.
    expect(loroToPortable(doc)).toEqual(emptyPage(id(8)));
    expect(doc.getMap("doc").get("attributes")).toBeInstanceOf(LoroMap);
    // The root's containers are mergeable (no operation id), so concurrent creations merge.
    const rootChildren = doc.getMap("doc").get("children");
    expect(isList(rootChildren) && !rootChildren.id.includes("@")).toBe(true);
    // The document exists from now on: opening again and reading return the same paragraph.
    const again = new LoroDoc();
    again.import(await transport.docs.open(alpha));
    expect(loroToPortable(again)).toEqual(emptyPage(id(8)));
    await expect(client.pageContentGet(alpha)).resolves.toEqual({ pageId: alpha, content: emptyPage(id(8)) });
    expect(events).toEqual([]);
  });

  it("are created by any first use, not only by open", async () => {
    const { client, beta } = await demo();
    await expect(client.pageContentGet(beta)).resolves.toEqual({ pageId: beta, content: emptyPage(id(8)) });
    await expect(client.pageExportMarkdown(beta)).resolves.toBe("\n");
  });

  it("are NotFound for unknown and purged pages; trashed pages still open", async () => {
    const { transport, client, alpha, beta, gamma } = await demo();
    await client.pageTrash(gamma);
    await expect(transport.docs.open(gamma)).resolves.toBeInstanceOf(Uint8Array);
    await client.pagePurge(alpha);
    for (const pageId of [UNKNOWN, alpha, beta]) {
      for (const call of [
        () => transport.docs.open(pageId),
        () => transport.docs.import(pageId, new Uint8Array([1])),
        () => transport.docs.export(pageId),
        () => client.pageContentGet(pageId),
        () => client.pageExportMarkdown(pageId),
        () => client.backlinksGet(pageId),
      ]) {
        expect((await failure(call())).toApiError()).toEqual({ code: "NotFound", message: "page not found" });
      }
    }
  });

  it("import applies editor updates, then emits PageContentChanged from the editor", async () => {
    const { transport, client, alpha, events } = await demo();
    const editor = await openEditor(transport, alpha);
    appendText(editor.doc, 0, "Hello world");
    appendParagraph(editor.doc, blockId(2), "Second block");
    await editor.flush();
    expect(events).toEqual([{ type: "PageContentChanged", data: { pageId: alpha, source: "Editor" } }]);
    const expected = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: id(8) }, content: [{ type: "text", text: "Hello world" }] },
        { type: "paragraph", attrs: { id: blockId(2) }, content: [{ type: "text", text: "Second block" }] },
      ],
    };
    await expect(client.pageContentGet(alpha)).resolves.toEqual({ pageId: alpha, content: expected });
    expect(loroToPortable(editor.doc)).toEqual(expected);
    await expect(client.pageExportMarkdown(alpha)).resolves.toBe("Hello world\n\nSecond block\n");
    await expect(client.search("second")).resolves.toMatchObject([{ pageId: alpha, blockId: blockId(2) }]);
  });

  it("emits the events before the import resolves", async () => {
    const { transport, alpha } = await demo();
    const editor = await openEditor(transport, alpha);
    const seen: string[] = [];
    transport.subscribe((event) => seen.push(event.type));
    appendText(editor.doc, 0, "x");
    const pending = editor.flush();
    expect(seen).toEqual(["PageContentChanged"]);
    await pending;
  });

  it("export returns what a version lacks, nothing when it is up to date, everything without a version", async () => {
    const { transport, alpha } = await demo();
    const editor = await openEditor(transport, alpha);
    await expect(transport.docs.export(alpha, editor.doc.oplogVersion().encode())).resolves.toEqual(new Uint8Array());

    // Another writer changes the page: the editor catches up with exactly that.
    transport.changePage(alpha, (doc) => {
      appendText(doc, 0, "from elsewhere");
    });
    const delta = await transport.docs.export(alpha, editor.doc.oplogVersion().encode());
    expect(delta.length).toBeGreaterThan(0);
    editor.doc.import(delta);
    expect(loroToPortable(editor.doc)).toEqual({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: id(8) }, content: [{ type: "text", text: "from elsewhere" }] }],
    });

    // A version that knows more than the core (unsent editor edits) also gets nothing.
    appendText(editor.doc, 0, "!");
    await expect(transport.docs.export(alpha, editor.doc.oplogVersion().encode())).resolves.toEqual(new Uint8Array());

    const everything = [await transport.docs.export(alpha), await transport.docs.export(alpha, new Uint8Array())];
    for (const update of everything) {
      const fresh = new LoroDoc();
      fresh.import(update);
      expect(loroToPortable(fresh)).toEqual({
        type: "doc",
        content: [{ type: "paragraph", attrs: { id: id(8) }, content: [{ type: "text", text: "from elsewhere" }] }],
      });
    }
  });

  it("export of a concurrent version sends what the other side lacks", async () => {
    const { transport, client, alpha } = await demo();
    const editor = await openEditor(transport, alpha);
    appendParagraph(editor.doc, blockId(1), "editor side");
    transport.changePage(alpha, (doc) => {
      appendParagraph(doc, blockId(2), "core side");
    });
    editor.doc.import(await transport.docs.export(alpha, editor.doc.oplogVersion().encode()));
    await editor.flush();
    const content = (await client.pageContentGet(alpha)).content;
    expect(loroToPortable(editor.doc)).toEqual(content);
    const blocks = (content as { content: { attrs: { id: string } }[] }).content.map((block) => block.attrs.id);
    expect(blocks.sort()).toEqual([id(8), blockId(1), blockId(2)].sort());
  });

  it("refuses empty, oversized and malformed updates and malformed versions, changing nothing", async () => {
    const { transport, client, alpha, events } = await demo();
    const before = await client.pageContentGet(alpha);
    expect((await failure(transport.docs.export(alpha, new Uint8Array([9, 9, 9])))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "not a valid version vector",
      details: { field: "since" },
    });
    expect((await failure(transport.docs.import(alpha, new Uint8Array()))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must not be empty",
      details: { field: "update" },
    });
    const rejected = await failure(transport.docs.import(alpha, new Uint8Array([1, 2, 3])));
    expect(rejected).toMatchObject({ code: "InvalidInput", details: { field: "update" } });
    expect(rejected.message).toMatch(/^rejected: /u);
    const huge = new Uint8Array(16 * 1024 * 1024 + 1);
    expect((await failure(transport.docs.import(alpha, huge))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "must be at most 16777216 bytes",
      details: { field: "update" },
    });
    // Size problems are found before the page is looked up, as in the core.
    expect(await failure(transport.docs.import(UNKNOWN, new Uint8Array()))).toMatchObject({ code: "InvalidInput" });
    await expect(client.pageContentGet(alpha)).resolves.toEqual(before);
    expect(events).toEqual([]);
  });

  it("close keeps the document: reopening shows the same content", async () => {
    const { transport, client, alpha } = await demo();
    const editor = await openEditor(transport, alpha);
    appendText(editor.doc, 0, "kept");
    await editor.flush();
    await expect(transport.docs.close(alpha)).resolves.toBeUndefined();
    await expect(transport.docs.close(UNKNOWN)).resolves.toBeUndefined();
    const reopened = await openEditor(transport, alpha);
    expect(loroToPortable(reopened.doc)).toEqual((await client.pageContentGet(alpha)).content);
    expect(loroToPortable(reopened.doc)).toEqual(loroToPortable(editor.doc));
  });

  it("reject like dispatch while the core is opening or failed, and on injected failures", async () => {
    const { transport, alpha } = await demo();
    transport.setCoreState("opening");
    expect(await failure(transport.docs.open(alpha))).toMatchObject({ code: "Internal", message: "core is starting" });
    transport.setCoreState({ failed: { code: "KeystoreUnavailable", message: "no keystore" } });
    expect((await failure(transport.docs.export(alpha))).toApiError()).toEqual({
      code: "KeystoreUnavailable",
      message: "no keystore",
    });
    transport.setCoreState("ready");
    transport.failNextDocs({ code: "Storage", message: "disk full" }, 2);
    expect(await failure(transport.docs.open(alpha))).toMatchObject({ code: "Storage" });
    expect(await failure(transport.docs.close(alpha))).toMatchObject({ code: "Storage" });
    await expect(transport.docs.open(alpha)).resolves.toBeInstanceOf(Uint8Array);
    // Failures injected for dispatch do not hit the document calls, and the other way round.
    transport.failNext({ code: "Storage", message: "disk full" });
    await expect(transport.docs.open(alpha)).resolves.toBeInstanceOf(Uint8Array);
    expect(await failure(new CoreClient(transport).appStatus())).toMatchObject({ code: "Storage" });
  });

  it("changePage notifies with the given source, Remote by default, and refuses unknown pages", async () => {
    const { transport, alpha, events } = await demo();
    transport.changePage(alpha, (doc) => {
      appendText(doc, 0, "a");
    });
    transport.changePage(
      alpha,
      (doc) => {
        appendText(doc, 0, "b");
      },
      "Core",
    );
    expect(events).toEqual([
      { type: "PageContentChanged", data: { pageId: alpha, source: "Remote" } },
      { type: "PageContentChanged", data: { pageId: alpha, source: "Core" } },
    ]);
    expect(() => {
      transport.changePage(UNKNOWN, () => undefined);
    }).toThrow(CoreError);
  });
});

describe("links and backlinks", () => {
  it("an import emits BacklinksChanged for the pages it starts or stops referencing, in page id order", async () => {
    const { transport, alpha, beta, gamma, events } = await demo();
    const editor = await openEditor(transport, alpha);
    const contentChanged: Event = { type: "PageContentChanged", data: { pageId: alpha, source: "Editor" } };

    appendPageRef(editor.doc, 0, gamma);
    appendPageRef(editor.doc, 0, beta);
    await editor.flush();
    expect(events).toEqual([contentChanged, backlinksChanged(beta), backlinksChanged(gamma)]);

    // Text, a second reference to the same page and a reference from another block change
    // nothing about which pages are referenced.
    events.length = 0;
    appendText(editor.doc, 0, " and ");
    appendPageRef(editor.doc, 0, beta);
    appendParagraph(editor.doc, blockId(2), "");
    appendPageRef(editor.doc, 1, beta);
    await editor.flush();
    expect(events).toEqual([contentChanged]);

    events.length = 0;
    removeLast(editor.doc, 1);
    blockChildren(editor.doc, 0).delete(0, 1); // the reference to Gamma
    editor.doc.commit();
    await editor.flush();
    expect(events).toEqual([contentChanged, backlinksChanged(gamma)]);
  });

  it("lists live referencing pages per block with the block's text and resolves titles when read", async () => {
    const { transport, client, personal, alpha, beta, gamma } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "See ");
      appendPageRef(doc, 0, beta);
      appendText(doc, 0, " and ");
      appendPageRef(doc, 0, gamma);
      appendPageRef(doc, 0, beta);
    });
    await expect(client.backlinksGet(beta)).resolves.toEqual([
      {
        pageId: alpha,
        spaceId: personal,
        title: "Alpha",
        icon: "🅰",
        blockId: id(8),
        text: "See Beta and GammaBeta",
      },
    ]);
    await client.pageRename(gamma, "Gamma ray");
    await expect(client.backlinksGet(beta)).resolves.toMatchObject([{ text: "See Beta and Gamma rayBeta" }]);
    await expect(client.backlinksGet(alpha)).resolves.toEqual([]);
  });

  it("show the block text as indexed: breaks as spaces, nothing for a blank block", async () => {
    const { transport, client, alpha, beta } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "one");
      appendInline(doc, 0, "hardBreak", {});
      appendText(doc, 0, "two ");
      appendPageRef(doc, 0, beta);
      appendParagraph(doc, blockId(2), "  ");
      appendPageRef(doc, 1, UNKNOWN);
      appendPageRef(doc, 1, beta);
    });
    await client.pageRename(beta, "   ");
    // Beta's title is now empty; the second block is only whitespace and references.
    await expect(client.backlinksGet(beta)).resolves.toMatchObject([
      { blockId: id(8), text: "one two " },
      { blockId: blockId(2), text: "" },
    ]);
  });

  it("drop trashed sources and bring them back on restore; purged sources are gone", async () => {
    const { transport, client, alpha, beta, gamma } = await demo();
    await write(transport, gamma, (doc) => {
      appendPageRef(doc, 0, beta);
    });
    await expect(client.backlinksGet(beta)).resolves.toHaveLength(1);
    await client.pageTrash(gamma);
    await expect(client.backlinksGet(beta)).resolves.toEqual([]);
    await client.pageRestore(gamma);
    await expect(client.backlinksGet(beta)).resolves.toMatchObject([{ pageId: gamma }]);
    // A trashed target keeps its backlinks (it opens too).
    await client.pageTrash(alpha);
    await expect(client.backlinksGet(beta)).resolves.toMatchObject([{ pageId: gamma }]);
    await client.pagePurge(gamma);
    await expect(client.backlinksGet(beta)).resolves.toEqual([]);
  });

  it("are reported changed when their sources are trashed, restored or purged, after StructureChanged", async () => {
    const { transport, client, personal, alpha, beta, gamma, delta, events } = await demo();
    await write(transport, beta, (doc) => {
      appendPageRef(doc, 0, delta);
      appendPageRef(doc, 0, gamma);
      appendPageRef(doc, 0, alpha);
    });
    events.length = 0;
    // Trashing Alpha moves Beta (its child) too: every page Beta references is told.
    await client.pageTrash(alpha);
    const all = [alpha, gamma, delta].map(backlinksChanged);
    expect(events).toEqual([structureChanged(personal), ...all]);
    events.length = 0;
    await client.pageRestore(alpha);
    expect(events).toEqual([structureChanged(personal), ...all]);
    events.length = 0;
    // Purging removes Alpha and Beta: only the pages that stay are told.
    await client.pagePurge(alpha);
    expect(events).toEqual([structureChanged(personal), backlinksChanged(gamma), backlinksChanged(delta)]);
  });

  it("are reported changed before StructureChanged when an undo removes their source", async () => {
    const { transport, client, personal, gamma, events } = await demo();
    const temp = await client.pageCreate({ spaceId: personal, title: "Temp" });
    await write(transport, temp.id, (doc) => {
      appendText(doc, 0, "temporary note ");
      appendPageRef(doc, 0, gamma);
    });
    events.length = 0;
    await client.structureUndo(personal);
    expect(events).toEqual([backlinksChanged(gamma), structureChanged(personal)]);
    await expect(client.backlinksGet(gamma)).resolves.toEqual([]);
    // Redo brings the page back with its content, as in the core (no event for that).
    events.length = 0;
    await client.structureRedo(personal);
    expect(events).toEqual([structureChanged(personal)]);
    await expect(client.backlinksGet(gamma)).resolves.toMatchObject([{ pageId: temp.id }]);
    await expect(client.search("temporary")).resolves.toMatchObject([{ pageId: temp.id }]);
  });

  it("order by source page id, then block id, and cut the text at 200 characters", async () => {
    const { transport, client, alpha, gamma, delta } = await demo();
    const long = "é".repeat(250);
    const [low, high] = ["aaaaaaaa-0000-4000-8000-000000000001", "ffffffff-0000-4000-8000-000000000001"];
    for (const source of [gamma, alpha]) {
      await write(transport, source, (doc) => {
        appendParagraph(doc, high, long);
        appendPageRef(doc, 1, delta);
        appendParagraph(doc, low, "short ");
        appendPageRef(doc, 2, delta);
      });
    }
    const found = await client.backlinksGet(delta);
    expect(found.map((link) => [link.pageId, link.blockId])).toEqual([
      [alpha, low],
      [alpha, high],
      [gamma, low],
      [gamma, high],
    ]);
    expect(found[0]?.text).toBe("short Delta");
    expect(found[1]?.text).toBe("é".repeat(200));
    expect(found.map((link) => link.icon)).toEqual(["🅰", "🅰", undefined, undefined]);
  });
});

describe("search", () => {
  it("validates the query like the core", async () => {
    const { client } = await demo();
    const cases = [
      ["", "must not be empty"],
      ["   ", "must not be empty"],
      ["x".repeat(201), "must be at most 200 characters"],
      ["a\u0007b", "must not contain control characters"],
    ] as const;
    for (const [query, message] of cases) {
      expect((await failure(client.search(query))).toApiError()).toEqual({
        code: "InvalidInput",
        message,
        details: { field: "query" },
      });
    }
    await expect(client.search(` ${"x".repeat(200)} `)).resolves.toEqual([]);
    expect(await failure(client.search("a", { limit: -1 }))).toMatchObject({ details: { field: "limit" } });
    expect(await failure(client.search("a", { limit: 1.5 }))).toMatchObject({ details: { field: "limit" } });
    expect((await failure(client.search("a", { spaceId: UNKNOWN }))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
  });

  it("finds titles and blocks: title hits first, each in display order, with snippets and highlights", async () => {
    const { transport, client, personal, work, alpha, beta, delta } = await demo();
    await write(transport, delta, (doc) => {
      appendText(doc, 0, "Notes about the alpha release");
    });
    await write(transport, beta, (doc) => {
      appendText(doc, 0, "Alphabet soup");
    });
    await expect(client.search("ALPH")).resolves.toEqual([
      {
        pageId: alpha,
        spaceId: personal,
        title: "Alpha",
        icon: "🅰",
        snippet: "Alpha",
        highlights: [{ start: 0, end: 5 }],
      },
      {
        pageId: beta,
        spaceId: personal,
        title: "Beta",
        blockId: id(9),
        snippet: "Alphabet soup",
        highlights: [{ start: 0, end: 8 }],
      },
      {
        pageId: delta,
        spaceId: work,
        title: "Delta",
        blockId: id(8),
        snippet: "Notes about the alpha release",
        highlights: [{ start: 16, end: 21 }],
      },
    ]);
    await expect(client.search("alph", { scope: "Titles" })).resolves.toMatchObject([{ pageId: alpha }]);
    await expect(client.search("alph", { spaceId: work })).resolves.toMatchObject([{ pageId: delta }]);
  });

  it("matches a page when every word is in its title or its blocks, ignores case and diacritics, matches prefixes", async () => {
    const { transport, client, alpha } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "Café crème au Zürichsee");
      appendParagraph(doc, blockId(2), "zurich");
    });
    await expect(client.search("cafe CREME")).resolves.toMatchObject([{ blockId: id(8) }]);
    await expect(client.search("zur")).resolves.toMatchObject([{ blockId: id(8) }, { blockId: blockId(2) }]);
    // "alpha" is in the title, "cafe" in a block: no single row has both, the page matches with one hit
    // (the title, which comes first on a tie).
    await expect(client.search("alpha cafe")).resolves.toMatchObject([{ pageId: alpha, snippet: "Alpha" }]);
    await expect(client.search("afe")).resolves.toEqual([]);
    await expect(client.search("!!! ---")).resolves.toEqual([]);
    // Operators and FTS5 syntax are not special.
    await expect(client.search('"café" AND -crème*')).resolves.toMatchObject([{ blockId: id(8) }]);
  });

  it("counts page references as the referenced page's current title", async () => {
    const { transport, client, alpha, gamma } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "See ");
      appendPageRef(doc, 0, gamma);
    });
    await expect(client.search("gamma")).resolves.toMatchObject([
      { pageId: gamma, snippet: "Gamma" },
      { pageId: alpha, blockId: id(8), snippet: "See Gamma", highlights: [{ start: 4, end: 9 }] },
    ]);
    await client.pageRename(gamma, "Omega");
    await expect(client.search("gamma")).resolves.toEqual([]);
    await expect(client.search("omega")).resolves.toMatchObject([
      { pageId: gamma },
      { pageId: alpha, snippet: "See Omega" },
    ]);
  });

  it("leaves trashed pages out, and their subtrees", async () => {
    const { client, alpha } = await demo();
    await client.pageTrash(alpha);
    await expect(client.search("alpha")).resolves.toEqual([]);
    await expect(client.search("beta")).resolves.toEqual([]);
    await expect(client.search("gamma")).resolves.toHaveLength(1);
    await client.pageRestore(alpha);
    await expect(client.search("beta")).resolves.toHaveLength(1);
  });

  it("clamps limit to 1..=100 and defaults to 20", async () => {
    const pages = Array.from({ length: 120 }, (_, i) => ({ title: `Note ${String(i)}` }));
    const { client } = setup({ spaces: [{ name: "Many", pages }] });
    await expect(client.search("note")).resolves.toHaveLength(20);
    await expect(client.search("note", { limit: 0 })).resolves.toHaveLength(1);
    await expect(client.search("note", { limit: 5 })).resolves.toHaveLength(5);
    await expect(client.search("note", { limit: 1000 })).resolves.toHaveLength(100);
    const titles = (await client.search("note", { limit: 3 })).map((hit) => hit.title);
    expect(titles).toEqual(["Note 0", "Note 1", "Note 2"]);
  });

  it("refuses an unknown scope from untyped callers", async () => {
    const { transport } = await demo();
    const request = { type: "Search", data: { query: "a", scope: "Everything" } } as unknown as Parameters<
      MemoryTransport["dispatch"]
    >[0];
    expect(await failure(transport.dispatch(request))).toMatchObject({ details: { field: "scope" } });
  });
});

describe("content requests", () => {
  it("PageExportMarkdown renders the page with references resolved to current titles", async () => {
    const { transport, client, alpha, gamma } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "Go to ");
      appendPageRef(doc, 0, gamma);
      appendPageRef(doc, 0, UNKNOWN);
    });
    await expect(client.pageExportMarkdown(alpha)).resolves.toBe(`Go to [[Gamma]][[${UNKNOWN}]]\n`);
    await client.pageTrash(gamma);
    await expect(client.pageExportMarkdown(alpha)).resolves.toBe(`Go to [[Gamma]][[${UNKNOWN}]]\n`);
    await client.pagePurge(gamma);
    await expect(client.pageExportMarkdown(alpha)).resolves.toBe(`Go to [[${gamma}]][[${UNKNOWN}]]\n`);
  });

  it("IndexRebuild answers Ok, leaves results unchanged and refuses unknown spaces", async () => {
    const { transport, client, personal, work, alpha, beta, events } = await demo();
    await write(transport, alpha, (doc) => {
      appendText(doc, 0, "rebuild me ");
      appendPageRef(doc, 0, beta);
    });
    events.length = 0;
    const hits = await client.search("rebuild");
    const links = await client.backlinksGet(beta);
    await expect(client.indexRebuild(personal)).resolves.toBeUndefined();
    await expect(client.indexRebuild(work)).resolves.toBeUndefined();
    await expect(client.indexRebuild()).resolves.toBeUndefined();
    await expect(client.search("rebuild")).resolves.toEqual(hits);
    await expect(client.backlinksGet(beta)).resolves.toEqual(links);
    expect(events).toEqual([]);
    expect((await failure(client.indexRebuild(UNKNOWN))).toApiError()).toEqual({
      code: "NotFound",
      message: "space not found",
    });
  });

  it("PagePurge hides the documents of the whole subtree, and an undone purge brings the content back", async () => {
    const { transport, client, personal, alpha, beta } = await demo();
    for (const pageId of [alpha, beta]) {
      await write(transport, pageId, (doc) => {
        appendText(doc, 0, "precious");
      });
    }
    await expect(client.search("precious")).resolves.toHaveLength(2);
    await client.pagePurge(alpha);
    await expect(client.search("precious")).resolves.toEqual([]);
    await client.structureUndo(personal);
    await expect(client.search("precious")).resolves.toMatchObject([{ pageId: alpha }, { pageId: beta }]);
    await expect(client.pageContentGet(beta)).resolves.toMatchObject({
      content: { content: [{ content: [{ text: "precious" }] }] },
    });
  });
});

describe("seeded content", () => {
  const [heading, paragraph] = [blockId(1), blockId(2)];
  const content: JsonValue = {
    type: "doc",
    content: [
      { type: "heading", attrs: { id: heading, level: 1 }, content: [{ type: "text", text: "Seeded heading" }] },
      {
        type: "paragraph",
        attrs: { id: paragraph },
        content: [
          { type: "text", text: "Links to " },
          // Pages are created in order after the device (1) and the space (2): Target is 4.
          { type: "pageRef", attrs: { id: blockId(3), pageId: id(4) } },
        ],
      },
    ],
  };

  it("is in place from the start, indexed, without events", async () => {
    const { client, events } = setup({
      spaces: [{ name: "S", pages: [{ title: "Source", content }, { title: "Target" }] }],
    });
    const [space] = await client.spaceList();
    const ids = idsOf(await client.treeGet(space?.id ?? ""));
    expect(ids("Target")).toBe(id(4));
    await expect(client.pageContentGet(ids("Source"))).resolves.toEqual({ pageId: ids("Source"), content });
    await expect(client.search("seeded")).resolves.toMatchObject([{ pageId: ids("Source"), blockId: heading }]);
    await expect(client.backlinksGet(id(4))).resolves.toMatchObject([
      { pageId: ids("Source"), blockId: paragraph, text: "Links to Target" },
    ]);
    await expect(client.pageExportMarkdown(ids("Source"))).resolves.toBe("# Seeded heading\n\nLinks to [[Target]]\n");
    expect(events).toEqual([]);
  });

  it("opens in the editor as the seeded document", async () => {
    const { transport, client } = setup({ spaces: [{ name: "S", pages: [{ title: "Source", content }] }] });
    const [space] = await client.spaceList();
    const source = idsOf(await client.treeGet(space?.id ?? ""))("Source");
    const editor = await openEditor(transport, source);
    expect(loroToPortable(editor.doc)).toEqual(content);
  });

  it("must be a portable page", () => {
    const spaces = [{ name: "S", pages: [{ title: "Bad", content: { type: "paragraph" } }] }];
    expect(() => new MemoryTransport({ spaces })).toThrow("the root node must be a doc");
  });
});

describe("openExternal", () => {
  it("is absent by default so the UI falls back to window.open", async () => {
    const { transport, client } = setup();
    expect(transport.openExternal).toBeUndefined();
    await expect(client.openExternal("https://example.com")).resolves.toBe(false);
  });

  it("forwards to the function given in the options", async () => {
    const opened = vi.fn((url: string) => Promise.resolve(void url));
    const { client } = setup({ openExternal: opened });
    await expect(client.openExternal("https://example.com/a")).resolves.toBe(true);
    expect(opened).toHaveBeenCalledExactlyOnceWith("https://example.com/a");
  });
});

describe("version vectors from the editor", () => {
  it("round-trip through encode and decode as the transport expects", () => {
    const doc = new LoroDoc();
    doc.getText("t").insert(0, "x");
    doc.commit();
    const encoded = doc.oplogVersion().encode();
    expect(VersionVector.decode(encoded).compare(doc.oplogVersion())).toBe(0);
  });
});

describe("first images (PageFirstImages, ADR-0054)", () => {
  /** Attachment ids the seeded blocks name (nothing needs the attachments themselves). */
  const att = (n: number): string => `cccccccc-0000-4000-8000-${String(n).padStart(12, "0")}`;
  const para = (n: number): JsonValue => ({ type: "paragraph", attrs: { id: blockId(n) } });
  const image = (n: number, attachment: number): JsonValue => ({ type: "image", attrs: { id: blockId(n), attachmentId: att(attachment), alt: "" } });
  const file = (n: number, attachment: number): JsonValue => ({ type: "file", attrs: { id: blockId(n), attachmentId: att(attachment) } });
  const gallery = (n: number, items: JsonValue[]): JsonValue => ({ type: "gallery", attrs: { id: blockId(n), items } });
  const entry = (attachment: number): JsonValue => ({ attachmentId: att(attachment) });
  const page = (content: JsonValue[]): JsonValue => ({ type: "doc", content });

  async function album() {
    const ctx = setup({
      spaces: [
        {
          name: "S",
          pages: [
            { title: "Plain", content: page([para(1)]) },
            { title: "Photo", content: page([para(2), image(3, 1), image(4, 2)]) },
            // A file is no picture; the image in the callout inside the toggle comes before the later one.
            {
              title: "Deep",
              content: page([
                file(5, 3),
                {
                  type: "details",
                  attrs: { id: blockId(6) },
                  content: [
                    { type: "detailsSummary", attrs: { id: blockId(7) } },
                    { type: "detailsContent", attrs: { id: blockId(8) }, content: [{ type: "callout", attrs: { id: blockId(9) }, content: [image(10, 4)] }] },
                  ],
                },
                image(11, 5),
              ]),
            },
            { title: "Album", content: page([gallery(12, [entry(6), entry(7)]), image(13, 8)]) },
            { title: "Image first", content: page([image(14, 9), gallery(15, [entry(10)])]) },
            { title: "Damaged gallery", content: page([gallery(16, [{ caption: "no picture" }, entry(11)]), gallery(17, [])]) },
            { title: "Only a file", content: page([file(18, 12)]) },
            { title: "Never opened" },
            { title: "Gone", content: page([image(19, 13)]) },
          ],
        },
        { name: "Other", pages: [{ title: "Elsewhere", content: page([image(20, 14)]) }] },
      ],
    });
    const [space, other] = await ctx.client.spaceList();
    const ids = idsOf(await ctx.client.treeGet(space?.id ?? ""));
    const elsewhere = idsOf(await ctx.client.treeGet(other?.id ?? ""))("Elsewhere");
    return { ...ctx, space: space?.id ?? "", other: other?.id ?? "", ids, elsewhere };
  }

  it("answers the attachment of the first picture in document order, in the order asked", async () => {
    const { client, space, ids } = await album();
    const asked = ["Album", "Plain", "Photo", "Deep", "Never opened", "Image first", "Damaged gallery", "Only a file"].map(ids);
    await expect(client.pageFirstImages(space, asked)).resolves.toEqual([
      { pageId: ids("Album"), attachmentId: att(6) },
      { pageId: ids("Photo"), attachmentId: att(1) },
      { pageId: ids("Deep"), attachmentId: att(4) },
      { pageId: ids("Image first"), attachmentId: att(9) },
      { pageId: ids("Damaged gallery"), attachmentId: att(11) },
    ]);
  });

  it("leaves out unknown pages and pages of another space, asks a page once and answers trashed pages too", async () => {
    const { client, space, ids, elsewhere } = await album();
    await client.pageTrash(ids("Gone"));
    const unknown = "00000000-0000-4000-8000-0000000000fe";
    await expect(client.pageFirstImages(space, [elsewhere, unknown, ids("Photo"), ids("Photo"), ids("Gone")])).resolves.toEqual([
      { pageId: ids("Photo"), attachmentId: att(1) },
      { pageId: ids("Gone"), attachmentId: att(13) },
    ]);
    await expect(client.pageFirstImages(space, [])).resolves.toEqual([]);
  });

  it("follows the document: an image added in the editor is the first one when it comes first", async () => {
    const { transport, client, space, ids } = await album();
    const plain = ids("Plain");
    await expect(client.pageFirstImages(space, [plain])).resolves.toEqual([]);
    await write(transport, plain, (doc) => {
      const blocks = doc.getMap("doc").get("children");
      if (!isList(blocks)) {
        throw new Error("no blocks");
      }
      const block = blocks.insertContainer(blocks.length, new LoroMap());
      block.set("nodeName", "image");
      const attributes = block.setContainer("attributes", new LoroMap());
      attributes.set("id", blockId(30));
      attributes.set("attachmentId", att(30));
      block.setContainer("children", new LoroList());
      doc.commit();
    });
    await expect(client.pageFirstImages(space, [plain])).resolves.toEqual([{ pageId: plain, attachmentId: att(30) }]);
  });

  it("is refused for an unknown space and for more than 500 pages", async () => {
    const { client, space, ids } = await album();
    expect((await failure(client.pageFirstImages(UNKNOWN, [ids("Photo")]))).code).toBe("NotFound");
    const many = Array.from({ length: 501 }, (_, n) => id(1000 + n));
    const refused = await failure(client.pageFirstImages(space, many));
    expect(refused).toMatchObject({ code: "InvalidInput", details: { field: "pageIds" } });
    await expect(client.pageFirstImages(space, many.slice(0, 500))).resolves.toEqual([]);
  });
});
