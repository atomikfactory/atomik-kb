// What the feed layout asks of the outside (ADR-0058, `docs/architecture/database-power-ups.md` section 6.1):
//
// - `usePageExcerpts`: the excerpt of each page the feed shows. The core finds it (`PageExcerpts`, answered from the text
//   index) and the store (`pageExcerpts`: chunks of 500 ids, a cache, no repeated request) answers it for the pages it is
//   given. The hook asks again when the store says an answer may be out of date (`watchExcerpts`: the content of a page it
//   asked about changed) and keeps the last answer while the next is on its way. A lookup that fails leaves the cards
//   without an excerpt.
// - `useReachedEnd`: calls back when the last card comes into view (an `IntersectionObserver` on the scrolling body of the
//   page), so the feed adds a page of cards as the person scrolls; "Show more" does the same for a keyboard or a webview with
//   no observer.
//
// Owner: agent F of the database power-ups batch.

import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { useContext, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";

import { StoreContext } from "../store-context";
import { excerptMap, FEED_REFRESH_DELAY_MS, sameExcerpts } from "./page-feed-model";
import { NEAR_MARGIN_PX, scrollParentOf } from "./page-gallery-images";

const NO_EXCERPTS: ReadonlyMap<PageId, string> = new Map();

interface Answer {
  readonly spaceId: SpaceId;
  readonly texts: ReadonlyMap<PageId, string>;
}

/**
 * The excerpt of each of `pageIds` (pages of `spaceId`) that has one; an empty map while the first answer is on its way, with
 * no store, and for a space other than the one the last answer was for. The map keeps its identity while nothing changed.
 */
export function usePageExcerpts(spaceId: SpaceId, pageIds: readonly PageId[]): ReadonlyMap<PageId, string> {
  const store = useContext(StoreContext);
  const [answer, setAnswer] = useState<Answer>({ spaceId, texts: NO_EXCERPTS });
  // Bumped when the store says an answer may be out of date: asks again.
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (store === null) {
      return;
    }
    let timer: ReturnType<typeof setTimeout> | null = null;
    const stop = store.getState().watchExcerpts(() => {
      if (timer !== null) {
        clearTimeout(timer);
      }
      timer = setTimeout(() => {
        timer = null;
        setRevision((current) => current + 1);
      }, FEED_REFRESH_DELAY_MS);
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
      .pageExcerpts(spaceId, pageIds)
      .then(
        (found) => {
          if (!current) {
            return;
          }
          const texts = excerptMap(found);
          setAnswer((last) => (last.spaceId === spaceId && sameExcerpts(last.texts, texts) ? last : { spaceId, texts }));
        },
        () => {
          // The cards keep what they showed (no excerpt, or the last answer): a lookup that fails is not worth a message.
        },
      );
    return () => {
      current = false;
    };
  }, [store, spaceId, pageIds, revision]);

  return answer.spaceId === spaceId ? answer.texts : NO_EXCERPTS;
}

/**
 * Calls `onReach` when the element in `target` comes near the viewport (within `NEAR_MARGIN_PX` of the scrolling ancestor, or of
 * the window), once for each value of `cardCount`: the feed passes how many cards it draws, so the observer is made again for
 * the new last card, and answers again at once when that card is in view too (a tall window fills up). Does nothing while
 * `enabled` is false or where there is no `IntersectionObserver` (a test, an old webview): "Show more" is there for that.
 */
export function useReachedEnd(target: RefObject<Element | null>, enabled: boolean, onReach: () => void, cardCount: number): void {
  // The latest callback, so that a feed that draws again does not make the observer again (it would answer at once).
  const latest = useRef(onReach);
  useEffect(() => {
    latest.current = onReach;
  });

  useEffect(() => {
    const element = target.current;
    if (!enabled || element === null || typeof IntersectionObserver === "undefined") {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          latest.current();
        }
      },
      { root: scrollParentOf(element), rootMargin: `${String(NEAR_MARGIN_PX)}px 0px` },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
    };
  }, [target, enabled, cardCount]);
}
