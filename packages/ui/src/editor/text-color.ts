// The two colour marks of block schema v1 (ADR-0016, amendment of 2026-10-07): `textColor`
// and `highlight`, each with one attribute, `color`, a name of the shared palette
// (`palette.ts`). Both expand after (inclusive, like bold: typing at the end of a coloured
// run continues it) and exclude themselves (one colour at a time, a pick replaces the old one;
// a text colour and a highlight go together).
//
// `color` is `null` by default, which is no colour and is not stored: the commands only write
// a name of the palette, and HTML without a colour makes no mark. A name this version does not
// know (written by a newer one, or pasted from our own HTML of a newer one) is kept as it
// is and shown as plain text, until someone picks another colour. Not `@tiptap/extension-text-style`:
// its inline `style="color: ..."` would store values that cannot follow the theme.

import { Mark } from "@tiptap/core";
import type { Editor } from "@tiptap/core";
import type { Mark as ProseMirrorMark } from "@tiptap/pm/model";

import { isPaletteColor } from "./palette";
import type { PaletteColor } from "./palette";

/** The highlight a bare `<mark>` from foreign HTML becomes (the browsers' own yellow). */
export const FOREIGN_HIGHLIGHT_COLOR: PaletteColor = "yellow";

/** The names of the two marks. */
export type ColorMarkName = "textColor" | "highlight";

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    textColor: {
      /** Colours the selected text; with a cursor only, the text typed next (stored marks). */
      setTextColor: (color: PaletteColor) => ReturnType;
      /** Takes the text colour off the selection (or off the text typed next). */
      unsetTextColor: () => ReturnType;
    };
    highlight: {
      /** Highlights the selected text; with a cursor only, the text typed next (stored marks). */
      setHighlight: (color: PaletteColor) => ReturnType;
      /** Takes the highlight off the selection (or off the text typed next). */
      unsetHighlight: () => ReturnType;
    };
  }
}

/** The attribute as HTML gives it: empty is no colour. */
function colorOfElement(element: HTMLElement, attribute: string): string | null {
  const value = element.getAttribute(attribute);
  return value === null || value === "" ? null : value;
}

/** The colour a mark holds: `null` when it holds none. */
function colorOfMark(mark: ProseMirrorMark | undefined): string | null {
  const color: unknown = mark?.attrs.color;
  return typeof color === "string" ? color : null;
}

/** What `renderHTML` writes for a colour: nothing for none. */
function renderColor(attribute: string, color: unknown): Record<string, string> {
  return typeof color === "string" && color !== "" ? { [attribute]: color } : {};
}

/** A text colour: `<span data-text-color="red">`. */
export const TextColor = Mark.create({
  name: "textColor",
  inclusive: true,
  excludes: "textColor",

  addAttributes() {
    return {
      color: {
        default: null,
        parseHTML: (element) => colorOfElement(element, "data-text-color"),
        renderHTML: (attributes) => renderColor("data-text-color", attributes.color),
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: "span[data-text-color]",
        getAttrs: (element) => (colorOfElement(element, "data-text-color") === null ? false : null),
      },
    ];
  },

  renderHTML({ HTMLAttributes }) {
    return ["span", HTMLAttributes, 0];
  },

  addCommands() {
    return {
      setTextColor:
        (color) =>
        ({ commands }) =>
          isPaletteColor(color) && commands.setMark(this.name, { color }),
      unsetTextColor:
        () =>
        ({ commands }) =>
          commands.unsetMark(this.name),
    };
  },
});

/**
 * A highlight: `<mark data-highlight="yellow">`. A `<mark>` without `data-highlight` (HTML
 * from another app or a web page) becomes a yellow highlight; an inline `style` is ignored
 * (here and for the text colour), so only our own names come in with their meaning.
 */
export const Highlight = Mark.create({
  name: "highlight",
  inclusive: true,
  excludes: "highlight",

  addAttributes() {
    return {
      color: {
        default: null,
        parseHTML: (element) => colorOfElement(element, "data-highlight"),
        renderHTML: (attributes) => renderColor("data-highlight", attributes.color),
      },
    };
  },

  parseHTML() {
    return [
      {
        tag: "mark[data-highlight]",
        priority: 60,
        getAttrs: (element) => (colorOfElement(element, "data-highlight") === null ? false : null),
      },
      { tag: "mark", getAttrs: () => ({ color: FOREIGN_HIGHLIGHT_COLOR }) },
    ];
  },

  renderHTML({ HTMLAttributes }) {
    return ["mark", HTMLAttributes, 0];
  },

  addCommands() {
    return {
      setHighlight:
        (color) =>
        ({ commands }) =>
          isPaletteColor(color) && commands.setMark(this.name, { color }),
      unsetHighlight:
        () =>
        ({ commands }) =>
          commands.unsetMark(this.name),
    };
  },
});

/**
 * The colour the selection (or the cursor, with the marks typing would take) has for `name`:
 * the stored name when the whole selection carries that one colour; `null` when none of it
 * is coloured, or its mark holds no colour; `undefined` when the selection is mixed (part of
 * it coloured, or two colours). The name may be one outside the palette (a newer version's).
 */
export function selectionColor(editor: Editor, name: ColorMarkName): string | null | undefined {
  const { state } = editor;
  const type = state.schema.marks[name];
  if (type === undefined) {
    return null;
  }
  const { selection } = state;
  if (selection.empty) {
    return colorOfMark(type.isInSet(state.storedMarks ?? selection.$from.marks()));
  }
  const colors = new Set<string | null>();
  let uncovered = false;
  state.doc.nodesBetween(selection.from, selection.to, (node) => {
    if (node.isText) {
      const mark = type.isInSet(node.marks);
      if (mark === undefined) {
        uncovered = true;
      } else {
        colors.add(colorOfMark(mark));
      }
    }
  });
  if (colors.size === 0) {
    return null;
  }
  const [only] = colors;
  return colors.size === 1 && !uncovered && only !== undefined ? only : undefined;
}
