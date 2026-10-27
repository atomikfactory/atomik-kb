// Page history in `MemoryTransport` (ADR-0057, `docs/architecture/page-history.md`): the twin of the core's `history.rs`.
// Version ids, the journal's sessions (author, gap, title), the undated runs, the list's order and paging, the content at a
// version, restore (content, title, events, undo by restoring the previous version) and the errors.

import { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { PageNode } from "./generated/PageNode";
import type { PageVersion } from "./generated/PageVersion";
import type { JsonValue } from "./generated/serde_json/JsonValue";
import { kitWordsFromEntropy } from "./kit";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { SESSION_GAP_MS, undatedVersions, versionId } from "./memory-history";
import { configurePageDoc, loroToPortable } from "./page-doc";
import type { SeedSpace } from "./structure";

const T0 = 1_700_000_000_000;
const MINUTE = 60_000;
const UNKNOWN = "00000000-0000-4000-8000-0000000000ff";
const KEY = Uint8Array.from({ length: 32 }, (_, index) => index);
/** Matches the id of a version: `<peer>@<counter>`. */
const AN_ID = expect.stringMatching(/^\d+@\d+$/u) as unknown;

/** The ids `setup` hands out, in order: the device is 1, then spaces and pages as seeded. */
function id(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Block ids written by the seeds and the test "editor". */
function blockId(n: number): string {
  return `bbbbbbbb-0000-4000-8000-${n.toString(16).padStart(12, "0")}`;
}

/** Deterministic ids and a clock the test moves. */
function setup(options: MemoryTransportOptions = {}) {
  let counter = 0;
  const clock = { now: T0 };
  const transport = new MemoryTransport({
    newId: () => id(++counter),
    now: () => clock.now,
    ...options,
  });
  const client = new CoreClient(transport);
  const events: Event[] = [];
  transport.subscribe((event) => events.push(event));
  return { transport, client, events, clock };
}

/** The id of the page titled `title` (live, any depth). */
async function pageNamed(client: CoreClient, title: string): Promise<string> {
  const search = (nodes: PageNode[]): string | undefined => {
    for (const node of nodes) {
      const found = node.page.title === title ? node.page.id : search(node.children);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  };
  for (const space of await client.spaceList()) {
    const found = search((await client.treeGet(space.id)).roots);
    if (found !== undefined) {
      return found;
    }
  }
  throw new Error(`no page titled ${title}`);
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

function paragraph(blockNumber: number, text: string): JsonValue {
  return { type: "paragraph", attrs: { id: blockId(blockNumber) }, content: [{ type: "text", text }] };
}

const ONE: JsonValue = { type: "doc", content: [paragraph(1, "One")] };

// Device 1, space 2, Alpha 3, Beta 4 (under Alpha), Gamma 5; Work 6 and Delta 7.
const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [{ title: "Alpha", content: ONE, children: [{ title: "Beta" }] }, { title: "Gamma" }],
  },
  { name: "Work", pages: [{ title: "Delta" }] },
];
const ALPHA = id(3);
const BETA = id(4);
const GAMMA = id(5);
const DEVICE = id(1);

function demo(options: MemoryTransportOptions = {}) {
  return setup({ spaces: SPACES, ...options });
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

function appendParagraph(doc: LoroDoc, blockNumber: number, value: string): void {
  const blocks = doc.getMap("doc").get("children");
  if (!isList(blocks)) {
    throw new Error("no blocks");
  }
  const block = blocks.insertContainer(blocks.length, new LoroMap());
  block.set("nodeName", "paragraph");
  block.setContainer("attributes", new LoroMap()).set("id", blockId(blockNumber));
  const children = block.setContainer("children", new LoroList());
  if (value !== "") {
    children.insertContainer(0, new LoroText()).insert(0, value);
  }
  doc.commit();
}

function appendPageRef(doc: LoroDoc, index: number, pageId: string): void {
  const children = blockChildren(doc, index);
  const node = children.insertContainer(children.length, new LoroMap());
  node.set("nodeName", "pageRef");
  node.setContainer("attributes", new LoroMap()).set("pageId", pageId);
  node.setContainer("children", new LoroList());
  doc.commit();
}

/** Removes the top-level block at `index`. */
function removeBlock(doc: LoroDoc, index: number): void {
  const blocks = doc.getMap("doc").get("children");
  if (!isList(blocks)) {
    throw new Error("no blocks");
  }
  blocks.delete(index, 1);
  doc.commit();
}

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
    async flush(): Promise<Uint8Array> {
      const update = doc.export({ mode: "update", from: acked });
      await transport.docs.import(pageId, update);
      acked = doc.oplogVersion();
      return update;
    },
    /** Takes what the core changed since (a core write, a restore), as the editor session does on `PageContentChanged`. */
    async pull(): Promise<void> {
      doc.import(await transport.docs.export(pageId, doc.oplogVersion().encode()));
      acked = doc.oplogVersion();
    },
  };
}

/** A new editor replica opens `pageId`, lets `edit` change it and sends the change. */
async function write(transport: MemoryTransport, pageId: string, edit: (doc: LoroDoc) => void): Promise<void> {
  const editor = await openEditor(transport, pageId);
  edit(editor.doc);
  await editor.flush();
}

/** The text of the paragraphs of portable content, in order. */
function textsOf(content: JsonValue): string[] {
  const out: string[] = [];
  const visit = (node: JsonValue): void => {
    if (typeof node !== "object" || node === null || Array.isArray(node)) {
      return;
    }
    const children = Array.isArray(node.content) ? node.content : [];
    if (node.type === "paragraph") {
      out.push(
        children
          .map((child) =>
            typeof child === "object" && child !== null && !Array.isArray(child) && typeof child.text === "string"
              ? child.text
              : "",
          )
          .join(""),
      );
    }
    children.forEach(visit);
  };
  visit(content);
  return out;
}

async function texts(client: CoreClient, pageId: string): Promise<string[]> {
  return textsOf((await client.pageContentGet(pageId)).content);
}

const contentChanged = (pageId: string, source: "Core" | "Editor" | "Remote" = "Core"): Event => ({
  type: "PageContentChanged",
  data: { pageId, source },
});
const backlinksChanged = (pageId: string): Event => ({ type: "BacklinksChanged", data: { pageId } });
const structureChanged = (spaceId: string): Event => ({ type: "StructureChanged", data: { spaceId } });

/** The ids of the versions of a list, newest first. */
function ids(versions: readonly PageVersion[]): string[] {
  return versions.map((version) => version.id);
}

/** The versions of a page (all of them: a short list). */
async function versionsOf(client: CoreClient, pageId: string): Promise<PageVersion[]> {
  return (await client.pageHistoryList(pageId)).versions;
}

/** Writes `count` core sessions, each past the gap of the previous one: one version each (a letter on the first line). */
function sessions(transport: MemoryTransport, clock: { now: number }, pageId: string, count: number): void {
  for (let n = 0; n < count; n += 1) {
    clock.now += SESSION_GAP_MS + 1;
    transport.changePage(
      pageId,
      (doc) => {
        const children = blockChildren(doc, 0);
        const text = children.get(0);
        if (!(text instanceof LoroText)) {
          throw new Error("no text on the first line");
        }
        text.insert(text.length, "x");
      },
      "Core",
    );
  }
}

// ----- ids ------------------------------------------------------------------------------

describe("version ids", () => {
  it("write each head as peer@counter, peers sorted by their number, joined with commas", () => {
    expect(versionId([{ peer: "5", counter: 0 }])).toBe("5@0");
    expect(versionId([{ peer: "8291736450123", counter: 57 }, { peer: "991827364", counter: 1203 }])).toBe(
      "991827364@1203,8291736450123@57",
    );
    expect(versionId([{ peer: "18446744073709551615", counter: 2 }, { peer: "9", counter: 7 }, { peer: "9", counter: 3 }])).toBe(
      "9@3,9@7,18446744073709551615@2",
    );
  });

  it("are the frontiers of the document: the same text on the editor's replica and the core's", async () => {
    const { transport, client } = demo();
    const editor = await openEditor(transport, ALPHA);
    appendParagraph(editor.doc, 2, "Two");
    await editor.flush();
    const [current] = await versionsOf(client, ALPHA);
    expect(current?.id).toBe(versionId(editor.doc.oplogFrontiers()));
    expect(current?.current).toBe(true);
    expect(current?.id).toMatch(/^\d+@\d+$/u);
  });

  it("have several heads after concurrent writes, and name both of them", async () => {
    const { transport, client } = demo();
    const editor = await openEditor(transport, ALPHA);
    // The core writes while the editor, which has not pulled, writes too.
    transport.changePage(ALPHA, (doc) => appendParagraph(doc, 2, "From the core"), "Core");
    appendParagraph(editor.doc, 3, "From the editor");
    await editor.flush();
    const [current] = await versionsOf(client, ALPHA);
    const heads = (current?.id ?? "").split(",");
    expect(heads).toHaveLength(2);
    expect(current?.current).toBe(true);
    const peers = heads.map((head) => BigInt(head.split("@")[0] ?? "0"));
    expect(peers[0] ?? 0n).toBeLessThan(peers[1] ?? 0n);
    await editor.pull();
    expect(current?.id).toBe(versionId(editor.doc.oplogFrontiers()));
    // Both writes are in the version (their order is the concurrent merge's).
    expect(textsOf((await client.pageHistoryGet(ALPHA, current?.id ?? "")).content).sort()).toEqual([
      "From the core",
      "From the editor",
      "One",
    ]);
  });
});

// ----- the journal and the list ---------------------------------------------------------

describe("the list", () => {
  it("is empty for a page that has no document yet", async () => {
    const { client } = demo();
    await expect(client.pageHistoryList(GAMMA)).resolves.toEqual({ pageId: GAMMA, versions: [], more: false });
  });

  it("lists the history of a page nobody edited as one undated version, which is the current one", async () => {
    const { transport, client } = demo();
    const [seeded] = await versionsOf(client, ALPHA);
    expect(seeded).toEqual({ id: AN_ID, kind: "edit", current: true });
    expect(seeded).not.toHaveProperty("startedAt");
    expect(seeded).not.toHaveProperty("author");
    expect(seeded).not.toHaveProperty("title");
    // A page opened for the first time has its first change undated too.
    await transport.docs.open(GAMMA);
    const opened = await versionsOf(client, GAMMA);
    expect(opened).toEqual([{ id: AN_ID, kind: "edit", current: true }]);
  });

  it("dates an editing session: its times, this device as the author, the title; the older history follows undated", async () => {
    const { transport, client, clock } = demo();
    const undated = (await versionsOf(client, ALPHA))[0];
    clock.now = T0 + 5_000;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    const { versions, more } = await client.pageHistoryList(ALPHA);
    expect(more).toBe(false);
    expect(versions).toHaveLength(2);
    expect(versions[0]).toEqual({
      id: AN_ID,
      kind: "edit",
      startedAt: T0 + 5_000,
      endedAt: T0 + 5_000,
      author: { deviceId: DEVICE, thisDevice: true },
      title: "Alpha",
      current: true,
    });
    // The history from before the session stays, undated and no longer current.
    expect(versions[1]).toEqual({ id: undated?.id, kind: "edit" });
  });

  it("puts the account of the kit in the author", async () => {
    const { transport, client } = demo({ workspaceName: "Home", security: { kitCreatedAt: T0, accountId: id(0xaa) } });
    const alpha = await pageNamed(client, "Alpha");
    await write(transport, alpha, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    expect((await versionsOf(client, alpha))[0]?.author).toEqual({
      deviceId: DEVICE,
      accountId: id(0xaa),
      thisDevice: true,
    });
  });

  it("continues a session while writes come within ten minutes, and starts another after", async () => {
    const { transport, client, clock } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    clock.now = T0 + 4 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    // Exactly the gap after the end still continues it.
    clock.now = T0 + 4 * MINUTE + SESSION_GAP_MS;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 4, "Four");
    });
    let versions = await versionsOf(client, ALPHA);
    expect(versions.filter((version) => version.startedAt !== undefined)).toHaveLength(1);
    expect(versions[0]).toMatchObject({ startedAt: T0, endedAt: T0 + 4 * MINUTE + SESSION_GAP_MS, current: true });
    // One millisecond more starts a new one.
    clock.now = T0 + 4 * MINUTE + 2 * SESSION_GAP_MS + 1;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 5, "Five");
    });
    versions = await versionsOf(client, ALPHA);
    expect(versions.filter((version) => version.startedAt !== undefined).map((version) => [version.startedAt, version.endedAt])).toEqual([
      [clock.now, clock.now],
      [T0, T0 + 4 * MINUTE + SESSION_GAP_MS],
    ]);
    expect(versions[0]?.current).toBe(true);
    expect(versions[1]?.current).toBeUndefined();
  });

  it("titles a session with the page's title at its end, and each session with the title of its time", async () => {
    const { transport, client, clock } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    await client.pageRename(ALPHA, "Alpha renamed");
    clock.now = T0 + MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    // The first session went on, so it ends with the new title.
    let dated = (await versionsOf(client, ALPHA)).filter((version) => version.startedAt !== undefined);
    expect(dated.map((version) => version.title)).toEqual(["Alpha renamed"]);
    await client.pageRename(ALPHA, "Alpha again");
    clock.now = T0 + 20 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 4, "Four");
    });
    dated = (await versionsOf(client, ALPHA)).filter((version) => version.startedAt !== undefined);
    expect(dated.map((version) => version.title)).toEqual(["Alpha again", "Alpha renamed"]);
  });

  it("records the core's own writes as this device's, and lists a change from elsewhere as a row without an author", async () => {
    const { transport, client, clock } = demo();
    transport.changePage(ALPHA, (doc) => appendParagraph(doc, 2, "Core"), "Core");
    clock.now = T0 + MINUTE;
    // A change that arrives from a remote device is never part of this device's session.
    transport.changePage(ALPHA, (doc) => appendParagraph(doc, 3, "Remote"), "Remote");
    clock.now = T0 + 2 * MINUTE;
    transport.changePage(ALPHA, (doc) => appendParagraph(doc, 4, "Core again"), "Core");
    const dated = (await versionsOf(client, ALPHA)).filter((version) => version.startedAt !== undefined);
    expect(dated.map((version) => [version.kind, version.startedAt, version.author])).toEqual([
      ["edit", T0 + 2 * MINUTE, { deviceId: DEVICE, thisDevice: true }],
      ["edit", T0 + MINUTE, undefined],
      ["edit", T0, { deviceId: DEVICE, thisDevice: true }],
    ]);
  });

  it("gives a copy a first row at its creation and no undated history", async () => {
    const { client, clock } = demo();
    clock.now = T0 + 7_000;
    const copy = await client.pageDuplicate(ALPHA);
    const versions = await versionsOf(client, copy.id);
    expect(versions).toEqual([
      {
        id: AN_ID,
        kind: "edit",
        startedAt: T0 + 7_000,
        endedAt: T0 + 7_000,
        author: { deviceId: DEVICE, thisDevice: true },
        title: "Alpha (copy)",
        current: true,
      },
    ]);
    await expect(texts(client, copy.id)).resolves.toEqual(["One"]);
  });

  it("lists a session that changed nothing once: a repeated update adds no version", async () => {
    const { transport, client, clock } = demo();
    const editor = await openEditor(transport, ALPHA);
    appendParagraph(editor.doc, 2, "Two");
    const update = await editor.flush();
    const before = await versionsOf(client, ALPHA);
    clock.now = T0 + 30 * MINUTE;
    await transport.docs.import(ALPHA, update);
    await expect(versionsOf(client, ALPHA)).resolves.toEqual(before);
  });

  it("lists a page in the trash, and is NotFound for unknown and purged pages", async () => {
    const { client } = demo();
    await client.pageTrash(GAMMA);
    await expect(client.pageHistoryList(GAMMA)).resolves.toMatchObject({ pageId: GAMMA, versions: [] });
    await client.pageTrash(ALPHA);
    expect((await versionsOf(client, ALPHA))[0]?.current).toBe(true);
    await client.pagePurge(ALPHA);
    for (const pageId of [UNKNOWN, ALPHA, BETA]) {
      const error = await failure(client.pageHistoryList(pageId));
      expect(error.toApiError()).toEqual({ code: "NotFound", message: "page not found" });
    }
  });
});

