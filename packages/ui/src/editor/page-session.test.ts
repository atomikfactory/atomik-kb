import { CoreClient, loroToPortable, MemoryTransport } from "@atomik-kb/core-client";
import type { DocTransport, JsonValue, PageId, Transport } from "@atomik-kb/core-client";
import type { LoroDoc, LoroList, LoroMap, LoroText } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { paragraphs } from "../test/editor";
import { FAILED_FLUSHES_NOTICE, FLUSH_INTERVAL_MS, PageDocSession, PageDocSessions } from "./page-session";

/** A core with one page holding `content`, and spies on the raw document calls. */
async function setup(content: JsonValue = paragraphs("Hello")) {
  const memory = new MemoryTransport({
    workspaceName: "Research",
    spaces: [{ name: "Personal", pages: [{ title: "Notes", content }] }],
  });
  const docs = {
    open: vi.fn((pageId: PageId) => memory.docs.open(pageId)),
    import: vi.fn((pageId: PageId, update: Uint8Array) => memory.docs.import(pageId, update)),
    export: vi.fn((pageId: PageId, since?: Uint8Array) => memory.docs.export(pageId, since)),
    close: vi.fn((pageId: PageId) => memory.docs.close(pageId)),
  } satisfies DocTransport;
  const transport: Transport = {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs,
  };
  const client = new CoreClient(transport);
  const [space] = await client.spaceList();
  const tree = await client.treeGet(space?.id ?? "");
  const pageId = tree.roots[0]?.page.id ?? "";
  return { memory, client, docs, pageId };
}

/** The text of the first paragraph of a page document. */
function firstText(doc: LoroDoc): LoroText {
  const paragraph = (doc.getMap("doc").get("children") as LoroList).get(0) as LoroMap;
  return (paragraph.get("children") as LoroList).get(0) as LoroText;
}

async function readySession(client: CoreClient, pageId: PageId): Promise<{ session: PageDocSession; doc: LoroDoc }> {
  const session = new PageDocSession(client, pageId);
  await vi.waitFor(() => {
    expect(session.getSnapshot().status).toBe("ready");
  });
  const { doc } = session.getSnapshot();
  if (doc === null) {
    throw new Error("no document");
  }
  return { session, doc };
}

