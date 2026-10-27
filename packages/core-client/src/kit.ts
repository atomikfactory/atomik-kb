// Recovery kit encodings (ADR-0017) in TypeScript, for the in-memory core mirror and for
// tests: the 24 BIP-39 English words of a 32-byte recovery key and the `xxxx-…-xxxx-cc`
// code form, both decodable back to the key.
//
// The words are byte-compatible with kb-crypto's `kit::words`/`from_words` (same vendored
// list, standard SHA-256 checksum), so they are safe to use on a kit the real core issued.
// The code form is NOT: kb-crypto's 2 checksum characters are the first 10 bits of
// BLAKE3(`atomik-kb/v1/recovery-kit` || key), and browsers have no BLAKE3, so this mirror
// takes them from SHA-256 over the same bytes. Its 52 data characters (RFC 4648 base32 of
// the key) are identical; its checksum characters differ, and a real core's code fails
// `parseKitCode` here. Errors are `InvalidInput` for the field `kit` with kb-crypto's
// messages (`kit::from_words`, `from_code`, `parse`) and never quote the kit, which is a
// secret: a bad word is reported by its position.

import { sha256 } from "./digest";
import { internalError } from "./errors";
import { invalid } from "./validate";
import { BIP39_ENGLISH } from "./wordlist-english";

/** Words in a kit. */
export const KIT_WORDS = 24;
/** Bytes of the recovery key a kit encodes. */
export const KIT_KEY_BYTES = 32;

const BITS_PER_WORD = 11;
const BASE32_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const CODE_DATA_CHARS = 52;
const CODE_CHECK_CHARS = 2;
const CODE_GROUP_CHARS = 4;
/** Domain of the code checksum (ADR-0017), hashed with SHA-256 here instead of BLAKE3. */
const CODE_CHECKSUM_DOMAIN = "atomik-kb/v1/recovery-kit";

/** Position of every word in the list. */
const WORD_INDEX: ReadonlyMap<string, number> = new Map(BIP39_ENGLISH.map((word, index) => [word, index]));
/** Position of every word by its first four letters (a three-letter word is its own prefix). */
const PREFIX_INDEX: ReadonlyMap<string, number> = new Map(
  BIP39_ENGLISH.map((word, index) => [word.slice(0, 4), index]),
);

/** Reads `count` bits (at most 31) from `bytes`, most significant first; bits past the end read as 0. */
function readBits(bytes: Uint8Array, start: number, count: number): number {
  let value = 0;
  for (let bit = start; bit < start + count; bit += 1) {
    value = (value << 1) | (((bytes[bit >> 3] ?? 0) >> (7 - (bit & 7))) & 1);
  }
  return value;
}

/** Writes the low `count` bits of `value` into zero-initialised `bytes`, most significant first. */
function writeBits(bytes: Uint8Array, start: number, count: number, value: number): void {
  for (let offset = 0; offset < count; offset += 1) {
    if (((value >> (count - 1 - offset)) & 1) === 1) {
      const bit = start + offset;
      bytes[bit >> 3] = (bytes[bit >> 3] ?? 0) | (0x80 >> (bit & 7));
    }
  }
}

function requireKey(key: Uint8Array): void {
  if (key.length !== KIT_KEY_BYTES) {
    throw internalError(`a recovery key is ${String(KIT_KEY_BYTES)} bytes, not ${String(key.length)}`);
  }
}

function wordAt(index: number): string {
  const word = BIP39_ENGLISH[index];
  if (word === undefined) {
    throw internalError("word index out of range");
  }
  return word;
}

function base32At(value: number): string {
  const char = BASE32_ALPHABET[value];
  if (char === undefined) {
    throw internalError("base32 value out of range");
  }
  return char;
}

/** The whitespace-separated tokens of `text` (none for blank text), like Rust's `split_whitespace`. */
function tokensOf(text: string): string[] {
  const trimmed = text.trim();
  return trimmed === "" ? [] : trimmed.split(/\s+/u);
}

