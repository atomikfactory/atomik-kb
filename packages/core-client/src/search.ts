// Text matching for `MemoryTransport`'s `Search`: a small, deterministic stand-in for the
// core's SQLite FTS5 index (`kb-storage`, `unicode61 remove_diacritics 2`). It follows the
// rules the UI can see:
//
// - the query is cut into words like `kb-storage` builds its FTS5 query: whitespace,
//   control characters and `" * ( ) : ^ - +` separate words, the bare operators `AND`,
//   `OR`, `NOT` and `NEAR` are dropped, and every word becomes a prefix phrase
//   (`"word"*`): its tokens must follow each other, the last one as a prefix;
// - tokens are runs of letters, digits and private-use characters (unicode61's default
//   categories), compared without case and diacritics;
// - the snippet is chosen like FTS5's `snippet(..., 12)`, highlights are UTF-16 ranges.
//
// Ranking (bm25) is not reproduced; folding is close to unicode61's, not identical.

import type { TextRange } from "./generated/TextRange";

/** Tokens in a snippet, the last argument of `snippet()` in `kb-storage`. */
const SNIPPET_TOKENS = 12;
/** Marks a snippet that does not reach the start or the end of the text (one UTF-16 unit). */
const ELLIPSIS = "…";
/** FTS5's preference for a snippet that starts a sentence (more for the start of the text). */
const SENTENCE_BONUS = 100;
const TEXT_START_BONUS = 120;

