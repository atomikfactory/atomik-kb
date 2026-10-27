import { describe, expect, it } from "vitest";

import { sha1, uuidV5 } from "./uuid-v5";

const hex = (bytes: Uint8Array): string => Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");

describe("sha1", () => {
  it("matches the published vectors", () => {
    const text = new TextEncoder();
    expect(hex(sha1(text.encode("")))).toBe("da39a3ee5e6b4b0d3255bfef95601890afd80709");
    expect(hex(sha1(text.encode("abc")))).toBe("a9993e364706816aba3e25717850c26c9cd0d89d");
    expect(hex(sha1(text.encode("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")))).toBe(
      "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
    );
    // A message that needs a second padding block (55 and 56 bytes straddle the length field).
    expect(hex(sha1(text.encode("a".repeat(55))))).toBe("c1c8bbdc22796e28c0e15163d20899b65621d65a");
    expect(hex(sha1(text.encode("a".repeat(56))))).toBe("c2db330f6083854c99d4b5bfb6e8f29f201be699");
    expect(hex(sha1(text.encode("a".repeat(1000))))).toBe("291e9a6c66994949b57ba5e650361e98fc36b1ba");
  });
});

describe("uuidV5", () => {
  it("matches the published name-based UUID of python.org in the DNS namespace", () => {
    expect(uuidV5("6ba7b810-9dad-11d1-80b4-00c04fd430c8", "python.org")).toBe("886313e1-3b8a-5372-9b90-0c9aee199e5d");
  });

  it("accepts a namespace without hyphens and refuses one that is not a UUID", () => {
    expect(uuidV5("6ba7b8109dad11d180b400c04fd430c8", "python.org")).toBe("886313e1-3b8a-5372-9b90-0c9aee199e5d");
    expect(uuidV5("not a uuid", "x")).toBeUndefined();
  });
});