/** Position of a typed word in the list; see {@link resolveKitWord}. */
function wordIndex(token: string): number | undefined {
  // ASCII letters only, as kb-crypto: `toLowerCase` would also turn some other letters
  // (the Kelvin sign, for one) into ASCII ones.
  if (!/^[A-Za-z]+$/u.test(token)) {
    return undefined;
  }
  const lower = token.toLowerCase();
  const exact = WORD_INDEX.get(lower);
  if (exact !== undefined || lower.length < 4) {
    return exact;
  }
  const index = PREFIX_INDEX.get(lower.slice(0, 4));
  return index !== undefined && wordAt(index).startsWith(lower) ? index : undefined;
}

/**
 * The list word a typed token stands for, or `undefined`: any case; the whole word, or a
 * prefix of at least four letters (BIP-39 words are unique in their first four letters, so
 * "aban" and "abando" both mean "abandon"; "abanx" means nothing). For the UI's prefix
 * help and for checking typed words against a shown kit.
 */
export function resolveKitWord(token: string): string | undefined {
  const index = wordIndex(token.trim());
  return index === undefined ? undefined : wordAt(index);
}

/**
 * The 24 words of a 32-byte key (BIP-39: the 256 key bits followed by the first 8 bits of
 * SHA-256(key), cut into 24 indexes of 11 bits). Equals kb-crypto's `kit::words`.
 */
export async function kitWordsFromEntropy(entropy: Uint8Array): Promise<string[]> {
  requireKey(entropy);
  const bits = new Uint8Array(KIT_KEY_BYTES + 1);
  bits.set(entropy);
  bits[KIT_KEY_BYTES] = (await sha256(entropy))[0] ?? 0;
  const words: string[] = [];
  for (let position = 0; position < KIT_WORDS; position += 1) {
    words.push(wordAt(readBits(bits, position * BITS_PER_WORD, BITS_PER_WORD)));
  }
  bits.fill(0);
  return words;
}

/**
 * Decodes 24 words back to the key: a string (whitespace between words, surrounding space
 * ignored) or the words one by one. Each word is resolved like {@link resolveKitWord}.
 * A wrong count, a word outside the list (reported by its position, never quoted) or a
 * checksum mismatch rejects with `InvalidInput` for the field `kit`.
 */
export async function parseKitWords(words: string | readonly string[]): Promise<Uint8Array> {
  const tokens = typeof words === "string" ? tokensOf(words) : words.map((word) => word.trim());
  if (tokens.length !== KIT_WORDS) {
    throw invalid("kit", `expected ${String(KIT_WORDS)} words, got ${String(tokens.length)}`);
  }
  const bits = new Uint8Array(KIT_KEY_BYTES + 1);
  tokens.forEach((token, position) => {
    const index = wordIndex(token);
    if (index === undefined) {
      throw invalid(
        "kit",
        `word ${String(position + 1)} is not in the word list (nor a unique prefix of 4 letters or more)`,
      );
    }
    writeBits(bits, position * BITS_PER_WORD, BITS_PER_WORD, index);
  });
  const key = bits.slice(0, KIT_KEY_BYTES);
  const checksum = bits[KIT_KEY_BYTES];
  bits.fill(0);
  if ((await sha256(key))[0] !== checksum) {
    key.fill(0);
    throw invalid("kit", "the checksum does not match: a word is wrong or out of order");
  }
  return key;
}

/** The mirror's code checksum: the first 10 bits of SHA-256(domain || key) (kb-crypto: BLAKE3). */
async function codeChecksum(key: Uint8Array): Promise<number> {
  return readBits(await sha256(CODE_CHECKSUM_DOMAIN, key), 0, CODE_CHECK_CHARS * 5);
}