// ----- the undated runs -----------------------------------------------------------------

describe("undated versions", () => {
  /** A document with the history P10[0..1] (two commits), P20[0], P10[2]: three runs. */
  function history(): LoroDoc {
    const doc = new LoroDoc();
    doc.setPeerId("10");
    const map = doc.getMap("root");
    map.set("a", 1);
    doc.commit();
    map.set("b", 2);
    doc.commit();
    const other = new LoroDoc();
    other.setPeerId("20");
    other.import(doc.export({ mode: "snapshot" }));
    other.getMap("root").set("c", 3);
    other.commit();
    doc.import(other.export({ mode: "update" }));
    map.set("d", 4);
    doc.commit();
    return doc;
  }

  it("are the runs of consecutive changes of one peer, newest first, each named by its last change", () => {
    const doc = history();
    expect(undatedVersions(doc, doc.oplogVersion().toJSON())).toEqual(["10@2", "20@0", "10@1"]);
  });

  it("stop where the journal begins, cutting a run in the middle when its session began there", () => {
    const doc = history();
    expect(undatedVersions(doc, new Map([["10", 2], ["20", 1]]))).toEqual(["20@0", "10@1"]);
    expect(undatedVersions(doc, new Map([["10", 1]]))).toEqual(["10@0"]);
    expect(undatedVersions(doc, new Map())).toEqual([]);
  });

  it("are listed after the dated versions and can be read like them", async () => {
    const { transport, client, clock } = demo();
    clock.now = T0 + MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    const [dated, undated] = await versionsOf(client, ALPHA);
    expect(dated?.startedAt).toBe(T0 + MINUTE);
    expect(undated).toEqual({ id: AN_ID, kind: "edit" });
    const old = await client.pageHistoryGet(ALPHA, undated?.id ?? "");
    expect(textsOf(old.content)).toEqual(["One"]);
    expect(old).not.toHaveProperty("title");
  });
});

