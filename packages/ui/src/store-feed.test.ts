// The feed's lookup of the excerpts of pages (ADR-0058, section 6): chunks of at most 500 ids, no repeated or duplicate request,
// the cache that lives while a feed watches, and what drops an entry (the content of the page changed), over a fake client with
// the requests under the test's control.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { Event as CoreEvent, PageExcerpt, PageId, Request, Response, SpaceId, Transport } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { EXCERPTS_CHUNK, ExcerptCache, createFeedSlice } from "./store-feed";

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const OTHER_SPACE: SpaceId = "11111111-1111-4111-8111-222222222222";

const page = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;
const text = (n: number): string => `Text of page ${String(n)}`;

interface Call {
  spaceId: SpaceId;
  pageIds: readonly PageId[];
  answer: (found: PageExcerpt[]) => void;
  fail: (error: unknown) => void;
}

/** A source whose requests wait for the test to answer them. */
function manualSource() {
  const calls: Call[] = [];
  const source = (spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageExcerpt[]> =>
    new Promise((resolve, reject) => {
      calls.push({ spaceId, pageIds, answer: resolve, fail: reject });
    });
  return { calls, source };
}

/** A source that answers at once: page n has the text `text(n)` when `has(n)`. */
function instantSource(has: (n: number) => boolean = () => true) {
  const calls: { spaceId: SpaceId; pageIds: readonly PageId[] }[] = [];
  const source = (spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageExcerpt[]> => {
    calls.push({ spaceId, pageIds });
    const found: PageExcerpt[] = [];
    for (const pageId of pageIds) {
      const n = Number(pageId.slice(-12));
      if (has(n)) {
        found.push({ pageId, text: text(n) });
      }
    }
    return Promise.resolve(found);
  };
  return { calls, source };
}

describe("ExcerptCache.lookup", () => {
  it("answers the pages that have a text, in the order asked, and leaves the others out", async () => {
    const { source, calls } = instantSource((n) => n % 2 === 1);
    const cache = new ExcerptCache(source);
    const found = await cache.lookup(SPACE, [page(3), page(2), page(1), page(4)]);
    expect(found).toEqual([
      { pageId: page(3), text: text(3) },
      { pageId: page(1), text: text(1) },
    ]);
    expect(calls).toEqual([{ spaceId: SPACE, pageIds: [page(3), page(2), page(1), page(4)] }]);
  });

  it("answers in the order asked even when the core answers in another", async () => {
    const { source, calls } = manualSource();
    const lookup = new ExcerptCache(source).lookup(SPACE, [page(1), page(2), page(3)]);
    calls[0]?.answer([
      { pageId: page(3), text: text(3) },
      { pageId: page(1), text: text(1) },
    ]);
    await expect(lookup).resolves.toEqual([
      { pageId: page(1), text: text(1) },
      { pageId: page(3), text: text(3) },
    ]);
  });

  it("asks nothing for no ids", async () => {
    const { source, calls } = instantSource();
    await expect(new ExcerptCache(source).lookup(SPACE, [])).resolves.toEqual([]);
    expect(calls).toEqual([]);
  });

  it("asks for an id once however many times the list has it", async () => {
    const { source, calls } = instantSource();
    const found = await new ExcerptCache(source).lookup(SPACE, [page(1), page(2), page(1), page(2), page(1)]);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.pageIds).toEqual([page(1), page(2)]);
    expect(found.map((entry) => entry.pageId)).toEqual([page(1), page(2)]);
  });

  it("asks for the 25 cards a feed shows in one request", async () => {
    const { source, calls } = instantSource();
    const ids = Array.from({ length: 25 }, (_, n) => page(n + 1));
    await new ExcerptCache(source).lookup(SPACE, ids);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.pageIds).toEqual(ids);
  });

  it("splits more than 500 ids into requests of at most 500, the core refusing more", async () => {
    const { source, calls } = instantSource();
    const ids = Array.from({ length: 1201 }, (_, n) => page(n + 1));
    const found = await new ExcerptCache(source).lookup(SPACE, ids);
    expect(EXCERPTS_CHUNK).toBe(500);
    expect(calls.map((call) => call.pageIds.length)).toEqual([500, 500, 201]);
    expect(calls.flatMap((call) => call.pageIds)).toEqual(ids);
    expect(found.map((entry) => entry.pageId)).toEqual(ids);
  });

  it("sends exactly 500 ids in one request", async () => {
    const { source, calls } = instantSource();
    await new ExcerptCache(source).lookup(SPACE, Array.from({ length: 500 }, (_, n) => page(n + 1)));
    expect(calls).toHaveLength(1);
  });

  it("waits for a request already on its way instead of asking again, for the ids it has and asks only for the rest", async () => {
    const { source, calls } = manualSource();
    const cache = new ExcerptCache(source);
    const first = cache.lookup(SPACE, [page(1), page(2)]);
    const second = cache.lookup(SPACE, [page(2), page(3)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(3)]]);
    calls[1]?.answer([{ pageId: page(3), text: text(3) }]);
    calls[0]?.answer([{ pageId: page(2), text: text(2) }]);
    await expect(first).resolves.toEqual([{ pageId: page(2), text: text(2) }]);
    await expect(second).resolves.toEqual([
      { pageId: page(2), text: text(2) },
      { pageId: page(3), text: text(3) },
    ]);
  });

  it("rejects when a request fails, and the next lookup asks again", async () => {
    const { source, calls } = manualSource();
    const cache = new ExcerptCache(source);
    const failing = cache.lookup(SPACE, [page(1)]);
    calls[0]?.fail(new CoreError({ code: "Internal", message: "the index is busy" }));
    await expect(failing).rejects.toBeInstanceOf(CoreError);
    const again = cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    calls[1]?.answer([{ pageId: page(1), text: text(1) }]);
    await expect(again).resolves.toHaveLength(1);
  });

  it("keeps what a chunk that answered found when another chunk fails", async () => {
    const { source, calls } = manualSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    const ids = Array.from({ length: 600 }, (_, n) => page(n + 1));
    const lookup = cache.lookup(SPACE, ids);
    calls[0]?.answer([{ pageId: page(1), text: text(1) }]);
    calls[1]?.fail(new CoreError({ code: "Internal", message: "boom" }));
    await expect(lookup).rejects.toBeInstanceOf(CoreError);
    // The first 500 are cached; only the last 100 are asked again.
    const again = cache.lookup(SPACE, ids);
    expect(calls).toHaveLength(3);
    expect(calls[2]?.pageIds).toEqual(ids.slice(500));
    calls[2]?.answer([]);
    await expect(again).resolves.toEqual([{ pageId: page(1), text: text(1) }]);
    release();
  });

  it("does not mix pages of two spaces", async () => {
    const { source, calls } = instantSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1)]);
    await cache.lookup(OTHER_SPACE, [page(1)]);
    expect(calls.map((call) => call.spaceId)).toEqual([SPACE, OTHER_SPACE]);
    release();
  });

  it("ignores a page the core answers that was not asked for", async () => {
    const cache = new ExcerptCache(() =>
      Promise.resolve([
        { pageId: page(1), text: text(1) },
        { pageId: page(9), text: "stranger" },
      ]),
    );
    await expect(cache.lookup(SPACE, [page(1)])).resolves.toEqual([{ pageId: page(1), text: text(1) }]);
  });
});

