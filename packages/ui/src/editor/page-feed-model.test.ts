// The pure rules of the feed layout (ADR-0058, `docs/architecture/database-power-ups.md` section 6.1): how many cards are drawn, what
// "Show more" asks for, the cards that follow from it, and how an answer of excerpts is read.

import type { PageExcerpt, PageId } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import {
  excerptMap,
  FEED_PAGE_SIZE,
  hasMoreCards,
  nextRequested,
  sameExcerpts,
  shownCount,
  shownRows,
} from "./page-feed-model";

const id = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;
const rows = (count: number): { id: PageId }[] => Array.from({ length: count }, (_, n) => ({ id: id(n + 1) }));

describe("paging", () => {
  it("draws 25 cards at first", () => {
    expect(FEED_PAGE_SIZE).toBe(25);
    expect(shownCount(FEED_PAGE_SIZE, 100)).toBe(25);
    expect(shownRows(rows(100), FEED_PAGE_SIZE)).toHaveLength(25);
  });

  it("never draws more cards than there are rows, or a negative number", () => {
    expect(shownCount(25, 10)).toBe(10);
    expect(shownCount(25, 0)).toBe(0);
    expect(shownCount(-5, 10)).toBe(0);
    expect(shownCount(7.9, 10)).toBe(7);
    expect(shownRows(rows(3), 25).map((row) => row.id)).toEqual([id(1), id(2), id(3)]);
  });

  it("draws the first rows, in the order the view gives them", () => {
    expect(shownRows(rows(60), 50).map((row) => row.id)).toEqual(rows(50).map((row) => row.id));
    expect(shownRows([], 25)).toEqual([]);
  });

  it("has more cards while rows are left over, and none when all are drawn", () => {
    expect(hasMoreCards(25, 26)).toBe(true);
    expect(hasMoreCards(25, 25)).toBe(false);
    expect(hasMoreCards(25, 10)).toBe(false);
    expect(hasMoreCards(25, 0)).toBe(false);
    expect(hasMoreCards(50, 60)).toBe(true);
    expect(hasMoreCards(75, 60)).toBe(false);
  });

  it("adds 25 cards at a time", () => {
    expect(nextRequested(25, 100)).toBe(50);
    expect(nextRequested(50, 100)).toBe(75);
    // The last page can be short: asking for more than there are draws them all.
    expect(shownCount(nextRequested(75, 80), 80)).toBe(80);
    expect(hasMoreCards(nextRequested(75, 80), 80)).toBe(false);
  });

  it("adds one page to what is drawn, not to a count left over from a longer list", () => {
    // 200 were asked for, then the filters left 30 rows: all 30 are drawn, and nothing more comes from the next click.
    expect(shownCount(200, 30)).toBe(30);
    expect(nextRequested(200, 30)).toBe(30 + FEED_PAGE_SIZE);
    expect(hasMoreCards(nextRequested(200, 30), 30)).toBe(false);
    // And when the list grows again to 100, the next page is 25 more than drawn.
    expect(nextRequested(25, 100)).toBe(50);
  });

  it("walks a list of 60 rows in three steps: 25, 50, 60", () => {
    let requested = FEED_PAGE_SIZE;
    const seen: number[] = [shownCount(requested, 60)];
    while (hasMoreCards(requested, 60)) {
      requested = nextRequested(requested, 60);
      seen.push(shownCount(requested, 60));
    }
    expect(seen).toEqual([25, 50, 60]);
  });
});

describe("excerptMap", () => {
  it("reads the text of each page of an answer", () => {
    const found: PageExcerpt[] = [
      { pageId: id(2), text: "Second page" },
      { pageId: id(1), text: "First\npage" },
    ];
    const texts = excerptMap(found);
    expect(texts.get(id(1))).toBe("First\npage");
    expect(texts.get(id(2))).toBe("Second page");
    expect(texts.size).toBe(2);
  });

  it("leaves out an empty text and keeps the first of a repeated page", () => {
    const texts = excerptMap([
      { pageId: id(1), text: "" },
      { pageId: id(2), text: "one" },
      { pageId: id(2), text: "two" },
    ]);
    expect(texts.has(id(1))).toBe(false);
    expect(texts.get(id(2))).toBe("one");
  });

  it("is empty for an empty answer", () => {
    expect(excerptMap([]).size).toBe(0);
  });
});

describe("sameExcerpts", () => {
  const map = (entries: [number, string][]): Map<PageId, string> => new Map(entries.map(([n, text]) => [id(n), text]));

  it("says two answers are the same when they hold the same pages with the same texts, in any order", () => {
    expect(sameExcerpts(map([]), map([]))).toBe(true);
    expect(
      sameExcerpts(
        map([
          [1, "a"],
          [2, "b"],
        ]),
        map([
          [2, "b"],
          [1, "a"],
        ]),
      ),
    ).toBe(true);
  });

  it("says they differ for another text, another page or another size", () => {
    expect(sameExcerpts(map([[1, "a"]]), map([[1, "b"]]))).toBe(false);
    expect(sameExcerpts(map([[1, "a"]]), map([[2, "a"]]))).toBe(false);
    expect(sameExcerpts(map([[1, "a"]]), map([]))).toBe(false);
  });
});