// ----- paging ---------------------------------------------------------------------------

describe("paging", () => {
  it("resumes after the version `before` names, with `more` while older versions remain", async () => {
    const { transport, client, clock } = demo();
    sessions(transport, clock, ALPHA, 5); // five dated versions and the undated seed: six
    const all = ids((await client.pageHistoryList(ALPHA)).versions);
    expect(all).toHaveLength(6);
    const first = await client.pageHistoryList(ALPHA, { limit: 2 });
    expect(ids(first.versions)).toEqual(all.slice(0, 2));
    expect(first.more).toBe(true);
    const second = await client.pageHistoryList(ALPHA, { limit: 2, before: all[1] ?? "" });
    expect(ids(second.versions)).toEqual(all.slice(2, 4));
    expect(second.more).toBe(true);
    const third = await client.pageHistoryList(ALPHA, { limit: 2, before: all[3] ?? "" });
    expect(ids(third.versions)).toEqual(all.slice(4));
    expect(third.more).toBe(false);
    // The last version has nothing older.
    const none = await client.pageHistoryList(ALPHA, { before: all[5] ?? "" });
    expect(none).toEqual({ pageId: ALPHA, versions: [], more: false });
  });

  it("answers 100 versions by default and at most 500", async () => {
    const { transport, client, clock } = demo();
    sessions(transport, clock, ALPHA, 510);
    const byDefault = await client.pageHistoryList(ALPHA);
    expect(byDefault.versions).toHaveLength(100);
    expect(byDefault.more).toBe(true);
    const most = await client.pageHistoryList(ALPHA, { limit: 100_000 });
    expect(most.versions).toHaveLength(500);
    expect(most.more).toBe(true);
    const rest = await client.pageHistoryList(ALPHA, { limit: 500, before: most.versions[499]?.id ?? "" });
    expect(rest.versions).toHaveLength(11); // 510 dated versions and the undated seed, 500 already seen
    expect(rest.more).toBe(false);
  });

  it("takes a limit of 0 as 1 and refuses one that is negative or not whole", async () => {
    const { transport, client, clock } = demo();
    sessions(transport, clock, ALPHA, 2);
    await expect(client.pageHistoryList(ALPHA, { limit: 0 })).resolves.toMatchObject({ more: true });
    expect((await client.pageHistoryList(ALPHA, { limit: 0 })).versions).toHaveLength(1);
    for (const limit of [-1, 1.5, Number.NaN]) {
      const error = await failure(client.pageHistoryList(ALPHA, { limit }));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "limit" } });
    }
  });

  it("refuses a `before` that is no id, names unknown changes, or is a version the list does not have", async () => {
    const { transport, client } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    const [current] = await versionsOf(client, ALPHA);
    const [peer, counter] = (current?.id ?? "0@1").split("@");
    const inside = `${peer ?? ""}@${String(Number(counter) - 1)}`; // a change of the document, but in no version of the list
    for (const before of ["", "nope", "1@", "@1", "1@-1", "1@1,", "99999999999999999999999@1", "9@0", `${peer ?? ""}@${String(Number(counter) + 1)}`, inside]) {
      const error = await failure(client.pageHistoryList(ALPHA, { before }));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "before" } });
    }
  });
});

