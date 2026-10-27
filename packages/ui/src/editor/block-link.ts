// The app's internal link text for a page and for a block of a page, and what pasting one does.
//
//   atomik-kb://page/<pageId>#<blockId>    a block of a page
//   atomik-kb://page/<pageId>              a page
//
// Both ids are the canonical lowercase UUIDs the page document stores. This is text the app
// writes ("Copy link to block" in the block menu, "Copy link to page" in the page menu) and reads
// (paste). The desktop app also registers the scheme with the operating system (ADR-0040): the
// shell accepts only these two shapes and hands them to the store (`store-deep-links.ts`), which
// opens the page in the active tab and scrolls to the block, so the text works outside the editor
// there; in the web build it still does nothing outside the editor. Pasting
// exactly one such link makes a reference: an inline `pageRef` chip, or a `pageLink` card when
// the paste lands on an empty paragraph, and a click on either opens the page and scrolls to
// the block (`navigateTo`; Ctrl/Cmd + click and a middle click do it in a background tab, with the
// block kept, ADR-0034).

import { Extension } from "@tiptap/core";
import { Plugin, PluginKey } from "@tiptap/pm/state";

import { placementFor } from "./attachments";
import { insertPageLinkCard } from "./insert-page-link";

/** What every link of the app starts with. */
export const BLOCK_LINK_PREFIX = "atomik-kb://page/";

const UUID = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
const BLOCK_LINK = new RegExp(`^${BLOCK_LINK_PREFIX}(${UUID})(?:#(${UUID}))?$`, "i");

/** A page, and optionally one of its blocks. */
export interface BlockLink {
  pageId: string;
  /** `null` for a link to the page itself. */
  blockId: string | null;
}

/** The link text for a page, or for one of its blocks when `blockId` is given. */
export function blockLinkText(pageId: string, blockId?: string | null): string {
  return typeof blockId === "string" && blockId !== ""
    ? `${BLOCK_LINK_PREFIX}${pageId}#${blockId}`
    : `${BLOCK_LINK_PREFIX}${pageId}`;
}

/**
 * The page and block a link text names, or `null` when `text` is anything else. Surrounding
 * whitespace is ignored; the text must be exactly one link, so a link in a sentence, two
 * links, or a link with more after the block id are all something else.
 */
export function parseBlockLink(text: string): BlockLink | null {
  const match = BLOCK_LINK.exec(text.trim());
  const pageId = match?.[1];
  if (match === null || pageId === undefined) {
    return null;
  }
  return { pageId: pageId.toLowerCase(), blockId: match[2]?.toLowerCase() ?? null };
}

/**
 * Pasting exactly one link of the app (plain text, trimmed) inserts a reference instead of
 * the text: a `pageLink` card where the paste lands on an empty paragraph that a block can
 * take the place of, an inline `pageRef` chip anywhere else text may go. Anything else, and
 * every paste into a place a chip cannot go (a code block), is left to the editor. It runs
 * before the other paste handlers (it only ever takes a bare link).
 */
export const BlockLinkPaste = Extension.create({
  name: "blockLinkPaste",
  priority: 1000,

  addProseMirrorPlugins() {
    const { editor } = this;
    return [
      new Plugin({
        key: new PluginKey("blockLinkPaste"),
        props: {
          handlePaste: (view, event) => {
            if (!view.editable) {
              return false;
            }
            const link = parseBlockLink(event.clipboardData?.getData("text/plain") ?? "");
            if (link === null) {
              return false;
            }
            const { state } = view;
            const { selection, schema } = state;
            const card = schema.nodes.pageLink;
            const chip = schema.nodes.pageRef;
            const { $from, $to } = selection;
            if (
              card !== undefined &&
              selection.empty &&
              $from.parent.type.name === "paragraph" &&
              $from.parent.content.size === 0 &&
              placementFor(state.doc, selection.from, card).replace !== null
            ) {
              return insertPageLinkCard(editor, link);
            }
            if (
              chip === undefined ||
              !$from.sameParent($to) ||
              !$from.parent.canReplaceWith($from.index(), $to.indexAfter(), chip)
            ) {
              return false;
            }
            view.dispatch(state.tr.replaceSelectionWith(chip.create(link), false).scrollIntoView());
            return true;
          },
        },
      }),
    ];
  },
});