describe("page document session", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it("opens the page's document from the core's snapshot", async () => {
    const { client, pageId, docs } = await setup(paragraphs("Hello", "World"));
    const session = new PageDocSession(client, pageId);
    expect(session.getSnapshot().status).toBe("opening");
    await vi.waitFor(() => {
      expect(session.getSnapshot().status).toBe("ready");
    });
    expect(docs.open).toHaveBeenCalledWith(pageId);
    expect(loroToPortable(session.getSnapshot().doc as LoroDoc)).toEqual(paragraphs("Hello", "World"));
    await session.close();
  });

  it("reports a page that cannot be opened, and can try again", async () => {
    const { client, memory, pageId } = await setup();
    memory.failNextDocs({ code: "Storage", message: "disk I/O error" });
    const session = new PageDocSession(client, pageId);
    await vi.waitFor(() => {
      expect(session.getSnapshot().status).toBe("failed");
    });
    expect(session.getSnapshot().error).toMatchObject({ code: "Storage" });
    session.retry();
    await vi.waitFor(() => {
      expect(session.getSnapshot().status).toBe("ready");
    });
    await session.close();
  });

  it("sends local changes in one batch at most every 250 ms", async () => {
    const { client, pageId, docs } = await setup();
    const { session, doc } = await readySession(client, pageId);

    firstText(doc).insert(5, ",");
    doc.commit();
    await vi.advanceTimersByTimeAsync(0);
    expect(docs.import).toHaveBeenCalledTimes(1); // the first change goes out right away

    firstText(doc).insert(6, " my");
    doc.commit();
    firstText(doc).insert(9, " friend");
    doc.commit();
    await vi.advanceTimersByTimeAsync(FLUSH_INTERVAL_MS - 10);
    expect(docs.import).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(20);
    expect(docs.import).toHaveBeenCalledTimes(2); // both later changes in one update

    const saved = await client.pageContentGet(pageId);
    expect(saved.content).toEqual(paragraphs("Hello, my friend"));
    await session.close();
  });

  it("retries what the core did not acknowledge and says so after three failures", async () => {
    const { client, memory, pageId } = await setup();
    const { session, doc } = await readySession(client, pageId);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    memory.failNextDocs({ code: "Storage", message: "disk full" }, FAILED_FLUSHES_NOTICE);

    firstText(doc).insert(5, "!");
    doc.commit();
    firstText(doc).insert(6, "?");
    doc.commit();
    await session.flush();
    expect(session.getSnapshot().failedFlushes).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(session.getSnapshot().failedFlushes).toBe(2);
    await vi.advanceTimersByTimeAsync(2000);
    expect(session.getSnapshot().failedFlushes).toBe(FAILED_FLUSHES_NOTICE);

    await vi.advanceTimersByTimeAsync(5000); // the next retry succeeds and sends both changes
    expect(session.getSnapshot().failedFlushes).toBe(0);
    expect((await client.pageContentGet(pageId)).content).toEqual(paragraphs("Hello!?"));
    await session.close();
  });

  it("imports changes made elsewhere, without sending them back", async () => {
    const { client, memory, pageId, docs } = await setup();
    const { session, doc } = await readySession(client, pageId);

    memory.changePage(pageId, (core) => {
      firstText(core).insert(0, "Oh, ");
    });
    await vi.waitFor(() => {
      expect(loroToPortable(doc)).toEqual(paragraphs("Oh, Hello"));
    });
    expect(docs.export).toHaveBeenCalledWith(pageId, expect.any(Uint8Array));

    await session.flush();
    expect(docs.import).not.toHaveBeenCalled();

    firstText(doc).insert(9, "!");
    doc.commit();
    await session.flush();
    expect((await client.pageContentGet(pageId)).content).toEqual(paragraphs("Oh, Hello!"));
    await session.close();
  });

  it("ignores the core's echo of the editor's own changes", async () => {
    const { client, pageId, docs } = await setup();
    const { session, doc } = await readySession(client, pageId);
    firstText(doc).insert(5, "!");
    doc.commit();
    await session.flush();
    expect(docs.import).toHaveBeenCalledTimes(1);
    expect(docs.export).not.toHaveBeenCalled(); // PageContentChanged { source: "Editor" }
    await session.close();
  });

  it("sends what is left when it closes, then closes the document in the core", async () => {
    const { client, pageId, docs } = await setup();
    const { session, doc } = await readySession(client, pageId);
    firstText(doc).insert(5, "!");
    doc.commit();
    await session.close();
    expect(docs.import).toHaveBeenCalledTimes(1);
    expect(docs.close).toHaveBeenCalledWith(pageId);
    expect(docs.import.mock.invocationCallOrder[0]).toBeLessThan(docs.close.mock.invocationCallOrder[0] ?? 0);
    expect((await client.pageContentGet(pageId)).content).toEqual(paragraphs("Hello!"));
  });

  it("flushes when the page is hidden", async () => {
    const { client, pageId, docs } = await setup();
    const { session, doc } = await readySession(client, pageId);
    firstText(doc).insert(5, "!");
    doc.commit();
    await session.flush();
    firstText(doc).insert(6, "?"); // would wait for the 250 ms interval
    doc.commit();
    window.dispatchEvent(new Event("pagehide"));
    await vi.advanceTimersByTimeAsync(0);
    expect(docs.import).toHaveBeenCalledTimes(2);
    await session.close();
  });
});

describe("page document sessions", () => {
  it("shares one session per page and closes it once nobody uses it", async () => {
    const { client, pageId, docs } = await setup();
    const sessions = new PageDocSessions(client);
    const first = sessions.acquire(pageId);
    const second = sessions.acquire(pageId);
    expect(second).toBe(first);
    expect(sessions.peek(pageId)).toBe(first);

    sessions.release(first);
    sessions.release(second);
    // Taken again before the close runs (React StrictMode, a quick switch back): reused.
    const again = sessions.acquire(pageId);
    expect(again).toBe(first);
    sessions.release(again);
    await vi.waitFor(() => {
      expect(docs.close).toHaveBeenCalledWith(pageId);
    });
    expect(sessions.peek(pageId)).toBeNull();
    expect(docs.open).toHaveBeenCalledTimes(1);
  });
});
