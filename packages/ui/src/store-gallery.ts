// The gallery's part of the app store (ADR-0054, `docs/architecture/database-views.md` section 6): the lookup of the
// first image of pages. The core answers `PageFirstImages` from its device-local index of the pages' first image
// blocks; this slice adds what a screen of cards needs on top of the raw request:
//
// - **chunks**: at most 500 ids per request (the core refuses more), so a long gallery asks in several;
// - **de-duplication**: an id that is already being asked is waited for, not asked again, and an id is asked once
//   however many times the list repeats it (two galleries on one page share their requests);
// - **a cache** of the answers, pages without an image included (so a gallery that draws again does not ask again),
//   kept only while a gallery is watching (`watchFirstImages`): the cache listens to the core's events while it is
//   watched, marks a page stale when its content changes (`PageContentChanged`), the pages of a space when its tree
//   changes (`StructureChanged`) and the pages that moved (`PagesMovedSpace`), and drops everything when the last
//   watcher goes (it would no longer hear the events).
//
// Owner: agent D.

import type { AttachmentId, CoreClient, Event as CoreEvent, PageFirstImage, PageId, SpaceId } from "@atomik-kb/core-client";

/** The most ids one `PageFirstImages` request takes (`InvalidInput` for more). */
export const FIRST_IMAGES_CHUNK = 500;

export interface GalleryActions {
  /**
   * The first image of each of `pageIds` that has one, in the order asked, each page once (any number of ids: they go to
   * the core in chunks of 500). Pages without an image, trashed pages and pages of another space are not in the answer.
   * Answers from the cache while a gallery watches (`watchFirstImages`). Rejects with a `CoreError`; what the chunks that
   * did answer found stays cached.
   */
  pageFirstImages: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageFirstImage[]>;
  /**
   * Starts watching: `listener` is called when an answer already given may be out of date (a page whose first image was
   * asked changed, or its space's tree did), so the gallery asks again. Returns what stops watching.
   */
  watchFirstImages: (listener: () => void) => () => void;
}

export type GalleryState = GalleryActions;

export interface GallerySlice {
  /** The actions, to be part of the store's initial state. */
  initial: GalleryState;
}

/** What the cache knows of one page: its first image (`null`: none) in a space, and whether that may be out of date. */
interface Entry {
  readonly spaceId: SpaceId;
  readonly attachmentId: AttachmentId | null;
  readonly stale: boolean;
}

/** A request on its way for the pages of one chunk. */
interface Pending {
  readonly spaceId: SpaceId;
  /** The invalidation clock when the request left: a later invalidation makes its answer stale on arrival. */
  readonly startedAt: number;
  promise: Promise<ReadonlyMap<PageId, AttachmentId | null>>;
}

/** The cache and the request logic, apart from the store so that tests can drive it with a fake `source`. */
export class FirstImageCache {
  private readonly entries = new Map<PageId, Entry>();
  private readonly pending = new Map<PageId, Pending>();
  private readonly invalidatedAt = new Map<PageId, number>();
  private clock = 0;
  private watchers = 0;

  constructor(private readonly source: (spaceId: SpaceId, pageIds: readonly PageId[]) => Promise<PageFirstImage[]>) {}

  /** A gallery starts watching: answers are kept while at least one does. Returns what ends it. */
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

  /** Marks a page's answer out of date. True when the cache knew the page (a gallery has to ask again). */
  invalidatePage(pageId: PageId): boolean {
    const entry = this.entries.get(pageId);
    const asking = this.pending.has(pageId);
    if (entry === undefined && !asking) {
      return false;
    }
    this.clock += 1;
    this.invalidatedAt.set(pageId, this.clock);
    if (entry !== undefined && !entry.stale) {
      this.entries.set(pageId, { ...entry, stale: true });
    }
    return true;
  }

