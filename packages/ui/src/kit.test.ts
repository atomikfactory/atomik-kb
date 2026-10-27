import { BIP39_ENGLISH } from "@atomik-kb/core-client";
import type { RecoveryKit } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { CONFIRM_WORDS, KIT_WORDS, kitInputText, kitText, matchesWord, pickPositions, readKitInput } from "./kit";

const KIT: RecoveryKit = {
  words: BIP39_ENGLISH.slice(0, KIT_WORDS),
  code: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567-ABCD-EFGH-IJKL-MNOP-QRST-UV",
  accountId: "account",
  createdAt: 0,
};

/** A deterministic stand-in for `Math.random`. */
function sequence(...values: number[]): () => number {
  let at = 0;
  return () => values[at++ % values.length] ?? 0;
}

describe("kitText", () => {
  it("joins the words with single spaces, or gives the code", () => {
    expect(kitText(KIT, "words")).toBe(KIT.words.join(" "));
    expect(kitText(KIT, "words").split(" ")).toHaveLength(KIT_WORDS);
    expect(kitText(KIT, "code")).toBe(KIT.code);
  });
});

describe("pickPositions", () => {
  it("picks distinct positions in ascending order", () => {
    for (let run = 0; run < 50; run += 1) {
      const positions = pickPositions(KIT_WORDS, CONFIRM_WORDS);
      expect(positions).toHaveLength(CONFIRM_WORDS);
      expect(new Set(positions).size).toBe(CONFIRM_WORDS);
      expect([...positions].sort((a, b) => a - b)).toEqual(positions);
      expect(positions.every((position) => position >= 0 && position < KIT_WORDS)).toBe(true);
    }
  });

  it("follows the random source, including its extremes", () => {
    expect(pickPositions(24, 3, sequence(0))).toEqual([0, 1, 2]);
    expect(pickPositions(24, 3, sequence(0.999999))).toEqual([21, 22, 23]);
    expect(pickPositions(24, 3, sequence(0.5, 0, 0.999999))).toEqual([0, 12, 23]);
  });

  it("never asks for more positions than there are", () => {
    expect(pickPositions(2, 3, sequence(0.3))).toEqual([0, 1]);
  });
});

describe("matchesWord", () => {
  it("accepts the word in any case and with surrounding spaces", () => {
    expect(matchesWord("abandon", "abandon")).toBe(true);
    expect(matchesWord("  ABANDON ", "abandon")).toBe(true);
    expect(matchesWord("Abandon", "abandon")).toBe(true);
  });

  it("accepts a prefix of at least four letters", () => {
    expect(matchesWord("aban", "abandon")).toBe(true);
    expect(matchesWord("ABANDO", "abandon")).toBe(true);
    expect(matchesWord("aba", "abandon")).toBe(false);
    expect(matchesWord("abanx", "abandon")).toBe(false);
  });

  it("accepts three-letter words typed in full", () => {
    expect(matchesWord("act", "act")).toBe(true);
    expect(matchesWord("ac", "act")).toBe(false);
  });

  it("rejects other words and nothing", () => {
    expect(matchesWord("ability", "abandon")).toBe(false);
    expect(matchesWord("", "abandon")).toBe(false);
    expect(matchesWord("   ", "abandon")).toBe(false);
  });
});

describe("readKitInput", () => {
  it("is empty for blank text", () => {
    expect(readKitInput("")).toEqual({ kind: "empty" });
    expect(readKitInput("  \n ")).toEqual({ kind: "empty" });
  });

  it("reads words separated by spaces, commas and new lines, lower-cased", () => {
    expect(readKitInput(" Abandon, ability\nABLE  about ")).toEqual({
      kind: "words",
      words: ["abandon", "ability", "able", "about"],
      unknown: [],
    });
  });

  it("drops the numbering of a kit copied from paper", () => {
    expect(readKitInput("1. abandon 2) ability 3 able")).toEqual({
      kind: "words",
      words: ["abandon", "ability", "able"],
      unknown: [],
    });
  });

  it("lists the words that are not in the list, by position, and accepts prefixes", () => {
    expect(readKitInput("aban abilityx zzzz able abo")).toEqual({
      kind: "words",
      words: ["aban", "abilityx", "zzzz", "able", "abo"],
      unknown: [
        { position: 2, word: "abilityx" },
        { position: 3, word: "zzzz" },
        { position: 5, word: "abo" },
      ],
    });
  });

  it("takes text with digits or dashes as the code, as typed", () => {
    expect(readKitInput(` ${KIT.code} `)).toEqual({ kind: "code", code: KIT.code });
    expect(readKitInput("abcd 2345")).toEqual({ kind: "code", code: "abcd 2345" });
  });

  it("takes exactly a code's 54 letters as the code even without digits or dashes", () => {
    const letters = "abcd ".repeat(13) + "ef";
    expect(readKitInput(letters)).toEqual({ kind: "code", code: letters.trim() });
  });
});

describe("kitInputText", () => {
  it("sends the normalised words, or the code as typed", () => {
    expect(kitInputText(readKitInput("1. Abandon,  ability"))).toBe("abandon ability");
    expect(kitInputText(readKitInput(`  ${KIT.code}`))).toBe(KIT.code);
    expect(kitInputText(readKitInput(""))).toBe("");
  });
});
