// The fuzzy title matching of `MemoryTransport`'s `Search` (ADR-0040, item 9): the same algorithm as
// `kb-storage`'s `fuzzy.rs`, held to the same results by the fixture that Rust test writes.

import { describe, expect, it } from "vitest";

import { MIN_FUZZY_LETTERS, fuzzyQuery, fuzzyTitle, rankFuzzyTitles, withinOneEdit } from "./search-fuzzy";
import { readRepoText, repoFileExists } from "./testing/repo-files";

const FIXTURE = "fixtures/search/fuzzy-titles-v1.json";
const fixtureMissing = !repoFileExists(FIXTURE);
const note = fixtureMissing ? ` [skipped: ${FIXTURE} is missing; kb-storage's fuzzy_fixture test writes it]` : "";

interface Fixture {
  matches: { query: string; title: string; match: { score: number; highlights: [number, number][] } | null }[];
  rankings: { query: string; titles: string[]; ranked: number[] }[];
}

function fixture(): Fixture {
  return JSON.parse(readRepoText(FIXTURE)) as Fixture;
}

function matched(query: string, title: string) {
  const prepared = fuzzyQuery(query);
  return prepared === undefined ? undefined : fuzzyTitle(prepared, title);
}

/** The highlighted parts of a title, cut by UTF-16 offsets. */
function parts(title: string, found: { highlights: [number, number][] }): string[] {
  return found.highlights.map(([start, end]) => title.slice(start, end));
}

describe("the shared fixture with kb-storage", () => {
  it.skipIf(fixtureMissing)(`gives the core's score and highlights for every case${note}`, () => {
    const { matches } = fixture();
    expect(matches.length).toBeGreaterThan(30);
    for (const { query, title, match } of matches) {
      expect(matched(query, title) ?? null, `${query} in ${title}`).toEqual(match);
    }
  });

  it.skipIf(fixtureMissing)(`ranks titles like the core${note}`, () => {
    const { rankings } = fixture();
    expect(rankings.length).toBeGreaterThanOrEqual(3);
    for (const { query, titles, ranked } of rankings) {
      const found = rankFuzzyTitles(
        query,
        titles.map((title, at) => [at, title] as const),
        100,
      );
      expect(
        found.map((hit) => hit.key),
        query,
      ).toEqual(ranked);
    }
  });
});

describe("fuzzy titles", () => {
  it("find a title by the letters of the query in order", () => {
    const found = matched("rcp", "Recipes");
    expect(found && parts("Recipes", found)).toEqual(["R", "c", "p"]);
    expect(found?.score).toBe(8);
    expect(matched("gm", "Meeting")).toBeUndefined();
    expect(matched("mtg nts", "Meeting notes")).toBeDefined();
  });

  it("tolerate one typo in a word of four or more characters, against a prefix of a title word", () => {
    for (const query of ["meetign", "meeteng", "meetting", "meetng", "meetings"]) {
      const found = matched(query, "Meeting notes");
      expect(found && parts("Meeting notes", found), query).toEqual(["Meeting"]);
    }
    expect(matched("agxnd", "Agenda for monday")).toBeDefined();
    expect(matched("mxn", "Menu")).toBeUndefined();
    expect(matched("mneux", "Menu")).toBeUndefined();
  });

  it("score the start of a word and a run higher, and a typo lower", () => {
    const run = matched("note", "Notes");
    const scattered = matched("note", "Nxoxtxex");
    expect(run && scattered && run.score > scattered.score).toBe(true);
    const typo = matched("meetign", "Meeting");
    const right = matched("meeting", "Meeting");
    expect(typo && right && right.score > typo.score).toBe(true);
  });

  it("fold case and accents, and count highlights in UTF-16 units", () => {
    expect(matched("cafe", "Café noir")?.highlights).toEqual([[0, 4]]);
    expect(matched("cafe", "café")).toBeDefined();
    expect(matched("rcp", "😀 Recipes")?.highlights).toEqual([
      [3, 4],
      [5, 6],
      [7, 8],
    ]);
  });

  it("need at least two letters, and nothing for words that hold nothing", () => {
    expect(MIN_FUZZY_LETTERS).toBe(2);
    expect(fuzzyQuery("a")).toBeUndefined();
    expect(fuzzyQuery("!?")).toBeUndefined();
    expect(fuzzyQuery("AND OR")).toBeUndefined();
    expect(fuzzyQuery("ANDROID")).toBeDefined();
    expect(matched("ab", "")).toBeUndefined();
  });

  it("rank by score, then the shorter title, then the order given", () => {
    const titles = ["Scattered notes of every kind", "Notes", "Notes again", "Another one", "Notes!"];
    const keys = (query: string, list: readonly string[], limit = 10) =>
      rankFuzzyTitles(
        query,
        list.map((title, at) => [at, title] as const),
        limit,
      ).map((hit) => hit.key);
    expect(keys("note", titles)).toEqual([1, 4, 2, 0, 3]);
    expect(keys("note", ["Notes", "Notes", "Notes"])).toEqual([0, 1, 2]);
    expect(keys("note", titles, 2)).toEqual([1, 4]);
    expect(keys("n", titles)).toEqual([]);
  });
});

describe("withinOneEdit", () => {
  const edit = (a: string, b: string): [boolean, boolean] => [withinOneEdit([...a], [...b]), withinOneEdit([...b], [...a])];

  it("allows a letter changed, added, left out, or two neighbours swapped", () => {
    for (const other of ["abcd", "abxd", "abd", "abcde", "bacd", "abdc", "xbcd", "abcx", "bcd"]) {
      expect(edit("abcd", other), other).toEqual([true, true]);
    }
    expect(edit("", "a")).toEqual([true, true]);
  });

  it("refuses two edits", () => {
    for (const other of ["dcba", "axyd", "ab", "acbe"]) {
      expect(edit("abcd", other), other).toEqual([false, false]);
    }
  });
});
