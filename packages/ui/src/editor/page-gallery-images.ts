// The pictures of the gallery's cards (ADR-0054, `docs/architecture/database-views.md` section 6):
//
// - `usePageFirstImages`: the first image of each page a gallery shows. The core finds it (`PageFirstImages`, answered
//   from a device-local index of the pages' first image blocks) and the card shows it through the blob URL of the
//   attachment (`useIconImage`, refcounted, with the cover's size limit). A page with no image is not in the answer.
//   The hook asks the store (`pageFirstImages`: chunks of 500 ids, a cache, no repeated request) for the pages it is
//   given, asks again when the store says an answer may be out of date (`watchFirstImages`: a page's content or the
//   tree changed), and keeps the last answer while the next is on its way. A lookup that fails leaves the cards on
//   the icon.
// - `NearViewport`: tells a card when it is close to the viewport, so that only cards near it read their attachment.
//
// Owner: agent D.

import type { AttachmentId, PageId, SpaceId } from "@atomik-kb/core-client";
import { useCallback, useContext, useEffect, useState } from "react";

import { StoreContext } from "../store-context";

const NO_IMAGES: ReadonlyMap<PageId, AttachmentId> = new Map();

/** How long after the last change event the hook asks again: a burst (an import, a sync) is one request. */
export const REFRESH_DELAY_MS = 120;

/** How far, in pixels, above and below the viewport a card is already "near". */
export const NEAR_MARGIN_PX = 400;

interface Answer {
  readonly spaceId: SpaceId;
  readonly images: ReadonlyMap<PageId, AttachmentId>;
}

function sameImages(a: ReadonlyMap<PageId, AttachmentId>, b: ReadonlyMap<PageId, AttachmentId>): boolean {
  if (a.size !== b.size) {
    return false;
  }
  for (const [pageId, attachmentId] of a) {
    if (b.get(pageId) !== attachmentId) {
      return false;
    }
  }
  return true;
}

/**
 * The first image of each of `pageIds` (pages of `spaceId`) that has one; an empty map while the first answer is on its
 * way, with no store, and for a space other than the one the last answer was for. A gallery gives only the pages that
 * may need one (`needsFirstImage`). The map keeps its identity while nothing changed.
 */
export function usePageFirstImages(spaceId: SpaceId, pageIds: readonly PageId[]): ReadonlyMap<PageId, AttachmentId> {
  const store = useContext(StoreContext);
  const [answer, setAnswer] = useState<Answer>({ spaceId, images: NO_IMAGES });
  // Bumped when the store says an answer may be out of date: asks again.
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (store === null) {
      return;
    }
    let timer: ReturnType<typeof setTimeout> | null = null;
    const stop = store.getState().watchFirstImages(() => {
      if (timer !== null) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        timer = null;
        setRevision((current) => current + 1);
      }, REFRESH_DELAY_MS);
    });
    return () => {
      stop();
      if (timer !== null) {
        clearTimeout(timer);
      }
    };
  }, [store]);

  useEffect(() => {
    if (store === null || pageIds.length === 0) {
      return;
    }
    let current = true;
    store
      .getState()
      .pageFirstImages(spaceId, pageIds)
      .then(
        (found) => {
          if (!current) {
            return;
          }
          const images = new Map<PageId, AttachmentId>(found.map((entry): [PageId, AttachmentId] => [entry.pageId, entry.attachmentId]));
          setAnswer((last) => (last.spaceId === spaceId && sameImages(last.images, images) ? last : { spaceId, images }));
        },
        () => {
          // The cards keep what they showed (the icon, or the last answer): a lookup that fails is not worth a message.
        },
      );
    return () => {
      current = false;
    };
  }, [store, spaceId, pageIds, revision]);

  return answer.spaceId === spaceId ? answer.images : NO_IMAGES;
}

/** The nearest ancestor that scrolls (the page's body), or `null` for the viewport. */
export function scrollParentOf(element: Element): Element | null {
  for (let node = element.parentElement; node !== null; node = node.parentElement) {
    const overflow = getComputedStyle(node).overflowY;
    if (overflow === "auto" || overflow === "scroll" || overflow === "overlay") {
      return node;
    }
  }
  return null;
}

/**
 * Tells watched elements when they come near the viewport (within `NEAR_MARGIN_PX` of the scrolling ancestor of the first
 * element watched, or of the window): once per element, one `IntersectionObserver` for all of them, made when the first is
 * watched.
 * Where there is no `IntersectionObserver` (a test, an old webview) every element is near at once, so a picture is never
 * withheld for lack of the API; `supported` says which it is, for a caller that wants to know before it renders.
 */
export class NearViewport {
  private observer: IntersectionObserver | null = null;
  private readonly callbacks = new Map<Element, () => void>();

  constructor(private readonly margin: number = NEAR_MARGIN_PX) {}

  /** Whether this environment can tell. */
  static get supported(): boolean {
    return typeof IntersectionObserver !== "undefined";
  }

  /** Calls `onNear` once, when `element` is near. Returns what stops watching it. */
  watch(element: Element, onNear: () => void): () => void {
    if (!NearViewport.supported) {
      onNear();
      return () => undefined;
    }
    this.callbacks.set(element, onNear);
    this.observerOf(element).observe(element);
    return () => {
      this.callbacks.delete(element);
      this.observer?.unobserve(element);
    };
  }

  /** Stops watching everything; a later `watch` starts again. */
  dispose(): void {
    this.observer?.disconnect();
    this.observer = null;
    this.callbacks.clear();
  }

  private observerOf(first: Element): IntersectionObserver {
    this.observer ??= new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) {
            continue;
          }
          const onNear = this.callbacks.get(entry.target);
          if (onNear !== undefined) {
            this.callbacks.delete(entry.target);
            this.observer?.unobserve(entry.target);
            onNear();
          }
        }
      },
      { root: scrollParentOf(first), rootMargin: `${String(this.margin)}px 0px` },
    );
    return this.observer;
  }
}

/**
 * Whether the element in `ref` has been near the viewport (it stays true once it was: a picture that was read is not
 * given up on scrolling away). `viewport` is the gallery's, so the cards share an observer.
 */
export function useNearViewport(viewport: NearViewport): { ref: (element: Element | null) => (() => void) | undefined; near: boolean } {
  // Without the API every card is near from the first render (no state to set in an effect).
  const [near, setNear] = useState(!NearViewport.supported);
  const ref = useCallback(
    (element: Element | null) => {
      if (element === null || near) {
        return undefined;
      }
      return viewport.watch(element, () => {
        setNear(true);
      });
    },
    [viewport, near],
  );
  return { ref, near };
}
