// Helpers for editor and binding tests: page documents built from portable JSON (with
// core-client's `portableToLoro`, the TypeScript twin of the core's writer) and plain
// ProseMirror views bound to a LoroDoc, without Tiptap or React.

import { portableToLoro } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import type { Node as PmNode, Schema } from "@tiptap/pm/model";
import { EditorState } from "@tiptap/pm/state";
import { EditorView } from "@tiptap/pm/view";
import { LoroDoc, UndoManager } from "loro-crdt";

import pageFixture from "../../../../fixtures/doc/page-v1.portable.json";
import { LoroEphemeralCursorPlugin, LoroSyncPlugin, LoroUndoPlugin } from "../editor/loro-binding";
import type { CursorEphemeralStore, LoroDocType } from "../editor/loro-binding";

/** `fixtures/doc/page-v1.portable.json`: every node and mark of block schema v1 once. */
export const PAGE_FIXTURE = pageFixture as unknown as JsonValue;

/** A configured page document holding `json` (portable JSON), as the core would write it. */
export function loroDocFrom(json: JsonValue): LoroDoc {
  const doc = new LoroDoc();
  portableToLoro(doc, json);
  return doc;
}

/** The snapshot bytes the core sends when it opens a page holding `json`. */
export function snapshotOf(json: JsonValue): Uint8Array {
  return loroDocFrom(json).export({ mode: "snapshot" });
}

/**
 * A block id in the canonical UUID form the core indexes (ids that are not UUIDs are
 * not indexed, so search and backlinks would not see their blocks): `pid(3)` is
 * `00000000-0000-4000-8000-000000000003`.
 */
export function pid(index: number): string {
  return `00000000-0000-4000-8000-${String(index).padStart(12, "0")}`;
}

/** A document of paragraphs with the given texts and ids `pid(0)`, `pid(1)`, ... */
export function paragraphs(...texts: string[]): JsonValue {
  return {
    type: "doc",
    content: texts.map((text, index) => ({
      type: "paragraph",
      attrs: { id: pid(index) },
      ...(text === "" ? {} : { content: [{ type: "text", text }] }),
    })),
  };
}

/** Waits for timers scheduled now (the binding initialises its view in a `setTimeout(0)`). */
export function tick(ms = 0): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

export interface BoundView {
  view: EditorView;
  undoManager: UndoManager;
  destroy: () => void;
}

/**
 * A plain ProseMirror view bound to `doc` through the vendored sync and undo plugins.
 * `content` is the view's initial document (ProseMirror's default empty one when left
 * out); the binding replaces it with the Loro document once initialised (`await tick()`). `cursors` adds the cursor
 * plugin (ADR-0050) on that store, after the sync plugin, and `attach` puts the view's element in the page (so that it can have
 * focus).
 */
export function bindView(
  schema: Schema,
  doc: LoroDoc,
  options: { content?: PmNode; readOnly?: boolean; cursors?: CursorEphemeralStore; attach?: boolean } = {},
): BoundView {
  const undoManager = new UndoManager(doc, {});
  const state = EditorState.create({
    schema,
    doc: options.content,
    plugins: [
      LoroSyncPlugin({ doc: doc as LoroDocType, readOnly: options.readOnly }),
      LoroUndoPlugin({ doc, undoManager }),
      ...(options.cursors === undefined ? [] : [LoroEphemeralCursorPlugin(options.cursors)]),
    ],
  });
  const host = document.createElement("div");
  if (options.attach === true) {
    document.body.append(host);
  }
  const view = new EditorView(host, { state });
  return {
    view,
    undoManager,
    destroy: () => {
      view.destroy();
      undoManager.free();
      host.remove();
    },
  };
}

/** Everything `from` has that `to` lacks, imported into `to`. */
export function sync(from: LoroDoc, to: LoroDoc): void {
  const update = from.export({ mode: "update", from: to.oplogVersion() });
  to.import(update);
}

/**
 * `page-v1.portable.json` gives its `hardBreak` an `id`, but block schema v1 has none on
 * `hardBreak` (the table in milestone-03.md), so ProseMirror does not hold it.
 */
export function withoutHardBreakIds(json: JsonValue): JsonValue {
  if (Array.isArray(json)) {
    return json.map(withoutHardBreakIds);
  }
  if (typeof json !== "object" || json === null) {
    return json;
  }
  const result: { [key: string]: JsonValue } = {};
  for (const [key, value] of Object.entries(json)) {
    result[key] = withoutHardBreakIds(value);
  }
  if (result.type === "hardBreak") {
    delete result.attrs;
  }
  return result;
}
