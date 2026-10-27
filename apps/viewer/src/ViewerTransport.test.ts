// `ViewerTransport` against a fake server (M13 slice 4): the real `kb-wasm` module (built by `scripts/build-wasm.mjs`) opens
// the fixtures that `crates/kb-wasm/examples/viewer_fixture.rs` made with the real publication codec, and a fake `fetch`
// answers the way `kb-server` does. What is checked: the key never leaves the page, the poll sends `If-None-Match` and
// updates, the states (gone, wrong key, offline, rate limited), and that the document and the pictures come out right.

import { LoroDoc } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { KbWasm } from "./kb-wasm";
import { FakeServer, fixtureServer, loadFixtures, realWasm } from "./testing";
import type { Fixtures } from "./testing";
import { ViewerTransport, VIEWER_SPACE_ID } from "./ViewerTransport";
import type { LinkLoad } from "./ViewerTransport";

const fixtures: Fixtures = loadFixtures();
const { fragment } = fixtures.description;
const wasm: KbWasm = realWasm();

function make(
  options: { fragment?: string; server?: FakeServer } = {},
): { server: FakeServer; transport: ViewerTransport } {
  const server = options.server ?? fixtureServer(fixtures);
  const transport = new ViewerTransport({ fragment: options.fragment ?? fragment, wasm, fetch: server.fetch, pollSeconds: 60 });
  return { server, transport };
}

/** The request records as one string: everything a server (or anything on the way) could have been sent. */
function everythingSent(server: FakeServer): string {
  return JSON.stringify(server.seen.map((request) => [request.url, request.init]));
}

describe("loading", () => {
  it("asks for the link its key derives and opens the publication", async () => {
    const { server, transport } = make();
    const load = await transport.load();
    expect(load.status).toBe("ready");
    if (load.status !== "ready") {
      return;
    }
    expect(load.pubSeq).toBe(1);
    expect(load.publication.title).toBe(fixtures.description.titles[0]);
    expect(load.publication.icon).toBe(fixtures.description.icon);
    expect(load.publication.cover).toBe(fixtures.description.cover);
    expect(load.publication.coverY).toBe(fixtures.description.coverY);
    expect(load.publication.pageId).toBe(fixtures.description.pageId);
    expect(JSON.parse(load.publication.mentionsJson())).toEqual(fixtures.description.mentions);
    expect(server.seen.map((request) => request.url)).toEqual([`/v2/links/${fixtures.description.linkId}`]);
  });

  it("takes the key with or without the # of the address", async () => {
    const { transport } = make({ fragment: `#${fragment}` });
    expect((await transport.load()).status).toBe("ready");
  });

  it("never sends the key: not in an address, a header or a body, and every request is anonymous and relative", async () => {
    const { server, transport } = make();
    const load = await transport.load();
    expect(load.status).toBe("ready");
    await transport.docs.open(fixtures.description.pageId);
    await transport.attachments?.read(fixtures.description.attachment.attachmentId);
    await transport.load();
    expect(server.seen.length).toBeGreaterThanOrEqual(3);

    const sent = everythingSent(server);
    expect(sent).not.toContain(fragment);
    // Nor a part of it long enough to be the key's: a prefix, a suffix, or its standard-base64 spelling.
    expect(sent).not.toContain(fragment.slice(0, 16));
    expect(sent).not.toContain(fragment.slice(-16));
    expect(fixtures.description.linkId).not.toContain(fragment.slice(0, 8));

    for (const request of server.seen) {
      expect(request.url.startsWith("/v2/links/")).toBe(true);
      expect(request.init).toMatchObject({ method: "GET", credentials: "omit", referrerPolicy: "no-referrer", cache: "no-store" });
      expect(request.init?.body).toBeUndefined();
      const headers = new Headers(request.init?.headers);
      expect([...headers.keys()].filter((name) => name !== "if-none-match")).toEqual([]);
    }
  });

  it("sends nothing at all for a fragment that is not a key", async () => {
    for (const bad of ["", "#", "not a key", `${fragment}A`, fragment.slice(1)]) {
      const { server, transport } = make({ fragment: bad });
      expect(await transport.load()).toEqual({ status: "invalid" });
      expect(server.seen).toEqual([]);
    }
  });

  it("answers one request for callers that ask at the same time", async () => {
    const { server, transport } = make();
    const [first, second] = await Promise.all([transport.load(), transport.load()]);
    expect(first.status).toBe("ready");
    expect(second).toBe(first);
    expect(server.seen).toHaveLength(1);
  });
});

