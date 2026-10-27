import { mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";

import { isPaletteColor, PALETTE_COLORS } from "./palette";
import type { PaletteColor } from "./palette";

/** The icon of a new callout. */
export const DEFAULT_CALLOUT_ICON = "💡";

/**
 * The named background colours of a callout (ADR-0016, amendment of 2026-10-06), in the
 * order the picker lists them: the palette the text colour and highlight marks share
 * (`palette.ts`). The stored `color` is one of these names, or absent for the default
 * background. The tints themselves are CSS variables (`--callout-<name>-bg` and
 * `--callout-<name>-border` in `app.css`), different in the light and dark themes.
 */
export const CALLOUT_COLORS = PALETTE_COLORS;

export type CalloutColor = PaletteColor;

/** Whether `value` is one of the palette's names (a stored colour can be anything). */
export function isCalloutColor(value: unknown): value is CalloutColor {
  return isPaletteColor(value);
}

export interface CalloutOptions {
  /** How the editor shows a callout; `null` renders `renderHTML` (schema-only use, tests). */
  nodeView: NodeViewRenderer | null;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    callout: {
      /** Wraps the selected blocks in a callout. */
      setCallout: (attributes?: { icon?: string; color?: CalloutColor }) => ReturnType;
    };
  }
}

/**
 * A callout: blocks next to an icon on a tinted background (block schema v1, `callout`
 * with `content: block+` and the attributes `icon` and `color`). In HTML it is
 * `<div data-type="callout" data-icon data-color>` with the icon and a content element.
 *
 * `color` is `null` by default, which is the default background and is not stored. It is
 * any string as far as the schema goes: a name this version does not know (written by a
 * newer one) shows the default background, and stays in the document until someone
 * picks another colour (nothing rewrites it).
 */
export const Callout = Node.create<CalloutOptions>({
  name: "callout",
  group: "block",
  content: "block+",
  defining: true,

  addOptions() {
    return { nodeView: null };
  },

  addAttributes() {
    return {
      icon: {
        default: DEFAULT_CALLOUT_ICON,
        parseHTML: (element) => element.getAttribute("data-icon") ?? DEFAULT_CALLOUT_ICON,
        renderHTML: (attributes) => ({ "data-icon": String(attributes.icon ?? DEFAULT_CALLOUT_ICON) }),
      },
      color: {
        default: null,
        parseHTML: (element) => {
          const color = element.getAttribute("data-color");
          return color === null || color === "" ? null : color;
        },
        renderHTML: (attributes) =>
          typeof attributes.color === "string" && attributes.color !== "" ? { "data-color": attributes.color } : {},
      },
    };
  },

  parseHTML() {
    return [{ tag: 'div[data-type="callout"]', contentElement: "div.callout-content" }];
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      "div",
      mergeAttributes({ "data-type": "callout", class: "callout" }, HTMLAttributes),
      ["span", { class: "callout-icon", contenteditable: "false" }, String(node.attrs.icon ?? DEFAULT_CALLOUT_ICON)],
      ["div", { class: "callout-content" }, 0],
    ];
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      setCallout:
        (attributes = {}) =>
        ({ commands }) =>
          commands.wrapIn(this.name, attributes),
    };
  },
});