/** A token character: letters, digits and private-use characters as in unicode61, plus combining marks. */
const TOKEN = /[\p{L}\p{N}\p{M}\p{Co}]+/gu;
const COMBINING_MARKS = /\p{M}/gu;
/** What separates words of a query (`kb-storage`'s `match_expression`). */
const QUERY_SEPARATORS = /[\s\p{Cc}"*():^\-+]+/u;
/** FTS5 operators, which `kb-storage` drops when they stand alone in capitals. */
const QUERY_OPERATORS = new Set(["AND", "OR", "NOT", "NEAR"]);

interface Token {
  /** First UTF-16 code unit in the text. */
  start: number;
  /** One past the last UTF-16 code unit. */
  end: number;
  /** Folded form compared with the query. */
  folded: string;
}

/** A match of one query word: `size` consecutive tokens from `first`. */
interface Instance {
  phrase: number;
  first: number;
  size: number;
}

/** The snippet of a matching text and where the matches are inside it. */
export interface TextMatch {
  snippet: string;
  highlights: TextRange[];
}

/** Lower case, decomposed, combining marks removed: "Café" and "cafe" fold alike. */
export function foldText(text: string): string {
  return text.toLowerCase().normalize("NFD").replace(COMBINING_MARKS, "");
}

function tokenize(text: string): Token[] {
  const found: Token[] = [];
  for (const match of text.matchAll(TOKEN)) {
    const folded = foldText(match[0]);
    if (folded !== "") {
      found.push({ start: match.index, end: match.index + match[0].length, folded });
    }
  }
  return found;
}

/**
 * The words of a (validated) query as the core cuts them: `kb-storage`'s `query_words`. Whitespace,
 * control characters and `" * ( ) : ^ - +` separate words, and a bare `AND`, `OR`, `NOT` or `NEAR`
 * is dropped. A word can still hold nothing searchable (`&`).
 */
export function queryWords(query: string): string[] {
  return query.split(QUERY_SEPARATORS).filter((word) => word !== "" && !QUERY_OPERATORS.has(word));
}

/**
 * Cuts a (validated) query into words, each a phrase of folded tokens: `"a.b"` is the
 * phrase `a b`, `atomik-kb` the two words `atomik` and `kb`. Words without any token
 * (punctuation, emoji) are dropped, so the result is empty when nothing searchable
 * remains (which matches nothing, as in the core).
 */
export function parseQuery(query: string): string[][] {
  return queryWords(query)
    .map((word) => tokenize(word).map((token) => token.folded))
    .filter((phrase) => phrase.length > 0);
}

/**
 * The words of {@link parseQuery} with a repeated word (any case) left out: what the page-level match
 * looks for one by one (the core's `distinct_words`).
 */
export function parseDistinctQuery(query: string): string[][] {
  const seen = new Set<string>();
  const phrases: string[][] = [];
  for (const word of queryWords(query)) {
    const phrase = tokenize(word).map((token) => token.folded);
    if (phrase.length > 0 && !seen.has(word.toLowerCase())) {
      seen.add(word.toLowerCase());
      phrases.push(phrase);
    }
  }
  return phrases;
}

/** Every place where `phrase` matches: all tokens equal, the last one as a prefix. */
function instancesOf(tokens: readonly Token[], phrase: readonly string[], index: number): Instance[] {
  const found: Instance[] = [];
  const last = phrase.length - 1;
  for (let first = 0; first + last < tokens.length; first += 1) {
    const matches = phrase.every((query, offset) => {
      const token = tokens[first + offset]?.folded ?? "";
      return offset === last ? token.startsWith(query) : token === query;
    });
    if (matches) {
      found.push({ phrase: index, first, size: phrase.length });
    }
  }
  return found;
}

/**
 * Token positions where a sentence starts, as FTS5's sentence finder sees them: the first
 * token, and every token preceded by whitespace that follows a `.` or a `:`.
 */
function sentenceStarts(text: string, tokens: readonly Token[]): number[] {
  const starts: number[] = [];
  tokens.forEach((token, position) => {
    if (position === 0) {
      starts.push(0);
      return;
    }
    let i = token.start - 1;
    while (i >= 0 && " \t\n\r".includes(text.charAt(i))) {
      i -= 1;
    }
    const char = i >= 0 ? text.charAt(i) : "";
    if (i !== token.start - 1 && (char === "." || char === ":")) {
      starts.push(position);
    }
  });
  return starts;
}

/**
 * FTS5's `fts5SnippetScore`: 1000 for every phrase seen first in the window starting at
 * `from`, 1 for repeats, and the start that centres those matches in the window.
 */
function scoreWindow(instances: readonly Instance[], from: number, total: number): { score: number; start: number } {
  const seen = new Set<number>();
  let score = 0;
  let first = -1;
  let last = 0;
  for (const instance of instances) {
    if (instance.first >= from && instance.first < from + SNIPPET_TOKENS) {
      score += seen.has(instance.phrase) ? 1 : 1000;
      seen.add(instance.phrase);
      if (first < 0) {
        first = instance.first;
      }
      last = instance.first + instance.size;
    }
  }
  let start = first - Math.trunc((SNIPPET_TOKENS - (last - first)) / 2);
  if (start + SNIPPET_TOKENS > total) {
    start = total - SNIPPET_TOKENS;
  }
  return { score, start: Math.max(0, start) };
}

/** FTS5's choice of the first snippet token: the best-scoring window, sentence starts preferred. */
function snippetStart(text: string, tokens: readonly Token[], instances: readonly Instance[]): number {
  const starts = sentenceStarts(text, tokens);
  let bestScore = 0;
  let bestStart = 0;
  for (const instance of instances) {
    const centred = scoreWindow(instances, instance.first, tokens.length);
    if (centred.score > bestScore) {
      bestScore = centred.score;
      bestStart = centred.start;
    }
    if (starts.length > 0 && tokens.length > SNIPPET_TOKENS) {
      let at = 0;
      while (at < starts.length - 1 && (starts[at + 1] ?? Infinity) <= instance.first) {
        at += 1;
      }
      const sentence = starts[at] ?? 0;
      if (sentence < instance.first) {
        const bonus = sentence === 0 ? TEXT_START_BONUS : SENTENCE_BONUS;
        const score = scoreWindow(instances, sentence, tokens.length).score + bonus;
        if (score > bestScore) {
          bestScore = score;
          bestStart = sentence;
        }
      }
    }
  }
  return bestStart;
}

/**
 * Matches `text` against a parsed query: `undefined` unless every word matches. The
 * snippet holds {@link SNIPPET_TOKENS} tokens chosen as FTS5 does, cut at token
 * boundaries with an ellipsis where the text goes on (leading and trailing text kept
 * where the snippet reaches the start or the end); `highlights` cover the matches that
 * start inside the snippet (overlapping ones merged, clipped at its end).
 */
export function matchText(text: string, phrases: readonly (readonly string[])[]): TextMatch | undefined {
  if (phrases.length === 0) {
    return undefined;
  }
  const tokens = tokenize(text);
  const instances: Instance[] = [];
  for (const [index, phrase] of phrases.entries()) {
    const found = instancesOf(tokens, phrase, index);
    if (found.length === 0) {
      return undefined;
    }
    instances.push(...found);
  }
  instances.sort((a, b) => a.first - b.first || a.phrase - b.phrase);

  const start = snippetStart(text, tokens, instances);
  const end = Math.min(tokens.length, start + SNIPPET_TOKENS) - 1;
  const from = start === 0 ? 0 : (tokens[start]?.start ?? 0);
  const to = end >= tokens.length - 1 ? text.length : (tokens[end]?.end ?? text.length);
  const prefix = start === 0 ? "" : ELLIPSIS;
  const suffix = end >= tokens.length - 1 ? "" : ELLIPSIS;
  const shift = prefix.length - from;

  // Coalesce overlapping matches (in tokens), as FTS5's highlight iterator does.
  const merged: { first: number; last: number }[] = [];
  for (const instance of instances) {
    const last = instance.first + instance.size - 1;
    const previous = merged.at(-1);
    if (previous !== undefined && instance.first <= previous.last) {
      previous.last = Math.max(previous.last, last);
    } else {
      merged.push({ first: instance.first, last });
    }
  }
  const highlights: TextRange[] = merged
    .filter((range) => range.first >= start && range.first <= end)
    .map((range) => ({
      start: (tokens[range.first]?.start ?? 0) + shift,
      end: (tokens[Math.min(range.last, end)]?.end ?? 0) + shift,
    }));
  return { snippet: `${prefix}${text.slice(from, to)}${suffix}`, highlights };
}
