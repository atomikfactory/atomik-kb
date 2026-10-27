// The math block: block schema v1's `mathBlock` (content `text*` without marks, attribute
// `id` only), a LaTeX equation typeset offline by KaTeX (`./math`).
//
// The document holds the LaTeX as plain text, like a code block holds code, and nothing
// else: the typeset equation is view state, drawn into the node view from the text after it
// rests for a moment, never written back. While the caret is in the block (or the equation
// has not been drawn, or cannot be) its source is shown, above the equation; otherwise the
// source is collapsed and only the equation shows. A click on the equation puts the caret
// at the end of the source. A read-only editor shows the equation only.
//
// The node is `code: true` for what that gives a text node of source: Enter writes a newline
// instead of splitting the block, pasted content is plain text, input rules stay out of it
// and the `/` menu does not open in it. The collapsed source is hidden visually but stays
// part of the layout (never `display: none`), so that a caret placed in it is still a caret.

import { InputRule, mergeAttributes, Node as TiptapNode } from "@tiptap/core";
import type { Editor, NodeViewRendererProps } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { Plugin, PluginKey, Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorView, NodeView, ViewMutationRecord } from "@tiptap/pm/view";

import { t } from "../i18n";
import { describeMathError, loadMath, renderMath } from "./math";

/** The attribute holding a block's id (`BLOCK_ID_ATTRIBUTE` in `schema.ts`, which imports this file). */
const ID_ATTRIBUTE = "id";

/** The node's name in the schema. */
export const MATH_BLOCK_NODE = "mathBlock";

/** How long the source must rest before its equation is drawn again. */
const RENDER_DEBOUNCE_MS = 200;

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    mathBlock: {
      /** Turns the block with the caret into a math block (its text becomes the LaTeX). */
      setMathBlock: () => ReturnType;
      /** Turns the block into a math block, or a math block back into a paragraph. */
      toggleMathBlock: () => ReturnType;
    };
  }
}

/** What the equation area shows: nothing to draw, drawing, drawn, or KaTeX's message. */
type RenderState = "empty" | "loading" | "ready" | "error";

/** The typeset equation of a math block. */
interface MathRender {
  readonly element: HTMLElement;
  /** The source changed: draw it again once typing pauses. */
  changed: () => void;
  destroy: () => void;
}

/**
 * The equation below a math block's source: `div.math-block-render`, not editable. It draws
 * `sourceOf()` at once and again `RENDER_DEBOUNCE_MS` after each `changed()`. The last good
 * equation stays while the next one is drawn; a source that does not parse shows KaTeX's
 * message in its place, an empty one nothing. `onState` hears every change of what is shown.
 * The markup comes from `renderMath` only (KaTeX with `trust: false`).
 */
function createMathRender(sourceOf: () => string, onState: (state: RenderState) => void): MathRender {
  const element = document.createElement("div");
  element.className = "math-block-render";
  element.setAttribute("contenteditable", "false");
  element.setAttribute("role", "group");
  element.setAttribute("aria-label", t("mathBlock.label"));

  let timer: ReturnType<typeof setTimeout> | undefined;
  /** Counts the draws: a result is shown only while it is the latest one's. */
  let latest = 0;

  const place = (state: RenderState, ...content: Node[]): void => {
    element.dataset.state = state;
    element.replaceChildren(...content);
    onState(state);
  };
  const failed = (reason: string): void => {
    const error = document.createElement("div");
    error.className = "math-block-error";
    const title = document.createElement("p");
    title.textContent = t("mathBlock.error");
    error.append(title);
    if (reason !== "") {
      const detail = document.createElement("div");
      detail.className = "math-block-error-detail";
      detail.textContent = reason;
      error.append(detail);
    }
    place("error", error);
  };
  const drawn = (markup: string): void => {
    const equation = document.createElement("div");
    equation.className = "math-block-equation";
    equation.innerHTML = markup; // KaTeX's own output (trust: false), never the document's text
    place("ready", equation);
  };

  const draw = (): void => {
    clearTimeout(timer);
    latest += 1;
    const mine = latest;
    const source = sourceOf();
    if (source.trim() === "") {
      place("empty");
      return;
    }
    if (element.firstElementChild === null) {
      place("loading");
    }
    loadMath().then(
      () => {
        if (mine !== latest) {
          return;
        }
        try {
          drawn(renderMath(source));
        } catch (error) {
          failed(describeMathError(error));
        }
      },
      (error: unknown) => {
        if (mine === latest) {
          failed(describeMathError(error));
        }
      },
    );
  };

  draw();

  return {
    element,
    changed: () => {
      clearTimeout(timer);
      timer = setTimeout(draw, RENDER_DEBOUNCE_MS);
    },
    destroy: () => {
      clearTimeout(timer);
      latest += 1; // a draw still running shows nothing
    },
  };
}

type MathBlockViewProps = Pick<NodeViewRendererProps, "node" | "editor" | "getPos">;

