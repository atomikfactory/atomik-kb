// Selecting several top-level blocks at once. `BlockRangeSelection` is a ProseMirror selection
// that covers a contiguous run of the page's top-level blocks (between two blocks, like a
// `NodeSelection` but for more than one); a plugin draws the blocks as selected and takes
// Escape and Shift+Up/Down (create, extend, shrink, clear) and Shift+click (extend). The bulk
// actions on a range (delete, duplicate, move, turn into) are in `block-range-commands.ts`; the
// block handle's menu and drag use them.
//
// A selection is editor state and never stored, so the document and the Loro binding know
// nothing about it: the binding reads `selection.$anchor.depth` before it makes cursors (a range
// is at depth 0, so it makes none) and maps the selection through a remote change with
// `Selection.map` (below).

import { Extension } from "@tiptap/core";
import { Slice } from "@tiptap/pm/model";
import type { Node as PmNode, ResolvedPos } from "@tiptap/pm/model";
import { NodeSelection, Plugin, PluginKey, Selection, TextSelection } from "@tiptap/pm/state";
import type { EditorState, SelectionBookmark, Transaction } from "@tiptap/pm/state";
import type { Mappable } from "@tiptap/pm/transform";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { EditorView } from "@tiptap/pm/view";

/** The class the decorations put on the blocks of a selected range. */
export const BLOCK_SELECTED_CLASS = "is-block-selected";

/** The position before the top-level block `index` (the end of the page for an index past the last block). */
export function blockStart(doc: PmNode, index: number): number {
  let pos = 0;
  for (let i = 0; i < index && i < doc.childCount; i += 1) {
    pos += doc.child(i).nodeSize;
  }
  return pos;
}

function clampIndex(doc: PmNode, index: number): number {
  return Math.min(Math.max(index, 0), doc.childCount - 1);
}

/** The block id of a node, `null` for none (a new block that UniqueID has not named yet, a read-only page). */
function blockId(node: PmNode | null | undefined): string | null {
  const id: unknown = node?.attrs.id;
  return typeof id === "string" && id !== "" ? id : null;
}

/**
 * A contiguous run of top-level blocks. `$anchor` and `$head` are the two sides of the run, each
 * between two blocks: the anchor stays where it is when the range is extended, the head moves. A
 * range of one block has its anchor before and its head after it.
 *
 * Replacing the range (typing, Backspace, cut, a paste, a drop of what was dragged out of it) works
 * like replacing any selection; `content()` is the whole blocks, so copying one and pasting it
 * gives the blocks back.
 */
export class BlockRangeSelection extends Selection {
  /** Like a node selection, the browser's own highlight and caret stay hidden. */
  override visible = false;

  /** The ids of the first and the last block: a change that moves positions around finds the range again by them. */
  readonly #firstId: string | null;
  readonly #lastId: string | null;

  constructor($anchor: ResolvedPos, $head: ResolvedPos) {
    super($anchor, $head);
    if ($anchor.depth !== 0 || $head.depth !== 0 || $anchor.pos === $head.pos) {
      throw new RangeError("A block range selection runs between top-level blocks");
    }
    this.#firstId = blockId(this.$from.nodeAfter);
    this.#lastId = blockId(this.$to.nodeBefore);
  }

  /** The blocks `anchorIndex` to `headIndex` (inclusive; indexes beyond the page's blocks are clamped). */
  static create(doc: PmNode, anchorIndex: number, headIndex: number = anchorIndex): BlockRangeSelection {
    if (doc.childCount === 0) {
      throw new RangeError("A page without blocks has none to select");
    }
    const anchor = clampIndex(doc, anchorIndex);
    const head = clampIndex(doc, headIndex);
    // The anchor is on the far side of its block from the head, so that the block stays inside.
    const $anchor = doc.resolve(blockStart(doc, anchor <= head ? anchor : anchor + 1));
    const $head = doc.resolve(blockStart(doc, anchor <= head ? head + 1 : head));
    return new BlockRangeSelection($anchor, $head);
  }

