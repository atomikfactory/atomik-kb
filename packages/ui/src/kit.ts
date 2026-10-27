// Pure helpers around the recovery kit (ADR-0017): how it is shown, the local check of a
// few words after it was written down, and the live hints while it is typed back in.
// The kit itself never leaves the component that shows or asks for it: nothing here
// keeps a copy.

import { resolveKitWord } from "@atomik-kb/core-client";
import type { RecoveryKit } from "@atomik-kb/core-client";

/** How the kit is shown: the 24 words, or the same key as a code. */
export type KitForm = "words" | "code";

/** Words in a kit. */
export const KIT_WORDS = 24;

/** Words the confirmation step asks for. */
export const CONFIRM_WORDS = 3;

/** Characters of the code form without its dashes (52 data and 2 checksum characters). */
const CODE_CHARS = 54;

/** The kit as one line of text: the words separated by single spaces, or the code. */
export function kitText(kit: RecoveryKit, form: KitForm): string {
  return form === "words" ? kit.words.join(" ") : kit.code;
}

/**
 * `count` distinct positions out of `0..total`, in ascending order, for the confirmation
 * step. `random` returns numbers in [0, 1) like `Math.random`.
 */
export function pickPositions(total: number, count: number, random: () => number = Math.random): number[] {
  const pool = Array.from({ length: total }, (_, index) => index);
  const picked: number[] = [];
  while (picked.length < Math.min(count, total)) {
    const at = Math.min(pool.length - 1, Math.floor(random() * pool.length));
    const [position] = pool.splice(at, 1);
    if (position !== undefined) {
      picked.push(position);
    }
  }
  return picked.sort((a, b) => a - b);
}

/**
 * True when `typed` stands for `word` the way the core reads a kit: any case, surrounding
 * spaces ignored, the whole word or at least its first four letters (BIP-39 words are
 * unique in their first four letters).
 */
export function matchesWord(typed: string, word: string): boolean {
  return resolveKitWord(typed) === word.toLowerCase();
}

/** What a typed kit looks like so far. */
export type KitInput =
  | { kind: "empty" }
  /** Words; `unknown` lists those that stand for no word of the list (1-based positions). */
  | { kind: "words"; words: string[]; unknown: { position: number; word: string }[] }
  /** Anything with digits or dashes (or exactly a code's length of letters): the base32 code. */
  | { kind: "code"; code: string };

/**
 * Reads a kit as typed: words separated by spaces, commas or new lines (numbering such as
 * `1.` copied from a paper kit is dropped), or the code, told apart like the core does.
 */
export function readKitInput(text: string): KitInput {
  // Positions 1 to 24 ("1", "2.", "3)") are numbering; a code's groups have four characters.
  const tokens = text
    .trim()
    .toLowerCase()
    .split(/[\s,;]+/)
    .filter((token) => token !== "" && !/^([1-9]|1\d|2[0-4])[.):]?$/.test(token));
  if (tokens.length === 0) {
    return { kind: "empty" };
  }
  const lettersOnly = tokens.every((token) => /^[a-z]+$/.test(token));
  if (!lettersOnly || tokens.join("").length === CODE_CHARS) {
    return { kind: "code", code: text.trim() };
  }
  const unknown = tokens.flatMap((word, at) => (resolveKitWord(word) === undefined ? [{ position: at + 1, word }] : []));
  return { kind: "words", words: tokens, unknown };
}

/** The text sent to the core for a typed kit: the words, normalised, or the code as typed. */
export function kitInputText(input: KitInput): string {
  switch (input.kind) {
    case "empty":
      return "";
    case "words":
      return input.words.join(" ");
    case "code":
      return input.code;
  }
}
