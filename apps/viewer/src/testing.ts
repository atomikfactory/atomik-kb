// Helpers of the viewer's tests (M13 slice 4): the real `kb-wasm` module (the one `scripts/build-wasm.mjs` made),
// the fixtures `crates/kb-wasm/examples/viewer_fixture.rs` made with the real publication codec, and a fake server that
// answers the way `kb-server` does (`GET /v2/links/{id}` with an `ETag` and 304, the chunks, 404 for a link that is
// gone) and remembers every request it was sent.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import type { KbWasm } from "./kb-wasm";
import * as glue from "./wasm/kb_wasm.js";

const root = join(fileURLToPath(new URL(".", import.meta.url)), "..");

/** `fixtures/fixture.json`: what the fixtures hold. */
export interface Fixture {
  fragment: string;
  linkId: string;
  pageId: string;
  titles: [string, string];
  icon: string;
  cover: string;
  coverY: number;
  mentions: { accountId: string; name: string }[];
  attachment: { attachmentId: string; name: string; mime: string; size: number; chunkSize: number; chunks: string[] };
  texts: [string, string, string];
}

export interface Fixtures {
  description: Fixture;
  /** The first and the second publication of the link (the second has one more paragraph and another title). */
  publications: [Uint8Array, Uint8Array];
  /** The picture's one sealed chunk. */
  chunk: Uint8Array;
  /** The picture's plaintext. */
  picture: Uint8Array;
}

/** The fixture files. */
export function loadFixtures(): Fixtures {
  // A plain `Uint8Array` (Node.js hands out a `Buffer`, which is not equal to one).
  const read = (name: string): Uint8Array => new Uint8Array(readFileSync(join(root, "fixtures", name)));
  return {
    description: JSON.parse(readFileSync(join(root, "fixtures", "fixture.json"), "utf8")) as Fixture,
    publications: [read("publication-1.bin"), read("publication-2.bin")],
    chunk: read("chunk.bin"),
    picture: read("picture.png"),
  };
}

let wasm: KbWasm | undefined;

/** The real module, instantiated from the file `build-wasm.mjs` wrote. */
export function realWasm(): KbWasm {
  if (wasm === undefined) {
    glue.initSync({ module: readFileSync(join(root, "src", "wasm", "kb_wasm_bg.wasm")) });
    wasm = glue;
  }
  return wasm;
}

/** One request the fake server saw. */
export interface SeenRequest {
  url: string;
  init: RequestInit | undefined;
}

/** What the fake server answers next, besides the link's publications. */
export type Failure = "network" | 404 | 410 | 429 | 500 | 503;

/**
 * A server with one link: `publications[current]` is served at `/v2/links/{linkId}` with `ETag: "<current + 1>"`
 * (`If-None-Match` of that gives 304) and its chunks at `/v2/links/{linkId}/chunks/{hash}`.
 */
export class FakeServer {
  readonly seen: SeenRequest[] = [];
  /** Index into `publications` of the one served now. */
  current = 0;
  /** Answer every request with this instead (`undefined`: serve). */
  failure: Failure | undefined;
  /** Answer the chunks with this body instead of the real one. */
  chunkBody: Uint8Array | undefined;

  constructor(
    readonly linkId: string,
    readonly publications: readonly Uint8Array[],
    readonly chunks: ReadonlyMap<string, Uint8Array>,
  ) {}

  /** The `fetch` the transport is given. */
  readonly fetch = (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = input instanceof Request ? input.url : String(input);
    this.seen.push({ url, init });
    return Promise.resolve(this.#answer(url, init));
  };

  #answer(url: string, init: RequestInit | undefined): Response {
    if (this.failure === "network") {
      throw new TypeError("Failed to fetch");
    }
    if (this.failure !== undefined) {
      return new Response(null, { status: this.failure });
    }
    const prefix = `/v2/links/${this.linkId}`;
    if (url === prefix) {
      const etag = `"${String(this.current + 1)}"`;
      const headers = new Headers(init?.headers);
      if (headers.get("If-None-Match") === etag) {
        return new Response(null, { status: 304, headers: { ETag: etag } });
      }
      const body = this.publications[this.current];
      return body === undefined
        ? new Response(null, { status: 404 })
        : new Response(body as BodyInit, { status: 200, headers: { ETag: etag } });
    }
    const hash = url.startsWith(`${prefix}/chunks/`) ? url.slice(prefix.length + "/chunks/".length) : undefined;
    const chunk = hash === undefined ? undefined : (this.chunkBody ?? this.chunks.get(hash));
    return chunk === undefined ? new Response(null, { status: 404 }) : new Response(chunk as BodyInit, { status: 200 });
  }

  /** The `If-None-Match` of each request to the link itself, in order (`undefined` for none). */
  conditions(): (string | undefined)[] {
    return this.seen
      .filter((request) => request.url === `/v2/links/${this.linkId}`)
      .map((request) => new Headers(request.init?.headers).get("If-None-Match") ?? undefined);
  }
}

/** A fake server over the fixtures. */
export function fixtureServer(fixtures: Fixtures): FakeServer {
  const [hash] = fixtures.description.attachment.chunks;
  return new FakeServer(
    fixtures.description.linkId,
    fixtures.publications,
    new Map(hash === undefined ? [] : [[hash, fixtures.chunk]]),
  );
}