  /** The index of the first selected block among the page's top-level blocks. */
  get first(): number {
    return this.$from.index(0);
  }

  /** The index of the last selected block. */
  get last(): number {
    return this.$to.index(0) - 1;
  }

  /** How many blocks are selected. */
  get count(): number {
    return this.last - this.first + 1;
  }

  /** The index of the block the range grows from. */
  get anchorIndex(): number {
    return this.anchor < this.head ? this.$anchor.index(0) : this.$anchor.index(0) - 1;
  }

  /** The index of the block at the end that moves. */
  get headIndex(): number {
    return this.anchor < this.head ? this.$head.index(0) - 1 : this.$head.index(0);
  }

  override eq(other: Selection): boolean {
    return other instanceof BlockRangeSelection && other.anchor === this.anchor && other.head === this.head;
  }

  /**
   * Maps the range through a change. Blocks added at its edges stay out of it and blocks added
   * inside it join it. A change the positions cannot follow (the Loro binding hands a remote change
   * over as one replaced stretch of the page, which can reach from before the range to after it)
   * finds the range again by the ids of its first and last block. When the blocks it covered are
   * gone, or the change moved an edge into a block (a wrap, a lift), the selection becomes a caret
   * near where it was.
   */
  override map(doc: PmNode, mapping: Mappable): Selection {
    const from = Math.min(mapping.map(this.from, 1), doc.content.size);
    const to = Math.min(mapping.map(this.to, -1), doc.content.size);
    const mapped = this.#between(doc, from, to);
    if (mapped !== null && mapped.#firstId === this.#firstId && mapped.#lastId === this.#lastId) {
      return mapped;
    }
    return this.#byIds(doc) ?? mapped ?? Selection.near(doc.resolve(from));
  }

  /** The range of the same direction between `from` and `to`, `null` unless both are between top-level blocks and apart. */
  #between(doc: PmNode, from: number, to: number): BlockRangeSelection | null {
    const $from = doc.resolve(from);
    const $to = doc.resolve(to);
    if (from >= to || $from.depth !== 0 || $to.depth !== 0) {
      return null;
    }
    return this.anchor < this.head ? new BlockRangeSelection($from, $to) : new BlockRangeSelection($to, $from);
  }

  /** The range between the blocks that have the first and the last block's ids now, if both are there in that order. */
  #byIds(doc: PmNode): BlockRangeSelection | null {
    if (this.#firstId === null || this.#lastId === null) {
      return null;
    }
    let first = -1;
    let last = -1;
    doc.forEach((node, _offset, index) => {
      const id = blockId(node);
      if (id === this.#firstId) {
        first = index;
      }
      if (id === this.#lastId) {
        last = index;
      }
    });
    if (first < 0 || last < first) {
      return null;
    }
    const forward = this.anchor < this.head;
    return BlockRangeSelection.create(doc, forward ? first : last, forward ? last : first);
  }

  /** The whole blocks of the range. */
  override content(): Slice {
    return this.$from.doc.slice(this.from, this.to);
  }

  /**
   * Deletes the range, or replaces it with `content`. Deleting every block leaves one empty
   * paragraph (a page always has a block); the caret goes where the range was.
   */
  override replace(tr: Transaction, content: Slice = Slice.empty): void {
    if (content.size > 0) {
      super.replace(tr, content);
      return;
    }
    const { from, to } = this;
    tr.delete(from, to);
    tr.setSelection(Selection.near(tr.doc.resolve(Math.min(from, tr.doc.content.size))));
  }

  override toJSON(): { type: "blockRange"; anchor: number; head: number } {
    return { type: "blockRange", anchor: this.anchor, head: this.head };
  }

  override getBookmark(): SelectionBookmark {
    return new BlockRangeBookmark(this.anchor, this.head);
  }

  static override fromJSON(doc: PmNode, json: unknown): BlockRangeSelection {
    if (
      typeof json !== "object" ||
      json === null ||
      !("anchor" in json) ||
      !("head" in json) ||
      typeof json.anchor !== "number" ||
      typeof json.head !== "number"
    ) {
      throw new RangeError("Invalid input for BlockRangeSelection.fromJSON");
    }
    return new BlockRangeSelection(doc.resolve(json.anchor), doc.resolve(json.head));
  }
}

