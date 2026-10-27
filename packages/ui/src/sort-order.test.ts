// The order of the sort commands (ADR-0052): numbers by value, accents and case ignored, empty last, ties broken
// by creation time and id; and the same by date.

import { describe, expect, it } from "vitest";

import { collatorFor, compareTitles, isSortedBy, pageComparator, sortedPageIds } from "./sort-order";
import type { SortablePage } from "./sort-order";

/** Pages with ids `p1`, `p2`, ... created one second apart in the order given. */
function pages(...titles: string[]): SortablePage[] {
  return titles.map((title, index) => ({ id: `p${String(index + 1)}`, title, createdAt: 1_000 * (index + 1) }));
}

/** The titles of `list` in the order of `by`. */
function titlesBy(list: SortablePage[], by: "title" | "created", locale = "en"): string[] {
  const byId = new Map(list.map((page) => [page.id, page.title]));
  return sortedPageIds(list, by, locale).map((id) => byId.get(id) ?? "?");
}

describe("the title order", () => {
  it("puts numbers by value: Page 2 before Page 10", () => {
    expect(titlesBy(pages("Page 10", "Page 2", "Page 1"), "title")).toEqual(["Page 1", "Page 2", "Page 10"]);
    expect(compareTitles("Page 2", "Page 10", "en")).toBeLessThan(0);
    expect(compareTitles("Page 10", "Page 2", "en")).toBeGreaterThan(0);
  });

  it("ignores case and accents at the primary level", () => {
    expect(titlesBy(pages("zebra", "Éclair", "apple", "Banana", "eclair"), "title")).toEqual([
      "apple",
      "Banana",
      "Éclair", // created before "eclair": the tie is the creation time
      "eclair",
      "zebra",
    ]);
    expect(compareTitles("école", "Ecole", "en")).toBe(0);
    expect(compareTitles("apple", "Apple", "en")).toBe(0);
    expect(compareTitles("Zèbre", "école", "en")).toBeGreaterThan(0);
  });

  it("sorts an empty or untitled title last, and two empty ones as equal", () => {
    expect(titlesBy(pages("", "Beta", "   ", "Alpha"), "title")).toEqual(["Alpha", "Beta", "", "   "]);
    expect(compareTitles("", "zzz", "en")).toBeGreaterThan(0);
    expect(compareTitles("zzz", "", "en")).toBeLessThan(0);
    expect(compareTitles("", "  ", "en")).toBe(0);
  });

  it("breaks ties by creation time, then by id, so the order is deterministic", () => {
    const tied: SortablePage[] = [
      { id: "b", title: "Same", createdAt: 5 },
      { id: "a", title: "same", createdAt: 5 },
      { id: "c", title: "SAME", createdAt: 1 },
    ];
    expect(sortedPageIds(tied, "title", "en")).toEqual(["c", "a", "b"]);
    // The same set in any input order gives the same answer.
    expect(sortedPageIds([...tied].reverse(), "title", "en")).toEqual(["c", "a", "b"]);
  });

  it("follows the language: French folds the accents like English, and a Swedish ä comes after z", () => {
    expect(titlesBy(pages("Zèbre", "Élan", "Écureuil", "abeille"), "title", "fr")).toEqual([
      "abeille",
      "Écureuil",
      "Élan",
      "Zèbre",
    ]);
    // The letter ä is a letter of its own after z in Swedish, and a variant of a in English.
    expect(titlesBy(pages("Zebra", "Apple", "Äpple"), "title", "sv")).toEqual(["Apple", "Zebra", "Äpple"]);
    expect(titlesBy(pages("Zebra", "Apple", "Äpple"), "title", "en")).toEqual(["Apple", "Äpple", "Zebra"]);
  });

  it("uses the active locale when none is given, and one collator per locale", () => {
    expect(collatorFor("fr")).toBe(collatorFor("fr"));
    expect(collatorFor("fr")).not.toBe(collatorFor("en"));
    expect(collatorFor("en").resolvedOptions()).toMatchObject({ numeric: true, sensitivity: "base" });
    // Tests run in English.
    expect(titlesBy(pages("b", "a"), "title")).toEqual(["a", "b"]);
    expect(compareTitles("a", "b")).toBeLessThan(0);
  });
});

describe("the date order", () => {
  it("puts the oldest first, whatever the titles", () => {
    const list: SortablePage[] = [
      { id: "c", title: "Alpha", createdAt: 300 },
      { id: "a", title: "Zulu", createdAt: 100 },
      { id: "b", title: "Mike", createdAt: 200 },
    ];
    expect(sortedPageIds(list, "created", "en")).toEqual(["a", "b", "c"]);
  });

  it("breaks ties by title, with an empty title last, then by id", () => {
    const list: SortablePage[] = [
      { id: "d", title: "", createdAt: 10 },
      { id: "c", title: "beta", createdAt: 10 },
      { id: "b", title: "Alpha", createdAt: 10 },
      { id: "a", title: "alpha", createdAt: 10 },
      { id: "z", title: "Early", createdAt: 1 },
    ];
    expect(sortedPageIds(list, "created", "en")).toEqual(["z", "a", "b", "c", "d"]);
  });
});

describe("what the sorts say about a list", () => {
  it("knows when pages already are in order, and leaves its input alone", () => {
    const list = pages("Apple", "Banana", "Cherry");
    expect(isSortedBy(list, "title", "en")).toBe(true);
    expect(isSortedBy(list, "created", "en")).toBe(true);
    const shuffled = [list[2], list[0], list[1]].filter((page): page is SortablePage => page !== undefined);
    expect(isSortedBy(shuffled, "title", "en")).toBe(false);
    expect(isSortedBy(shuffled, "created", "en")).toBe(false);
    expect(shuffled.map((page) => page.title)).toEqual(["Cherry", "Apple", "Banana"]); // not sorted in place
    expect(isSortedBy([], "title", "en")).toBe(true);
  });

  it("gives a comparator that is a total order (sign and transitivity on a sample)", () => {
    const list = pages("b", "A", "", "a", "10", "9", "é");
    const compare = pageComparator("title", "en");
    for (const x of list) {
      for (const y of list) {
        expect(Math.sign(compare(x, y)) + Math.sign(compare(y, x)), `${x.title}/${y.title}`).toBe(0);
      }
    }
  });
});