describe("the cache", () => {
  it("answers again from memory while a feed watches, pages with no text included", async () => {
    const { source, calls } = instantSource((n) => n !== 2);
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    const again = await cache.lookup(SPACE, [page(1), page(2)]);
    // Page 2 has no text and is not asked about again.
    expect(calls).toHaveLength(1);
    expect(again).toEqual([{ pageId: page(1), text: text(1) }]);
    expect(cache.size).toBe(2);
    release();
  });

  it("asks only for the pages it does not know, as when a feed shows 25 more cards", async () => {
    const { source, calls } = instantSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    const firstPage = Array.from({ length: 25 }, (_, n) => page(n + 1));
    const secondPage = Array.from({ length: 25 }, (_, n) => page(n + 26));
    await cache.lookup(SPACE, firstPage);
    const found = await cache.lookup(SPACE, [...firstPage, ...secondPage]);
    expect(calls.map((call) => call.pageIds)).toEqual([firstPage, secondPage]);
    expect(found).toHaveLength(50);
    release();
  });

  it("keeps nothing when nobody watches: it would not hear the events that drop an entry", async () => {
    const { source, calls } = instantSource();
    const cache = new ExcerptCache(source);
    await cache.lookup(SPACE, [page(1)]);
    await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    expect(cache.size).toBe(0);
  });

  it("drops everything when the last watcher goes, and counts watchers", async () => {
    const { source, calls } = instantSource();
    const cache = new ExcerptCache(source);
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

  it("drops a page whose content changed, and only it: the next lookup asks for that page again", async () => {
    const { source, calls } = instantSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1), page(2)]);
    expect(cache.invalidatePage(page(2))).toBe(true);
    expect(cache.size).toBe(1);
    await cache.lookup(SPACE, [page(1), page(2)]);
    expect(calls.map((call) => call.pageIds)).toEqual([[page(1), page(2)], [page(2)]]);
    release();
  });

  it("says a page it never asked about is of no interest", async () => {
    const { source } = instantSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    await cache.lookup(SPACE, [page(1)]);
    expect(cache.invalidatePage(page(9))).toBe(false);
    release();
  });

  it("gives the new text after a change", async () => {
    let version = 1;
    const cache = new ExcerptCache((_space, pageIds) =>
      Promise.resolve(pageIds.map((pageId) => ({ pageId, text: `version ${String(version)}` }))),
    );
    const release = cache.retain();
    await expect(cache.lookup(SPACE, [page(1)])).resolves.toEqual([{ pageId: page(1), text: "version 1" }]);
    version = 2;
    await expect(cache.lookup(SPACE, [page(1)])).resolves.toEqual([{ pageId: page(1), text: "version 1" }]);
    cache.invalidatePage(page(1));
    await expect(cache.lookup(SPACE, [page(1)])).resolves.toEqual([{ pageId: page(1), text: "version 2" }]);
    release();
  });

  it("does not keep an answer that was on its way when the page changed, and a newer one is not replaced by it", async () => {
    const { source, calls } = manualSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    const first = cache.lookup(SPACE, [page(1)]);
    expect(cache.invalidatePage(page(1))).toBe(true);
    // The change happened after the request left: the next lookup asks again rather than waiting for it.
    const second = cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    // The older answer lands last and must not replace the newer one.
    calls[1]?.answer([{ pageId: page(1), text: "new" }]);
    await expect(second).resolves.toEqual([{ pageId: page(1), text: "new" }]);
    calls[0]?.answer([{ pageId: page(1), text: "old" }]);
    await expect(first).resolves.toEqual([{ pageId: page(1), text: "old" }]);
    const third = await cache.lookup(SPACE, [page(1)]);
    expect(calls).toHaveLength(2);
    expect(third).toEqual([{ pageId: page(1), text: "new" }]);
    release();
  });

  it("gives an answer that is out of date on arrival to the caller and asks again next time", async () => {
    const { source, calls } = manualSource();
    const cache = new ExcerptCache(source);
    const release = cache.retain();
    const first = cache.lookup(SPACE, [page(1)]);
    cache.invalidatePage(page(1));
    calls[0]?.answer([{ pageId: page(1), text: "old" }]);
    await expect(first).resolves.toEqual([{ pageId: page(1), text: "old" }]);
    expect(cache.size).toBe(0);
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

  /** A client over a transport that answers `PageExcerpts` itself and lets the test emit core events. */
  function clientOf(has: (n: number) => boolean = () => true) {
    const requests: Request[] = [];
    const listeners = new Set<(event: CoreEvent) => void>();
    const transport: Transport = {
      dispatch: (request): Promise<Response> => {
        requests.push(request);
        if (request.type !== "PageExcerpts") {
          return Promise.reject(new Error(`unexpected request ${request.type}`));
        }
        const data: PageExcerpt[] = [];
        for (const pageId of request.data.pageIds) {
          const n = Number(pageId.slice(-12));
          if (has(n)) {
            data.push({ pageId, text: text(n) });
          }
        }
        return Promise.resolve({ type: "PageExcerpts", data });
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
      slice: createFeedSlice(new CoreClient(transport)).initial,
      emit: (event: CoreEvent): void => {
        for (const listener of [...listeners]) {
          listener(event);
        }
      },
    };
  }

  const idsOf = (request: Request | undefined): number =>
    request?.type === "PageExcerpts" ? request.data.pageIds.length : -1;

  it("sends PageExcerpts requests of at most 500 ids", async () => {
    const { slice, requests } = clientOf();
    const ids = Array.from({ length: 1000 }, (_, n) => page(n + 1));
    await expect(slice.pageExcerpts(SPACE, ids)).resolves.toHaveLength(1000);
    expect(requests.map(idsOf)).toEqual([500, 500]);
  });

  it("names the space and the ids of the request", async () => {
    const { slice, requests } = clientOf();
    await slice.pageExcerpts(SPACE, [page(2), page(1)]);
    expect(requests).toEqual([{ type: "PageExcerpts", data: { spaceId: SPACE, pageIds: [page(2), page(1)] } }]);
  });

  it("listens to the core only while someone watches", () => {
    const { slice, listeners } = clientOf();
    expect(listeners.size).toBe(0);
    const one = slice.watchExcerpts(() => undefined);
    const two = slice.watchExcerpts(() => undefined);
    expect(listeners.size).toBe(1);
    one();
    expect(listeners.size).toBe(1);
    two();
    two();
    expect(listeners.size).toBe(0);
  });

  it("tells the watchers when a page whose excerpt was asked changed, and not for another page", async () => {
    const { slice, emit } = clientOf();
    const listener = vi.fn();
    const stop = slice.watchExcerpts(listener);
    await slice.pageExcerpts(SPACE, [page(1)]);
    emit({ type: "PageContentChanged", data: { pageId: page(9), source: "Editor" } });
    expect(listener).not.toHaveBeenCalled();
    emit({ type: "PageContentChanged", data: { pageId: page(1), source: "Remote" } });
    expect(listener).toHaveBeenCalledTimes(1);
    stop();
    emit({ type: "PageContentChanged", data: { pageId: page(1), source: "Remote" } });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("does not tell them about events that leave a page's text as it is", async () => {
    const { slice, emit } = clientOf();
    const listener = vi.fn();
    const stop = slice.watchExcerpts(listener);
    await slice.pageExcerpts(SPACE, [page(1)]);
    emit({ type: "StructureChanged", data: { spaceId: SPACE } });
    emit({ type: "SpacesChanged" });
    expect(listener).not.toHaveBeenCalled();
    stop();
  });

  it("asks the core again after a change event, only for the page that changed, and answers from memory in between", async () => {
    const { slice, emit, requests } = clientOf();
    const stop = slice.watchExcerpts(() => undefined);
    await slice.pageExcerpts(SPACE, [page(1), page(2)]);
    await slice.pageExcerpts(SPACE, [page(1), page(2)]);
    expect(requests).toHaveLength(1);
    emit({ type: "PageContentChanged", data: { pageId: page(2), source: "Editor" } });
    await slice.pageExcerpts(SPACE, [page(1), page(2)]);
    expect(requests).toHaveLength(2);
    expect(requests[1]).toEqual({ type: "PageExcerpts", data: { spaceId: SPACE, pageIds: [page(2)] } });
    stop();
  });

  it("asks again for everything once nobody watches any more", async () => {
    const { slice, requests } = clientOf();
    const stop = slice.watchExcerpts(() => undefined);
    await slice.pageExcerpts(SPACE, [page(1)]);
    stop();
    await slice.pageExcerpts(SPACE, [page(1)]);
    expect(requests).toHaveLength(2);
  });
});
