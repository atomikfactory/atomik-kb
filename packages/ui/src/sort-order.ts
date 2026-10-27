// The order the "sort" commands put things in (ADR-0052): subpages in the page tree, and the items of a list in a page.
// One place, so the sidebar, the space menu, the slash menu and the list command agree.
//
// - **Collation.** `Intl.Collator` of the active language (`activeLocale()`), `numeric` (Page 2 before Page 10) and
//   `sensitivity: "base"` (case and accents do not decide the order: "école" and "Ecole" are the same word to it).
//   Collators are cached per locale: building one is the expensive part of a sort. Database views keep their own
//   locale-independent comparison on purpose (`page-list-model.ts`), this is not it.
// - **Empty last.** An empty or untitled title sorts after every other, whatever the language.
// - **Deterministic.** Equal titles break the tie by creation time and then by id (a plain string comparison), so two
//   devices that sort the same pages in the same language get the same order.

import type { PageId } from "@atomik-kb/core-client";

import { activeLocale } from "./i18n";

/** What a sort orders pages by: their title (A to Z) or the time they were created (oldest first). */
export type SortKey = "title" | "created";

/** The part of a page a sort reads. */
export interface SortablePage {
  id: PageId;
  title: string;
  /** Creation time, Unix ms. */
  createdAt: number;
}

const collators = new Map<string, Intl.Collator>();

/** The collator of a locale (default: the active one): numeric, accents and case ignored at the primary level. */
export function collatorFor(locale: string = activeLocale()): Intl.Collator {
  let collator = collators.get(locale);
  if (collator === undefined) {
    collator = new Intl.Collator(locale, { numeric: true, sensitivity: "base" });
    collators.set(locale, collator);
  }
  return collator;
}

/** Whether a title counts as none: empty, or only spaces. */
function isBlank(text: string): boolean {
  return text.trim() === "";
}

/**
 * Compares two texts in the order of a locale: numbers by value, accents and case ignored, an empty text after any
 * other (two empty ones are equal). Returns what a sort comparator returns.
 */
export function compareTitles(a: string, b: string, locale: string = activeLocale()): number {
  const blankA = isBlank(a);
  const blankB = isBlank(b);
  if (blankA || blankB) {
    return Number(blankA) - Number(blankB);
  }
  return collatorFor(locale).compare(a, b);
}

function compareIds(a: PageId, b: PageId): number {
  if (a === b) {
    return 0;
  }
  return a < b ? -1 : 1;
}

/** The comparator of a sort key: title, then creation time, then id; or creation time, then title, then id. */
export function pageComparator(by: SortKey, locale: string = activeLocale()): (a: SortablePage, b: SortablePage) => number {
  if (by === "title") {
    return (a, b) =>
      compareTitles(a.title, b.title, locale) || a.createdAt - b.createdAt || compareIds(a.id, b.id);
  }
  return (a, b) => a.createdAt - b.createdAt || compareTitles(a.title, b.title, locale) || compareIds(a.id, b.id);
}

/** The ids of `pages` in the order of `by` (a new array; the pages are left as they are). */
export function sortedPageIds(pages: readonly SortablePage[], by: SortKey, locale: string = activeLocale()): PageId[] {
  return [...pages].sort(pageComparator(by, locale)).map((page) => page.id);
}

/** Whether `pages`, in the order given, already stand in the order of `by`. */
export function isSortedBy(pages: readonly SortablePage[], by: SortKey, locale: string = activeLocale()): boolean {
  const sorted = sortedPageIds(pages, by, locale);
  return pages.every((page, at) => sorted[at] === page.id);
}
