// The elements of whiteboard blocks (ADR-0016, amendment of 2026-10-10; ADR-0033), the
// TypeScript side of `kb_model::page::whiteboard`.
//
// A `whiteboard` node holds no elements. They live in the root map `whiteboards` of the page's
// Loro document, one plain map value per element under the flat key `<block id>/<element id>`
// (the block id in its canonical hyphenated form, 36 characters). The value is the Excalidraw
// element as JSON (`id`, `type`, `index`, `version`, `versionNonce`, `isDeleted`, ...). A flat
// key space in an always-existing root map means no container is ever created concurrently:
// two devices adding or editing different elements merge without conflict, and two devices
// editing the same element keep one value (last writer wins per element; the editor repairs a
// stale winner by `version`, see the ADR).

import { isContainer } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";

import type { JsonValue } from "./generated/serde_json/JsonValue";
import { invalid } from "./validate";

/** Root map of a page document that holds the whiteboard elements. */
export const WHITEBOARDS = "whiteboards";

/**
 * The origin of the commits that write elements. The page's undo manager excludes it (Loro's
 * `excludeOriginPrefixes`): page undo never rewinds a drawing, the drawing has its own history.
 */
export const WHITEBOARD_ORIGIN = "whiteboard";

/** Longest element id accepted in a key (as `kb-model`). */
const MAX_ELEMENT_ID = 256;

const KEY = /^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\/(.+)$/u;

/** One Excalidraw element as stored: a JSON object whose `id` is the key's element id. */
export type WhiteboardValue = { [key: string]: JsonValue };

/** An element of a whiteboard with its id. */
export interface WhiteboardElement {
  id: string;
  value: WhiteboardValue;
}

/** The key of an element in the `whiteboards` map. */
export function whiteboardKey(blockId: string, elementId: string): string {
  return `${blockId}/${elementId}`;
}

/** Splits a key into the block id and the element id; `undefined` when it is not one. */
export function splitWhiteboardKey(key: string): { blockId: string; elementId: string } | undefined {
  const match = KEY.exec(key);
  const elementId = match?.[2];
  if (match === null || elementId === undefined || elementId.length > MAX_ELEMENT_ID) {
    return undefined;
  }
  return { blockId: match[1] ?? "", elementId };
}

function isRecord(value: unknown): value is { [key: string]: unknown } {
  return typeof value === "object" && value !== null && !Array.isArray(value) && !(value instanceof Uint8Array);
}

/** A stored value as JSON: bigints become numbers, containers and binary are dropped. */
function jsonOf(value: unknown): JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : null;
  }
  if (typeof value === "bigint") {
    return Number(value);
  }
  if (Array.isArray(value)) {
    return value.map((item: unknown) => jsonOf(item));
  }
  if (isRecord(value) && !(value instanceof Uint8Array)) {
    const out: { [key: string]: JsonValue } = {};
    for (const key of Object.keys(value).sort()) {
      out[key] = jsonOf(value[key]);
    }
    return out;
  }
  return null;
}

/** The element when `value` is a map whose `id` is `elementId`. */
function wellFormed(elementId: string, value: unknown): WhiteboardElement | undefined {
  if (!isRecord(value) || value.id !== elementId) {
    return undefined;
  }
  const json = jsonOf(value);
  return isRecord(json) ? { id: elementId, value: json } : undefined;
}

/** Every plain entry of the `whiteboards` root map by key (a nested container is skipped). */
function entriesOf(doc: LoroDoc): [string, unknown][] {
  const out: [string, unknown][] = [];
  for (const [key, value] of doc.getMap(WHITEBOARDS).entries()) {
    if (!isContainer(value)) {
      out.push([key, value]);
    }
  }
  return out;
}

function indexOf(element: WhiteboardElement): string {
  const index = element.value.index;
  return typeof index === "string" ? index : "";
}