/**
 * The code form of a 32-byte key: 52 base32 characters (RFC 4648 alphabet, uppercase, the
 * key's 256 bits then 4 zero bits) in groups of four, then a 2-character checksum group,
 * joined by `-`. The checksum is the mirror's (SHA-256, see the module documentation), so
 * only the first 52 characters match kb-crypto's `kit::code`.
 */
export async function kitCodeFromEntropy(entropy: Uint8Array): Promise<string> {
  requireKey(entropy);
  const groups: string[] = [];
  let group = "";
  for (let index = 0; index < CODE_DATA_CHARS; index += 1) {
    group += base32At(readBits(entropy, index * 5, 5));
    if (group.length === CODE_GROUP_CHARS) {
      groups.push(group);
      group = "";
    }
  }
  const checksum = await codeChecksum(entropy);
  groups.push(base32At(checksum >> 5) + base32At(checksum & 0x1f));
  return groups.join("-");
}

/**
 * Decodes the code form (any case; `-` and whitespace anywhere are ignored) back to the key.
 * A character outside the alphabet, a wrong length, non-zero padding bits or a checksum
 * mismatch (checked in that order, as kb-crypto does) rejects with `InvalidInput` for `kit`.
 */
export async function parseKitCode(text: string): Promise<Uint8Array> {
  // Per character and ASCII only: `toUpperCase` on the whole text would turn "ß" into "SS".
  const values = Array.from(text.replace(/[\s-]/gu, ""), (char) =>
    /^[A-Za-z2-7]$/u.test(char) ? BASE32_ALPHABET.indexOf(char.toUpperCase()) : -1,
  );
  if (values.some((value) => value < 0)) {
    throw invalid("kit", "the code may only contain the letters A-Z, the digits 2-7, dashes and spaces");
  }
  if (values.length !== CODE_DATA_CHARS + CODE_CHECK_CHARS) {
    throw invalid(
      "kit",
      `expected ${String(CODE_DATA_CHARS + CODE_CHECK_CHARS)} code characters, got ${String(values.length)}`,
    );
  }
  const bytes = new Uint8Array(KIT_KEY_BYTES + 1);
  values.slice(0, CODE_DATA_CHARS).forEach((value, index) => {
    writeBits(bytes, index * 5, 5, value);
  });
  const padding = readBits(bytes, KIT_KEY_BYTES * 8, CODE_DATA_CHARS * 5 - KIT_KEY_BYTES * 8);
  const key = bytes.slice(0, KIT_KEY_BYTES);
  bytes.fill(0);
  if (padding !== 0) {
    key.fill(0);
    throw invalid("kit", "the last data character of the code is not valid in that position");
  }
  const [high = 0, low = 0] = values.slice(CODE_DATA_CHARS);
  if (((high << 5) | low) !== (await codeChecksum(key))) {
    key.fill(0);
    throw invalid("kit", "the checksum does not match: a character is wrong");
  }
  return key;
}

/**
 * Decodes a typed kit in either form, deciding like kb-crypto's `kit::parse`: exactly 24
 * tokens of ASCII letters are words; anything else is read as the code, and when that fails
 * for text that looked like a word list of the wrong length (several letter-only tokens that
 * do not add up to the 54 characters of a code), the error says `expected 24 words, got N`.
 */
export async function parseKit(text: string): Promise<Uint8Array> {
  const trimmed = text.trim();
  const tokens = tokensOf(trimmed);
  const alphabetic = tokens.length > 0 && tokens.every((token) => /^[a-z]+$/iu.test(token));
  if (alphabetic && tokens.length === KIT_WORDS) {
    return parseKitWords(tokens);
  }
  try {
    return await parseKitCode(trimmed);
  } catch (error) {
    const letters = tokens.reduce((total, token) => total + token.length, 0);
    if (alphabetic && tokens.length > 1 && letters !== CODE_DATA_CHARS + CODE_CHECK_CHARS) {
      throw invalid("kit", `expected ${String(KIT_WORDS)} words, got ${String(tokens.length)}`);
    }
    throw error;
  }
}
