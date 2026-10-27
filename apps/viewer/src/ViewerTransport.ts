// The viewer's transport (M13 slice 4, ADR-0051, `docs/development/milestone-13.md` 4.2 and its contract notes): what
// `@atomik-kb/ui`'s read-only page view talks to in place of the core. It fetches the link's publication from the server
// the viewer came from (`GET /v2/links/{id}`, same origin, no credentials, no referrer), opens it with `kb-wasm` under
// the key in the URL fragment, and answers the editor's two binary calls from it: `docs.open` (the page's document, a
// shallow snapshot) and `attachments.read` (an attachment's bytes, from the chunks the server serves at
// `GET /v2/links/{id}/chunks/{hash}` and `kb-wasm` decrypts). Everything that would write is refused, and every
// request the page does not need answers `Forbidden`: the viewer shows exactly one page.
//
// What the server learns from the viewer is the link id (derived from the key, which tells it nothing about the key),
// the chunk hashes of the page's pictures, the viewer's address and the time of each poll. The key (the fragment) is
// never part of a request: it is the input of `kb-wasm`'s `linkId` and `openPublication` and nothing else, and no
// message, log line or error of this file repeats it.

import {
  CoreError,
  LINK_POLL_SECONDS,
  type AttachmentId,
  type AttachmentInfo,
  type AttachmentTransport,
  type ByteRange,
  type DocTransport,
  type Event,
  type PageId,
  type PersonInfo,
  type Request,
  type Response as ApiResponse,
  type SpaceId,
  type Transport,
} from "@atomik-kb/core-client";

import type { KbWasm, OpenedPublication, PublishedAttachmentInfo, PublishedPersonInfo } from "./kb-wasm";

/** The space the viewer names for its one page: nothing outside the page sees it. */
export const VIEWER_SPACE_ID: SpaceId = "00000000-0000-4000-8000-000000000000";

/** Bytes of the `kb-doc` blob header (`KBD1`, engine, format version, minimum reader version, kind) before the engine's snapshot. */
const BLOB_HEADER_LEN = 10;
/** The blob's magic and the engine id of Loro. */
const BLOB_MAGIC = [0x4b, 0x42, 0x44, 0x31];
const ENGINE_LORO = 1;
/** The newest `kb-doc` format this viewer reads (`FORMAT_VERSION` of `kb-doc`). */
const FORMAT_VERSION = 1;

/** What loading or polling the link ended in. */
export type LinkLoad =
  /**
   * A publication was fetched and opened (the first, or one newer than the last: `pubSeq` is its `ETag`). `retry` is set
   * when it is the publication already shown, handed over again because a picture of the page was not available when it
   * was asked for (a chunk the server did not hold yet): the page is drawn again and asks for the pictures once more.
   */
  | { status: "ready"; publication: OpenedPublication; pubSeq: number; retry?: true }
  /** The server answered 304: what is shown is current. */
  | { status: "unchanged" }
  /** The server answered 404: the link was revoked, has expired, or never existed (one answer for all three). */
  | { status: "gone" }
  /** The key is not a key, or the content does not open under it (`bad_key`, `authentication`, `malformed`). */
  | { status: "invalid" }
  /** The network failed, or the server answered 429 or 5xx: show what is there, try again at the next poll. */
  | { status: "unavailable" };

/** What a `ViewerTransport` is made of. */
export interface ViewerTransportOptions {
  /** What follows the `#` of the page's address (`location.hash`; a leading `#` is fine): the link's key. */
  fragment: string;
  /** The `kb-wasm` module (loaded by the caller; tests pass a fake). */
  wasm: KbWasm;
  /**
   * `fetch` for the requests to the server (default `globalThis.fetch`); every request is made with
   * `credentials: "omit"`, `referrerPolicy: "no-referrer"` and `cache: "no-store"`, to relative `/v2/links/...` URLs
   * only (the policy's `connect-src 'self'`).
   */
  fetch?: typeof globalThis.fetch;
  /** Seconds between polls (default `LINK_POLL_SECONDS`, 60); tests shorten it. */
  pollSeconds?: number;
}