/**
 * The block's view: `div.math-block` holding `div.math-block-source` (the text, editable)
 * and `div.math-block-render` (the equation). The source is `is-collapsed` while the
 * selection is elsewhere and an equation is on show, `is-empty` while it holds no text.
 */
function createMathBlockView({ node: initial, editor, getPos }: MathBlockViewProps): NodeView {
  let node = initial;
  /** Whether the selection is in this block, as of the last transaction. */
  let inside = false;
  let state: RenderState = "empty";

  const dom = document.createElement("div");
  dom.className = "math-block";
  dom.setAttribute("data-type", MATH_BLOCK_NODE);

  const source = document.createElement("div");
  source.className = "math-block-source";
  source.setAttribute("data-placeholder", t("mathBlock.placeholder"));
  source.setAttribute("spellcheck", "false"); // LaTeX is not prose

  /**
   * Shows what the state asks for: the source is collapsed when someone reads (a read-only
   * editor) or the selection is elsewhere and the equation is on show; while it is empty,
   * loading or does not parse, it stays in view.
   */
  const apply = (): void => {
    const editable = editor.isEditable;
    const collapsed = !editable || (!inside && node.content.size > 0 && state === "ready");
    source.classList.toggle("is-collapsed", collapsed);
    if (collapsed) {
      source.setAttribute("aria-hidden", "true");
    } else {
      source.removeAttribute("aria-hidden");
    }
    source.classList.toggle("is-empty", node.content.size === 0);
    dom.classList.toggle("is-editing", editable && inside);
    dom.classList.toggle("is-readonly", !editable);
  };

  const equation = createMathRender(
    () => node.textContent,
    (next) => {
      state = next;
      apply();
    },
  );
  dom.append(source, equation.element);

  const show = (): void => {
    const id: unknown = node.attrs[ID_ATTRIBUTE];
    if (typeof id === "string") {
      dom.setAttribute("data-id", id);
    } else {
      dom.removeAttribute("data-id");
    }
    apply();
  };

  /** Whether the selection (its anchor or its head) is in the text of this block. */
  const selectionInside = (): boolean => {
    const pos = getPos();
    if (pos === undefined) {
      return false;
    }
    const { anchor, head } = editor.state.selection;
    const start = pos + 1;
    const end = pos + node.nodeSize - 1;
    return (anchor >= start && anchor <= end) || (head >= start && head <= end);
  };
  const sync = (): void => {
    inside = selectionInside();
    apply();
  };
  // After each transaction, positions are final (`update` runs while the view is still being
  // brought up to date, when `getPos` is not to be trusted). `setEditable` is no transaction
  // but emits `update`.
  editor.on("transaction", sync);
  editor.on("update", sync);
  sync();
  show();

  // A click on the equation edits it: the caret goes to the end of the source, which expands.
  equation.element.addEventListener("mousedown", (event) => {
    const pos = getPos();
    if (!editor.isEditable || event.button !== 0 || pos === undefined) {
      return;
    }
    event.preventDefault(); // no native selection in the equation; the editor keeps the focus
    const end = pos + node.nodeSize - 1;
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, end)));
    editor.view.focus();
  });

  return {
    dom,
    contentDOM: source,
    update: (updated: PmNode) => {
      if (updated.type !== node.type) {
        return false;
      }
      const textChanged = updated.textContent !== node.textContent;
      node = updated;
      show();
      if (textChanged) {
        equation.changed();
      }
      return true;
    },
    // Events on the equation are this view's own (the click above), not ProseMirror's.
    stopEvent: (event: Event) => event.target instanceof Node && equation.element.contains(event.target),
    // Only the text (the source) is the document's: what is drawn in the equation, and the
    // classes and attributes this view sets, are no edit.
    ignoreMutation: (mutation: ViewMutationRecord) =>
      mutation.type === "attributes" || !source.contains(mutation.target),
    destroy: () => {
      editor.off("transaction", sync);
      editor.off("update", sync);
      equation.destroy();
    },
  };
}

/** The empty cursor inside a math block, or `null`. */
function cursorInMath(editor: Editor) {
  const { empty, $from } = editor.state.selection;
  return empty && $from.parent.type.name === MATH_BLOCK_NODE ? $from : null;
}

/**
 * Leaves the block: the caret goes to the start of the next block, or to a new paragraph
 * after the last one (what a code block does on ArrowDown at its end).
 */
function leaveMathBlock(editor: Editor): boolean {
  const $from = cursorInMath(editor);
  if ($from === null) {
    return false;
  }
  const after = $from.after();
  if (editor.state.doc.nodeAt(after) === null) {
    return editor.commands.exitCode();
  }
  return editor.commands.command(({ tr }) => {
    tr.setSelection(Selection.near(tr.doc.resolve(after)));
    return true;
  });
}

