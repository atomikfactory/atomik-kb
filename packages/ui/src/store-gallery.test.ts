// The gallery's lookup of the first image of pages (ADR-0054, section 6): chunks of at most 500 ids, no repeated or duplicate
// request, the cache that lives while a gallery watches, and what makes an answer stale (a page's content, its space's tree,
// a move), over a fake client with the requests under the test's control.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { AttachmentId, Event as CoreEvent, PageFirstImage, PageId, Request, Response, SpaceId, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { FIRST_IMAGES_CHUNK, FirstImageCache, createGallerySlice } from "./store-gallery";

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const OTHER_SPACE: SpaceId = "11111111-1111-4111-8111-222222222222";

const page = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;
const picture = (n: number): AttachmentId => `a0000000-0000-4000-8000-${String(n).padStart(12, "0")}`;

interface Call {
  spaceId: SpaceId;
  pageIds: readonly PageId[];
  answer: (images: PageFirstImage[]) => void;
  fail: (error: unknown) => void;
}

/** A source whose requests wait for the test to answer them. */
function manualSource() {
  const calls: Call[] = [];
  const source = (spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageFirstImage[]> =>
    new Promise((resolve, reject) => {
      calls.push({ spaceId, pageIds, answer: resolve, fail: reject });
    });
  return { calls, source };
}

/** A source that answers at once: page n has picture n when `has(n)`. */
function instantSource(has: (n: number) => boolean = () => true) {
  const calls: { spaceId: SpaceId; pageIds: readonly PageId[] }[] = [];
  const source = (spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageFirstImage[]> => {
    calls.push({ spaceId, pageIds });
    const images: PageFirstImage[] = [];
    for (const pageId of pageIds) {
      const n = Number(pageId.slice(-12));
      if (has(n)) {
        images.push({ pageId, attachmentId: picture(n) });
      }
    }
    return Promise.resolve(images);
  };
  return { calls, source };
}

describe("FirstImageCache.lookup", () => {
  it("answers the pages that have an image, in the order asked, and leaves the others out", async () => {
    const { source, calls } = instantSource((n) => n % 2 === 1);
    const cache = new FirstImageCache(source);
    const found = await cache.lookup(SPACE, [page(3), page(2), page(1), page(4)]);
    expect(found).toEqual([
      { pageId: page(3), attachmentId: picture(3) },
      { pageId: page(1), attachmentId: picture(1) },
    ]);
    expect(calls).toEqual([{ spaceId: SPACE, pageIds: [page(3), page(2), page(1), page(4)] }]);
  });

  it("asks nothing for no ids", async () => {
    const { source, calls } = instantSource();
    await expect(new FirstImageCache(source).lookup(SPACE, [])).resolves.toEqual([]);
    expect(calls).toEqual([]);
  });

  it("asks for an id once however many times the list has it", async () => {
    const { source, calls } = instantSource();
    const found = await new FirstImageCache(source).lookup(SPACE, [page(1), page(2), page(1), page(2), page(1)]);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.pageIds).toEqual([page(1), page(2)]);
    expect(found.map((entry) => entry.pageId)).toEqual([page(1), page(2)]);
  });

  it("splits more than 500 ids into requests of at most 500, the core refusing more", async () => {
    const { source, calls } = instantSource();
    const ids = Array.from({ length: 1201 }, (_, n) => page(n + 1));
    const found = await new FirstImageCache(source).lookup(SPACE, ids);
    expect(FIRST_IMAGES_CHUNK).toBe(500);
    expect(calls.map((call) => call.pageIds.length)).toEqual([500, 500, 201]);
    expect(calls.flatMap((call) => call.pageIds)).toEqual(ids);
    expect(found.map((entry) => entry.pageId)).toEqual(ids);
  });

  it("sends exactly 500 ids in one request", async () => {
    const { source, calls } = instantSource();
    await new FirstImageCache(source).lookup(SPACE, Array.from({ length: 500 }, (_, n) => page(n + 1)));
    expect(calls).toHaveLength(1);
  });

  it("waits for a request already on its way instead of asking again, for the ids it has and asks only for the rest", async () => {
    const { source, calls } = manualSource();
    const cache = new FirstImageCache(source);
    const first = cache.lookup(SPACE, [page(1), page(2)]);
    const second = cache.lookup(SPACE, [page(2), page(3)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(3)]]);
    calls[1]?.answer([{ pageId: page(3), attachmentId: picture(3) }]);
    calls[0]?.answer([{ pageId: page(2), attachmentId: picture(2) }]);
    await expect(first).resolves.toEqual([{ pageId: page(2), attachmentId: picture(2) }]);
    await expect(second).resolves.toEqual([
      { pageId: page(2), attachmentId: picture(2) },
      { pageId: page(3), attachmentId: picture(3) },
    ]);
  });

  it("rejects when a request fails, and the next lookup asks again", async () => {
    const { source, calls } = manualSource();
    const cache = new FirstImageCache(source);
    const failing = cache.lookup(SPACE, [page(1)]);
    calls[0]?.fail(new CoreError({ code: "Internal", message: "the index is busy" }));
    await expect(failing).rejects.toBeInstanceOf(CoreError);
    const again = cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    calls[1]?.answer([{ pageId: page(1), attachmentId: picture(1) }]);
    await expect(again).resolves.toHaveLength(1);
  });

  it("keeps what a chunk that answered found when another chunk fails", async () => {
    const { source, calls } = manualSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    const ids = Array.from({ length: 600 }, (_, n) => page(n + 1));
    const lookup = cache.lookup(SPACE, ids);
    calls[0]?.answer([{ pageId: page(1), attachmentId: picture(1) }]);
    calls[1]?.fail(new CoreError({ code: "Internal", message: "boom" }));
    await expect(lookup).rejects.toBeInstanceOf(CoreError);
    // The first 500 are cached; only the last 100 are asked again.
    const again = cache.lookup(SPACE, ids);
    expect(calls).toHaveLength(3);
    expect(calls[2]?.pageIds).toEqual(ids.slice(500));
    calls[2]?.answer([]);
    await expect(again).resolves.toEqual([{ pageId: page(1), attachmentId: picture(1) }]);
    release();
  });

  it("does not mix pages of two spaces", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1)]);
    await cache.lookup(OTHER_SPACE, [page(1)]);
    expect(calls.map((call) => call.spaceId)).toEqual([SPACE, OTHER_SPACE]);
    release();
  });
});

