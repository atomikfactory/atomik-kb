// Where and how a recognised phrase goes into the page (ADR-0053, `docs/architecture/dictation.md` section 6, "Insertion
// rules"). Each phrase is one transaction, as if typed: at the end of the selection (the selected text is never deleted or
// replaced; the selection is whatever the editor holds, so a blurred editor, because the person clicked the bar or
// another window, takes it at its last cursor), joined to what is before it by `joinPhrase` (so the spaces, the
// punctuation and the capital of a phrase that continues a sentence come out as the person would type them), with the
// marks of the cursor, and the selection afterwards is a
// cursor after the new text. It is its own undo step, and the next change of the page starts another one (`asOwnUndoStep`
// of the Loro binding), so Ctrl+Z takes away the last phrase and only it. Nothing is written to a page that is read-only.

import { joinPhrase } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import type { Transaction } from "@tiptap/pm/state";

import type { LoroBindingStorage } from "../extensions";

/** What stands for an inline atom (a mention, a date, an inline equation) in the text before the cursor: not a space. */
const ATOM = "￼";

/** The text of a leaf inside a text block for `joinPhrase`: a line break reads as white space, anything else as a character. */
function leafText(node: PmNode): string {
  return node.type.name === "hardBreak" ? "\n" : ATOM;
}

/**
 * The text before `offset` in the text block `parent` (`""` at its start): the `before` of `joinPhrase`. An inline atom counts
 * as a character that is not a space, so a space follows a mention, and it does not end a sentence, so what follows keeps its capital.
 */
export function textBefore(parent: PmNode, offset: number): string {
  return parent.textBetween(0, offset, undefined, leafText);
}

/** The Loro binding's storage of `editor`, `null` where there is none (an editor without a page document). */
function bindingOf(editor: Editor): LoroBindingStorage | null {
  const storage: unknown = (editor.storage as unknown as Record<string, unknown>).loroBinding;
  if (typeof storage !== "object" || storage === null || !("asOwnUndoStep" in storage)) {
    return null;
  }
  return typeof storage.asOwnUndoStep === "function" ? (storage as LoroBindingStorage) : null;
}

/**
 * The transaction that puts `phrase` at the end of the selection, `null` when there is nothing to put or no place for it. At
 * a text block it is the phrase joined to the text before it; with the selection on a block (a picture, a divider) or on
 * a whole page it is a new paragraph after it.
 */
function phraseTransaction(editor: Editor, phrase: string): Transaction | null {
  const { state } = editor;
  const { $to } = state.selection;
  if ($to.parent.inlineContent) {
    const text = joinPhrase(textBefore($to.parent, $to.parentOffset), phrase);
    if (text === null) {
      return null;
    }
    const tr = state.tr.insertText(text, $to.pos);
    return tr.setSelection(TextSelection.create(tr.doc, $to.pos + text.length));
  }
  // A selection that is not in a text block: the block that is selected gets a paragraph after it, where one can go.
  const text = joinPhrase("", phrase);
  const paragraph = state.schema.nodes.paragraph;
  if (text === null || paragraph === undefined || !$to.parent.canReplaceWith($to.index(), $to.index(), paragraph)) {
    return null;
  }
  const tr = state.tr.insert($to.pos, paragraph.create(null, state.schema.text(text)));
  // The paragraph's content starts one after its own start: the cursor goes to the end of the text.
  return tr.setSelection(TextSelection.create(tr.doc, $to.pos + 1 + text.length));
}

/**
 * Inserts a recognised phrase at the end of the editor's selection, as one undo step of its own. Answers whether
 * anything was inserted: `false` for a destroyed or read-only editor, a phrase of nothing but white space, and a selection
 * where text cannot go.
 */
export function insertPhrase(editor: Editor, phrase: string): boolean {
  if (editor.isDestroyed || !editor.isEditable) {
    return false;
  }
  const tr = phraseTransaction(editor, phrase);
  if (tr === null) {
    return false;
  }
  const dispatch = (): void => {
    editor.view.dispatch(tr.scrollIntoView());
  };
  const binding = bindingOf(editor);
  if (binding === null) {
    dispatch();
  } else {
    binding.asOwnUndoStep(dispatch);
  }
  return true;
}
