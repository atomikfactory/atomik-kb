// A name-based UUID (version 5, RFC 4122) with a small synchronous SHA-1, for the in-memory import of an
// AppFlowy backup (ADR-0039): a database row's document is stored under `uuid_v5(namespace = row id,
// name = "document_id")`, and the plan is synchronous. Not for security: SHA-1 here only derives an id.

const utf8 = new TextEncoder();

function rotl(value: number, bits: number): number {
  return ((value << bits) | (value >>> (32 - bits))) >>> 0;
}

/** SHA-1 of `message` (FIPS 180-4). */
export function sha1(message: Uint8Array): Uint8Array {
  const length = message.length;
  const total = Math.ceil((length + 9) / 64) * 64;
  const data = new Uint8Array(total);
  data.set(message);
  data[length] = 0x80;
  const view = new DataView(data.buffer);
  view.setUint32(total - 8, Math.floor((length * 8) / 0x1_0000_0000));
  view.setUint32(total - 4, (length * 8) >>> 0);
  let h0 = 0x67452301;
  let h1 = 0xefcdab89;
  let h2 = 0x98badcfe;
  let h3 = 0x10325476;
  let h4 = 0xc3d2e1f0;
  const w: number[] = new Array<number>(80).fill(0);
  const word = (at: number): number => w[at] ?? 0;
  for (let block = 0; block < total; block += 64) {
    for (let t = 0; t < 16; t += 1) {
      w[t] = view.getUint32(block + t * 4);
    }
    for (let t = 16; t < 80; t += 1) {
      w[t] = rotl(word(t - 3) ^ word(t - 8) ^ word(t - 14) ^ word(t - 16), 1);
    }
    let a = h0;
    let b = h1;
    let c = h2;
    let d = h3;
    let e = h4;
    for (let t = 0; t < 80; t += 1) {
      let f: number;
      let k: number;
      if (t < 20) {
        f = (b & c) | (~b & d);
        k = 0x5a827999;
      } else if (t < 40) {
        f = b ^ c ^ d;
        k = 0x6ed9eba1;
      } else if (t < 60) {
        f = (b & c) | (b & d) | (c & d);
        k = 0x8f1bbcdc;
      } else {
        f = b ^ c ^ d;
        k = 0xca62c1d6;
      }
      const next = (rotl(a, 5) + (f >>> 0) + e + k + word(t)) >>> 0;
      e = d;
      d = c;
      c = rotl(b, 30);
      b = a;
      a = next;
    }
    h0 = (h0 + a) >>> 0;
    h1 = (h1 + b) >>> 0;
    h2 = (h2 + c) >>> 0;
    h3 = (h3 + d) >>> 0;
    h4 = (h4 + e) >>> 0;
  }
  const out = new Uint8Array(20);
  const outView = new DataView(out.buffer);
  [h0, h1, h2, h3, h4].forEach((h, at) => {
    outView.setUint32(at * 4, h);
  });
  return out;
}

/**
 * The version 5 UUID of `name` in the namespace `namespace` (UUID text, hyphens optional), in lowercase
 * hyphenated form; `undefined` when `namespace` is not 32 hex digits.
 */
export function uuidV5(namespace: string, name: string): string | undefined {
  const hex = namespace.replaceAll("-", "");
  if (!/^[0-9a-fA-F]{32}$/u.test(hex)) {
    return undefined;
  }
  const bytes = Uint8Array.from(hex.match(/../gu) ?? [], (pair) => Number.parseInt(pair, 16));
  const nameBytes = utf8.encode(name);
  const input = new Uint8Array(bytes.length + nameBytes.length);
  input.set(bytes);
  input.set(nameBytes, bytes.length);
  const hash = sha1(input).slice(0, 16);
  hash[6] = ((hash[6] ?? 0) & 0x0f) | 0x50;
  hash[8] = ((hash[8] ?? 0) & 0x3f) | 0x80;
  const digits = Array.from(hash, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return [digits.slice(0, 8), digits.slice(8, 12), digits.slice(12, 16), digits.slice(16, 20), digits.slice(20)].join("-");
}