describe("the cache", () => {
  it("answers again from memory while a gallery watches, pages without an image included", async () => {
    const { source, calls } = instantSource((n) => n !== 2);
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    const again = await cache.lookup(SPACE, [page(1), page(2)]);
    expect(calls).toHaveLength(1);
    expect(again).toEqual([{ pageId: page(1), attachmentId: picture(1) }]);
    expect(cache.size).toBe(2);
    release();
  });

  it("asks only for the pages it does not know", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    await cache.lookup(SPACE, [page(2), page(3), page(1)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(3)]]);
    release();
  });

  it("keeps nothing when nobody watches: it would not hear the events that make an answer stale", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    await cache.lookup(SPACE, [page(1)]);
    await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    expect(cache.size).toBe(0);
  });

  it("drops everything when the last watcher goes, and counts watchers", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    const one = cache.retain();
    const two = cache.retain();
    await cache.lookup(SPACE, [page(1)]);
    one();
    one(); // a second release of the same watcher counts for nothing
    await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(1);
    two();
    expect(cache.size).toBe(0);
    await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
  });

  it("asks again for a page whose content changed, and only for it", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    expect(cache.invalidatePage(page(2))).toBe(true);
    await cache.lookup(SPACE, [page(1), page(2)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(2)]]);
    release();
  });

  it("says a page it never asked about is of no interest", async () => {
    const { source } = instantSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1)]);
    expect(cache.invalidatePage(page(9))).toBe(false);
    expect(cache.invalidateSpace(OTHER_SPACE)).toBe(false);
    release();
  });

  it("asks again for every page of a space whose tree changed", async () => {
    const { source, calls } = instantSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    await cache.lookup(OTHER_SPACE, [page(3)]);
    expect(cache.invalidateSpace(SPACE)).toBe(true);
    await cache.lookup(SPACE, [page(1), page(2)]);
    await cache.lookup(OTHER_SPACE, [page(3)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(3)], [page(1), page(2)]]);
    release();
  });

  it("marks an answer that was on its way when the page changed as out of date on arrival", async () => {
    const { source, calls } = manualSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    const first = cache.lookup(SPACE, [page(1)]);
    expect(cache.invalidatePage(page(1))).toBe(true);
    // The change happened after the request left: the next lookup asks again rather than waiting for it.
    const second = cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    // The older answer lands last and must not replace the newer one.
    calls[1]?.answer([{ pageId: page(1), attachmentId: picture(11) }]);
    await expect(second).resolves.toEqual([{ pageId: page(1), attachmentId: picture(11) }]);
    calls[0]?.answer([{ pageId: page(1), attachmentId: picture(10) }]);
    await expect(first).resolves.toEqual([{ pageId: page(1), attachmentId: picture(10) }]);
    const third = await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    expect(third).toEqual([{ pageId: page(1), attachmentId: picture(11) }]);
    release();
  });

  it("gives an answer that is out of date on arrival to the caller and asks again next time", async () => {
    const { source, calls } = manualSource();
    const cache = new FirstImageCache(source);
    const release = cache.retain();
    const first = cache.lookup(SPACE, [page(1)]);
    cache.invalidatePage(page(1));
    calls[0]?.answer([{ pageId: page(1), attachmentId: picture(10) }]);
    await expect(first).resolves.toEqual([{ pageId: page(1), attachmentId: picture(10) }]);
    const next = cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    calls[1]?.answer([]);
    await expect(next).resolves.toEqual([]);
    release();
  });
});

