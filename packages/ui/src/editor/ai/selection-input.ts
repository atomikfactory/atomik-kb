// What the writing assistant sends (ADR-0037, `docs/architecture/ai.md` section 8.3): a range of the
// page as Markdown (the renderer of core-client over the range's portable JSON, as the export uses),
// plain text when that fails, and the context: the page title with up to 4,000 characters before.

import { renderMarkdown } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";

import { MAX_CONTEXT_CHARS, MAX_TEXT_CHARS } from "./text";

/** How much of the page before the selection goes in the context. */
export const CONTEXT_BEFORE_CHARS = 4_000;

/** How the Markdown of a range resolves references. */
export interface Resolvers {
  /** The title of a referenced page, `null` when unknown. */
  titleOf: (pageId: string) => string | null;
  /** The name of an attachment, `null` when unknown. */
  nameOf: (attachmentId: string) => string | null;
}

/** The range `[from, to)` of the editor's document as Markdown; the plain text of it when rendering fails. */
export function markdownOfRange(editor: Editor, from: number, to: number, resolvers: Resolvers): string {
  const { doc } = editor.state;
  const start = Math.max(0, Math.min(from, doc.content.size));
  const end = Math.max(start, Math.min(to, doc.content.size));
  if (start === end) {
    return "";
  }
  try {
    const content = doc.slice(start, end).content.toJSON() as JsonValue;
    const markdown = renderMarkdown(
      { type: "doc", content: Array.isArray(content) ? content : [] },
      (pageId) => resolvers.titleOf(pageId) ?? undefined,
      { resolveAttachmentName: (attachmentId) => resolvers.nameOf(attachmentId) ?? undefined },
    );
    return markdown.trim();
  } catch {
    return doc.textBetween(start, end, "\n\n", " ").trim();
  }
}

/** The part of `text` the shell takes: the first `max` characters. */
export function clampText(text: string, max: number = MAX_TEXT_CHARS): { text: string; truncated: boolean } {
  return text.length <= max ? { text, truncated: false } : { text: text.slice(0, max), truncated: true };
}

/** The context of a request: the page title, then the text before `before` (its last 4,000 characters). */
export function contextBefore(editor: Editor, before: number, title: string, resolvers: Resolvers): string {
  const preceding = markdownOfRange(editor, 0, before, resolvers);
  const tail = preceding.length > CONTEXT_BEFORE_CHARS ? preceding.slice(-CONTEXT_BEFORE_CHARS) : preceding;
  const head = title.trim() === "" ? "" : `Page title: ${title.trim()}`;
  const context = head === "" ? tail : tail === "" ? head : `${head}\n\n${tail}`;
  return context.length > MAX_CONTEXT_CHARS ? context.slice(-MAX_CONTEXT_CHARS) : context;
}
