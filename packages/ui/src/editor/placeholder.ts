// The "Type '/' for commands" hint, as in Notion: it shows on the empty text block that holds the
// cursor, and only while the editor has focus; unfocused, only a completely empty page keeps one
// on its first line. Tiptap's own `Placeholder` is not used: with `includeChildren` (needed to
// reach blocks inside lists, toggles, callouts and columns) it keeps its decorations in a state
// field that is updated per changed range, and a decoration it misses (a remote change, a move
// of the selection between nested blocks) stays on screen, so empty lines piled up hints. Here
// the decorations are computed from the current selection each time, so there is at most one.

import { Extension } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";

export interface PlaceholderOptions {
  /** The hint of a text block (empty string for none). */
  placeholder: (props: { node: PmNode }) => string;
}

const KEY = new PluginKey<boolean>("blockPlaceholder");

/** The class of the empty block and the attribute that carries its hint (the CSS draws it). */
export const PLACEHOLDER_CLASS = "is-empty";
export const PLACEHOLDER_ATTRIBUTE = "data-placeholder";

function decorate(node: PmNode, pos: number, text: string): Decoration | null {
  if (text === "") {
    return null;
  }
  return Decoration.node(pos, pos + node.nodeSize, { class: PLACEHOLDER_CLASS, [PLACEHOLDER_ATTRIBUTE]: text });
}

export const BlockPlaceholder = Extension.create<PlaceholderOptions>({
  name: "blockPlaceholder",

  addOptions() {
    return { placeholder: () => "" };
  },

  addProseMirrorPlugins() {
    const editor = this.editor;
    const hint = this.options.placeholder;
    return [
      new Plugin<boolean>({
        key: KEY,
        state: {
          init: () => false,
          apply: (tr, focused) => {
            const meta = tr.getMeta(KEY) as boolean | undefined;
            return meta ?? focused;
          },
        },
        props: {
          handleDOMEvents: {
            focus: (view) => {
              if (!KEY.getState(view.state)) {
                view.dispatch(view.state.tr.setMeta(KEY, true).setMeta("addToHistory", false));
              }
              return false;
            },
            blur: (view) => {
              if (KEY.getState(view.state)) {
                view.dispatch(view.state.tr.setMeta(KEY, false).setMeta("addToHistory", false));
              }
              return false;
            },
          },
          decorations(state) {
            if (!editor.isEditable) {
              return null;
            }
            const { $anchor } = state.selection;
            if (KEY.getState(state) === true) {
              const node = $anchor.parent;
              if (node.isTextblock && node.content.size === 0) {
                const deco = decorate(node, $anchor.before(), hint({ node }));
                return deco ? DecorationSet.create(state.doc, [deco]) : null;
              }
              return null;
            }
            // Unfocused: only the first line of a page with nothing on it.
            if (editor.isEmpty) {
              const first = state.doc.firstChild;
              if (first?.isTextblock) {
                const deco = decorate(first, 0, hint({ node: first }));
                return deco ? DecorationSet.create(state.doc, [deco]) : null;
              }
            }
            return null;
          },
        },
      }),
    ];
  },
});
