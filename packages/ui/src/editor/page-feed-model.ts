// The rules of the feed layout that need no DOM (ADR-0058, `docs/architecture/database-power-ups.md` section 6.1): how many
// cards are drawn and how many more a "Show more" adds, and how an answer of excerpts is read. `PageFeedView.tsx` draws the
// cards; `page-feed-hooks.ts` asks the store for the excerpts (in chunks of 500, `store-feed.ts`) and watches the last card.
//
// Owner: agent F of the database power-ups batch.

import type { PageExcerpt, PageId } from "@atomik-kb/core-client";

/** Cards drawn at first, and added by each "Show more" (or by the last card coming into view). */
export const FEED_PAGE_SIZE = 25;

/** How long after the last change event the feed asks again: a burst (an import, a sync, typing) is one request. */
export const FEED_REFRESH_DELAY_MS = 200;

/** How many cards are drawn when `requested` were asked for and `total` rows exist: never more than there are, never a negative number. */
export function shownCount(requested: number, total: number): number {
  return Math.max(0, Math.min(Math.floor(requested), total));
}

/**
 * What a "Show more" asks for next: one more page of cards than are drawn now (so a count left over from a longer list of
 * rows does not make the next click add a whole screen).
 */
export function nextRequested(requested: number, total: number): number {
  return shownCount(requested, total) + FEED_PAGE_SIZE;
}

/** Whether rows are left that the feed does not draw. */
export function hasMoreCards(requested: number, total: number): boolean {
  return shownCount(requested, total) < total;
}

/** The cards drawn: the first rows, in the order the view gives them. */
export function shownRows<T>(rows: readonly T[], requested: number): readonly T[] {
  return rows.slice(0, shownCount(requested, rows.length));
}

/** The text of each page of an answer; a page that is not in it has none, and an empty text is none too. */
export function excerptMap(found: readonly PageExcerpt[]): ReadonlyMap<PageId, string> {
  const texts = new Map<PageId, string>();
  for (const { pageId, text } of found) {
    if (text !== "" && !texts.has(pageId)) {
      texts.set(pageId, text);
    }
  }
  return texts;
}

/** Whether two answers say the same (a feed keeps the map it has while nothing changed, so its cards do not draw again). */
export function sameExcerpts(a: ReadonlyMap<PageId, string>, b: ReadonlyMap<PageId, string>): boolean {
  if (a.size !== b.size) {
    return false;
  }
  for (const [pageId, text] of a) {
    if (b.get(pageId) !== text) {
      return false;
    }
  }
  return true;
}