Selection.jsonID("blockRange", BlockRangeSelection);

/** What the history keeps of a range: its two sides, mapped through changes without a document. */
class BlockRangeBookmark implements SelectionBookmark {
  readonly #anchor: number;
  readonly #head: number;

  constructor(anchor: number, head: number) {
    this.#anchor = anchor;
    this.#head = head;
  }

  map(mapping: Mappable): SelectionBookmark {
    const forward = this.#anchor < this.#head;
    return new BlockRangeBookmark(mapping.map(this.#anchor, forward ? 1 : -1), mapping.map(this.#head, forward ? -1 : 1));
  }

  resolve(doc: PmNode): Selection {
    const $anchor = doc.resolve(Math.min(this.#anchor, doc.content.size));
    const $head = doc.resolve(Math.min(this.#head, doc.content.size));
    if ($anchor.depth === 0 && $head.depth === 0 && $anchor.pos !== $head.pos) {
      return new BlockRangeSelection($anchor, $head);
    }
    return Selection.near($head);
  }
}

/** A run of top-level blocks as the actions on it see it. */
export interface BlockRange {
  /** Where the first block starts. */
  from: number;
  /** Where the last block ends. */
  to: number;
  /** The indexes of the first and the last block among the page's top-level blocks. */
  first: number;
  last: number;
  /** The index of the block the run grows from, and of the one at the end that moves. */
  anchorIndex: number;
  headIndex: number;
}

/** The blocks `anchorIndex` to `headIndex` (inclusive, clamped to the page) as a range. */
export function rangeOfBlocks(doc: PmNode, anchorIndex: number, headIndex: number = anchorIndex): BlockRange {
  const anchor = clampIndex(doc, anchorIndex);
  const head = clampIndex(doc, headIndex);
  const first = Math.min(anchor, head);
  const last = Math.max(anchor, head);
  return { from: blockStart(doc, first), to: blockStart(doc, last + 1), first, last, anchorIndex: anchor, headIndex: head };
}

/** The top-level block that starts at `pos`, as a range of one; `null` where no top-level block starts. */
export function singleBlockRange(doc: PmNode, pos: number): BlockRange | null {
  if (pos < 0 || pos >= doc.content.size) {
    return null;
  }
  const $pos = doc.resolve(pos);
  return $pos.depth === 0 && $pos.nodeAfter !== null ? rangeOfBlocks(doc, $pos.index(0)) : null;
}

/** The range a selection covers when it is a block range or the selection of one top-level block; else `null`. */
export function blockRangeOf(selection: Selection): BlockRange | null {
  if (selection instanceof BlockRangeSelection) {
    return {
      from: selection.from,
      to: selection.to,
      first: selection.first,
      last: selection.last,
      anchorIndex: selection.anchorIndex,
      headIndex: selection.headIndex,
    };
  }
  if (selection instanceof NodeSelection && selection.$from.depth === 0) {
    const index = selection.$from.index(0);
    return { from: selection.from, to: selection.to, first: index, last: index, anchorIndex: index, headIndex: index };
  }
  return null;
}

/** How many blocks a range covers. */
export function blockCount(range: BlockRange): number {
  return range.last - range.first + 1;
}

/** True when the top-level block that starts at `pos` is in the range. */
export function rangeHas(range: BlockRange, pos: number): boolean {
  return pos >= range.from && pos < range.to;
}

/** The top-level blocks of a range. */
export function blocksOf(doc: PmNode, range: BlockRange): PmNode[] {
  const blocks: PmNode[] = [];
  for (let index = range.first; index <= range.last; index += 1) {
    blocks.push(doc.child(index));
  }
  return blocks;
}

/** The first and the last top-level block a selection touches. */
function touchedBlocks(doc: PmNode, selection: Selection): { first: number; last: number } {
  const { $from, $to } = selection;
  const first = clampIndex(doc, $from.index(0));
  // A side between blocks (a node selection, select all) ends with the block before it.
  const last = clampIndex(doc, $to.depth === 0 && $to.pos > $from.pos ? $to.index(0) - 1 : $to.index(0));
  return { first, last: Math.max(first, last) };
}

/** Selects the blocks `anchorIndex` to `headIndex` (inclusive). */
export function selectBlocks(state: EditorState, anchorIndex: number, headIndex: number = anchorIndex): Transaction | null {
  if (state.doc.childCount === 0) {
    return null;
  }
  return state.tr.setSelection(BlockRangeSelection.create(state.doc, anchorIndex, headIndex));
}

/**
 * Selects the top-level blocks the selection is in (the one with the caret, or all those a text
 * selection runs through). `null` when a block range is selected already.
 */
export function selectContainingBlocks(state: EditorState): Transaction | null {
  const { doc, selection } = state;
  if (selection instanceof BlockRangeSelection) {
    return null;
  }
  const { first, last } = touchedBlocks(doc, selection);
  const forward = selection.anchor <= selection.head;
  return selectBlocks(state, forward ? first : last, forward ? last : first);
}

/**
 * True when Shift+Up (`-1`) or Shift+Down (`1`) in a text selection should leave the text and
 * select blocks: the head is at the edge of its block (the end of its last textblock going
 * down, the start of its first going up) and the selection is a caret, or reaches into another
 * block, or covers the block's whole text. Anything else is the browser's own text selection.
 */
function leavesBlock(state: EditorState, direction: -1 | 1): boolean {
  const { doc, selection } = state;
  const { $head, $anchor } = selection;
  const index = $head.index(0);
  const start = blockStart(doc, index);
  const end = start + doc.child(index).nodeSize;
  const edge = Selection.findFrom(doc.resolve(direction > 0 ? end : start), direction > 0 ? -1 : 1, true);
  if (edge === null || $head.pos !== edge.head) {
    return false;
  }
  if (selection.empty || $anchor.index(0) !== index) {
    return true;
  }
  const opposite = Selection.findFrom(doc.resolve(direction > 0 ? start : end), direction > 0 ? 1 : -1, true);
  return opposite !== null && $anchor.pos === opposite.head;
}

/**
 * Shift+Down (`1`) or Shift+Up (`-1`): grows or shrinks a block range at its moving end; from a
 * text selection at the edge of its block, selects the block (the next press extends). `null`
 * for a selection this does not apply to (the browser handles it).
 */
export function extendBlockSelection(state: EditorState, direction: -1 | 1): Transaction | null {
  const { doc, selection } = state;
  if (doc.childCount === 0) {
    return null;
  }
  const range = blockRangeOf(selection);
  if (range !== null) {
    return selectBlocks(state, range.anchorIndex, range.headIndex + direction)?.scrollIntoView() ?? null;
  }
  if (!(selection instanceof TextSelection) || !leavesBlock(state, direction)) {
    return null;
  }
  return selectContainingBlocks(state)?.scrollIntoView() ?? null;
}

/**
 * Extends the selection to the block `index`: the range it grows from is the one selected, or
 * the block the text selection starts in, and the block `index` becomes its moving end.
 */
export function extendBlockSelectionTo(state: EditorState, index: number): Transaction | null {
  const { doc, selection } = state;
  const range = blockRangeOf(selection);
  const touched = touchedBlocks(doc, selection);
  const anchorIndex = range?.anchorIndex ?? (selection.anchor <= selection.head ? touched.first : touched.last);
  return selectBlocks(state, anchorIndex, index);
}

/** Clears a block range: the caret goes to the edge of the block at its moving end. `null` when none is selected. */
export function collapseBlockSelection(state: EditorState): Transaction | null {
  const { doc, selection } = state;
  if (!(selection instanceof BlockRangeSelection)) {
    return null;
  }
  const forward = selection.anchor < selection.head;
  return state.tr.setSelection(Selection.near(doc.resolve(selection.head), forward ? -1 : 1)).scrollIntoView();
}

/** The direct child of the editor's root that holds `target`, if any. */
export function blockElementAt(root: HTMLElement, target: EventTarget | null): HTMLElement | null {
  let element = target instanceof Element ? target : null;
  while (element !== null && element.parentElement !== root) {
    element = element.parentElement;
  }
  return element instanceof HTMLElement ? element : null;
}

/** The start position of the top-level block shown by `element`. */
export function blockPositionOf(view: EditorView, element: HTMLElement): number | null {
  let found: number | null = null;
  view.state.doc.forEach((_node, offset) => {
    if (found === null && view.nodeDOM(offset) === element) {
      found = offset;
    }
  });
  return found;
}

export const blockSelectionKey = new PluginKey("blockSelection");

/** Escape and Shift+Up/Down (no other modifier, no composition in progress). */
function onKeyDown(view: EditorView, event: KeyboardEvent): boolean {
  if (event.isComposing || event.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey) {
    return false;
  }
  const { state } = view;
  if (event.key === "Escape" && !event.shiftKey) {
    // A range clears; anything else selects the block it is in. The slash menu, the page picker
    // and a math block take Escape before this plugin (it comes last).
    const tr = collapseBlockSelection(state) ?? selectContainingBlocks(state);
    if (tr !== null) {
      view.dispatch(tr);
    }
    return true;
  }
  if (event.shiftKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
    const tr = extendBlockSelection(state, event.key === "ArrowDown" ? 1 : -1);
    if (tr === null) {
      return false;
    }
    view.dispatch(tr);
    return true;
  }
  return false;
}

/** Shift+click on a block, with a block selected, extends the range to that block (not the text selection). */
function onMouseDown(view: EditorView, event: MouseEvent): boolean {
  if (event.button !== 0 || !event.shiftKey || event.altKey || event.ctrlKey || event.metaKey) {
    return false;
  }
  if (blockRangeOf(view.state.selection) === null) {
    return false;
  }
  const element = blockElementAt(view.dom, event.target);
  const pos = element === null ? null : blockPositionOf(view, element);
  const tr = pos === null ? null : extendBlockSelectionTo(view.state, view.state.doc.resolve(pos).index(0));
  if (tr === null) {
    return false;
  }
  event.preventDefault(); // no text selection, no focus change
  view.dispatch(tr);
  return true;
}

/**
 * Block selection for the editor: the selected blocks drawn with `is-block-selected`, Escape
 * and Shift+Up/Down to select, extend, shrink and clear, Shift+click to extend. The plugin comes
 * after the other extensions' key handling (a lower priority), so their Escape wins.
 */
export const BlockSelection = Extension.create({
  name: "blockSelection",
  priority: 50,

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: blockSelectionKey,
        props: {
          decorations: (state) => {
            const { selection } = state;
            if (!(selection instanceof BlockRangeSelection)) {
              return null;
            }
            const decorations: Decoration[] = [];
            state.doc.forEach((node, offset, index) => {
              if (index >= selection.first && index <= selection.last) {
                decorations.push(Decoration.node(offset, offset + node.nodeSize, { class: BLOCK_SELECTED_CLASS }));
              }
            });
            return DecorationSet.create(state.doc, decorations);
          },
          handleKeyDown: onKeyDown,
          handleDOMEvents: { mousedown: onMouseDown },
          // Keeps the moving end of the range in view (the browser has no caret to follow).
          handleScrollToSelection: (view) => {
            const { selection, doc } = view.state;
            if (!(selection instanceof BlockRangeSelection)) {
              return false;
            }
            const head = view.nodeDOM(blockStart(doc, selection.headIndex));
            if (head instanceof HTMLElement) {
              head.scrollIntoView({ block: "nearest" });
            }
            return true;
          },
        },
      }),
    ];
  },
});
