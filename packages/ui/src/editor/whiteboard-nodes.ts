// The whiteboard block of the page schema (ADR-0033, ADR-0016 amendment of 2026-10-10):
// `whiteboard { id, height }`, a leaf block (group `block`, no content) that shows a drawing.
//
// The node holds no element of the drawing. The ProseMirror binding cannot put containers in node
// attributes, so the elements live in the page document's root map `whiteboards`, one plain value
// per element under `<block id>/<element id>` (`@atomik-kb/core-client`, `whiteboard.ts`). The
// node is the place in the page and the size of the inline view:
//
// - `height`: the height of the inline view in CSS pixels, 160 to 1200 (default 360, stored).
//
// In HTML it is `<div data-type="whiteboard" data-height>`; the drawing's text is not in the node,
// so the HTML and the plain-text forms have none.

import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

import { insertLeafBlock } from "./derived-blocks";
import type { WhiteboardStore } from "./whiteboard-store";

export const WHITEBOARD_NODE = "whiteboard";

/** The height of the inline view, in CSS pixels. */
export const MIN_WHITEBOARD_HEIGHT = 160;
export const MAX_WHITEBOARD_HEIGHT = 1200;
export const DEFAULT_WHITEBOARD_HEIGHT = 360;

/** The height of a stored value: an integer from 160 to 1200 (a numeric string counts), else 360. */
export function readWhiteboardHeight(value: unknown): number {
  const number = typeof value === "string" ? Number.parseInt(value, 10) : value;
  return typeof number === "number" &&
    Number.isInteger(number) &&
    number >= MIN_WHITEBOARD_HEIGHT &&
    number <= MAX_WHITEBOARD_HEIGHT
    ? number
    : DEFAULT_WHITEBOARD_HEIGHT;
}

export interface WhiteboardOptions {
  /** How the editor shows the block; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
  /** The page's drawing elements (the node holds none); `null` for schema-only use and tests. */
  store: WhiteboardStore | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    whiteboard: {
      /** Inserts an empty whiteboard where the selection is, the caret in the block after it. */
      insertWhiteboard: () => ReturnType;
    };
  }
}

/** The whiteboard (ADR-0033): a leaf block with a `height`; the drawing is drawn by `WhiteboardView`. */
export const Whiteboard = Node.create<WhiteboardOptions>({
  name: WHITEBOARD_NODE,
  group: "block",
  atom: true,
  selectable: true,
  draggable: true,

  addOptions() {
    return { nodeView: null, store: null };
  },

  addAttributes() {
    return {
      height: {
        default: DEFAULT_WHITEBOARD_HEIGHT,
        parseHTML: (element) => readWhiteboardHeight(element.getAttribute("data-height")),
        renderHTML: (attributes) => ({ "data-height": String(readWhiteboardHeight(attributes.height)) }),
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="whiteboard"]' }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": WHITEBOARD_NODE, class: "whiteboard-block" }, HTMLAttributes)];
  },

  renderText() {
    return "";
  },

  addCommands() {
    return {
      insertWhiteboard:
        () =>
        (props) =>
          insertLeafBlock(this.name, props),
    };
  },

  addNodeView() {
    return this.options.nodeView;
  },
});