  /** Marks the answers of every page of `spaceId` out of date. True when the cache knew any. */
  invalidateSpace(spaceId: SpaceId): boolean {
    let known = false;
    for (const [pageId, entry] of [...this.entries]) {
      if (entry.spaceId === spaceId) {
        known = this.invalidatePage(pageId) || known;
      }
    }
    for (const [pageId, pending] of [...this.pending]) {
      if (pending.spaceId === spaceId) {
        known = this.invalidatePage(pageId) || known;
      }
    }
    return known;
  }

  /** The first images of `pageIds` (see `GalleryActions.pageFirstImages`). */
  async lookup(spaceId: SpaceId, pageIds: readonly PageId[]): Promise<PageFirstImage[]> {
    const ids = [...new Set(pageIds)];
    const values = new Map<PageId, AttachmentId | null>();
    const waits = new Set<Promise<ReadonlyMap<PageId, AttachmentId | null>>>();
    const toAsk: PageId[] = [];
    for (const id of ids) {
      const entry = this.entries.get(id);
      if (entry !== undefined && entry.spaceId === spaceId && !entry.stale) {
        values.set(id, entry.attachmentId);
        continue;
      }
      const pending = this.pending.get(id);
      if (pending !== undefined && pending.spaceId === spaceId && pending.startedAt >= (this.invalidatedAt.get(id) ?? 0)) {
        waits.add(pending.promise);
        continue;
      }
      toAsk.push(id);
    }
    for (let at = 0; at < toAsk.length; at += FIRST_IMAGES_CHUNK) {
      waits.add(this.ask(spaceId, toAsk.slice(at, at + FIRST_IMAGES_CHUNK)));
    }
    for (const answer of await Promise.all(waits)) {
      for (const [id, attachmentId] of answer) {
        if (!values.has(id)) {
          values.set(id, attachmentId);
        }
      }
    }
    const found: PageFirstImage[] = [];
    for (const id of ids) {
      const attachmentId = values.get(id);
      if (attachmentId !== undefined && attachmentId !== null) {
        found.push({ pageId: id, attachmentId });
      }
    }
    return found;
  }

  /** One request for at most 500 ids. */
  private ask(spaceId: SpaceId, ids: readonly PageId[]): Promise<ReadonlyMap<PageId, AttachmentId | null>> {
    const pending: Pending = { spaceId, startedAt: this.clock, promise: Promise.resolve(new Map()) };
    pending.promise = this.source(spaceId, ids).then(
      (answer) => {
        const found = new Map<PageId, AttachmentId | null>(ids.map((id): [PageId, null] => [id, null]));
        for (const { pageId, attachmentId } of answer) {
          if (found.has(pageId)) {
            found.set(pageId, attachmentId);
          }
        }
        for (const [id, attachmentId] of found) {
          // A newer request owns the id (it was invalidated and asked again): this answer is older.
          if (this.pending.get(id) !== pending) {
            continue;
          }
          this.pending.delete(id);
          if (this.watchers > 0) {
            this.entries.set(id, { spaceId, attachmentId, stale: (this.invalidatedAt.get(id) ?? 0) > pending.startedAt });
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

export function createGallerySlice(client: CoreClient): GallerySlice {
  const cache = new FirstImageCache((spaceId, pageIds) => client.pageFirstImages(spaceId, pageIds));
  const listeners = new Set<() => void>();
  let unsubscribe: (() => void) | null = null;

  function notify(): void {
    for (const listener of [...listeners]) {
      listener();
    }
  }

  function onEvent(event: CoreEvent): void {
    switch (event.type) {
      case "PageContentChanged":
        if (cache.invalidatePage(event.data.pageId)) {
          notify();
        }
        return;
      case "StructureChanged":
        if (cache.invalidateSpace(event.data.spaceId)) {
          notify();
        }
        return;
      case "PagesMovedSpace": {
        let known = false;
        for (const pageId of event.data.pageIds) {
          known = cache.invalidatePage(pageId) || known;
        }
        if (known) {
          notify();
        }
        return;
      }
      default:
        return;
    }
  }

  return {
    initial: {
      pageFirstImages: (spaceId, pageIds) => cache.lookup(spaceId, pageIds),
      watchFirstImages: (listener) => {
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
