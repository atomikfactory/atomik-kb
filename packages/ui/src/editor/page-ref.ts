import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

export interface PageRefOptions {
  /** How the editor shows a reference (the live chip); `null` renders `renderHTML`. */
  nodeView: NodeViewRenderer | null;
  /**
   * The text of a reference in HTML and plain text (copy, drag): the page's current
   * title when known. Without it the chip carries no text.
   */
  label: (pageId: string) => string | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    pageRef: {
      /** Inserts a reference to a page (and optionally one of its blocks) at the selection. */
      insertPageRef: (attributes: { pageId: string; blockId?: string | null }) => ReturnType;
    };
  }
}

/**
 * An internal page reference (block schema v1, inline atom `pageRef { pageId, blockId? }`).
 * External URLs use the `link` mark instead. In HTML it is a chip,
 * `<span data-page-ref data-page-id="…">`; in plain text `[[Title]]`.
 */
export const PageRef = Node.create<PageRefOptions>({
  name: "pageRef",
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

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
        tag: "span[data-page-ref]",
        // A chip without a page id is not a reference (the attribute is required).
        getAttrs: (element) => (element.getAttribute("data-page-id") === null ? false : null),
      },
    ];
  },

  renderHTML({ node, HTMLAttributes }) {
    const label = this.options.label(String(node.attrs.pageId)) ?? "";
    return ["span", mergeAttributes({ "data-page-ref": "", class: "page-ref" }, HTMLAttributes), label];
  },

  renderText({ node }) {
    const label = this.options.label(String(node.attrs.pageId));
    return `[[${label ?? String(node.attrs.pageId)}]]`;
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      insertPageRef:
        ({ pageId, blockId = null }) =>
        ({ commands }) =>
          commands.insertContent({ type: this.name, attrs: { pageId, blockId } }),
    };
  },
});
