// Fuzzy matching of page titles for `MemoryTransport`'s `Search` (ADR-0040, item 9): the same
// algorithm as `kb-storage`'s `fuzzy.rs`, held to the same results by the shared fixture
// `fixtures/search/fuzzy-titles-v1.json` (written by the Rust test, read by `search-fuzzy.test.ts`).
// Read the Rust module's documentation for the rules; in short:
//
// - query and title are folded (lower case, accents removed) and cut into words;
// - a title matches when every query word starts some title word, exactly or (words of 4 or more
//   characters) with one typo against a prefix of it, or else when the query's letters are found in
//   order in the title;
// - the score: 1 per matched letter, 5 more at a word start, 4 more for a letter that directly
//   follows the previous matched one inside a word, a typo costs 6;
// - the ranking: score, then the shorter title, then the order the titles came in.
//
// Differences from the core, none visible in the fixture: the core folds accents with a table of the
// Latin letters, this file with the canonical decomposition of the whole of Unicode.

import { queryWords } from "./search";

/** Fewer letters than this in the query and nothing is matched. */
export const MIN_FUZZY_LETTERS = 2;
/** A query word needs at least this many characters to be matched with one typo. */
export const TYPO_MIN_CHARS = 4;
/** Only the first this many letters of a title are looked at. */
const MAX_TITLE_LETTERS = 400;

const SCORE_LETTER = 1;
const SCORE_WORD_START = 5;
const SCORE_CONTIGUOUS = 4;
/** What a word matched with a typo loses against the same word matched exactly. */
export const TYPO_PENALTY = 6;

/** "No alignment" in the table of the letters-in-order search. */
const NONE = Number.NEGATIVE_INFINITY;

const WORD_CHAR = /[\p{L}\p{N}\p{M}\p{Co}]/u;
const COMBINING_MARKS = /\p{M}/gu;

/** One folded letter of a text, with where the character it came from sits in the text. */
interface Letter {
  ch: string;
  /** First UTF-16 code unit of the original character. */
  start: number;
  /** One past its last UTF-16 code unit. */
  end: number;
  /** The first letter of a word. */
  wordStart: boolean;
}

/** A text folded into letters and cut into words (`[from, to)` ranges of `letters`). */
interface Folded {
  letters: Letter[];
  words: [number, number][];
}

/** A query ready for matching titles. */
export interface FuzzyQuery {
  /** Every letter of the query, in order. */
  letters: string[];
  /** The words, as `[from, to)` ranges of `letters`. */
  words: [number, number][];
}

/** How a title matched a query. */
export interface FuzzyMatch {
  score: number;
  /** UTF-16 ranges `[start, end]` inside the title: sorted, not overlapping, none empty. */
  highlights: [number, number][];
}

/** The folded characters of one character: lower case, accents removed (a mark folds to nothing). */
function foldChar(c: string): string[] {
  const folded: string[] = [];
  for (const lower of c.toLowerCase()) {
    folded.push(...lower.normalize("NFD").replace(COMBINING_MARKS, ""));
  }
  return folded;
}

function foldText(text: string): Folded {
  const letters: Letter[] = [];
  let words: [number, number][] = [];
  let offset = 0;
  let wordFrom: number | undefined;
  for (const c of text) {
    const units = c.length;
    if (WORD_CHAR.test(c)) {
      wordFrom ??= letters.length;
      for (const ch of foldChar(c)) {
        letters.push({ ch, start: offset, end: offset + units, wordStart: letters.length === wordFrom });
      }
    } else if (wordFrom !== undefined) {
      if (wordFrom < letters.length) {
        words.push([wordFrom, letters.length]);
      }
      wordFrom = undefined;
    }
    offset += units;
  }
  if (wordFrom !== undefined && wordFrom < letters.length) {
    words.push([wordFrom, letters.length]);
  }
  if (letters.length > MAX_TITLE_LETTERS) {
    letters.length = MAX_TITLE_LETTERS;
    words = words
      .filter(([from]) => from < MAX_TITLE_LETTERS)
      .map(([from, to]) => [from, Math.min(to, MAX_TITLE_LETTERS)]);
  }
  return { letters, words };
}

/**
 * The query as a matcher, or `undefined` when it has fewer than {@link MIN_FUZZY_LETTERS} letters.
 * The words are cut like the full-text query's (separators, dropped bare operators).
 */
