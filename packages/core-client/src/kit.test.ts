import { describe, expect, it } from "vitest";

import { CoreError } from "./errors";
import {
  KIT_WORDS,
  kitCodeFromEntropy,
  kitWordsFromEntropy,
  parseKit,
  parseKitCode,
  parseKitWords,
  resolveKitWord,
} from "./kit";
import { readRepoText, repoFileExists } from "./testing/repo-files";
import { BIP39_ENGLISH } from "./wordlist-english";

function fromHex(hex: string): Uint8Array {
  return Uint8Array.from(hex.match(/../gu) ?? [], (byte) => Number.parseInt(byte, 16));
}

function toHexText(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function refusal(promise: Promise<unknown>): Promise<CoreError> {
  const error: unknown = await promise.then(
    () => undefined,
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(CoreError);
  return error as CoreError;
}

/** The key of the kit goldens (`fixtures/crypto/kit-v1.json`): the bytes 00 01 02 … 1f. */
const FIXTURE_KEY = Uint8Array.from({ length: 32 }, (_, index) => index);

/**
 * Its BIP-39 words. Hard-coded from an independent computation (Python, `hashlib` SHA-256 over
 * the vendored list) that also reproduces every Trezor vector below, so the check stands without
 * the Rust side; kb-crypto's own golden (`fixtures/crypto/kit-v1.json`) is compared further down.
 */
const FIXTURE_WORDS =
  "abandon amount liar amount expire adjust cage candy arch gather drum bullet absurd math era live " +
  "bid rhythm alien crouch range attend journey unaware";

/** RFC 4648 base32 of the fixture key without padding (Python's `base64.b32encode`). */
const FIXTURE_BASE32 = "AAAQEAYEAUDAOCAJBIFQYDIOB4IBCEQTCQKRMFYYDENBWHA5DYPQ";

/** Trezor's BIP-39 vectors with 256-bit entropy (python-mnemonic `vectors.json`, English). */
const TREZOR_256: readonly (readonly [string, string])[] = [
  [
    "0000000000000000000000000000000000000000000000000000000000000000",
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon " +
      "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
  ],
  [
    "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
    "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful " +
      "legal winner thank year wave sausage worth title",
  ],
  [
    "8080808080808080808080808080808080808080808080808080808080808080",
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor " +
      "acoustic avoid letter advice cage absurd amount doctor acoustic bless",
  ],
  [
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo vote",
  ],
  [
    "68a79eaca2324873eacc50cb9c6eca8cc68ea5d936f98787c60c7ebc74e6ce7c",
    "hamster diagram private dutch cause delay private meat slide toddler razor book happy fancy gospel " +
      "tennis maple dilemma loan word shrug inflict delay length",
  ],
  [
    "9f6a2878b2520799a44ef18bc7df394e7061a224d2c33cd015b157d746869863",
    "panda eyebrow bullet gorilla call smoke muffin taste mesh discover soft ostrich alcohol speed nation " +
      "flash devote level hobby quick inner drive ghost inside",
  ],
  [
    "066dca1a2bb7e8a1db2832148ce9933eea0f3ac9548d793112d9a95c9407efad",
    "all hour make first leader extend hole alien behind guard gospel lava path output census museum " +
      "junior mass reopen famous sing advance salt reform",
  ],
  [
    "f585c11aec520db57dd353c69554b21a89b20fb0650966fa0a9d6f74fd989d8f",
    "void come effort suffer camp survey warrior heavy shoot primary clutch crush open amazing screen " +
      "patrol group space point ten exist slush involve unfold",
  ],
];

describe("kitWordsFromEntropy and parseKitWords (BIP-39)", () => {
  it.each(TREZOR_256)("encode and decode the Trezor vector %s", async (hex, words) => {
    expect((await kitWordsFromEntropy(fromHex(hex))).join(" ")).toBe(words);
    expect(toHexText(await parseKitWords(words))).toBe(hex);
  });

  it("give the fixture key 00..1f the 24 words of the BIP-39 reference", async () => {
    const words = await kitWordsFromEntropy(FIXTURE_KEY);
    expect(words).toHaveLength(KIT_WORDS);
    expect(words.join(" ")).toBe(FIXTURE_WORDS);
    expect(await parseKitWords(words)).toEqual(FIXTURE_KEY);
  });

  it("accept any case, any whitespace and unique prefixes of four letters or more", async () => {
    const words = FIXTURE_WORDS.split(" ");
    const typed = words
      .map((word, index) => (index % 3 === 0 ? word.slice(0, 4) : index % 3 === 1 ? word.toUpperCase() : word))
      .join(" \n\t ");
    expect(await parseKitWords(`\n  ${typed}\t`)).toEqual(FIXTURE_KEY);
    expect(await parseKitWords(words.map((word) => ` ${word.slice(0, 5)} `))).toEqual(FIXTURE_KEY);
  });

  it("refuse a wrong number of words", async () => {
    const words = FIXTURE_WORDS.split(" ");
    for (const count of [1, 12, 23, 25]) {
      const typed = Array.from({ length: count }, (_, index) => words[index % words.length] ?? "").join(" ");
      expect((await refusal(parseKitWords(typed))).toApiError()).toEqual({
        code: "InvalidInput",
        message: `expected 24 words, got ${String(count)}`,
        details: { field: "kit" },
      });
    }
    // Blank text has no words, as kb-crypto's `from_words(&[])` says.
    expect((await refusal(parseKitWords("   "))).message).toBe("expected 24 words, got 0");
  });

  it("name an unknown word by its position, never by its text (kb-crypto's message)", async () => {
    const unknown = "word 7 is not in the word list (nor a unique prefix of 4 letters or more)";
    const words = FIXTURE_WORDS.split(" ");
    words[6] = "cagx";
    const error = await refusal(parseKitWords(words.join(" ")));
    expect(error.toApiError()).toEqual({ code: "InvalidInput", message: unknown, details: { field: "kit" } });
    // Three letters are not a prefix (only whole three-letter words count), the letters after
    // a prefix must continue the word, and only ASCII letters count.
    for (const typo of ["cag", "cagex", "cage1", "Kite"]) {
      words[6] = typo;
      const message = (await refusal(parseKitWords(words))).message;
      expect(message, typo).toBe(unknown);
      expect(message).not.toContain(typo);
    }
  });

  it("refuse a checksum mismatch, which a single changed or swapped word causes", async () => {
    const words = FIXTURE_WORDS.split(" ");
    // The last word's low 8 bits are the checksum: its neighbour in the list has the same key bits.
    const last = BIP39_ENGLISH.indexOf("unaware");
    const changed = [...words.slice(0, 23), BIP39_ENGLISH[last ^ 1] ?? ""];
    const error = await refusal(parseKitWords(changed));
    expect(error.toApiError()).toEqual({
      code: "InvalidInput",
      message: "the checksum does not match: a word is wrong or out of order",
      details: { field: "kit" },
    });
    const swapped = [words[1] ?? "", words[0] ?? "", ...words.slice(2)];
    expect((await refusal(parseKitWords(swapped))).message).toContain("checksum does not match");
  });

  it("refuse keys that are not 32 bytes", async () => {
    await expect(kitWordsFromEntropy(new Uint8Array(31))).rejects.toMatchObject({ code: "Internal" });
    await expect(kitCodeFromEntropy(new Uint8Array(33))).rejects.toMatchObject({ code: "Internal" });
  });
});

describe("resolveKitWord", () => {
  it("resolves whole words in any case and prefixes of at least four letters", () => {
    expect(resolveKitWord("abandon")).toBe("abandon");
    expect(resolveKitWord(" ZOO ")).toBe("zoo");
    expect(resolveKitWord("aban")).toBe("abandon");
    expect(resolveKitWord("ABANDO")).toBe("abandon");
    expect(resolveKitWord("acti")).toBe("action");
  });

  it("takes a three-letter word for itself even when longer words start with it", () => {
    expect(resolveKitWord("act")).toBe("act");
    expect(resolveKitWord("actu")).toBe("actual");
  });

  it("resolves nothing for short prefixes, wrong continuations and other text", () => {
    const tokens = ["", "ab", "aba", "abanx", "abandons", "zzzz", "zooo", "4bandon", "abandon1", "abandon ability"];
    for (const token of tokens) {
      expect(resolveKitWord(token), token).toBeUndefined();
    }
  });

  it("takes ASCII letters only, as kb-crypto does", () => {
    // The Kelvin sign lower-cases to an ASCII "k" in JavaScript, never in Rust's `to_ascii_lowercase`.
    expect(resolveKitWord("Kite")).toBeUndefined();
    expect(resolveKitWord("kite")).toBe("kite");
    expect(resolveKitWord("étude")).toBeUndefined();
  });
});

describe("kitCodeFromEntropy and parseKitCode (the mirror's code form)", () => {
  it("writes 13 groups of 4 base32 characters of the key, then 2 checksum characters", async () => {
    const code = await kitCodeFromEntropy(FIXTURE_KEY);
    expect(code).toMatch(/^(?:[A-Z2-7]{4}-){13}[A-Z2-7]{2}$/u);
    expect(code.slice(0, 64).replaceAll("-", "")).toBe(FIXTURE_BASE32);
    // The mirror's checksum: the first 10 bits of SHA-256("atomik-kb/v1/recovery-kit" || key).
    expect(code.slice(65)).toBe("KY");
  });

  it("round-trips, in any case, with or without separators", async () => {
    for (const hex of TREZOR_256.map(([key]) => key)) {
      const code = await kitCodeFromEntropy(fromHex(hex));
      expect(toHexText(await parseKitCode(code))).toBe(hex);
      expect(toHexText(await parseKitCode(code.toLowerCase()))).toBe(hex);
      expect(toHexText(await parseKitCode(code.replaceAll("-", "")))).toBe(hex);
      expect(toHexText(await parseKitCode(` ${code.replaceAll("-", " ")}\n`))).toBe(hex);
    }
  });

  it("refuses characters outside the alphabet, a wrong length and set padding bits (kb-crypto's messages)", async () => {
    const code = await kitCodeFromEntropy(FIXTURE_KEY);
    expect((await refusal(parseKitCode(code.slice(0, -1)))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "expected 54 code characters, got 53",
      details: { field: "kit" },
    });
    expect((await refusal(parseKitCode(`${code}A`))).message).toBe("expected 54 code characters, got 55");
    // "ß" upper-cases to "SS" in JavaScript: refused per character like any other non-ASCII one.
    for (const char of ["0", "1", "8", "9", "=", "é", "ß"]) {
      const bad = `${char}${code.slice(1)}`;
      expect((await refusal(parseKitCode(bad))).message, char).toBe(
        "the code may only contain the letters A-Z, the digits 2-7, dashes and spaces",
      );
    }
    // A bad character is reported before a wrong length, as in kb-crypto.
    expect((await refusal(parseKitCode("0AAA"))).message).toContain("may only contain");
    // The 52nd character carries the key's last bit and four zero bits: "Q" = 10000, "R" = 10001.
    expect(code.slice(63, 64)).toBe("Q");
    const padded = `${code.slice(0, 63)}R${code.slice(64)}`;
    expect((await refusal(parseKitCode(padded))).message).toBe(
      "the last data character of the code is not valid in that position",
    );
  });

  it("refuses a checksum mismatch", async () => {
    const code = await kitCodeFromEntropy(FIXTURE_KEY);
    const wrongCheck = `${code.slice(0, -1)}${code.endsWith("A") ? "B" : "A"}`;
    expect((await refusal(parseKitCode(wrongCheck))).toApiError()).toEqual({
      code: "InvalidInput",
      message: "the checksum does not match: a character is wrong",
      details: { field: "kit" },
    });
    const wrongData = `B${code.slice(1)}`;
    expect((await refusal(parseKitCode(wrongData))).message).toContain("checksum does not match");
  });

  it("starts like kb-crypto's code for the all-zero and all-ones keys", async () => {
    expect(await kitCodeFromEntropy(new Uint8Array(32))).toMatch(/^(?:AAAA-){13}[A-Z2-7]{2}$/u);
    const ones = await kitCodeFromEntropy(new Uint8Array(32).fill(0xff));
    expect(ones.split("-").slice(0, 13).join("")).toBe(`${"7".repeat(51)}Q`);
  });
});

describe("parseKit", () => {
  it("reads either form of the same key", async () => {
    const code = await kitCodeFromEntropy(FIXTURE_KEY);
    expect(await parseKit(FIXTURE_WORDS)).toEqual(FIXTURE_KEY);
    expect(await parseKit(FIXTURE_WORDS.toUpperCase())).toEqual(FIXTURE_KEY);
    expect(await parseKit(code)).toEqual(FIXTURE_KEY);
    expect(await parseKit(code.replaceAll("-", " "))).toEqual(FIXTURE_KEY);
    expect(await parseKit(code.replaceAll("-", "").toLowerCase())).toEqual(FIXTURE_KEY);
  });

  it("decides like kb-crypto: word problems for word lists, code problems for everything else", async () => {
    expect((await refusal(parseKit("abandon ability able"))).message).toBe("expected 24 words, got 3");
    const words23 = FIXTURE_WORDS.split(" ").slice(0, 23).join(" ");
    expect((await refusal(parseKit(words23))).message).toBe("expected 24 words, got 23");
    expect((await refusal(parseKit("abandon"))).message).toBe("expected 54 code characters, got 7");
    expect((await refusal(parseKit("AAAA-BBBB"))).message).toBe("expected 54 code characters, got 8");
    // Letter-only groups that add up to the 54 characters of a code are read as a code.
    const lettersOnly = `${"AAAA ".repeat(13)}AB`;
    expect((await refusal(parseKit(lettersOnly))).message).not.toContain("words");
    // Exactly 24 letter-only tokens are always words.
    const typo = FIXTURE_WORDS.replace("liar", "liqr");
    expect((await refusal(parseKit(typo))).message).toBe(
      "word 3 is not in the word list (nor a unique prefix of 4 letters or more)",
    );
    // Blank text is read as a code of no characters, as kb-crypto's `parse` does.
    for (const blank of ["", " \n\t "]) {
      expect((await refusal(parseKit(blank))).toApiError()).toEqual({
        code: "InvalidInput",
        message: "expected 54 code characters, got 0",
        details: { field: "kit" },
      });
    }
  });
});

const GOLDEN = "fixtures/crypto/kit-v1.json";
const goldenMissing = !repoFileExists(GOLDEN);
const goldenNote = goldenMissing ? ` [skipped: ${GOLDEN} is missing; kb-crypto's make_vectors example writes it]` : "";

/**
 * Finds, anywhere in the golden JSON, an object that has the fixture key as a hex string value
 * and its `words` (an array or one string), with its `code` when present.
 */
function findVector(json: unknown, keyHex: string): { words: string[]; code: string | undefined } | undefined {
  if (Array.isArray(json)) {
    for (const item of json as unknown[]) {
      const found = findVector(item, keyHex);
      if (found !== undefined) {
        return found;
      }
    }
    return undefined;
  }
  if (typeof json !== "object" || json === null) {
    return undefined;
  }
  const fields = json as Record<string, unknown>;
  const words = fields.words;
  const hasKey = Object.values(fields).some((value) => typeof value === "string" && value.toLowerCase() === keyHex);
  if (hasKey && (typeof words === "string" || Array.isArray(words))) {
    return {
      words: typeof words === "string" ? words.trim().split(/\s+/u) : (words as unknown[]).map(String),
      code: typeof fields.code === "string" ? fields.code : undefined,
    };
  }
  return findVector(Object.values(fields), keyHex);
}

const KAT = "fixtures/crypto/kat-bip39.json";
const katMissing = !repoFileExists(KAT);
const katNote = katMissing ? ` [skipped: ${KAT} is missing; kb-crypto's known-answer tests own it]` : "";

describe("cross-check with kb-crypto", () => {
  it.skipIf(katMissing)(`encodes every vector of kb-crypto's BIP-39 known-answer file${katNote}`, async () => {
    const file = JSON.parse(readRepoText(KAT)) as { vectors: { entropy_hex: string; mnemonic: string }[] };
    expect(file.vectors.length).toBeGreaterThanOrEqual(8);
    for (const { entropy_hex: hex, mnemonic } of file.vectors) {
      expect((await kitWordsFromEntropy(fromHex(hex))).join(" "), hex).toBe(mnemonic);
      expect(toHexText(await parseKit(mnemonic)), hex).toBe(hex);
    }
  });

  it.skipIf(goldenMissing)(`the words of the fixture key equal the Rust golden${goldenNote}`, async () => {
    const vector = findVector(JSON.parse(readRepoText(GOLDEN)), toHexText(FIXTURE_KEY));
    expect(vector, `no vector for the key 00..1f with its words in ${GOLDEN}`).toBeDefined();
    const words = await kitWordsFromEntropy(FIXTURE_KEY);
    expect(vector?.words).toEqual(words);
    expect(vector?.words.join(" ")).toBe(FIXTURE_WORDS);
    if (vector?.code !== undefined) {
      // Same 52 data characters; the 2 checksum characters differ by design (BLAKE3 there).
      const ours = await kitCodeFromEntropy(FIXTURE_KEY);
      expect(vector.code.toUpperCase().replaceAll(/[\s-]/gu, "").slice(0, 52)).toBe(
        ours.replaceAll("-", "").slice(0, 52),
      );
    }
  });

  it.skipIf(goldenMissing)(`agrees with every kit of the Rust golden${goldenNote}`, async () => {
    const golden = JSON.parse(readRepoText(GOLDEN)) as { kits?: { key_hex: string; words: string[]; code: string }[] };
    const kits = golden.kits ?? [];
    expect(kits.length, `${GOLDEN} has no "kits"`).toBeGreaterThan(0);
    for (const kit of kits) {
      const key = fromHex(kit.key_hex);
      expect(await kitWordsFromEntropy(key), kit.key_hex).toEqual(kit.words);
      expect(toHexText(await parseKitWords(kit.words)), kit.key_hex).toBe(kit.key_hex);
      expect(toHexText(await parseKit(kit.words.join(" "))), kit.key_hex).toBe(kit.key_hex);
      // The code: the same groups of data characters, a checksum of the mirror's own.
      const ours = await kitCodeFromEntropy(key);
      expect(ours.slice(0, 64), kit.key_hex).toBe(kit.code.slice(0, 64));
    }
  });
});