describe("the store's slice", () => {
  const docs = {
    open: () => Promise.resolve(new Uint8Array([1])),
    import: () => Promise.resolve(),
    export: () => Promise.resolve(new Uint8Array()),
    close: () => Promise.resolve(),
  };

  /** A client over a transport that answers `PageFirstImages` itself and lets the test emit core events. */
  function clientOf(has: (n: number) => boolean = () => true) {
    const requests: Request[] = [];
    const listeners = new Set<(event: CoreEvent) => void>();
    const transport: Transport = {
      dispatch: (request): Promise<Response> => {
        requests.push(request);
        if (request.type !== "PageFirstImages") {
          return Promise.reject(new Error(`unexpected request ${request.type}`));
        }
        const data: PageFirstImage[] = [];
        for (const pageId of request.data.pageIds) {
          const n = Number(pageId.slice(-12));
          if (has(n)) {
            data.push({ pageId, attachmentId: picture(n) });
          }
        }
        return Promise.resolve({ type: "PageFirstImages", data });
      },
      subscribe: (onEvent) => {
        listeners.add(onEvent);
        return () => {
          listeners.delete(onEvent);
        };
      },
      docs,
    };
    return {
      requests,
      listeners,
      slice: createGallerySlice(new CoreClient(transport)).initial,
      emit: (event: CoreEvent): void => {
        for (const listener of [...listeners]) {
          listener(event);
        }
      },
    };
  }

  it("sends PageFirstImages requests of at most 500 ids", async () => {
    const { slice, requests } = clientOf();
    const ids = Array.from({ length: 1000 }, (_, n) => page(n + 1));
    await expect(slice.pageFirstImages(SPACE, ids)).resolves.toHaveLength(1000);
    expect(requests.map((request) => (request.type === "PageFirstImages" ? request.data.pageIds.length : -1))).toEqual([500, 500]);
  });

  it("listens to the core only while someone watches", () => {
    const { slice, listeners } = clientOf();
    expect(listeners.size).toBe(0);
    const one = slice.watchFirstImages(() => undefined);
    const two = slice.watchFirstImages(() => undefined);
    expect(listeners.size).toBe(1);
    one();
    expect(listeners.size).toBe(1);
    two();
    two();
    expect(listeners.size).toBe(0);
  });

  it("tells the watchers when a page whose image was asked changed, and not for another page", async () => {
    const { slice, emit } = clientOf();
    const listener = vi.fn();
    const stop = slice.watchFirstImages(listener);
    await slice.pageFirstImages(SPACE, [page(1)]);
    emit({ type: "PageContentChanged", data: { pageId: page(9), source: "Editor" } });
    expect(listener).not.toHaveBeenCalled();
    emit({ type: "PageContentChanged", data: { pageId: page(1), source: "Remote" } });
    expect(listener).toHaveBeenCalledTimes(1);
    stop();
    emit({ type: "PageContentChanged", data: { pageId: page(1), source: "Remote" } });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("tells them when the tree of a space with asked pages changed, and when asked pages moved space", async () => {
    const { slice, emit } = clientOf();
    const listener = vi.fn();
    const stop = slice.watchFirstImages(listener);
    await slice.pageFirstImages(SPACE, [page(1)]);
    emit({ type: "StructureChanged", data: { spaceId: OTHER_SPACE } });
    expect(listener).not.toHaveBeenCalled();
    emit({ type: "StructureChanged", data: { spaceId: SPACE } });
    expect(listener).toHaveBeenCalledTimes(1);
    emit({ type: "PagesMovedSpace", data: { fromSpaceId: SPACE, toSpaceId: OTHER_SPACE, pageIds: [page(1)] } });
    expect(listener).toHaveBeenCalledTimes(2);
    emit({ type: "SpacesChanged" });
    expect(listener).toHaveBeenCalledTimes(2);
    stop();
  });

  it("asks the core again after a change event, and answers from memory in between", async () => {
    const { slice, emit, requests } = clientOf();
    const stop = slice.watchFirstImages(() => undefined);
    await slice.pageFirstImages(SPACE, [page(1), page(2)]);
    await slice.pageFirstImages(SPACE, [page(1), page(2)]);
    expect(requests).toHaveLength(1);
    emit({ type: "PageContentChanged", data: { pageId: page(2), source: "Editor" } });
    await slice.pageFirstImages(SPACE, [page(1), page(2)]);
    expect(requests).toHaveLength(2);
    stop();
  });
});
