// Test-only access to the shared document fixtures in `fixtures/doc/` (Node `fs`; this
// directory is excluded from the build). The fixtures are written by scripts only:
// `page-v1.portable.json` by hand with the spec, `page-v1.snapshot.bin`, `page-v1.md` and
// `page-v1-unknown.*` by kb-model (`cargo run -p kb-model --example make_golden`), and
// `page-v1.from-js.snapshot.bin` by this package (`pnpm --filter @atomik-kb/core-client fixtures`).

import { existsSync, readFileSync, writeFileSync } from "node:fs";

import { LoroDoc } from "loro-crdt";

import type { JsonValue } from "../generated/serde_json/JsonValue";
import { portableToLoro } from "../page-doc";

/** Peer id of the document behind `page-v1.from-js.snapshot.bin` ("JS" in ASCII, then 1). */
export const FIXTURE_PEER = 0x4a53_0001;

/** The canonical sample page in portable JSON. */
export const SAMPLE_JSON = "page-v1.portable.json";
/** Loro snapshot of the sample written by the Rust core (agent P's golden). */
export const RUST_SNAPSHOT = "page-v1.snapshot.bin";
/** Markdown of the sample written by the Rust core (the shared golden). */
export const MARKDOWN_GOLDEN = "page-v1.md";
/** Loro snapshot of the sample written by this package. */
export const JS_SNAPSHOT = "page-v1.from-js.snapshot.bin";
/** A page with node and mark names outside schema v1, in portable JSON (kb-model's passthrough fixture). */
export const UNKNOWN_JSON = "page-v1-unknown.portable.json";
/** The same page as a Loro snapshot written by the Rust core. */
export const UNKNOWN_SNAPSHOT = "page-v1-unknown.snapshot.bin";

const DOC_FIXTURES = new URL("../../../../fixtures/doc/", import.meta.url);

function fixtureUrl(name: string): URL {
  return new URL(name, DOC_FIXTURES);
}

export function fixtureExists(name: string): boolean {
  return existsSync(fixtureUrl(name));
}

/**
 * Whether a fixture written by another runtime exists, and the suffix to put on the title
 * of an `it.skipIf` test that needs it, saying who writes it.
 */
export function fixtureStatus(name: string, writer: string): { missing: boolean; note: string } {
  const missing = !fixtureExists(name);
  return { missing, note: missing ? ` [skipped: fixtures/doc/${name} is missing; ${writer} writes it]` : "" };
}

/** A text fixture with line endings normalised to `\n`. */
export function readFixtureText(name: string): string {
  return readFileSync(fixtureUrl(name), "utf8").replace(/\r\n/gu, "\n");
}

export function readFixtureBytes(name: string): Uint8Array {
  return new Uint8Array(readFileSync(fixtureUrl(name)));
}

export function writeFixtureBytes(name: string, bytes: Uint8Array): void {
  writeFileSync(fixtureUrl(name), bytes);
}

/** `page-v1.portable.json`, parsed. */
export function samplePage(): JsonValue {
  return readFixtureJson(SAMPLE_JSON);
}

export function readFixtureJson(name: string): JsonValue {
  return JSON.parse(readFixtureText(name)) as JsonValue;
}

/** The sample built exactly as the fixture script builds it: a fresh document with the fixed peer. */
export function buildSampleDoc(): LoroDoc {
  const doc = new LoroDoc();
  doc.setPeerId(FIXTURE_PEER);
  portableToLoro(doc, samplePage());
  return doc;
}

/**
 * The raw Loro bytes of a fixture snapshot: blobs persisted by the Rust side carry the
 * 10-byte `KBD1` envelope (`kb_doc::HEADER_LEN`), which the editor never sees.
 */
export function loroPayload(bytes: Uint8Array): Uint8Array {
  const magic = [0x4b, 0x42, 0x44, 0x31];
  return magic.every((byte, index) => bytes[index] === byte) ? bytes.subarray(10) : bytes;
}
