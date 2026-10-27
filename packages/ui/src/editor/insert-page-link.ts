// Putting a `pageLink` card into the page where the caret is: the `[[` picker in card mode,
// the "Link to page" and "New sub-page" commands of the `/` menu, and pasting a link to a block.

import type { Editor } from "@tiptap/core";
import { Selection } from "@tiptap/pm/state";

import { placementFor } from "./attachments";
import { PAGE_LINK_NODE } from "./page-link";
import type { PageLinkAttrs } from "./page-link";

/**
 * Inserts a card linking to `attrs` near `at` (the selection by default) and puts the caret
 * after it. It goes where a block fits best, like an attachment block (`placementFor`): in
 * place of the empty paragraph `at` is in, after the textblock otherwise (the text typed in
 * it is never split), further out where the parent does not take a block. A card that ends
 * its container gets an empty paragraph after it, so that there is always a block to type
 * into below. Returns false when the schema has no card.
 */
export function insertPageLinkCard(editor: Editor, attrs: PageLinkAttrs, at?: number): boolean {
  const { state } = editor;
  const cardType = state.schema.nodes[PAGE_LINK_NODE];
  const paragraph = state.schema.nodes.paragraph;
  if (cardType === undefined || paragraph === undefined) {
    return false;
  }
  const card = cardType.create({ pageId: attrs.pageId, blockId: attrs.blockId ?? null });
  const placement = placementFor(state.doc, at ?? state.selection.from, cardType);
  const empty = placement.replace === null ? null : state.doc.nodeAt(placement.pos);
  const to = empty === null ? placement.pos : placement.pos + empty.nodeSize;
  const tr = state.tr.replaceWith(placement.pos, to, card);
  const end = placement.pos + card.nodeSize;
  const $end = tr.doc.resolve(end);
  if ($end.nodeAfter === null && $end.parent.canReplaceWith($end.index(), $end.index(), paragraph)) {
    tr.insert(end, paragraph.create());
  }
  tr.setSelection(Selection.near(tr.doc.resolve(end), 1)).scrollIntoView();
  editor.view.dispatch(tr);
  return true;
}