describe("the states of a link", () => {
  it("is gone when the server answers 404 or 410 (revoked, expired and unknown are one answer)", async () => {
    for (const status of [404, 410] as const) {
      const { server, transport } = make();
      server.failure = status;
      expect(await transport.load()).toEqual({ status: "gone" });
    }
  });

  it("is invalid for a publication that does not open under the key", async () => {
    // The server answers at the link id of this key with a publication of another link: the tag does not match.
    const other = "A".repeat(43);
    const otherId = wasm.linkId(other);
    const server = new FakeServer(otherId, fixtures.publications, new Map());
    const { transport } = make({ fragment: other, server });
    expect(await transport.load()).toEqual({ status: "invalid" });
    expect(everythingSent(server)).not.toContain(other);
  });

  it("is invalid for bytes that are not a publication", async () => {
    const server = new FakeServer(fixtures.description.linkId, [new Uint8Array([1, 2, 3, 4])], new Map());
    const { transport } = make({ server });
    expect(await transport.load()).toEqual({ status: "invalid" });
  });

  it("is unavailable when the network fails or the server is busy or broken, and says nothing is wrong with the link", async () => {
    for (const failure of ["network", 429, 500, 503] as const) {
      const { server, transport } = make();
      server.failure = failure;
      expect(await transport.load()).toEqual({ status: "unavailable" });
    }
  });

  it("does not claim a publication it could not read the body of", async () => {
    const transport = new ViewerTransport({
      fragment,
      wasm,
      fetch: () => Promise.resolve(new Response(new ReadableStream({ start: (controller) => controller.error(new Error("cut")) }), { status: 200 })),
    });
    expect(await transport.load()).toEqual({ status: "unavailable" });
  });
});