// ----- content at a version -------------------------------------------------------------

describe("the content at a version", () => {
  it("is the page as it was: its portable JSON and the title the journal has for it", async () => {
    const { transport, client, clock } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    await client.pageRename(ALPHA, "Alpha 2");
    clock.now = T0 + 20 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    const [second, first, seeded] = await versionsOf(client, ALPHA);
    const atFirst = await client.pageHistoryGet(ALPHA, first?.id ?? "");
    expect(atFirst).toEqual({
      pageId: ALPHA,
      version: first?.id,
      content: { type: "doc", content: [paragraph(1, "One"), paragraph(2, "Two")] },
      title: "Alpha",
    });
    const atSecond = await client.pageHistoryGet(ALPHA, second?.id ?? "");
    expect(textsOf(atSecond.content)).toEqual(["One", "Two", "Three"]);
    expect(atSecond.title).toBe("Alpha 2");
    // The current content, read two ways, is the same.
    expect(atSecond.content).toEqual((await client.pageContentGet(ALPHA)).content);
    // An undated version has no title.
    const atSeed = await client.pageHistoryGet(ALPHA, seeded?.id ?? "");
    expect(atSeed).toEqual({ pageId: ALPHA, version: seeded?.id, content: ONE });
  });

  it("changes nothing and announces nothing", async () => {
    const { transport, client, events } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    const before = await client.pageHistoryList(ALPHA);
    const [, seeded] = before.versions;
    events.length = 0;
    await client.pageHistoryGet(ALPHA, seeded?.id ?? "");
    expect(events).toEqual([]);
    await expect(client.pageHistoryList(ALPHA)).resolves.toEqual(before);
    await expect(texts(client, ALPHA)).resolves.toEqual(["One", "Two"]);
  });

  it("answers the canonical id for an id written with its heads in another order", async () => {
    const { transport, client } = demo();
    const editor = await openEditor(transport, ALPHA);
    transport.changePage(ALPHA, (doc) => appendParagraph(doc, 2, "From the core"), "Core");
    appendParagraph(editor.doc, 3, "From the editor");
    await editor.flush();
    const [current] = await versionsOf(client, ALPHA);
    const reversed = (current?.id ?? "").split(",").reverse().join(",");
    expect(reversed).not.toBe(current?.id);
    await expect(client.pageHistoryGet(ALPHA, reversed)).resolves.toMatchObject({ version: current?.id });
  });

  it("is InvalidInput for an id that is no id or names changes the page does not have", async () => {
    const { transport, client } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    const [current] = await versionsOf(client, ALPHA);
    const [peer, counter] = (current?.id ?? "0@0").split("@");
    for (const version of ["", "nope", "1@", "1@x", "-1@0", "9@0", `${peer ?? ""}@${String(Number(counter) + 1)}`, `${current?.id ?? ""},9@0`]) {
      const error = await failure(client.pageHistoryGet(ALPHA, version));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "version" } });
    }
    // A page that has no document has no versions.
    expect(await failure(client.pageHistoryGet(GAMMA, "1@0"))).toMatchObject({ code: "InvalidInput", details: { field: "version" } });
  });

  it("is answered for a page in the trash, NotFound for an unknown or purged one", async () => {
    const { client } = demo();
    const [seeded] = await versionsOf(client, ALPHA);
    await client.pageTrash(ALPHA);
    await expect(client.pageHistoryGet(ALPHA, seeded?.id ?? "")).resolves.toMatchObject({ content: ONE });
    await client.pagePurge(ALPHA);
    for (const pageId of [UNKNOWN, ALPHA]) {
      const error = await failure(client.pageHistoryGet(pageId, seeded?.id ?? ""));
      expect(error.toApiError()).toEqual({ code: "NotFound", message: "page not found" });
    }
  });
});