/**
 * Arrow keys step into a math block from the block next to it. The browser's own caret
 * movement would do it too, but only by luck of how the collapsed source lays out; this makes
 * it certain: the caret goes to the start of the source coming down or from the left, to the
 * end coming up or from the right, and the source expands.
 */
function enterFromNeighbour(view: EditorView, event: KeyboardEvent): boolean {
  const forward = event.key === "ArrowDown" || event.key === "ArrowRight";
  const backward = event.key === "ArrowUp" || event.key === "ArrowLeft";
  if ((!forward && !backward) || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) {
    return false;
  }
  const { selection, doc } = view.state;
  if (!(selection instanceof TextSelection) || !selection.empty) {
    return false;
  }
  const { $from } = selection;
  if ($from.parent.type.name === MATH_BLOCK_NODE) {
    return false; // moving inside the source is the browser's
  }
  const vertical = event.key === "ArrowDown" || event.key === "ArrowUp";
  const direction = vertical ? (forward ? "down" : "up") : forward ? "forward" : "backward";
  if (!view.endOfTextblock(direction)) {
    return false;
  }
  const next = Selection.findFrom(doc.resolve(forward ? $from.after() : $from.before()), forward ? 1 : -1, true);
  if (next === null || next.$from.parent.type.name !== MATH_BLOCK_NODE) {
    return false;
  }
  const target = forward ? next.$from.start() : next.$from.end();
  view.dispatch(view.state.tr.setSelection(TextSelection.create(doc, target)).scrollIntoView());
  return true;
}

/**
 * The math block (ADR-0016, amendment of 2026-10-06): `content: text*`, `marks: ""`, the
 * attribute `id` only. In HTML it is `<div data-type="mathBlock">` with the LaTeX as its
 * text. Typing `$$ ` in an empty paragraph, `/equation` and the block menu make one.
 */
export const MathBlock = TiptapNode.create({
  name: MATH_BLOCK_NODE,
  group: "block",
  content: "text*",
  marks: "",
  code: true,
  defining: true,

  parseHTML() {
    return [{ tag: 'div[data-type="mathBlock"]', preserveWhitespace: "full" }];
  },

  renderHTML({ HTMLAttributes }) {
    return ["div", mergeAttributes({ "data-type": MATH_BLOCK_NODE }, HTMLAttributes), 0];
  },

  addNodeView() {
    return (props: NodeViewRendererProps) => createMathBlockView(props);
  },

  addCommands() {
    return {
      // `setNode` keeps the block's attributes that the new type has too: the id stays.
      setMathBlock:
        () =>
        ({ commands }) =>
          commands.setNode(this.name),
      toggleMathBlock:
        () =>
        ({ commands }) =>
          commands.toggleNode(this.name, "paragraph"),
    };
  },

  addInputRules() {
    return [
      // `$$ ` (or `$$` and Enter) typed at the start of an empty paragraph. Typed after other
      // text, or before some, it is text.
      new InputRule({
        find: /^\$\$[\s\n]$/u,
        handler: ({ state, range, chain }) => {
          const { parent } = state.doc.resolve(range.from);
          if (parent.type.name !== "paragraph" || parent.content.size !== range.to - range.from) {
            return null;
          }
          chain().deleteRange(range).setNode(this.name).run();
          return undefined;
        },
      }),
    ];
  },

  addKeyboardShortcuts() {
    return {
      // An empty math block goes back to a paragraph (same block, same id).
      Backspace: ({ editor }) => {
        const $from = cursorInMath(editor);
        if ($from === null || $from.parent.content.size > 0) {
          return false;
        }
        return editor.commands.setNode("paragraph");
      },

      // Enter writes a newline (Tiptap's `newlineInCode`, as the node is `code`); a third
      // one at the end leaves the block, as in a code block.
      Enter: ({ editor }) => {
        const $from = cursorInMath(editor);
        if ($from === null || $from.parentOffset !== $from.parent.content.size) {
          return false;
        }
        if (!$from.parent.textContent.endsWith("\n\n")) {
          return false;
        }
        return editor
          .chain()
          .command(({ tr }) => {
            tr.delete($from.pos - 2, $from.pos);
            return true;
          })
          .exitCode()
          .run();
      },

      Escape: ({ editor }) => leaveMathBlock(editor),

      ArrowDown: ({ editor }) => {
        const $from = cursorInMath(editor);
        if ($from === null || $from.parentOffset !== $from.parent.content.size) {
          return false;
        }
        return leaveMathBlock(editor);
      },

      // A math block that starts the page leaves a way to add a block above it.
      ArrowUp: ({ editor }) => {
        const $from = cursorInMath(editor);
        if ($from === null || $from.parentOffset !== 0 || $from.before() > 0) {
          return false;
        }
        return editor.commands.insertDefaultBlock({ pos: $from.before() });
      },
    };
  },

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("mathBlockNavigation"),
        props: {
          handleKeyDown: (view, event) => enterFromNeighbour(view, event),
        },
      }),
    ];
  },
});
