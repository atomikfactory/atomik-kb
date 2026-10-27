// SHA-256 through WebCrypto (`globalThis.crypto.subtle`: browsers in a secure context,
// workers, Node 20+) and hex text, for the in-memory core mirror. Nothing here protects
// anything: the mirror's hashes only stand in for the core's key derivations (the kit
// checksum of BIP-39 is SHA-256 for real, see `./kit`).

const utf8 = new TextEncoder();

/** SHA-256 of the concatenated parts; strings count as their UTF-8 bytes. */
export async function sha256(...parts: readonly (Uint8Array | string)[]): Promise<Uint8Array> {
  const chunks = parts.map((part) => (typeof part === "string" ? utf8.encode(part) : part));
  // A fresh, ArrayBuffer-backed copy: what `subtle.digest` accepts, and erasable afterwards.
  const input = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    input.set(chunk, offset);
    offset += chunk.length;
  }
  try {
    return new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", input));
  } finally {
    // The input may hold key bytes; `digest` copied them when it was called.
    input.fill(0);
  }
}

/** Lowercase hex of `bytes`. */
export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