// ----- restore --------------------------------------------------------------------------

describe("restore", () => {
  /** Alpha with three versions: seeded "One"; "One, Two" (title Alpha); "One, Two, Three" (title Alpha 2). */
  async function threeVersions() {
    const ctx = demo();
    const { transport, client, clock } = ctx;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    await client.pageRename(ALPHA, "Alpha 2");
    clock.now = T0 + 20 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    const [second, first, seeded] = await versionsOf(client, ALPHA);
    if (second === undefined || first === undefined || seeded === undefined) {
      throw new Error("expected three versions");
    }
    return { ...ctx, seeded, first, second };
  }

  it("makes the content and the title what they were, and answers the page", async () => {
    const { client, events, first } = await threeVersions();
    events.length = 0;
    const page = await client.pageHistoryRestore(ALPHA, first.id);
    expect(page).toMatchObject({ id: ALPHA, title: "Alpha" });
    await expect(texts(client, ALPHA)).resolves.toEqual(["One", "Two"]);
    await expect(client.pageGet(ALPHA)).resolves.toMatchObject({ title: "Alpha" });
    // The core's write, then the page's new title.
    const personal = (await client.spaceList())[0]?.id ?? "";
    expect(events).toEqual([contentChanged(ALPHA), structureChanged(personal)]);
  });

  it("records a restore row of its own, with this device, the clock and the title it made, and keeps the older versions", async () => {
    const { transport, client, clock, first, second, seeded } = await threeVersions();
    clock.now = T0 + 25 * MINUTE;
    await client.pageHistoryRestore(ALPHA, first.id);
    const versions = await versionsOf(client, ALPHA);
    expect(versions).toHaveLength(4);
    expect(versions[0]).toEqual({
      id: AN_ID,
      kind: "restore",
      startedAt: T0 + 25 * MINUTE,
      endedAt: T0 + 25 * MINUTE,
      author: { deviceId: DEVICE, thisDevice: true },
      title: "Alpha",
      current: true,
    });
    expect(ids(versions.slice(1))).toEqual([second.id, first.id, seeded.id]);
    expect(versions.slice(1).every((version) => version.current === undefined)).toBe(true);
    // A write soon after a restore is a session of its own: the restore row is never continued.
    clock.now = T0 + 26 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 9, "After");
    });
    const after = await versionsOf(client, ALPHA);
    expect(after.slice(0, 2).map((version) => [version.kind, version.startedAt, version.endedAt])).toEqual([
      ["edit", T0 + 26 * MINUTE, T0 + 26 * MINUTE],
      ["restore", T0 + 25 * MINUTE, T0 + 25 * MINUTE],
    ]);
  });

  it("is undone by restoring the version that was current before, and the editor session pulls each step", async () => {
    const { transport, client, first, second } = await threeVersions();
    const editor = await openEditor(transport, ALPHA);
    await client.pageHistoryRestore(ALPHA, first.id);
    await editor.pull();
    expect(textsOf(loroToPortable(editor.doc))).toEqual(["One", "Two"]);
    // Undo: the version that was current before is still listed, and restoring it brings back the content and the title.
    const page = await client.pageHistoryRestore(ALPHA, second.id);
    expect(page.title).toBe("Alpha 2");
    await expect(texts(client, ALPHA)).resolves.toEqual(["One", "Two", "Three"]);
    await editor.pull();
    expect(loroToPortable(editor.doc)).toEqual((await client.pageContentGet(ALPHA)).content);
    const versions = await versionsOf(client, ALPHA);
    expect(versions.map((version) => version.kind)).toEqual(["restore", "restore", "edit", "edit", "edit"]);
    expect(versions[0]?.current).toBe(true);
  });

  it("restores an undated version: the content, no rename", async () => {
    const { client, events, seeded } = await threeVersions();
    events.length = 0;
    const page = await client.pageHistoryRestore(ALPHA, seeded.id);
    expect(page.title).toBe("Alpha 2");
    await expect(texts(client, ALPHA)).resolves.toEqual(["One"]);
    expect(events).toEqual([contentChanged(ALPHA)]);
    expect((await versionsOf(client, ALPHA))[0]).toMatchObject({ kind: "restore", title: "Alpha 2", current: true });
  });

  it("announces the links it brings back and removes", async () => {
    const { transport, client, events, clock } = demo();
    await write(transport, ALPHA, (doc) => {
      appendPageRef(doc, 0, BETA);
    });
    clock.now = T0 + 20 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      removeLast(doc, 0);
    });
    const [unlinked, linked] = await versionsOf(client, ALPHA);
    expect(await client.backlinksGet(BETA)).toEqual([]);
    events.length = 0;
    await client.pageHistoryRestore(ALPHA, linked?.id ?? "");
    expect(events).toEqual([contentChanged(ALPHA), backlinksChanged(BETA)]);
    expect((await client.backlinksGet(BETA)).map((link) => link.pageId)).toEqual([ALPHA]);
    events.length = 0;
    await client.pageHistoryRestore(ALPHA, unlinked?.id ?? "");
    expect(events).toEqual([contentChanged(ALPHA), backlinksChanged(BETA)]);
    expect(await client.backlinksGet(BETA)).toEqual([]);
  });

  it("changes nothing for the current version: no change, no events, no row", async () => {
    const { client, events, clock, second } = await threeVersions();
    const before = await client.pageHistoryList(ALPHA);
    const content = await client.pageContentGet(ALPHA);
    events.length = 0;
    clock.now = T0 + 40 * MINUTE;
    const page = await client.pageHistoryRestore(ALPHA, second.id);
    expect(page).toMatchObject({ id: ALPHA, title: "Alpha 2" });
    expect(events).toEqual([]);
    await expect(client.pageHistoryList(ALPHA)).resolves.toEqual(before);
    await expect(client.pageContentGet(ALPHA)).resolves.toEqual(content);
  });

  it("renames the page even when only the title differs (the same state under another title)", async () => {
    const { transport, client, clock, events } = demo();
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    clock.now = T0 + 20 * MINUTE;
    await write(transport, ALPHA, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    await client.pageRename(ALPHA, "Alpha 2");
    clock.now = T0 + 40 * MINUTE;
    // Takes "Three" out again: the content is the first version's, under a title the first version did not have.
    const editor = await openEditor(transport, ALPHA);
    removeBlock(editor.doc, 2);
    await editor.flush();
    const versions = await versionsOf(client, ALPHA);
    const first = versions.find((version) => version.title === "Alpha" && version.startedAt === T0);
    expect(first).toBeDefined();
    events.length = 0;
    await client.pageHistoryRestore(ALPHA, first?.id ?? "");
    expect(await client.pageGet(ALPHA)).toMatchObject({ title: "Alpha" });
    await expect(texts(client, ALPHA)).resolves.toEqual(["One", "Two"]);
    expect(events).toContainEqual(structureChanged((await client.spaceList())[0]?.id ?? ""));
  });

  it("refuses a page in the trash, an unknown or purged page, and a version the page does not have", async () => {
    const { client, events, first } = await threeVersions();
    const bad = ["", "nope", "9@0", `${first.id},9@0`];
    for (const version of bad) {
      const error = await failure(client.pageHistoryRestore(ALPHA, version));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "version" } });
    }
    await client.pageTrash(ALPHA);
    events.length = 0;
    const trashed = await failure(client.pageHistoryRestore(ALPHA, first.id));
    expect(trashed).toMatchObject({ code: "InvalidInput", details: { field: "pageId" } });
    await client.pagePurge(ALPHA);
    events.length = 0;
    for (const pageId of [UNKNOWN, ALPHA]) {
      const error = await failure(client.pageHistoryRestore(pageId, first.id));
      expect(error.toApiError()).toEqual({ code: "NotFound", message: "page not found" });
    }
    // Nothing was written by a refusal.
    expect(events).toEqual([]);
  });
});