/** The refusal of everything that would change the page. */
function readOnly(what: string): Promise<never> {
  return Promise.reject(
    new CoreError({ code: "Forbidden", message: `${what}: a view link is read-only`, details: { reason: "role", required: "editor" } }),
  );
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** The most sealed chunk bytes a viewer keeps for the next read (the pictures of one page are usually a few). */
const CHUNK_CACHE_BYTES = 64 * 1024 * 1024;

/** The 64 lowercase hex characters of a chunk id: the only form that goes into a request path. */
const CHUNK_ID = /^[0-9a-f]{64}$/u;

/** The `pub_seq` of an `ETag` (`"N"`, with a `W/` prefix tolerated), or `null` for any other text. */
function parseEtag(header: string | null): number | null {
  const match = header === null ? null : /^(?:W\/)?"(\d{1,15})"$/u.exec(header.trim());
  return match?.[1] === undefined ? null : Number(match[1]);
}

/** The `kb-doc` blob's engine snapshot: the bytes `loro-crdt` imports, after the blob's header is checked. */
function engineSnapshot(blob: Uint8Array): Uint8Array {
  const header = blob.subarray(0, BLOB_HEADER_LEN);
  const magic = BLOB_MAGIC.every((byte, index) => header[index] === byte);
  if (header.length < BLOB_HEADER_LEN || !magic || header[4] !== ENGINE_LORO) {
    throw new CoreError({ code: "Crypto", message: "the page's document is not in a form this viewer reads" });
  }
  const minReader = (header[7] ?? 0) | ((header[8] ?? 0) << 8);
  if (minReader > FORMAT_VERSION) {
    throw new CoreError({ code: "Crypto", message: "the page was made by a newer version than this viewer reads" });
  }
  return blob.subarray(BLOB_HEADER_LEN);
}

/** The attachments of a publication as `kb-wasm` lists them; anything else in the text is a malformed publication. */
function parseAttachments(text: string): PublishedAttachmentInfo[] {
  const parsed: unknown = JSON.parse(text);
  if (!Array.isArray(parsed)) {
    throw new Error("malformed: the attachment list is not a list");
  }
  return parsed as PublishedAttachmentInfo[];
}

function parseMentions(text: string): PublishedPersonInfo[] {
  const parsed: unknown = JSON.parse(text);
  if (!Array.isArray(parsed)) {
    throw new Error("malformed: the mention list is not a list");
  }
  return parsed as PublishedPersonInfo[];
}

/** The transport the viewer's page view runs on. */
export class ViewerTransport implements Transport {
  readonly #fragment: string;
  readonly #wasm: KbWasm;
  readonly #fetch: typeof globalThis.fetch;
  readonly #pollMs: number;
  readonly #listeners = new Set<(event: Event) => void>();
  /** The link id the key derives (`null` while not derived, or when the fragment is not a key). */
  #linkId: string | null = null;
  #publication: OpenedPublication | null = null;
  #pubSeq: number | null = null;
  #loading: Promise<LinkLoad> | null = null;
  /** A picture of the shown publication was not available (404 or an error) since it was last handed out. */
  #pictureMissed = false;
  /** Consecutive answers of 429 ("too many requests"): each doubles the wait until the next poll. */
  #limited = 0;
  /** The sealed chunks fetched so far (or being fetched), by chunk id, and the size of each and of all of them. */
  readonly #sealed = new Map<string, Promise<Uint8Array>>();
  readonly #sizes = new Map<string, number>();
  #kept = 0;
  /** Counts publications when the server sends no usable `ETag`. */
  #sequence = 0;
  #attachments = new Map<string, PublishedAttachmentInfo>();
  #people: PersonInfo[] = [];

  constructor(options: ViewerTransportOptions) {
    this.#fragment = options.fragment;
    this.#wasm = options.wasm;
    // Called as a method of the global object, as a browser needs it.
    this.#fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
    this.#pollMs = (options.pollSeconds ?? LINK_POLL_SECONDS) * 1000;
  }

  /** The link id the fragment derives; `null` when the fragment is not a link key. */
  #id(): string | null {
    if (this.#linkId === null) {
      try {
        this.#linkId = this.#wasm.linkId(this.#fragment);
      } catch {
        return null;
      }
    }
    return this.#linkId;
  }

  /** One request to the viewer's own server: no credentials, no referrer, no cache, a relative address. */
  #get(path: string, headers?: Record<string, string>): Promise<globalThis.Response> {
    return this.#fetch(path, {
      method: "GET",
      credentials: "omit",
      referrerPolicy: "no-referrer",
      cache: "no-store",
      ...(headers === undefined ? {} : { headers }),
    });
  }

  /**
   * Fetches the link's publication (with `If-None-Match` once one is held) and opens it. Never rejects for a
   * link that does not work: that is the state it answers. (The fragment in the options is the link's key:
   * nothing here logs it or sends it anywhere.)
   */
  load(): Promise<LinkLoad> {
    // Two callers that ask while a request is under way (a page mounted twice, a poll and a retry) get one answer: the
    // second would otherwise be told "unchanged" about a publication the first has not shown yet.
    this.#loading ??= this.#fetchPublication().finally(() => {
      this.#loading = null;
    });
    return this.#loading;
  }

  async #fetchPublication(): Promise<LinkLoad> {
    const id = this.#id();
    if (id === null) {
      return { status: "invalid" };
    }
    let bytes: Uint8Array;
    let etag: number | null;
    try {
      const response = await this.#get(
        `/v2/links/${id}`,
        this.#pubSeq === null ? undefined : { "If-None-Match": `"${String(this.#pubSeq)}"` },
      );
      this.#limited = response.status === 429 ? this.#limited + 1 : 0;
      if (response.status === 304) {
        const shown = this.#publication;
        if (this.#pictureMissed && shown !== null) {
          // What is shown is current, but a picture of it was not there: draw the page again and ask once more.
          this.#pictureMissed = false;
          return { status: "ready", publication: shown, pubSeq: this.#pubSeq ?? this.#sequence, retry: true };
        }
        return { status: "unchanged" };
      }
      if (response.status === 404 || response.status === 410) {
        return { status: "gone" };
      }
      if (!response.ok) {
        return { status: "unavailable" };
      }
      etag = parseEtag(response.headers.get("ETag"));
      bytes = new Uint8Array(await response.arrayBuffer());
    } catch {
      return { status: "unavailable" };
    }
    if (etag !== null && etag === this.#pubSeq) {
      // The server answered with what is shown (a cache that ignored the condition): nothing changed.
      return { status: "unchanged" };
    }
    let publication: OpenedPublication;
    let attachments: PublishedAttachmentInfo[];
    let mentions: PublishedPersonInfo[];
    try {
      publication = this.#wasm.openPublication(this.#fragment, bytes);
      attachments = parseAttachments(publication.attachmentsJson());
      mentions = parseMentions(publication.mentionsJson());
    } catch {
      // `kb-wasm` says why in the code at the front of its message (`wasmErrorCode`: a fragment that is not a key, a
      // publication that does not open under it, bytes that are not one); for the page it is one state, and nothing
      // is logged: the module never repeats the key, but there is nothing here a person could use either.
      return { status: "invalid" };
    }
    this.#sequence += 1;
    const pubSeq = etag ?? this.#sequence;
    this.#pictureMissed = false;
    this.#publication = publication;
    this.#pubSeq = etag;
    this.#attachments = new Map(attachments.map((attachment) => [attachment.attachmentId, attachment]));
    this.#people = mentions.map((person) => ({ accountId: person.accountId, name: person.name, verified: false, isMe: false }));
    return { status: "ready", publication, pubSeq };
  }

  /**
   * Starts polling every `pollSeconds` and calls `onChange` with every outcome that is not `unchanged` (and with the
   * first `unchanged` after an `unavailable`, which is how the page learns the server is back); returns the function
   * that stops it. A link that is gone is asked for again at the next poll (its first upload may not have arrived
   * yet); a key that does not open is not: the answer cannot change.
   */
  watch(onChange: (load: LinkLoad) => void): () => void {
    let stopped = false;
    let last: LinkLoad["status"] | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = (): void => {
      // After a 429 the wait doubles with each one in a row (up to 32 times the interval): the server said "enough".
      const wait = this.#pollMs * 2 ** Math.min(this.#limited, 5);
      timer = setTimeout(() => {
        void tick();
      }, wait);
    };
    const tick = async (): Promise<void> => {
      const load = await this.load();
      if (stopped) {
        return;
      }
      if (load.status !== "unchanged" || last === "unavailable") {
        onChange(load);
      }
      last = load.status;
      if (load.status !== "invalid") {
        schedule();
      }
    };
    schedule();
    return () => {
      stopped = true;
      if (timer !== undefined) {
        clearTimeout(timer);
      }
    };
  }

  /** The publication shown now, or a `NotFound` for the page asked for. */
  #open(pageId: PageId): OpenedPublication {
    const publication = this.#publication;
    if (publication === null || publication.pageId !== pageId) {
      throw notFound("the link has no such page");
    }
    return publication;
  }

  /** The page document the editor opens: the publication's shallow snapshot, as raw Loro bytes. */
  readonly docs: DocTransport = {
    open: (pageId: PageId) => {
      try {
        return Promise.resolve(engineSnapshot(this.#open(pageId).snapshot()));
      } catch (error) {
        return Promise.reject(error instanceof Error ? error : new Error("the page could not be opened"));
      }
    },
    import: () => readOnly("docs.import"),
    export: () => readOnly("docs.export"),
    close: () => Promise.resolve(),
  };

  /**
   * The sealed bytes of a chunk, from the server (`GET /v2/links/{id}/chunks/{hash}`) or from what an earlier read of this
   * page fetched: a picture used twice (a cover and a block) and the pictures of a page drawn again for a newer publication
   * are one request each, which also keeps the page under the server's 120 reads a minute. Bytes are kept up to
   * `CHUNK_CACHE_BYTES`, the oldest first out; a chunk that could not be had is asked for again next time.
   */
  #sealedChunk(hash: string, name: string): Promise<Uint8Array> {
    const held = this.#sealed.get(hash);
    if (held !== undefined) {
      return held;
    }
    const pending = this.#download(hash, name).then(
      (bytes) => {
        this.#sizes.set(hash, bytes.length);
        this.#kept += bytes.length;
        for (const [oldest, size] of this.#sizes) {
          if (this.#kept <= CHUNK_CACHE_BYTES || oldest === hash) {
            break;
          }
          this.#sizes.delete(oldest);
          this.#sealed.delete(oldest);
          this.#kept -= size;
        }
        return bytes;
      },
      (error: unknown) => {
        this.#sealed.delete(hash);
        throw error;
      },
    );
    this.#sealed.set(hash, pending);
    return pending;
  }

  async #download(hash: string, name: string): Promise<Uint8Array> {
    const id = this.#id();
    if (id === null) {
      throw new CoreError({ code: "Storage", message: `${name} is damaged` });
    }
    try {
      const response = await this.#get(`/v2/links/${id}/chunks/${hash}`);
      if (response.status === 404 || response.status === 410) {
        // A chunk the feed does not hold yet: the picture shows as unavailable, and the next poll draws the page again.
        this.#pictureMissed = true;
        throw notFound(`${name} is not available`);
      }
      if (!response.ok) {
        this.#pictureMissed = true;
        throw new CoreError({ code: "Storage", message: `${name} could not be loaded` });
      }
      return new Uint8Array(await response.arrayBuffer());
    } catch (error) {
      if (error instanceof CoreError) {
        throw error;
      }
      this.#pictureMissed = true;
      throw new CoreError({ code: "Storage", message: `${name} could not be loaded` });
    }
  }

  /** One chunk of an attachment, fetched from the server and decrypted by `kb-wasm`. */
  async #chunk(publication: OpenedPublication, attachment: PublishedAttachmentInfo, index: number): Promise<Uint8Array> {
    const hash = attachment.chunks[index];
    if (hash === undefined || !CHUNK_ID.test(hash)) {
      throw new CoreError({ code: "Storage", message: `${attachment.name} is damaged` });
    }
    const sealed = await this.#sealedChunk(hash, attachment.name);
    try {
      return publication.decryptChunk(attachment.attachmentId, index, index === attachment.chunks.length - 1, sealed);
    } catch {
      // The bytes are not what the publication lists at that place (the server swapped a chunk) or do not open: not shown,
      // and not kept for the next read either.
      this.#sealed.delete(hash);
      this.#kept -= this.#sizes.get(hash) ?? 0;
      this.#sizes.delete(hash);
      throw new CoreError({ code: "Crypto", message: `${attachment.name} is damaged` });
    }
  }

  /** The plaintext of `range` (the whole attachment without one): only the chunks it touches are fetched. */
  async #read(attachmentId: AttachmentId, range?: ByteRange): Promise<Uint8Array> {
    const attachment = this.#attachments.get(attachmentId);
    const publication = this.#publication;
    if (attachment === undefined || publication === null) {
      throw notFound("the page has no such attachment");
    }
    const { start, end } = range ?? { start: 0, end: attachment.size };
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start < 0 || start > end || end > attachment.size) {
      throw new CoreError({ code: "InvalidInput", message: "the byte range is outside the attachment", details: { field: "range" } });
    }
    const out = new Uint8Array(end - start);
    const last = attachment.chunks.length - 1;
    const first = Math.floor(start / attachment.chunkSize);
    const needed = end === 0 ? 0 : Math.floor((end - 1) / attachment.chunkSize);
    let delivered = 0;
    for (let index = first; index <= Math.min(needed, last); index += 1) {
      const plain = await this.#chunk(publication, attachment, index);
      const chunkStart = index * attachment.chunkSize;
      const from = Math.min(Math.max(start - chunkStart, 0), plain.length);
      const to = Math.min(Math.max(end - chunkStart, 0), plain.length);
      if (from < to) {
        out.set(plain.subarray(from, to), delivered);
        delivered += to - from;
      }
    }
    if (delivered !== end - start) {
      throw new CoreError({ code: "Crypto", message: `${attachment.name} is damaged` });
    }
    return out;
  }

  /** Reads of the page's attachments; the uploads and the file operations are refused. */
  readonly attachments: AttachmentTransport = {
    write: () => readOnly("attachments.write"),
    read: (attachmentId: AttachmentId, range?: ByteRange) => this.#read(attachmentId, range),
    importPath: () => readOnly("attachments.importPath"),
    saveAs: () => readOnly("attachments.saveAs"),
    open: () => readOnly("attachments.open"),
  };

  /** What the core would say of an attachment of the page (`AttachmentGet`): complete, referenced once, never changed. */
  #info(attachment: PublishedAttachmentInfo): AttachmentInfo {
    return {
      attachmentId: attachment.attachmentId,
      spaceId: VIEWER_SPACE_ID,
      name: attachment.name,
      mime: attachment.mime,
      size: attachment.size,
      chunkCount: attachment.chunks.length,
      createdAt: 0,
      references: 1,
      localChunks: attachment.chunks.length,
      availability: "Complete",
    };
  }

  /** The few requests the page view needs, answered from the publication; every other one is `Forbidden`. */
  dispatch(request: Request): Promise<ApiResponse> {
    switch (request.type) {
      case "AttachmentGet": {
        const attachment = this.#attachments.get(request.data.attachmentId);
        return attachment === undefined
          ? Promise.reject(notFound("the page has no such attachment"))
          : Promise.resolve({ type: "Attachment", data: this.#info(attachment) });
      }
      case "PeopleMentionable":
        return Promise.resolve({ type: "People", data: this.#people });
      default:
        return Promise.reject(
          new CoreError({
            code: "Forbidden",
            message: `${request.type}: a view link shows one page`,
            details: { reason: "role", required: "editor" },
          }),
        );
    }
  }

  subscribe(onEvent: (event: Event) => void): () => void {
    this.#listeners.add(onEvent);
    return () => {
      this.#listeners.delete(onEvent);
    };
  }
}
