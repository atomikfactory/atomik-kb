// The inline equation (ADR-0016, amendment of 2026-10-07): an inline atom `mathInline { id, latex }`,
// a LaTeX formula inside a line of text, typeset by KaTeX (`./math`) in its view. The document holds the
// source and nothing else: `latex` is a plain string attribute (empty while the equation is being
// written), replaced as a whole when it is edited, so two people editing the same equation at once
// leave the last write, not a blend. In HTML it is `<span data-type="mathInline" data-latex>`; as text
// (the clipboard's plain text, the Markdown export) it is `$latex$`, and the text index holds the bare
// source, so a search for `E=mc^2` finds the block (`dateMention` and `pageRef` are the same kind of
// atom: a click opens `MathInlineView`'s popover to edit the source).
//
// Three ways to make one: typing `$x$` (the closing `$` converts, the caret goes after the node and
// Backspace takes the conversion back), `/inline equation` (an empty equation with its popover
// open) and pasting Markdown that holds `$x$` (`markdown-paste.ts`). `$$` is the math block's rule
// and never reaches this one: an equation needs a character that is not a `$` between its dollars.

import { mathInlineMarkdown } from "@atomik-kb/core-client";
import { InputRule, mergeAttributes, Node } from "@tiptap/core";
import type { NodeViewRenderer } from "@tiptap/core";
import { NodeSelection } from "@tiptap/pm/state";

import { takesInlineAtom } from "./date-mention";

/** The node's name in the schema. */
export const MATH_INLINE_NODE = "mathInline";

/**
 * What the text before the caret must end with for the closing `$` just typed to make an equation:
 * an opening `$` (not after another `$`, which would be the block's fence, nor after a backslash),
 * the source (no `$` and no line break inside, no whitespace right after the opening `$` or before
 * the closing one, no backslash last), and the closing `$`. Group 1 is what comes before the opening
 * `$` (a character, or nothing at the start of the text), group 2 the source. The same shape
 * `parseMarkdown` of core-client reads in text.
 */
const TYPED_EQUATION = /(^|[^$\\])\$([^\s$\\]|[^\s$][^$\n]*[^\s$\\])\$$/u;

/**
 * Which equation has its editor popover open, if any (by block id). The node view of an equation draws
 * its own popover, so a command that wants one opened (`/inline equation`) says whose it is, and the
 * view that holds that id opens it.
 */
export class MathInlineRequests {
  #openFor: string | null = null;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  /** The block id of the equation whose popover is open. */
  readonly getSnapshot = (): string | null => this.#openFor;

  open(id: string): void {
    this.#set(id);
  }

  /** Closes the popover of `id`, or whichever is open when no id is given. */
  close(id?: string): void {
    if (id === undefined || this.#openFor === id) {
      this.#set(null);
    }
  }

  #set(id: string | null): void {
    if (this.#openFor === id) {
      return;
    }
    this.#openFor = id;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

export interface MathInlineOptions {
  /** How the editor shows an equation (the live chip); `null` renders `renderHTML`. */
  nodeView: NodeViewRenderer | null;
}

export interface MathInlineStorage {
  requests: MathInlineRequests;
}

export interface InsertMathInline {
  /** The LaTeX source; empty by default. */
  latex?: string;
  /** Opens the equation's popover once it is in the page. */
  openEditor?: boolean;
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    mathInline: {
      /**
       * Inserts an equation (a block id of its own) at the selection, in place of it, the caret
       * after it. Refuses a place that takes no inline atoms (a code block, a math block, a
       * toggle's summary).
       */
      insertMathInline: (attributes?: InsertMathInline) => ReturnType;
    };
  }

  interface Storage {
    mathInline: MathInlineStorage;
  }
}

/** The text of an equation in HTML and plain text: `$latex$`, nothing while the source is empty. */
function equationText(latex: unknown): string {
  return mathInlineMarkdown(latex);
}

/** The inline atom of block schema v1 that stands for a LaTeX formula inside text. */
export const MathInline = Node.create<MathInlineOptions, MathInlineStorage>({
  name: MATH_INLINE_NODE,
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

  addOptions() {
    return { nodeView: null };
  },

  addStorage() {
    return { requests: new MathInlineRequests() };
  },

  addAttributes() {
    return {
      latex: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-latex") ?? "",
        renderHTML: (attributes) => ({ "data-latex": typeof attributes.latex === "string" ? attributes.latex : "" }),
      },
    };
  },

  parseHTML() {
    return [{ tag: `span[data-type="${MATH_INLINE_NODE}"]` }];
  },

  renderHTML({ node, HTMLAttributes }) {
    return [
      "span",
      mergeAttributes({ "data-type": MATH_INLINE_NODE, class: "math-inline" }, HTMLAttributes),
      equationText(node.attrs.latex),
    ];
  },

  renderText({ node }) {
    return equationText(node.attrs.latex);
  },

  addNodeView() {
    return this.options.nodeView;
  },

  addCommands() {
    return {
      insertMathInline:
        ({ latex = "", openEditor = false } = {}) =>
        ({ commands, editor }) => {
          if (!takesInlineAtom(editor, this.name)) {
            return false;
          }
          const id = crypto.randomUUID();
          const inserted = commands.insertContent({ type: this.name, attrs: { id, latex } });
          if (inserted && openEditor) {
            this.storage.requests.open(id);
          }
          return inserted;
        },
    };
  },

  addKeyboardShortcuts() {
    return {
      // An equation selected with the arrow keys opens its popover, as a click does.
      Enter: ({ editor }) => {
        const { selection } = editor.state;
        if (!editor.isEditable || !(selection instanceof NodeSelection) || selection.node.type.name !== this.name) {
          return false;
        }
        const id: unknown = selection.node.attrs.id;
        if (typeof id !== "string") {
          return false;
        }
        this.storage.requests.open(id);
        return true;
      },
    };
  },

  addInputRules() {
    return [
      new InputRule({
        find: TYPED_EQUATION,
        handler: ({ state, range, match, chain }) => {
          const before = match[1] ?? "";
          const latex = match[2] ?? "";
          const type = state.schema.nodes[this.name];
          const from = range.from + before.length;
          const $from = state.doc.resolve(from);
          // Where the text is code, or text only, it stays text.
          if (type === undefined || latex === "" || !$from.parent.canReplaceWith($from.index(), $from.index(), type)) {
            return null;
          }
          chain()
            .insertContentAt({ from, to: range.to }, { type: this.name, attrs: { id: crypto.randomUUID(), latex } })
            .run();
          return undefined;
        },
      }),
    ];
  },
});