// ----- roles ----------------------------------------------------------------------------

describe("roles", () => {
  function shared(role: "viewer" | "editor") {
    const ctx = setup({
      workspaceName: "Home",
      spaces: [{ name: "Work", pages: [{ title: "Plan", content: ONE }] }],
      security: { kitCreatedAt: T0 },
      sync: { state: "idle", serverUrl: "https://sync.example.com" },
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role }] },
    });
    return ctx;
  }

  it("lets a Viewer list and read versions but not restore, and an Editor restore", async () => {
    const { transport, client } = shared("viewer");
    const plan = await pageNamed(client, "Plan");
    const [seeded] = await versionsOf(client, plan);
    transport.changePage(plan, (doc) => appendParagraph(doc, 2, "Two"), "Remote");
    await expect(client.pageHistoryGet(plan, seeded?.id ?? "")).resolves.toMatchObject({ content: ONE });
    const refused = await failure(client.pageHistoryRestore(plan, seeded?.id ?? ""));
    expect(refused).toMatchObject({ code: "Forbidden" });
    await expect(texts(client, plan)).resolves.toEqual(["One", "Two"]);

    const editor = shared("editor");
    const editable = await pageNamed(editor.client, "Plan");
    const [first] = await versionsOf(editor.client, editable);
    editor.transport.changePage(editable, (doc) => appendParagraph(doc, 2, "Two"), "Core");
    await editor.client.pageHistoryRestore(editable, first?.id ?? "");
    await expect(texts(editor.client, editable)).resolves.toEqual(["One"]);
  });
});

