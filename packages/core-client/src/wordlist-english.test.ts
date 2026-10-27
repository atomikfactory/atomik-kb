import { describe, expect, it } from "vitest";

import { sha256, toHex } from "./digest";
import { readRepoText } from "./testing/repo-files";
import { BIP39_ENGLISH } from "./wordlist-english";

/** SHA-256 of the official BIP-39 English list, one word per line (ADR-0017: `2f5eed53…dbda`). */
const OFFICIAL_SHA256 = "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda";

describe("BIP39_ENGLISH", () => {
  it("is the official list: 2048 words with the published SHA-256", async () => {
    expect(BIP39_ENGLISH).toHaveLength(2048);
    expect(toHex(await sha256(`${BIP39_ENGLISH.join("\n")}\n`))).toBe(OFFICIAL_SHA256);
  });

  it("is generated from the list kb-crypto vendors (`pnpm --filter @atomik-kb/core-client wordlist`)", () => {
    const vendored = readRepoText("crates/kb-crypto/wordlists/english.txt")
      .split("\n")
      .filter((line) => line !== "");
    expect(BIP39_ENGLISH).toEqual(vendored);
  });

  it("holds sorted words of 3 to 8 letters a-z whose first four letters are unique", () => {
    expect([...BIP39_ENGLISH].sort()).toEqual(BIP39_ENGLISH);
    expect(BIP39_ENGLISH.filter((word) => !/^[a-z]{3,8}$/u.test(word))).toEqual([]);
    expect(new Set(BIP39_ENGLISH.map((word) => word.slice(0, 4))).size).toBe(2048);
  });

  it("cannot be changed at run time", () => {
    expect(Object.isFrozen(BIP39_ENGLISH)).toBe(true);
  });
});
