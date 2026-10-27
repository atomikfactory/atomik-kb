// Putting a transcription in the page (ADR-0037, `docs/architecture/ai.md` section 8.6): after a
// recording, the audio block (`view: "preview"`) and a toggle titled "Transcript" holding one paragraph
// per segment, in ONE transaction at the remembered place; then, in a second transaction, the meeting
// summary under the toggle. "Transcribe" on an existing file puts the toggle (and later the summary)
// right after its block. Places are found by block id, so the page may change meanwhile.

import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { Fragment } from "@tiptap/pm/model";
import type { Node as PmNode } from "@tiptap/pm/model";

import { t } from "../../i18n";
import { FILE_NODE } from "../attachment-nodes";
import { nodesFromPortable } from "../markdown-paste";
import { BLOCK_ID_ATTRIBUTE } from "../schema";
import { nodesOfMarkdown } from "./apply";
import { transcriptBlocks } from "./text";
import type { TranscriptSegment } from "./text";

/** The node with block id `id` and where it starts. */
function findById(editor: Editor, id: string): { node: PmNode; pos: number } | null {
  let found: { node: PmNode; pos: number } | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (found !== null) {
      return false;
    }
    if (node.attrs[BLOCK_ID_ATTRIBUTE] === id) {
      found = { node, pos };
      return false;
    }
    return true;
  });
  return found;
}

/**
 * The position at or after `pos` where `nodes` fit: `pos` itself between sibling blocks, else after the
 * nearest ancestor that takes them, else the end of the page.
 */
function slotAt(editor: Editor, pos: number, nodes: readonly PmNode[]): number {
  const { doc } = editor.state;
  const fragment = Fragment.from(nodes);
  const $pos = doc.resolve(pos);
  if ($pos.parent.canReplace($pos.index(), $pos.index(), fragment)) {
    return pos;
  }
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    const $at = doc.resolve($pos.after(depth));
    if ($at.parent.canReplace($at.index(), $at.index(), fragment)) {
      return $at.pos;
    }
  }
  return doc.content.size;
}

/** The "Transcript" toggle holding the transcript, as a portable node (its id is given, to find it again). */
function transcriptToggle(segments: readonly TranscriptSegment[], id: string): JsonValue {
  const paragraphs = transcriptBlocks(segments);
  return {
    type: "details",
    attrs: { id },
    content: [
      { type: "detailsSummary", content: [{ type: "text", text: t("transcribe.toggleTitle") }] },
      { type: "detailsContent", content: paragraphs },
    ],
  };
}

/** Whether the transcript has words in it. */
export function hasSpeech(segments: readonly TranscriptSegment[]): boolean {
  return segments.some((segment) => segment.text.trim() !== "");
}

/** What a recording adds to the page. */
export interface RecordingInsert {
  /** The top-level block the recording was started at: the audio goes after it (in place of it when empty). */
  anchorId: string | null;
  attachmentId: string;
  segments: readonly TranscriptSegment[];
}

/** The ids of what was inserted, to find it again. */
export interface Inserted {
  audioId: string;
  /** `null` when there was no speech to transcribe. */
  toggleId: string | null;
}

/**
 * The audio block of the recording, and the transcript toggle when there are words, in one transaction.
 * `null` when the page is gone or read-only.
 */
export function insertRecording(editor: Editor, spec: RecordingInsert): Inserted | null {
  const fileType = editor.schema.nodes[FILE_NODE];
  if (fileType === undefined || editor.isDestroyed || !editor.isEditable) {
    return null;
  }
  const audioId = crypto.randomUUID();
  const toggleId = hasSpeech(spec.segments) ? crypto.randomUUID() : null;
  const nodes: PmNode[] = [fileType.create({ [BLOCK_ID_ATTRIBUTE]: audioId, attachmentId: spec.attachmentId, view: "preview" })];
  if (toggleId !== null) {
    nodes.push(...nodesFromPortable(editor.schema, [transcriptToggle(spec.segments, toggleId)]));
  }
  const anchor = spec.anchorId === null ? null : findById(editor, spec.anchorId);
  const tr = editor.state.tr;
  try {
    if (anchor !== null && anchor.node.type.name === "paragraph" && anchor.node.content.size === 0) {
      tr.replaceWith(anchor.pos, anchor.pos + anchor.node.nodeSize, nodes); // the empty line the recording started on
    } else {
      const after = anchor === null ? editor.state.doc.content.size : anchor.pos + anchor.node.nodeSize;
      tr.insert(slotAt(editor, after, nodes), nodes);
    }
  } catch {
    return null;
  }
  editor.view.dispatch(tr.scrollIntoView());
  return { audioId, toggleId };
}

/** A transcript toggle right after the block `blockId`, in one transaction ("Transcribe" on a file block). */
export function insertTranscriptAfter(
  editor: Editor,
  blockId: string,
  segments: readonly TranscriptSegment[],
): { toggleId: string } | null {
  if (editor.isDestroyed || !editor.isEditable || !hasSpeech(segments)) {
    return null;
  }
  const toggleId = crypto.randomUUID();
  const nodes = nodesFromPortable(editor.schema, [transcriptToggle(segments, toggleId)]);
  const anchor = findById(editor, blockId);
  if (anchor === null || nodes.length === 0) {
    return null;
  }
  const tr = editor.state.tr;
  try {
    tr.insert(slotAt(editor, anchor.pos + anchor.node.nodeSize, nodes), nodes);
  } catch {
    return null;
  }
  editor.view.dispatch(tr.scrollIntoView());
  return { toggleId };
}

/** The summary's Markdown as blocks right after the toggle `toggleId`, in one transaction. `false` when it went. */
export function insertSummary(editor: Editor, toggleId: string, markdown: string): boolean {
  if (editor.isDestroyed || !editor.isEditable) {
    return false;
  }
  const anchor = findById(editor, toggleId);
  const nodes = nodesOfMarkdown(editor, markdown);
  if (anchor === null || nodes.length === 0) {
    return false;
  }
  const tr = editor.state.tr;
  try {
    tr.insert(slotAt(editor, anchor.pos + anchor.node.nodeSize, nodes), nodes);
  } catch {
    return false;
  }
  editor.view.dispatch(tr.scrollIntoView());
  return true;
}
