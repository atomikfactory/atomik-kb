// What the app asks the editor to insert (`{"insert": ..}`): text at the caret, or appended as new paragraphs when the editor
// has no caret; an attachment as an image or file block after the block the caret is in (at the end without a caret).

import type { Editor } from "@tiptap/core";

import type { InsertRequest } from "./host";

function paragraph(text: string) {
  return text === "" ? { type: "paragraph" } : { type: "paragraph", content: [{ type: "text", text }] };
}

/** Puts `request` into the page. Returns false when there was nothing to insert or the editor is not editable. */
export function insertInto(editor: Editor, request: InsertRequest): boolean {
  if (!editor.isEditable) {
    return false;
  }
  const end = editor.state.doc.content.size;
  const caret = editor.isFocused;
  if ("text" in request) {
    const lines = request.text.replace(/\r\n?/g, "\n").split("\n");
    if (lines.every((line) => line === "")) {
      return false;
    }
    if (caret && lines.length === 1) {
      return editor.chain().focus().insertContent({ type: "text", text: request.text }).run();
    }
    const blocks = lines.map(paragraph);
    return caret
      ? editor.chain().focus().insertContent(blocks).run()
      : editor.chain().insertContentAt(end, blocks).run();
  }
  const block = {
    type: request.mime.startsWith("image/") ? "image" : "file",
    attrs: { attachmentId: request.attachmentId },
  };
  const { $to } = editor.state.selection;
  const at = caret && $to.depth > 0 ? $to.after(1) : end;
  return editor.chain().insertContentAt(at, block).run();
}
