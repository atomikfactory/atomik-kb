// The surface of `crates/kb-wasm` as the viewer sees it (M13 slice 4, ADR-0051): the exports `wasm-bindgen` makes
// of its `#[wasm_bindgen]` items, written down by hand so that the transport can be typed, tested against a fake and
// built before the module itself is. `crates/kb-wasm/src/lib.rs` documents each member; the two have to agree (the
// glue `wasm-bindgen` makes, `src/wasm/`, is typed from the Rust and fails the typecheck where they do not).
//
// Errors are JavaScript `Error`s whose message is `"<code>: <text>"`; `wasmErrorCode` reads the code.

/** The code in front of the message of an error `kb-wasm` raised, or `undefined` for any other error. */
export type WasmErrorCode = "bad_key" | "authentication" | "malformed" | "unknown_attachment";

/** One attachment of a publication, as `OpenedPublication.attachmentsJson()` lists it. */
export interface PublishedAttachmentInfo {
  /** The attachment (UUID text). */
  attachmentId: string;
  /** Its name. */
  name: string;
  /** Its MIME type. */
  mime: string;
  /** Plaintext bytes. */
  size: number;
  /** BLAKE3 of the plaintext (64 hex characters). */
  hash: string;
  /** Plaintext bytes per chunk. */
  chunkSize: number;
  /** The sealed chunks' ids, in order (64 hex characters each): `GET /v2/links/{id}/chunks/{hash}`. */
  chunks: string[];
}

/** One person a mention names, as `OpenedPublication.mentionsJson()` lists them. */
export interface PublishedPersonInfo {
  /** The account (UUID text). */
  accountId: string;
  /** Their name. */
  name: string;
}

/** An opened publication (`kb-wasm`'s `OpenedPublication`): the attachment keys stay inside the module. */
export interface OpenedPublication {
  /** The page's id (UUID text). */
  readonly pageId: string;
  /** The page's title. */
  readonly title: string;
  /** The page's icon, when it has one. */
  readonly icon: string | undefined;
  /** The page's cover value (`attachment:<id>`, `color:<name>`, `gradient:<name>`), when it has one. */
  readonly cover: string | undefined;
  /** The cover's vertical position in percent. */
  readonly coverY: number;
  /** A `kb-doc` snapshot blob without history: the 10-byte `KBD1` header, then the engine's shallow snapshot. */
  snapshot(): Uint8Array;
  /** The attachments the page uses, as JSON text (`PublishedAttachmentInfo[]`). */
  attachmentsJson(): string;
  /** The people the page mentions, as JSON text (`PublishedPersonInfo[]`). */
  mentionsJson(): string;
  /** Decrypts one sealed chunk of an attachment (`index` its position, `isFinal` whether it is the last). */
  decryptChunk(attachmentId: string, index: number, isFinal: boolean, sealed: Uint8Array): Uint8Array;
  /** Frees the module's copy (wasm-bindgen's `free`). */
  free?(): void;
}

/** The module (`kb-wasm`'s functions). */
export interface KbWasm {
  /** The UUID text of the link id the fragment's key derives (`GET /v2/links/{id}`). */
  linkId(fragment: string): string;
  /** Opens a publication with the key of `fragment`. */
  openPublication(fragment: string, bytes: Uint8Array): OpenedPublication;
}

/** The code at the front of `error`'s message, when `kb-wasm` raised it. */
export function wasmErrorCode(error: unknown): WasmErrorCode | undefined {
  if (!(error instanceof Error)) {
    return undefined;
  }
  const code = /^(bad_key|authentication|malformed|unknown_attachment): /u.exec(error.message)?.[1];
  return code as WasmErrorCode | undefined;
}
