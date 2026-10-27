// The breadcrumb block: block schema v1's `breadcrumb` (a leaf atom, group `block`, attribute
// `id` only; ADR-0016, amendment of 2026-10-06). It stores nothing of the chain: its view
// (`BreadcrumbView.tsx`) draws where the page the editor shows sits in the page tree, from
// the app's structure store, so a rename or a move shows at once and nothing is written.

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

import { BREADCRUMB_NODE, insertLeafBlock } from "./derived-blocks";

export { BREADCRUMB_NODE } from "./derived-blocks";

export interface BreadcrumbOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
  /** The page the editor shows: the chain the view draws ends at it. `null` draws nothing. */
  pageId: string | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    breadcrumb: {
      /** Inserts a breadcrumb where the selection is, the caret in the block after it. */
      insertBreadcrumb: () => ReturnType;
    };
  }
}

/**
 * The breadcrumb (ADR-0016, amendment of 2026-10-06): a leaf block with the attribute `id`
 * only. In HTML it is an empty `<div data-type="breadcrumb">`: the chain is derived from the
 * page tree, so nothing of it is copied.
 */
export const Breadcrumb = Node.create<BreadcrumbOptions>({
  name: BREADCRUMB_NODE,
  group: "block",
  atom: true,
  selectable: true,

  addOptions() {
    return { nodeView: null, pageId: null };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="breadcrumb"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": BREADCRUMB_NODE, class: "breadcrumb-block" }, HTMLAttributes)];
  },

  addCommands() {
    return {
      insertBreadcrumb:
        () =>
        (props) =>
          insertLeafBlock(this.name, props),
    };
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