describe("the poll", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Lets the timers run for `seconds`, and the promises they start. */
  const wait = async (seconds: number): Promise<void> => {
    await vi.advanceTimersByTimeAsync(seconds * 1000);
  };

  it("sends If-None-Match with the ETag every 60 seconds, stays quiet on 304 and updates in place on a new publication", async () => {
    const { server, transport } = make();
    const seen: LinkLoad[] = [];
    const first = await transport.load();
    expect(first.status).toBe("ready");
    const stop = transport.watch((load) => seen.push(load));

    await wait(59);
    expect(server.conditions()).toEqual([undefined]);
    await wait(1);
    expect(server.conditions()).toEqual([undefined, '"1"']);
    expect(seen).toEqual([]);

    // A member's device published again: the same link, another `pub_seq`.
    server.current = 1;
    await wait(60);
    expect(server.conditions()).toEqual([undefined, '"1"', '"1"']);
    expect(seen).toHaveLength(1);
    const update = seen[0];
    expect(update?.status).toBe("ready");
    if (update?.status === "ready") {
      expect(update.pubSeq).toBe(2);
      expect(update.publication.title).toBe(fixtures.description.titles[1]);
      expect(update.retry).toBeUndefined();
    }
    // The document the editor opens now is the new one.
    const doc = new LoroDoc();
    doc.import(await transport.docs.open(fixtures.description.pageId));
    expect(JSON.stringify(doc.toJSON())).toContain(fixtures.description.texts[2]);

    await wait(60);
    expect(server.conditions().at(-1)).toBe('"2"');
    expect(seen).toHaveLength(1);

    stop();
    const requests = server.seen.length;
    await wait(300);
    expect(server.seen).toHaveLength(requests);
  });

  it("tells the page when the link is gone, and keeps asking: its first upload may not have come yet", async () => {
    const { server, transport } = make();
    server.current = 5; // no publication yet: 404
    const seen: LinkLoad["status"][] = [];
    const stop = transport.watch((load) => seen.push(load.status));
    await wait(60);
    expect(seen).toEqual(["gone"]);
    server.current = 0;
    await wait(60);
    expect(seen).toEqual(["gone", "ready"]);
    stop();
  });

  it("stops asking about a key that does not open", async () => {
    const server = new FakeServer(fixtures.description.linkId, [new Uint8Array([9, 9, 9])], new Map());
    const { transport } = make({ server });
    const seen: LinkLoad["status"][] = [];
    transport.watch((load) => seen.push(load.status));
    await wait(60);
    await wait(600);
    expect(seen).toEqual(["invalid"]);
    expect(server.seen).toHaveLength(1);
  });

  it("reports being offline and then being back, with the page staying where it is", async () => {
    const { server, transport } = make();
    await transport.load();
    const seen: LinkLoad["status"][] = [];
    const stop = transport.watch((load) => seen.push(load.status));
    server.failure = "network";
    await wait(60);
    await wait(60);
    expect(seen).toEqual(["unavailable", "unavailable"]);
    server.failure = undefined;
    await wait(60);
    // The server is back and nothing changed: told once, so that the page can drop its notice.
    expect(seen).toEqual(["unavailable", "unavailable", "unchanged"]);
    await wait(60);
    expect(seen).toHaveLength(3);
    stop();
  });

  it("waits twice as long after each answer of 429", async () => {
    const { server, transport } = make();
    await transport.load();
    const stop = transport.watch(() => undefined);
    server.failure = 429;
    await wait(60); // poll 1 -> 429
    const afterFirst = server.seen.length;
    await wait(60); // not yet: the wait is 120 s now
    expect(server.seen).toHaveLength(afterFirst);
    await wait(60); // poll 2 -> 429
    expect(server.seen).toHaveLength(afterFirst + 1);
    await wait(180); // the wait is 240 s now
    expect(server.seen).toHaveLength(afterFirst + 1);
    server.failure = undefined;
    await wait(60); // poll 3 -> 304, and the pace is back to 60 s
    expect(server.seen).toHaveLength(afterFirst + 2);
    await wait(60);
    expect(server.seen).toHaveLength(afterFirst + 3);
    stop();
  });
});

describe("the page document", () => {
  it("opens as raw Loro bytes that loro-crdt imports, with the page's text", async () => {
    const { transport } = make();
    await transport.load();
    const bytes = await transport.docs.open(fixtures.description.pageId);
    // The `KBD1` blob header is not part of what the editor imports.
    expect(String.fromCharCode(...bytes.subarray(0, 4))).not.toBe("KBD1");
    const doc = new LoroDoc();
    doc.import(bytes);
    const text = JSON.stringify(doc.toJSON());
    expect(text).toContain(fixtures.description.texts[0]);
    expect(text).toContain("tickets");
    expect(text).toContain(fixtures.description.attachment.attachmentId);
    expect(text).not.toContain(fixtures.description.texts[2]);
  });

  it("is NotFound before the link loaded and for a page that is not the link's", async () => {
    const { transport } = make();
    await expect(transport.docs.open(fixtures.description.pageId)).rejects.toMatchObject({ code: "NotFound" });
    await transport.load();
    await expect(transport.docs.open("00000000-0000-4000-8000-000000000001")).rejects.toMatchObject({ code: "NotFound" });
  });
});

