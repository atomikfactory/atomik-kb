// The feed's part of the app store (ADR-0058, `docs/architecture/database-power-ups.md` section 6): the lookup of the excerpts of
// pages. The core answers `PageExcerpts` from the text index (the first lines of a page's text, at most 280 characters); this
// slice adds what a screen of cards needs on top of the raw request, the way `store-gallery.ts` does for the first images:
//
// - **chunks**: at most 500 ids per request (the core refuses more), so a long feed asks in several;
// - **de-duplication**: an id that is already being asked is waited for, not asked again, and an id is asked once however
//   many times the list repeats it (two feeds on one page share their requests);
// - **a cache** of the answers by page, pages with no text included (so a feed that draws again does not ask again), kept
//   only while a feed is watching (`watchExcerpts`): the cache listens to the core's events while it is watched and drops a
//   page's entry when its content changes (`PageContentChanged`, the event the gallery's cache listens to as well), and
//   everything when the last watcher goes (it would no longer hear the events). A page moved to another space is asked
//   again for that space (an entry belongs to one space).
//
// Owner: agent F of the database power-ups batch.

import type { CoreClient, Event as CoreEvent, PageExcerpt, PageId, SpaceId } from "@atomik-kb/core-client";

/** The most ids one `PageExcerpts` request takes (`InvalidInput` for more). */
export const EXCERPTS_CHUNK = 500;

export interface FeedActions {
  /**
   * The excerpt of each of `pageIds` that has one, in the order asked, each page once (any number of ids: they go to the
   * core in chunks of 500). Pages with no body text, templates, trashed pages and pages of another space are not in the
   * answer. Answers from the cache while a feed watches (`watchExcerpts`). Rejects with a `CoreError`; what the chunks that
   * did answer found stays cached.
   */
  pageExcerpts: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageExcerpt[]>;
  /**
   * Starts watching: `listener` is called when an answer already given may be out of date (the content of a page whose
   * excerpt was asked changed), so the feed asks again. Returns what stops watching.
   */
  watchExcerpts: (listener: () => void) => () => void;
}

export type FeedState = FeedActions;

export interface FeedSlice {
  /** The actions, to be part of the store's initial state. */
  initial: FeedState;
}

/** What the cache knows of one page: its excerpt in a space (`""`: the page has none). */
interface Entry {
  readonly spaceId: SpaceId;
  readonly text: string;
}

/** A request on its way for the pages of one chunk. */
interface Pending {
  readonly spaceId: SpaceId;
  /** The invalidation clock when the request left: a change after it makes its answer old on arrival. */
  readonly startedAt: number;
  promise: Promise<ReadonlyMap<PageId, string>>;
}

/** The cache and the request logic, apart from the store so that tests can drive it with a fake `source`. */
export class ExcerptCache {
  private readonly entries = new Map<PageId, Entry>();
  private readonly pending = new Map<PageId, Pending>();
  private readonly invalidatedAt = new Map<PageId, number>();
  private clock = 0;
  private watchers = 0;

  constructor(private readonly source: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageExcerpt[]>) {}

  /** A feed starts watching: answers are kept while at least one does. Returns what ends it. */
  retain(): () => void {
    this.watchers += 1;
    let released = false;
    return () => {
      if (released) {
        return;
      }
      released = true;
      this.watchers -= 1;
      if (this.watchers === 0) {
        // Nobody hears the core's events now: what is kept could go stale unseen.
        this.entries.clear();
        this.pending.clear();
        this.invalidatedAt.clear();
      }
    };
  }

  /** How many pages have an answer held by the cache. */
  get size(): number {
    return this.entries.size;
  }

  /** Drops a page's answer. True when the cache knew the page (a feed has to ask again). */
  invalidatePage(pageId: PageId): boolean {
    const known = this.entries.delete(pageId);
    const asking = this.pending.has(pageId);
    if (!known && !asking) {
      return false;
    }
    this.clock += 1;
    this.invalidatedAt.set(pageId, this.clock);
    return true;
  }