export function fuzzyQuery(query: string): FuzzyQuery | undefined {
  const folded = foldText(queryWords(query).join(" "));
  if (folded.letters.length < MIN_FUZZY_LETTERS) {
    return undefined;
  }
  return { letters: folded.letters.map((letter) => letter.ch), words: folded.words };
}

/** The score of a run of `length` letters that starts a word and runs on without a break. */
function runScore(length: number): number {
  return SCORE_WORD_START + SCORE_LETTER * length + SCORE_CONTIGUOUS * Math.max(0, length - 1);
}

/**
 * The best way to find `query` as a subsequence of `title`: its score and the position in `title` of
 * each query letter. Best is the highest score; on a tie the earliest alignment.
 */
function bestSubsequence(query: readonly string[], title: readonly Letter[]): { score: number; positions: number[] } | undefined {
  const m = query.length;
  const n = title.length;
  if (m === 0 || n < m) {
    return undefined;
  }
  // Cheap test first: most titles are not a match at all.
  let next = 0;
  for (const letter of title) {
    if (query[next] === letter.ch) {
      next += 1;
    }
  }
  if (next < m) {
    return undefined;
  }
  // best[i * n + j]: the best score with query letter i matched to title letter j.
  const best = new Array<number>(m * n).fill(NONE);
  const from = new Array<number>(m * n).fill(-1);
  const bestAt = (i: number, j: number): number => best[i * n + j] ?? NONE;
  for (let i = 0; i < m; i += 1) {
    const wanted = query[i];
    // The best earlier alignment of letter i - 1 strictly before j: its score and position.
    let before: { value: number; at: number } | undefined;
    for (let j = 0; j < n; j += 1) {
      const letter = title[j];
      if (letter === undefined) {
        continue;
      }
      if (i > 0 && j > 0) {
        const previous = bestAt(i - 1, j - 1);
        if (previous !== NONE && (before === undefined || previous > before.value)) {
          before = { value: previous, at: j - 1 };
        }
      }
      if (letter.ch !== wanted) {
        continue;
      }
      const base = SCORE_LETTER + (letter.wordStart ? SCORE_WORD_START : 0);
      if (i === 0) {
        best[j] = base;
        continue;
      }
      let chosen = before;
      if (j > 0 && !letter.wordStart) {
        const previous = bestAt(i - 1, j - 1);
        if (previous !== NONE) {
          const run = previous + SCORE_CONTIGUOUS;
          if (chosen === undefined || run > chosen.value) {
            chosen = { value: run, at: j - 1 };
          }
        }
      }
      if (chosen !== undefined) {
        best[i * n + j] = base + chosen.value;
        from[i * n + j] = chosen.at;
      }
    }
  }
  let end: { value: number; at: number } | undefined;
  for (let j = 0; j < n; j += 1) {
    const value = bestAt(m - 1, j);
    if (value !== NONE && (end === undefined || value > end.value)) {
      end = { value, at: j };
    }
  }
  if (end === undefined) {
    return undefined;
  }
  const positions = new Array<number>(m).fill(0);
  let at = end.at;
  for (let i = m - 1; i >= 0; i -= 1) {
    positions[i] = at;
    if (i > 0) {
      at = from[i * n + at] ?? 0;
    }
  }
  return { score: end.value, positions };
}

function sameLetters(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((letter, at) => letter === b[at]);
}

/** True when `a` and `b` are equal or one edit apart: a letter changed, one added or left out, or two neighbours swapped. */
export function withinOneEdit(a: readonly string[], b: readonly string[]): boolean {
  if (Math.abs(a.length - b.length) > 1) {
    return false;
  }
  let common = 0;
  while (common < a.length && common < b.length && a[common] === b[common]) {
    common += 1;
  }
  if (common === a.length && common === b.length) {
    return true;
  }
  if (a.length === b.length) {
    if (sameLetters(a.slice(common + 1), b.slice(common + 1))) {
      return true; // one letter changed
    }
    return (
      a[common] === b[common + 1] &&
      a[common + 1] === b[common] &&
      a[common + 1] !== undefined &&
      sameLetters(a.slice(common + 2), b.slice(common + 2))
    );
  }
  if (a.length < b.length) {
    return sameLetters(a.slice(common), b.slice(common + 1)); // one letter added to b
  }
  return sameLetters(a.slice(common + 1), b.slice(common)); // one letter left out of b
}