describe("the attachments", () => {
  const id = fixtures.description.attachment.attachmentId;

  it("reads a picture whole, decrypted by kb-wasm from the chunk the server serves", async () => {
    const { server, transport } = make();
    await transport.load();
    const bytes = await transport.attachments?.read(id);
    expect(bytes).toEqual(fixtures.picture);
    const hash = fixtures.description.attachment.chunks[0];
    expect(server.seen.at(-1)?.url).toBe(`/v2/links/${fixtures.description.linkId}/chunks/${String(hash)}`);
  });

  it("fetches a chunk once for a picture used twice, and keeps it for the page drawn again", async () => {
    const { server, transport } = make();
    await transport.load();
    const [first, second] = await Promise.all([transport.attachments?.read(id), transport.attachments?.read(id)]);
    expect(first).toEqual(fixtures.picture);
    expect(second).toEqual(fixtures.picture);
    await transport.attachments?.read(id, { start: 0, end: 4 });
    expect(server.seen.filter((request) => request.url.includes("/chunks/"))).toHaveLength(1);
  });

  it("reads a byte range of it, and refuses one outside the file", async () => {
    const { transport } = make();
    await transport.load();
    expect(await transport.attachments?.read(id, { start: 10, end: 30 })).toEqual(fixtures.picture.subarray(10, 30));
    expect(await transport.attachments?.read(id, { start: 5, end: 5 })).toEqual(new Uint8Array());
    const size = fixtures.picture.length;
    for (const range of [{ start: 0, end: size + 1 }, { start: 9, end: 3 }, { start: -1, end: 4 }]) {
      await expect(transport.attachments?.read(id, range)).rejects.toMatchObject({ code: "InvalidInput" });
    }
  });

  it("describes it as the core would, and names the people the page mentions", async () => {
    const { transport } = make();
    await transport.load();
    const info = await transport.dispatch({ type: "AttachmentGet", data: { attachmentId: id } });
    expect(info).toEqual({
      type: "Attachment",
      data: expect.objectContaining({
        attachmentId: id,
        spaceId: VIEWER_SPACE_ID,
        name: "picture.png",
        mime: "image/png",
        size: fixtures.picture.length,
        availability: "Complete",
      }) as unknown,
    });
    expect(await transport.dispatch({ type: "PeopleMentionable", data: { spaceId: VIEWER_SPACE_ID } })).toEqual({
      type: "People",
      data: [{ accountId: fixtures.description.mentions[0]?.accountId, name: "Bob", verified: false, isMe: false }],
    });
  });

  it("is NotFound for an attachment the page does not use", async () => {
    const { transport } = make();
    await transport.load();
    const other = "00000000-0000-4000-8000-0000000000aa";
    await expect(transport.attachments?.read(other)).rejects.toMatchObject({ code: "NotFound" });
    await expect(transport.dispatch({ type: "AttachmentGet", data: { attachmentId: other } })).rejects.toMatchObject({ code: "NotFound" });
  });

  it("is damaged, not shown, when the chunk the server serves is not the one the page lists", async () => {
    const { server, transport } = make();
    await transport.load();
    const swapped = new Uint8Array(fixtures.chunk);
    swapped[40] = (swapped[40] ?? 0) ^ 1;
    server.chunkBody = swapped;
    await expect(transport.attachments?.read(id)).rejects.toMatchObject({ code: "Crypto" });
  });

  it("is unavailable while the feed does not hold the chunk, and the next poll draws the page again to ask once more", async () => {
    const server = new FakeServer(fixtures.description.linkId, fixtures.publications, new Map());
    const { transport } = make({ server });
    await transport.load();
    await expect(transport.attachments?.read(id)).rejects.toMatchObject({ code: "NotFound" });
    // What is shown is current (304), but a picture was missing: the same publication is handed over once more...
    const retry = await transport.load();
    expect(retry).toMatchObject({ status: "ready", pubSeq: 1, retry: true });
    // ...and only once; then the poll is quiet again.
    expect(await transport.load()).toEqual({ status: "unchanged" });
  });
});

describe("everything else", () => {
  it("refuses every write and every request the page does not need", async () => {
    const { transport } = make();
    await transport.load();
    await expect(transport.docs.import("p", new Uint8Array())).rejects.toMatchObject({ code: "Forbidden" });
    await expect(transport.attachments?.write("a", new Uint8Array())).rejects.toMatchObject({ code: "Forbidden" });
    for (const request of [{ type: "AppStatus" }, { type: "PageCreate", data: { spaceId: "s", parentId: null, title: "x" } }] as const) {
      await expect(transport.dispatch(request as never)).rejects.toMatchObject({ code: "Forbidden" });
    }
  });

  it("has no listeners to leak: subscribing and unsubscribing is harmless", () => {
    const { transport } = make();
    transport.subscribe(() => undefined)();
  });
});