  /** The excerpts of `pageIds` (see `FeedActions.pageExcerpts`). */
  async lookup(spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageExcerpt[]> {
    const ids = [...new Set(pageIds)];
    const values = new Map<PageId, string>();
    const waits = new Set<Promise<ReadonlyMap<PageId, string>>>();
    const toAsk: PageId[] = [];
    for (const id of ids) {
      const entry = this.entries.get(id);
      if (entry !== undefined && entry.spaceId === spaceId) {
        values.set(id, entry.text);
        continue;
      }
      const pending = this.pending.get(id);
      if (pending !== undefined && pending.spaceId === spaceId && pending.startedAt >= (this.invalidatedAt.get(id) ?? 0)) {
        waits.add(pending.promise);
        continue;
      }
      toAsk.push(id);
    }
    for (let at = 0; at < toAsk.length; at += EXCERPTS_CHUNK) {
      waits.add(this.ask(spaceId, toAsk.slice(at, at + EXCERPTS_CHUNK)));
    }
    for (const answer of await Promise.all(waits)) {
      for (const [id, text] of answer) {
        if (!values.has(id)) {
          values.set(id, text);
        }
      }
    }
    const found: PageExcerpt[] = [];
    for (const id of ids) {
      const text = values.get(id);
      if (text !== undefined && text !== "") {
        found.push({ pageId: id, text });
      }
    }
    return found;
  }

  /** One request for at most 500 ids. */
  private ask(spaceId: SpaceId, ids: readonly PageId[]): Promise<ReadonlyMap<PageId, string>> {
    const pending: Pending = { spaceId, startedAt: this.clock, promise: Promise.resolve(new Map()) };
    pending.promise = this.source(spaceId, ids).then(
      (answer) => {
        const found = new Map<PageId, string>(ids.map((id): [PageId, string] => [id, ""]));
        for (const { pageId, text } of answer) {
          if (found.has(pageId)) {
            found.set(pageId, text);
          }
        }
        for (const [id, text] of found) {
          // A newer request owns the id (it was changed and asked again): this answer is older.
          if (this.pending.get(id) !== pending) {
            continue;
          }
          this.pending.delete(id);
          // An answer older than a change of the page goes to its caller but is not kept: the next lookup asks again.
          if (this.watchers > 0 && (this.invalidatedAt.get(id) ?? 0) <= pending.startedAt) {
            this.entries.set(id, { spaceId, text });
          }
        }
        return found;
      },
      (error: unknown) => {
        for (const id of ids) {
          if (this.pending.get(id) === pending) {
            this.pending.delete(id);
          }
        }
        throw error;
      },
    );
    for (const id of ids) {
      this.pending.set(id, pending);
    }
    return pending.promise;
  }
}

export function createFeedSlice(client: CoreClient): FeedSlice {
  const cache = new ExcerptCache((spaceId, pageIds) => client.pageExcerpts(spaceId, pageIds));
  const listeners = new Set<() => void>();
  let unsubscribe: (() => void) | null = null;

  function notify(): void {
    for (const listener of [...listeners]) {
      listener();
    }
  }

  function onEvent(event: CoreEvent): void {
    if (event.type === "PageContentChanged" && cache.invalidatePage(event.data.pageId)) {
      notify();
    }
  }

  return {
    initial: {
      pageExcerpts: (spaceId, pageIds) => cache.lookup(spaceId, pageIds),
      watchExcerpts: (listener) => {
        // One function per watcher, so that the same listener given twice is two watchers.
        const mine = (): void => {
          listener();
        };
        listeners.add(mine);
        const release = cache.retain();
        unsubscribe ??= client.subscribe(onEvent);
        let stopped = false;
        return () => {
          if (stopped) {
            return;
          }
          stopped = true;
          listeners.delete(mine);
          release();
          if (listeners.size === 0) {
            unsubscribe?.();
            unsubscribe = null;
          }
        };
      },
    },
  };
}