// ----- a backup's restore ---------------------------------------------------------------

describe("a restored backup", () => {
  it("has no journal (it is in no backup), only the undated history of its documents", async () => {
    const origin = demo({
      workspaceName: "Home",
      security: { kitCreatedAt: T0 - 10_000, kitKey: KEY },
    });
    const alpha = await pageNamed(origin.client, "Alpha");
    await write(origin.transport, alpha, (doc) => {
      appendParagraph(doc, 2, "Two");
    });
    expect((await versionsOf(origin.client, alpha)).some((version) => version.startedAt !== undefined)).toBe(true);
    await origin.client.backupExport("C:/backups/home.atomikkb");
    const restored = setup({ files: origin.transport.files });
    await restored.client.backupImport(undefined, (await kitWordsFromEntropy(KEY)).join(" "));
    const versions = await versionsOf(restored.client, alpha);
    expect(versions).toEqual([{ id: AN_ID, kind: "edit", current: true }]);
    await expect(texts(restored.client, alpha)).resolves.toEqual(["One", "Two"]);
    // The restored page is written and listed like any other.
    restored.clock.now = T0 + MINUTE;
    await write(restored.transport, alpha, (doc) => {
      appendParagraph(doc, 3, "Three");
    });
    expect((await versionsOf(restored.client, alpha)).map((version) => version.startedAt)).toEqual([T0 + MINUTE, undefined]);
  });
});