/** Z-order: the fractional `index` string, then the id (`sort_z` of `kb-model`). */
export function sortZ(elements: WhiteboardElement[]): WhiteboardElement[] {
  return elements.sort((a, b) => {
    const [x, y] = [indexOf(a), indexOf(b)];
    if (x !== y) {
      return x < y ? -1 : 1;
    }
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}

/**
 * The elements of the whiteboard `blockId` (tombstones included) in z-order. Entries that are not
 * a map with an `id` equal to the key's element id are left out. Mirrors
 * `PageDoc::whiteboard_elements`.
 */
export function readWhiteboard(doc: LoroDoc, blockId: string): WhiteboardElement[] {
  const out: WhiteboardElement[] = [];
  for (const [key, value] of entriesOf(doc)) {
    const split = splitWhiteboardKey(key);
    if (split?.blockId !== blockId) {
      continue;
    }
    const element = wellFormed(split.elementId, value);
    if (element !== undefined) {
      out.push(element);
    }
  }
  return sortZ(out);
}

/** The text of a live `text` element: `originalText` when it is a string, else `text`. */
export function whiteboardElementText(element: WhiteboardElement): string | undefined {
  if (element.value.isDeleted === true || element.value.type !== "text") {
    return undefined;
  }
  const { originalText, text } = element.value;
  return typeof originalText === "string" ? originalText : typeof text === "string" ? text : undefined;
}

/**
 * The text of a whiteboard as the index and the Markdown export read it: the live text elements
 * in z-order, whitespace collapsed to single spaces, blank ones left out (`whiteboard::texts`).
 */
export function whiteboardTexts(elements: readonly WhiteboardElement[]): string[] {
  const out: string[] = [];
  for (const element of elements) {
    const text = whiteboardElementText(element);
    const collapsed = text?.split(/\s+/u).filter((word) => word !== "").join(" ");
    if (collapsed !== undefined && collapsed !== "") {
      out.push(collapsed);
    }
  }
  return out;
}

/** {@link whiteboardTexts} for the portable form: the elements of one block as `{ id: element }`. */
export function whiteboardTextsOfPortable(elements: unknown): string[] {
  if (!isRecord(elements)) {
    return [];
  }
  const list: WhiteboardElement[] = [];
  for (const [id, value] of Object.entries(elements)) {
    if (isRecord(value)) {
      list.push({ id, value: value as WhiteboardValue });
    }
  }
  return whiteboardTexts(sortZ(list));
}

/**
 * Stores elements of the whiteboard `blockId`, each under its key, and commits once (origin
 * {@link WHITEBOARD_ORIGIN}) when anything was written. An element without a string `id` of 1 to 256 characters is a programming error.
 * Returns how many were written.
 */
export function writeWhiteboardElements(
  doc: LoroDoc,
  blockId: string,
  elements: readonly WhiteboardValue[],
): number {
  const map = doc.getMap(WHITEBOARDS);
  for (const element of elements) {
    const id = element.id;
    if (typeof id !== "string" || id === "" || id.length > MAX_ELEMENT_ID) {
      throw invalid("content", "a whiteboard element has no usable id");
    }
    map.set(whiteboardKey(blockId, id), element);
  }
  if (elements.length > 0) {
    doc.commit({ origin: WHITEBOARD_ORIGIN });
  }
  return elements.length;
}

/** Keys of the elements whose block is not in `live` (`orphan_whiteboard_keys`). */
export function orphanWhiteboardKeys(doc: LoroDoc, live: ReadonlySet<string>): string[] {
  return entriesOf(doc)
    .map(([key]) => key)
    .filter((key) => {
      const split = splitWhiteboardKey(key);
      return split === undefined || !live.has(split.blockId);
    });
}

/**
 * The elements of the blocks in `live`, as the portable `{ block id: { element id: element } }`
 * (`undefined` when there are none). Orphans are not part of it.
 */
export function whiteboardsPortable(
  doc: LoroDoc,
  live: ReadonlySet<string>,
): { [blockId: string]: { [elementId: string]: JsonValue } } | undefined {
  const out: { [blockId: string]: { [elementId: string]: JsonValue } } = {};
  let any = false;
  for (const [key, value] of entriesOf(doc)) {
    const split = splitWhiteboardKey(key);
    if (split === undefined || !live.has(split.blockId)) {
      continue;
    }
    const element = wellFormed(split.elementId, value);
    if (element === undefined) {
      continue;
    }
    (out[split.blockId] ??= {})[element.id] = element.value;
    any = true;
  }
  return any ? out : undefined;
}

/**
 * Validates the portable `whiteboards` value and writes it into the document (`import_whiteboards`):
 * `{ block id: { element id: element with that id } }`, anything else is an `InvalidInput`.
 * Nothing is written unless the whole value is valid.
 */
export function importWhiteboards(doc: LoroDoc, whiteboards: JsonValue): void {
  if (!isRecord(whiteboards)) {
    throw invalid("content", "whiteboards must be an object");
  }
  const writes: [string, WhiteboardValue][] = [];
  for (const [block, elements] of Object.entries(whiteboards)) {
    if (!isRecord(elements)) {
      throw invalid("content", "a whiteboard must be an object");
    }
    if (splitWhiteboardKey(whiteboardKey(block, "x"))?.blockId !== block) {
      throw invalid("content", "a whiteboard key is not a block id");
    }
    for (const [id, element] of Object.entries(elements)) {
      const key = whiteboardKey(block, id);
      const split = splitWhiteboardKey(key);
      if (split?.blockId !== block || !isRecord(element) || element.id !== id) {
        throw invalid("content", "a whiteboard element is malformed");
      }
      writes.push([key, element]);
    }
  }
  const map = doc.getMap(WHITEBOARDS);
  for (const [key, element] of writes) {
    map.set(key, element);
  }
}
