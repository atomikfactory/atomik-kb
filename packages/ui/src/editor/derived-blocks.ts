// The derived blocks (ADR-0016, amendment of 2026-10-06): `tableOfContents` and `breadcrumb`, and,
// since the amendment of 2026-10-07, `pageList` (the list view). All are leaf blocks (group `block`,
// no content) whose content is derived: the table of contents lists the page's headings, the
// breadcrumb shows where the page sits in the tree, the list view shows a set of pages. The document
// stores the block and nothing of what it shows (the first two hold the attribute `id` only; the
// list view also holds the configuration of its view), and the views never write to it on their own
// (they read the editor's document and the app's page tree; the list view's toolbar writes its own
// attributes when the person changes them), so there is no list to sync, no conflict to merge and
// nothing to export: Markdown renders them as the empty string.
//
// What they share lives here: their names, the check the block menu uses, and the command that
// inserts a leaf block where the caret is.

import { canInsertNode, isNodeSelection } from "@tiptap/core";
import type { CommandProps } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";

export const TABLE_OF_CONTENTS_NODE = "tableOfContents";
export const BREADCRUMB_NODE = "breadcrumb";
export const PAGE_LIST_NODE = "pageList";

/** The derived blocks, in the order of the `/` menu. */
export const DERIVED_NODES: readonly string[] = [TABLE_OF_CONTENTS_NODE, BREADCRUMB_NODE, PAGE_LIST_NODE];

/** True for a block whose content is derived (nothing in it is the document's). */
export function isDerivedBlock(node: PmNode): boolean {
  return DERIVED_NODES.includes(node.type.name);
}

/**
 * Inserts a block of the leaf type `name` where the selection is (in place of an empty
 * paragraph, after a selected block, or splitting a paragraph), and leaves the caret in the
 * block after it, adding an empty paragraph when the leaf ends its container so that there is
 * always somewhere to type below it. Tiptap's `setHorizontalRule` does the same. `attrs` are
 * set on the new block (the others keep their defaults).
 */
export function insertLeafBlock(
  name: string,
  { chain, state }: Pick<CommandProps, "chain" | "state">,
  attrs?: Record<string, unknown>,
): boolean {
  const type = state.schema.nodes[name];
  if (type === undefined || !canInsertNode(state, type)) {
    return false;
  }
  const { selection } = state;
  const inserting = chain();
  if (isNodeSelection(selection)) {
    inserting.insertContentAt(selection.$to.pos, { type: name, attrs });
  } else {
    inserting.insertContent({ type: name, attrs });
  }
  return inserting
    .command(({ state: current, tr, dispatch }) => {
      if (dispatch) {
        const { $to } = tr.selection;
        const after = $to.nodeAfter;
        if (after === null) {
          const filler = current.schema.nodes.paragraph ?? $to.parent.type.contentMatch.defaultType;
          const node = filler?.create();
          if (node !== undefined) {
            const end = $to.end();
            tr.insert(end, node);
            tr.setSelection(TextSelection.create(tr.doc, end + 1));
          }
        } else if (after.isTextblock) {
          tr.setSelection(TextSelection.create(tr.doc, $to.pos + 1));
        } else if (after.isBlock) {
          tr.setSelection(NodeSelection.create(tr.doc, $to.pos));
        } else {
          tr.setSelection(TextSelection.create(tr.doc, $to.pos));
        }
        tr.scrollIntoView();
      }
      return true;
    })
    .run();
}