/**
 * Where query word `word` matches the start of a title word: a `[from, to)` range of title letters and
 * the score. An exact prefix of any title word wins over a typo (4+ characters only) against a prefix.
 */
function matchWord(word: readonly string[], title: Folded): { range: [number, number]; score: number } | undefined {
  const spell = ([from, to]: [number, number]): string[] => title.letters.slice(from, to).map((letter) => letter.ch);
  for (const titleWord of title.words) {
    const letters = spell(titleWord);
    if (letters.length >= word.length && sameLetters(letters.slice(0, word.length), word)) {
      return { range: [titleWord[0], titleWord[0] + word.length], score: runScore(word.length) };
    }
  }
  if (word.length < TYPO_MIN_CHARS) {
    return undefined;
  }
  for (const titleWord of title.words) {
    const letters = spell(titleWord);
    for (const length of [word.length, word.length - 1, word.length + 1]) {
      if (length <= letters.length && withinOneEdit(word, letters.slice(0, length))) {
        return { range: [titleWord[0], titleWord[0] + length], score: runScore(length) - TYPO_PENALTY };
      }
    }
  }
  return undefined;
}

/** The UTF-16 range of the title letters `[from, to)`. */
function unitsOf(title: Folded, [from, to]: [number, number]): [number, number] | undefined {
  const first = title.letters[from];
  const last = title.letters[to - 1];
  return first === undefined || last === undefined ? undefined : [first.start, last.end];
}

/** Merges ranges that touch or overlap; the input is sorted by start. */
function merge(ranges: readonly [number, number][]): [number, number][] {
  const merged: [number, number][] = [];
  for (const [start, end] of ranges) {
    const last = merged.at(-1);
    if (last !== undefined && start <= last[1]) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }
  return merged;
}

/** The reading of a match as words: every query word is the start of some title word, exactly or with one typo. */
function matchAsWords(query: FuzzyQuery, title: Folded): FuzzyMatch | undefined {
  let score = 0;
  const ranges: [number, number][] = [];
  for (const [from, to] of query.words) {
    const found = matchWord(query.letters.slice(from, to), title);
    if (found === undefined) {
      return undefined;
    }
    const units = unitsOf(title, found.range);
    if (units === undefined) {
      return undefined;
    }
    score += found.score;
    ranges.push(units);
  }
  ranges.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  return { score, highlights: merge(ranges) };
}

/** The reading of a match as the query's letters found in order in the title. */
function matchAsLetters(query: FuzzyQuery, title: Folded): FuzzyMatch | undefined {
  const found = bestSubsequence(query.letters, title.letters);
  if (found === undefined) {
    return undefined;
  }
  const ranges: [number, number][] = [];
  for (const at of found.positions) {
    const letter = title.letters[at];
    if (letter !== undefined) {
      ranges.push([letter.start, letter.end]);
    }
  }
  return { score: found.score, highlights: merge(ranges) };
}

/**
 * How `title` matches `query`, or `undefined`: the words reading when it holds (its highlights are
 * whole word starts), else the letters in order.
 */
export function fuzzyTitle(query: FuzzyQuery, title: string): FuzzyMatch | undefined {
  const folded = foldText(title);
  return matchAsWords(query, folded) ?? matchAsLetters(query, folded);
}

/**
 * The titles that match `query`, best first, at most `limit`: by score, then by the shorter title (in
 * characters), then in the order the titles came in. A query with fewer than {@link MIN_FUZZY_LETTERS}
 * letters matches nothing.
 */
export function rankFuzzyTitles<K>(query: string, titles: Iterable<readonly [K, string]>, limit: number): { key: K; match: FuzzyMatch }[] {
  const prepared = fuzzyQuery(query);
  if (prepared === undefined) {
    return [];
  }
  const found: { key: K; match: FuzzyMatch; length: number }[] = [];
  for (const [key, title] of titles) {
    const match = fuzzyTitle(prepared, title);
    if (match !== undefined) {
      found.push({ key, match, length: Array.from(title).length });
    }
  }
  found.sort((a, b) => b.match.score - a.match.score || a.length - b.length);
  return found.slice(0, limit).map(({ key, match }) => ({ key, match }));
}
