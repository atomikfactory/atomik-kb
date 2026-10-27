// The page link block (block schema v1, additive node of 2026-10-06, ADR-0016): a card that
// links to a page, `pageLink { id, pageId, blockId? }`. It is a leaf block (an atom), allowed
// wherever blocks are, with the same two attributes as the inline `pageRef` chip and the same
// meaning: the core counts it as a link (backlinks, the derived `links` table) and indexes it
// by the referenced page's title, exactly like a chip. In HTML it is
// `<div data-type="pageLink" data-page-id data-block-id>`; in plain text `[[Title]]`.

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

export const PAGE_LINK_NODE = "pageLink";

/** What a card links to: a page, and optionally one of its blocks. */
export interface PageLinkAttrs {
  pageId: string;
  blockId?: string | null;
}

export interface PageLinkOptions {
  /** How the editor shows a card (the live card); `null` renders `renderHTML`. */
  nodeView: NodeViewRenderer | null;
  /**
   * The text of a card in HTML and plain text (copy, drag): the page's current title when
   * known. Without it the card carries no text.
   */
  label: (pageId: string) => string | null;
}

export const PageLink = Node.create<PageLinkOptions>({
  name: PAGE_LINK_NODE,
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addOptions() {
    return { nodeView: null, label: () => null };
  },

  addAttributes() {
    return {
      pageId: {
        isRequired: true,
        parseHTML: (element) => element.getAttribute("data-page-id"),
        renderHTML: (attributes) => ({ "data-page-id": String(attributes.pageId) }),
      },
      blockId: {
        default: null,
        parseHTML: (element) => element.getAttribute("data-block-id"),
        renderHTML: (attributes) =>
          typeof attributes.blockId === "string" ? { "data-block-id": attributes.blockId } : {},
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: `div[data-type="${PAGE_LINK_NODE}"]`,
        // A card without a page id links nowhere (the attribute is required).
        getAttrs: (element) => (element.getAttribute("data-page-id") === null ? false : null),
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    const label = this.options.label(String(node.attrs.pageId)) ?? "";
    return ["div", mergeAttributes({ "data-type": PAGE_LINK_NODE, class: "page-link-block" }, HTMLAttributes), label];
  },

  renderText({ node }) {
    const label = this.options.label(String(node.attrs.pageId));
    return `[[${label ?? String(node.attrs.pageId)}]]`;
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
