// The attachment slice of the store (store-attachments.ts) under attachment sync (Milestone
// 8, ADR-0023): `AttachmentsChanged` reads again the lookups of its space and every lookup
// that is missing, failed or still loading; an answer that changes nothing leaves the store
// as it was; `downloadAttachment` asks the core and remembers its answer.

import { CoreClient, CoreError, MemoryTransport, demoImagePng } from "@atomik-kb/core-client";
import type { Request, Transport } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createAppStore } from "./store";
import { withTransfers } from "./test/transfers";

afterEach(() => {
  vi.restoreAllMocks();
});

const CAT = "a0000000-0000-4000-8000-0000000000ca";
const DOG = "a0000000-0000-4000-8000-0000000000d0";
const OTHER_SPACE = "b0000000-0000-4000-8000-000000000001";

async function setup() {
  const memory = new MemoryTransport({
    workspaceName: "Research",
    spaces: [{ name: "Personal" }],
    attachments: [
      { attachmentId: CAT, name: "cat.png", mime: "image/png", bytes: demoImagePng() },
      { attachmentId: DOG, name: "dog.png", mime: "image/png", bytes: demoImagePng() },
    ],
  });
  const transport: Transport = {
    dispatch: (request) => memory.dispatch(request),
    subscribe: (onEvent) => memory.subscribe(onEvent),
    coreState: () => memory.coreState(),
    docs: memory.docs,
    attachments: memory.attachments,
  };
  const transfers = withTransfers(transport);
  // Outside the rewriting: every request the store sends, and storage failures on demand.
  const requests: Request[] = [];
  let failGets = false;
  const rewritten = transport.dispatch.bind(transport);
  transport.dispatch = (request) => {
    requests.push(request);
    if (failGets && request.type === "AttachmentGet") {
      return Promise.reject(new CoreError({ code: "Storage", message: "disk I/O error" }));
    }
    return rewritten(request);
  };
  const store = createAppStore(new CoreClient(transport), { corePollMs: 5 });
  const stop = store.getState().start();
  await vi.waitFor(() => {
    expect(store.getState().phase).toBe("ready");
  });
  const [space] = await new CoreClient(memory).spaceList();
  const gets = (attachmentId: string): number =>
    requests.filter((request) => request.type === "AttachmentGet" && request.data.attachmentId === attachmentId).length;
  const lookup = (attachmentId: string) => store.getState().attachmentLookup[attachmentId];
  return {
    store,
    stop,
    transfers,
    gets,
    lookup,
    spaceId: space?.id ?? "",
    failGets: (fail: boolean) => {
      failGets = fail;
    },
  };
}

/** Lets the refreshes an event started settle. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 10));
}

describe("attachment lookups on AttachmentsChanged", () => {
  it("read again those of the space the event names, and those that are missing or failed whatever the space", async () => {
    const { store, stop, transfers, gets, lookup, spaceId, failGets } = await setup();
    transfers.set(DOG, "absent");
    store.getState().lookupAttachment(CAT);
    store.getState().lookupAttachment(DOG);
    await vi.waitFor(() => {
      expect(lookup(CAT)?.status).toBe("found");
      expect(lookup(DOG)).toEqual({ status: "missing" });
    });

    // Another space: the found lookup stays, the missing one is read again and is there now.
    transfers.set(DOG, null);
    transfers.changed(OTHER_SPACE);
    await vi.waitFor(() => {
      expect(lookup(DOG)?.status).toBe("found");
    });
    expect(gets(CAT)).toBe(1);

    // A failed lookup is read again too.
    failGets(true);
    transfers.changed(spaceId);
    await vi.waitFor(() => {
      expect(lookup(CAT)).toEqual({ status: "error", message: "disk I/O error" });
    });
    failGets(false);
    transfers.changed(OTHER_SPACE);
    await vi.waitFor(() => {
      expect(lookup(CAT)?.status).toBe("found");
    });
    stop();
  });

  it("follow the chunks of an attachment as they arrive", async () => {
    const { store, stop, transfers, lookup, spaceId } = await setup();
    transfers.set(CAT, { chunkCount: 3, localChunks: 1, availability: "Downloading" });
    store.getState().lookupAttachment(CAT);
    await vi.waitFor(() => {
      expect(lookup(CAT)).toMatchObject({ status: "found", info: { localChunks: 1, availability: "Downloading" } });
    });
    transfers.set(CAT, { chunkCount: 3, localChunks: 2, availability: "Downloading" });
    transfers.changed(spaceId);
    await vi.waitFor(() => {
      expect(lookup(CAT)).toMatchObject({ status: "found", info: { localChunks: 2 } });
    });
    transfers.set(CAT, null);
    transfers.changed(spaceId);
    await vi.waitFor(() => {
      expect(lookup(CAT)).toMatchObject({ status: "found", info: { localChunks: 1, availability: "Complete" } });
    });
    stop();
  });

  it("leave the store as it was when nothing changed", async () => {
    const { store, stop, transfers, gets, spaceId } = await setup();
    transfers.set(DOG, "absent");
    store.getState().lookupAttachment(CAT);
    store.getState().lookupAttachment(DOG);
    await vi.waitFor(() => {
      expect(Object.values(store.getState().attachmentLookup).map((lookup) => lookup.status)).toEqual(["found", "missing"]);
    });
    const before = store.getState().attachmentLookup;
    const listener = vi.fn();
    const unsubscribe = store.subscribe((state, previous) => {
      if (state.attachmentLookup !== previous.attachmentLookup) {
        listener();
      }
    });

    transfers.changed(spaceId);
    await vi.waitFor(() => {
      expect([gets(CAT), gets(DOG)]).toEqual([2, 2]);
    });
    await settle();
    expect(store.getState().attachmentLookup).toBe(before);
    expect(listener).not.toHaveBeenCalled();
    unsubscribe();
    stop();
  });
});

describe("downloadAttachment", () => {
  it("asks the core for the attachment and remembers its answer", async () => {
    const { store, stop, transfers, lookup } = await setup();
    transfers.set(CAT, { localChunks: 0, availability: "NotDownloaded" });
    store.getState().lookupAttachment(CAT);
    await vi.waitFor(() => {
      expect(lookup(CAT)).toMatchObject({ status: "found", info: { availability: "NotDownloaded" } });
    });
    const info = await store.getState().downloadAttachment(CAT);
    expect(info).toMatchObject({ attachmentId: CAT, availability: "Downloading", localChunks: 0 });
    expect(lookup(CAT)).toEqual({ status: "found", info });
    stop();
  });

  it("rejects with the core's error and keeps what it knew", async () => {
    const { store, stop, transfers, lookup } = await setup();
    transfers.set(CAT, "absent");
    await expect(store.getState().downloadAttachment(CAT)).rejects.toMatchObject({ code: "NotFound" });
    expect(lookup(CAT)).toBeUndefined();
    stop();
  });
});
