// Multi-block selection: the `BlockRangeSelection` class, the keyboard and mouse that make and
// change one, the bulk actions of the block handle's menu on a range, copy, cut, paste and drag
// of a range, and how it all meets the Loro binding (one update and one undo step per action).

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { AllSelection, EditorState, NodeSelection, Selection, TextSelection } from "@tiptap/pm/state";
import { findWrapping } from "@tiptap/pm/transform";
import { EditorView } from "@tiptap/pm/view";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc, UndoManager } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { FakeDataTransfer } from "../test/dnd";
import { loroDocFrom, paragraphs, pid, tick } from "../test/editor";
import { openPage, pasteEvent, renderPages } from "../test/page";
import {
  canMoveBlocks,
  canTurnBlocksInto,
  deleteBlocks,
  duplicateBlocks,
  moveBlocks,
  turnBlocksInto,
  turnIntoKindsOf,
} from "./block-range-commands";
import {
  BlockRangeSelection,
  BlockSelection,
  blockRangeOf,
  extendBlockSelection,
  extendBlockSelectionTo,
  rangeOfBlocks,
  selectBlocks,
  selectContainingBlocks,
} from "./block-selection";
import { startBlockDrag } from "./BlockHandle";
import { LoroBinding } from "./extensions";
import { LoroSyncPlugin, LoroUndoPlugin, redo, undo } from "./loro-binding";
import type { LoroDocType } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions } from "./schema";

const schema = createEditorSchema();
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// ---------------------------------------------------------------------------------------------
// Helpers

/** A page of paragraphs, ids `pid(0)`, `pid(1)`, ... Block `i` starts at `i * 5 + ...` for texts of 3 letters. */
function pageDoc(...texts: string[]): PmNode {
  return schema.nodeFromJSON(paragraphs(...texts));
}

/** A state over `doc` with the blocks `anchor` to `head` selected. */
function selected(doc: PmNode, anchor: number, head: number = anchor): EditorState {
  const base = EditorState.create({ doc });
  return base.apply(base.tr.setSelection(BlockRangeSelection.create(doc, anchor, head)));
}

/** The top-level blocks as `type:text`, with the level of a heading. */
function outline(doc: PmNode): string[] {
  const lines: string[] = [];
  doc.forEach((node) => {
    const level = node.type.name === "heading" ? String(node.attrs.level) : "";
    lines.push(`${node.type.name}${level}:${node.textContent}`);
  });
  return lines;
}

function idsOf(doc: PmNode): unknown[] {
  const result: unknown[] = [];
  doc.forEach((node) => {
    result.push(node.attrs.id);
  });
  return result;
}

function range(state: EditorState) {
  const found = blockRangeOf(state.selection);
  if (found === null) {
    throw new Error("no block range is selected");
  }
  return found;
}

/** The first and last block of a block range selection, as `[first, last]` (or `null`). */
function span(selection: Selection): [number, number] | null {
  return selection instanceof BlockRangeSelection ? [selection.first, selection.last] : null;
}

const mounted: { editor: Editor; element: HTMLElement; doc: LoroDoc }[] = [];

/** A Tiptap editor over the schema, the Loro binding and block selection, on a page of paragraphs. */
async function mount(...texts: string[]): Promise<{ editor: Editor; doc: LoroDoc }> {
  const json = paragraphs(...texts);
  const doc = loroDocFrom(json);
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({
    element,
    extensions: [...createSchemaExtensions(), LoroBinding.configure({ doc, readOnly: false }), BlockSelection],
    content: json as JSONContent,
    editable: true,
  });
  mounted.push({ editor, element, doc });
  await tick(); // the binding shows the document in a timeout
  return { editor, doc };
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
  vi.restoreAllMocks();
});

function caret(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** Presses a key in the editor; `false` when the editor took it (prevented the default). */
function press(editor: Editor, key: string, init: KeyboardEventInit = {}): boolean {
  return fireEvent.keyDown(editor.view.dom, { key, ...init });
}

/** The texts of the blocks the page draws as selected. */
function drawnSelected(editor: Editor): string[] {
  return [...editor.view.dom.children].filter((child) => child.classList.contains("is-block-selected")).map((child) => child.textContent ?? "");
}

/** Copies (or cuts) the selection with a clipboard event and returns what the clipboard got. */
function copyOrCut(editor: Editor, type: "copy" | "cut"): { html: string; text: string } {
  const data = new FakeDataTransfer();
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: data });
  editor.view.dom.dispatchEvent(event);
  return { html: data.getData("text/html"), text: data.getData("text/plain") };
}

// ---------------------------------------------------------------------------------------------
// The selection class

describe("BlockRangeSelection", () => {
  const doc = pageDoc("one", "two", "three", "four"); // blocks start at 0, 5, 10, 17 and end at 23

  it("covers whole blocks between two blocks and knows its blocks", () => {
    const selection = BlockRangeSelection.create(doc, 1, 2);
    expect([selection.from, selection.to, selection.anchor, selection.head]).toEqual([5, 17, 5, 17]);
    expect([selection.first, selection.last, selection.count]).toEqual([1, 2, 2]);
    expect([selection.anchorIndex, selection.headIndex]).toEqual([1, 2]);
    expect([selection.$from.depth, selection.$to.depth]).toEqual([0, 0]);
    expect(selection.empty).toBe(false);
    expect(selection.visible).toBe(false); // the browser's own caret and highlight stay out of it
  });

  it("keeps the side it grows from: the anchor is on the far side of its block", () => {
    const upward = BlockRangeSelection.create(doc, 2, 1);
    expect([upward.anchor, upward.head]).toEqual([17, 5]);
    expect([upward.from, upward.to]).toEqual([5, 17]);
    expect([upward.anchorIndex, upward.headIndex, upward.first, upward.last]).toEqual([2, 1, 1, 2]);

    const one = BlockRangeSelection.create(doc, 3);
    expect([one.anchor, one.head, one.count]).toEqual([17, 23, 1]);
  });

  it("clamps the indexes to the page", () => {
    const selection = BlockRangeSelection.create(doc, -3, 99);
    expect([selection.first, selection.last]).toEqual([0, 3]);
  });

  it("refuses anything that is not between top-level blocks", () => {
    expect(() => new BlockRangeSelection(doc.resolve(1), doc.resolve(10))).toThrow(RangeError); // inside a block
    expect(() => new BlockRangeSelection(doc.resolve(5), doc.resolve(5))).toThrow(RangeError); // nothing
    expect(() => new BlockRangeSelection(doc.resolve(5), doc.resolve(10))).not.toThrow();
  });

  it("has the whole blocks as its content", () => {
    const slice = BlockRangeSelection.create(doc, 1, 2).content();
    expect([slice.openStart, slice.openEnd, slice.content.childCount]).toEqual([0, 0, 2]);
    expect(slice.content.child(0).attrs.id).toBe(pid(1));
    expect(slice.content.child(1).textContent).toBe("three");
  });

  it("is equal to a range with the same sides, and to nothing else", () => {
    const a = BlockRangeSelection.create(doc, 1, 2);
    expect(a.eq(BlockRangeSelection.create(doc, 1, 2))).toBe(true);
    expect(a.eq(BlockRangeSelection.create(doc, 2, 1))).toBe(false); // same blocks, other anchor
    expect(a.eq(BlockRangeSelection.create(doc, 1, 3))).toBe(false);
    expect(a.eq(NodeSelection.create(doc, 5))).toBe(false);
  });

  it("round-trips through JSON, alone and inside a state", () => {
    const selection = BlockRangeSelection.create(doc, 2, 0);
    expect(selection.toJSON()).toEqual({ type: "blockRange", anchor: 17, head: 0 });
    const read = Selection.fromJSON(doc, selection.toJSON());
    expect(read).toBeInstanceOf(BlockRangeSelection);
    expect(read.eq(selection)).toBe(true);

    const state = EditorState.create({ doc }).apply(EditorState.create({ doc }).tr.setSelection(selection));
    const restored = EditorState.fromJSON({ schema }, state.toJSON());
    expect(restored.selection.eq(selection)).toBe(true);

    expect(() => Selection.fromJSON(doc, { type: "blockRange", anchor: "x", head: 2 })).toThrow(RangeError);
  });

  it("maps through a bookmark (what a history would keep)", () => {
    const state = selected(doc, 1, 2);
    const tr = state.tr.insert(0, schema.nodeFromJSON({ type: "paragraph", content: [{ type: "text", text: "new" }] }));
    const mapped = state.selection.getBookmark().map(tr.mapping).resolve(tr.doc);
    expect(span(mapped)).toEqual([2, 3]);
    // A bookmark whose blocks are gone becomes a caret.
    const gone = state.selection.getBookmark().map(state.tr.delete(5, 17).mapping).resolve(state.tr.delete(5, 17).doc);
    expect(gone).toBeInstanceOf(TextSelection);
  });

  describe("replacing it", () => {
    it("deletes the blocks, with the caret where they were", () => {
      const next = selected(doc, 1, 2).apply(selected(doc, 1, 2).tr.deleteSelection());
      expect(outline(next.doc)).toEqual(["paragraph:one", "paragraph:four"]);
      expect(idsOf(next.doc)).toEqual([pid(0), pid(3)]);
      expect(next.selection).toBeInstanceOf(TextSelection);
      expect(next.selection.from).toBe(6); // the start of "four"
      next.doc.check();
    });

    it("leaves one empty paragraph when every block goes", () => {
      const state = selected(doc, 0, 3);
      const next = state.apply(state.tr.deleteSelection());
      expect(outline(next.doc)).toEqual(["paragraph:"]);
      expect(next.selection).toBeInstanceOf(TextSelection);
      next.doc.check();
    });

    it("is replaced by a node, by a slice of blocks and by typed text like any selection", () => {
      const state = selected(doc, 1, 2);
      const rule = schema.nodes.horizontalRule?.create();
      if (rule === undefined) {
        throw new Error("no horizontalRule");
      }
      const withRule = state.apply(state.tr.replaceSelectionWith(rule));
      expect(outline(withRule.doc)).toEqual(["paragraph:one", "horizontalRule:", "paragraph:four"]);
      withRule.doc.check();

      const slice = pageDoc("x", "y").slice(0, 6);
      const withSlice = state.apply(state.tr.replaceSelection(slice));
      expect(outline(withSlice.doc)).toEqual(["paragraph:one", "paragraph:x", "paragraph:y", "paragraph:four"]);

      const typed = state.apply(state.tr.insertText("z"));
      expect(outline(typed.doc)).toEqual(["paragraph:one", "paragraph:z", "paragraph:four"]);
      typed.doc.check();
    });
  });

  describe("mapping it through changes", () => {
    const paragraphNode = (text: string) =>
      schema.nodeFromJSON({ type: "paragraph", content: [{ type: "text", text }] });

    it("follows blocks added before it", () => {
      const state = selected(doc, 1, 2);
      const next = state.apply(state.tr.insert(0, paragraphNode("new")));
      expect(span(next.selection)).toEqual([2, 3]);
    });

    it("takes in blocks added inside it, and leaves out blocks added at its edges", () => {
      const state = selected(doc, 1, 2);
      expect(span(state.apply(state.tr.insert(10, paragraphNode("mid"))).selection)).toEqual([1, 3]);
      expect(span(state.apply(state.tr.insert(5, paragraphNode("top"))).selection)).toEqual([2, 3]); // before the range
      expect(span(state.apply(state.tr.insert(17, paragraphNode("end"))).selection)).toEqual([1, 2]); // after it
    });

    it("shrinks with a block deleted from it and keeps its direction", () => {
      const state = selected(doc, 2, 1); // grown upward from block 2
      const next = state.apply(state.tr.delete(5, 10)); // "two" goes
      expect(span(next.selection)).toEqual([1, 1]);
      expect(next.selection.anchor).toBeGreaterThan(next.selection.head);
    });

    it("finds its blocks again by their ids when the change replaced the whole page, as a remote change does", () => {
      const state = selected(doc, 1, 2);
      const blocks = [paragraphNode("new"), ...[0, 1, 2, 3].map((index) => doc.child(index))];
      const next = state.apply(state.tr.replaceWith(0, doc.content.size, blocks));
      expect(span(next.selection)).toEqual([2, 3]); // "two" and "three", whichever way the positions moved

      const upward = selected(doc, 2, 1);
      const nextUpward = upward.apply(upward.tr.replaceWith(0, doc.content.size, blocks));
      expect(range(nextUpward)).toMatchObject({ anchorIndex: 3, headIndex: 2 });
    });

    it("becomes a caret when every block it covered is gone", () => {
      const state = selected(doc, 1, 2);
      const next = state.apply(state.tr.delete(5, 17));
      expect(next.selection).toBeInstanceOf(TextSelection);
      expect(next.selection.empty).toBe(true);
    });

    it("never ends inside a block: a wrap that moves its edges makes a valid selection", () => {
      const state = selected(doc, 1, 2);
      const blockRange = state.doc.resolve(5).blockRange(state.doc.resolve(17));
      const quote = schema.nodes.blockquote;
      const wrapping = blockRange === null || quote === undefined ? null : findWrapping(blockRange, quote);
      if (blockRange === null || wrapping === null) {
        throw new Error("cannot wrap the blocks");
      }
      const next = state.apply(state.tr.wrap(blockRange, wrapping));
      expect(outline(next.doc)).toEqual(["paragraph:one", "blockquote:twothree", "paragraph:four"]);
      if (next.selection instanceof BlockRangeSelection) {
        expect([next.selection.$from.depth, next.selection.$to.depth]).toEqual([0, 0]);
      }
    });
  });
});

// ---------------------------------------------------------------------------------------------
// Making and changing a selection

describe("selecting blocks", () => {
  const doc = pageDoc("one", "two", "three", "four");

  it("selectContainingBlocks takes the block with the caret and every block a text selection runs through", () => {
    const base = EditorState.create({ doc });
    const inTwo = base.apply(base.tr.setSelection(TextSelection.create(doc, 7)));
    const one = selectContainingBlocks(inTwo);
    expect(span(inTwo.apply(one ?? inTwo.tr).selection)).toEqual([1, 1]);

    const across = base.apply(base.tr.setSelection(TextSelection.create(doc, 7, 13)));
    const several = selectContainingBlocks(across);
    expect(span(across.apply(several ?? across.tr).selection)).toEqual([1, 2]);

    expect(selectContainingBlocks(selected(doc, 1))).toBeNull(); // already blocks
  });

  it("selectContainingBlocks takes the block of a node selection and every block of select all", () => {
    const base = EditorState.create({ doc });
    const node = base.apply(base.tr.setSelection(NodeSelection.create(doc, 10)));
    expect(span(node.apply(selectContainingBlocks(node) ?? node.tr).selection)).toEqual([2, 2]);
    const all = base.apply(base.tr.setSelection(new AllSelection(doc)));
    expect(span(all.apply(selectContainingBlocks(all) ?? all.tr).selection)).toEqual([0, 3]);
  });

  it("extendBlockSelection moves the head of a range and never past the page", () => {
    let state = selected(doc, 1);
    const step = (direction: -1 | 1): [number, number] | null => {
      const tr = extendBlockSelection(state, direction);
      state = state.apply(tr ?? state.tr);
      return span(state.selection);
    };
    expect(step(1)).toEqual([1, 2]);
    expect(step(1)).toEqual([1, 3]);
    expect(step(1)).toEqual([1, 3]); // the end of the page
    expect(step(-1)).toEqual([1, 2]);
    expect(step(-1)).toEqual([1, 1]);
    expect(step(-1)).toEqual([0, 1]); // other side of block 1
    expect(step(-1)).toEqual([0, 1]);
    expect(range(state)).toMatchObject({ anchorIndex: 1, headIndex: 0 });
  });

  it("extendBlockSelectionTo grows from the selected range, or from the block the caret is in", () => {
    const fromRange = selected(doc, 1);
    expect(span(fromRange.apply(extendBlockSelectionTo(fromRange, 3) ?? fromRange.tr).selection)).toEqual([1, 3]);
    expect(span(fromRange.apply(extendBlockSelectionTo(fromRange, 0) ?? fromRange.tr).selection)).toEqual([0, 1]);

    const base = EditorState.create({ doc }); // the caret is at the start of block 0
    expect(span(base.apply(extendBlockSelectionTo(base, 2) ?? base.tr).selection)).toEqual([0, 2]);
  });

  it("selectBlocks and rangeOfBlocks agree", () => {
    const state = EditorState.create({ doc });
    const tr = selectBlocks(state, 2, 0);
    expect(tr).not.toBeNull();
    const next = state.apply(tr ?? state.tr);
    expect(blockRangeOf(next.selection)).toEqual(rangeOfBlocks(doc, 2, 0));
  });
});

describe("the keyboard", () => {
  it("Escape selects the block the caret is in, draws it, and Escape again clears it", async () => {
    const { editor } = await mount("one", "two", "three");
    editor.view.focus(); // the browser's selection is ProseMirror's only while the editor has the focus
    caret(editor, 7); // in "two"

    expect(press(editor, "Escape")).toBe(false); // taken
    expect(span(editor.state.selection)).toEqual([1, 1]);
    expect(drawnSelected(editor)).toEqual(["two"]);
    expect(editor.view.dom).toHaveClass("ProseMirror-hideselection"); // no caret in the text

    expect(press(editor, "Escape")).toBe(false);
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.$from.parent.textContent).toBe("two"); // the caret is in the block it left
    expect(drawnSelected(editor)).toEqual([]);
  });

  it("Escape selects all the blocks a text selection runs through", async () => {
    const { editor } = await mount("one", "two", "three");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 2, 13)));
    press(editor, "Escape");
    expect(span(editor.state.selection)).toEqual([0, 2]);
    expect(drawnSelected(editor)).toEqual(["one", "two", "three"]);
  });

  it("Shift+Down at the end of a block selects it, then extends; Shift+Up shrinks", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    caret(editor, 4); // the end of "one"

    expect(press(editor, "ArrowDown", { shiftKey: true })).toBe(false);
    expect(span(editor.state.selection)).toEqual([0, 0]); // the first press selects the block
    press(editor, "ArrowDown", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 1]);
    press(editor, "ArrowDown", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 2]);
    expect(drawnSelected(editor)).toEqual(["one", "two", "three"]);

    press(editor, "ArrowUp", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 1]);
    press(editor, "ArrowUp", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 0]);
    press(editor, "ArrowUp", { shiftKey: true }); // at the top: the block stays selected
    expect(span(editor.state.selection)).toEqual([0, 0]);
    expect(drawnSelected(editor)).toEqual(["one"]);
  });

  it("Shift+Up at the start of a block selects it and extends upward", async () => {
    const { editor } = await mount("one", "two", "three");
    caret(editor, 11); // the start of "three"
    press(editor, "ArrowUp", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([2, 2]);
    press(editor, "ArrowUp", { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([1, 2]);
    expect(blockRangeOf(editor.state.selection)).toMatchObject({ anchorIndex: 2, headIndex: 1 });
    press(editor, "ArrowDown", { shiftKey: true }); // back to the one block
    expect(span(editor.state.selection)).toEqual([2, 2]);
  });

  it("leaves Shift+Up and Shift+Down to the browser inside a block's text", async () => {
    const { editor } = await mount("one", "two", "three");
    caret(editor, 7); // between "t" and "w"
    expect(press(editor, "ArrowDown", { shiftKey: true })).toBe(true); // not taken
    expect(press(editor, "ArrowUp", { shiftKey: true })).toBe(true);
    expect(editor.state.selection).toBeInstanceOf(TextSelection);

    // A partial selection that reaches the end of its block goes on as text, too.
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 7, 9)));
    expect(press(editor, "ArrowDown", { shiftKey: true })).toBe(true);
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
  });

  it("leaves plain arrows, other modifiers and Shift+Left alone", async () => {
    const { editor } = await mount("one", "two");
    caret(editor, 4);
    press(editor, "ArrowDown");
    press(editor, "ArrowDown", { shiftKey: true, ctrlKey: true });
    press(editor, "ArrowRight", { shiftKey: true });
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
  });

  it("Shift+click on a block, with blocks selected, extends the range (and does nothing special without)", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    const dom = (index: number): Element => {
      const found = editor.view.dom.children[index];
      if (found === undefined) {
        throw new Error(`no block ${String(index)}`);
      }
      return found;
    };

    expect(fireEvent.mouseDown(dom(2), { button: 0, shiftKey: true })).toBe(true); // no block selected: PM's own
    expect(editor.state.selection).toBeInstanceOf(TextSelection);

    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0)));
    expect(fireEvent.mouseDown(dom(2), { button: 0, shiftKey: true })).toBe(false); // taken
    expect(span(editor.state.selection)).toEqual([0, 2]);
    fireEvent.mouseDown(dom(3), { button: 0, shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 3]);
    fireEvent.mouseDown(dom(1), { button: 0, shiftKey: true }); // back inside
    expect(span(editor.state.selection)).toEqual([0, 1]);

    expect(fireEvent.mouseDown(dom(3), { button: 0 })).toBe(true); // a plain click is ProseMirror's own
  });

  it("Backspace deletes the selected blocks", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1, 2)));
    press(editor, "Backspace");
    expect(outline(editor.state.doc)).toEqual(["paragraph:one", "paragraph:four"]);
    expect(idsOf(editor.state.doc)).toEqual([pid(0), pid(3)]);
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
    expect(drawnSelected(editor)).toEqual([]);
  });

  it("Delete deletes them too, and a page of selected blocks keeps one empty paragraph", async () => {
    const { editor } = await mount("one", "two");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0, 1)));
    press(editor, "Delete");
    expect(outline(editor.state.doc)).toEqual(["paragraph:"]);
    expect(editor.state.doc.child(0).attrs.id).toMatch(UUID);
  });

  it("typing replaces the blocks with the typed text, as for any selection", async () => {
    const { editor } = await mount("one", "two", "three");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0, 1)));
    fireEvent.keyPress(editor.view.dom, { key: "x", charCode: 120, keyCode: 120 });
    expect(outline(editor.state.doc)).toEqual(["paragraph:x", "paragraph:three"]);
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
  });

  it("Ctrl+A is select all, as ever", async () => {
    const { editor } = await mount("one", "two");
    caret(editor, 2);
    press(editor, "a", { ctrlKey: true });
    expect(editor.state.selection).toBeInstanceOf(AllSelection);
  });

  it("lets a math block take Escape first (it leaves the block), the next Escape selects", () => {
    const json = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "one" }] },
        { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: "x^2" }] },
        { type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "two" }] },
      ],
    };
    const element = document.createElement("div");
    document.body.append(element);
    const editor = new Editor({
      element,
      extensions: [...createSchemaExtensions(), BlockSelection],
      content: json as JSONContent,
    });
    mounted.push({ editor, element, doc: new LoroDoc() });
    caret(editor, 7); // inside the equation

    press(editor, "Escape");
    expect(editor.state.selection).toBeInstanceOf(TextSelection); // the caret left the equation
    expect(editor.state.selection.$from.parent.type.name).toBe("paragraph");
    press(editor, "Escape");
    expect(span(editor.state.selection)).toEqual([2, 2]);
  });
});

// ---------------------------------------------------------------------------------------------
// The bulk actions

describe("bulk actions on a range", () => {
  const doc = pageDoc("one", "two", "three", "four", "five");

  it("deleteBlocks removes the range, in the middle, at the edges and as the whole page", () => {
    const run = (anchor: number, head: number) => {
      const state = selected(doc, anchor, head);
      const tr = deleteBlocks(state, range(state));
      return state.apply(tr ?? state.tr);
    };
    expect(outline(run(1, 2).doc)).toEqual(["paragraph:one", "paragraph:four", "paragraph:five"]);
    expect(outline(run(0, 1).doc)).toEqual(["paragraph:three", "paragraph:four", "paragraph:five"]);
    expect(outline(run(3, 4).doc)).toEqual(["paragraph:one", "paragraph:two", "paragraph:three"]);
    const all = run(0, 4);
    expect(outline(all.doc)).toEqual(["paragraph:"]);
    expect(all.selection).toBeInstanceOf(TextSelection);
    expect(idsOf(run(1, 3).doc)).toEqual([pid(0), pid(4)]);
    expect(run(1, 2).selection).toBeInstanceOf(TextSelection);
  });

  it("deleteBlocks of one block is the single-block command", () => {
    const state = selected(doc, 2);
    const next = state.apply(deleteBlocks(state, range(state)) ?? state.tr);
    expect(outline(next.doc)).toEqual(["paragraph:one", "paragraph:two", "paragraph:four", "paragraph:five"]);
  });

  it("moveBlocks moves the range together, keeps the ids and keeps the blocks selected", () => {
    const up = selected(doc, 2, 3);
    const movedUp = up.apply(moveBlocks(up, range(up), -1) ?? up.tr);
    expect(outline(movedUp.doc)).toEqual(["paragraph:one", "paragraph:three", "paragraph:four", "paragraph:two", "paragraph:five"]);
    expect(idsOf(movedUp.doc)).toEqual([pid(0), pid(2), pid(3), pid(1), pid(4)]);
    expect(span(movedUp.selection)).toEqual([1, 2]);

    const down = selected(doc, 0, 1);
    const movedDown = down.apply(moveBlocks(down, range(down), 1) ?? down.tr);
    expect(outline(movedDown.doc)).toEqual(["paragraph:three", "paragraph:one", "paragraph:two", "paragraph:four", "paragraph:five"]);
    expect(idsOf(movedDown.doc)).toEqual([pid(2), pid(0), pid(1), pid(3), pid(4)]);
    expect(span(movedDown.selection)).toEqual([1, 2]);
    movedDown.doc.check();
  });

  it("moveBlocks keeps the direction of the selection and stops at the edges", () => {
    const state = selected(doc, 3, 2); // grown upward
    const moved = state.apply(moveBlocks(state, range(state), 1) ?? state.tr);
    expect(range(moved)).toMatchObject({ anchorIndex: 4, headIndex: 3 });

    const top = selected(doc, 0, 1);
    expect(canMoveBlocks(top, range(top), -1)).toBe(false);
    expect(moveBlocks(top, range(top), -1)).toBeNull();
    const bottom = selected(doc, 3, 4);
    expect(canMoveBlocks(bottom, range(bottom), 1)).toBe(false);
    expect(moveBlocks(bottom, range(bottom), 1)).toBeNull();
    expect(canMoveBlocks(bottom, range(bottom), -1)).toBe(true);
  });

  it("moveBlocks of the whole page minus one block moves that block across", () => {
    const state = selected(doc, 1, 4);
    const moved = state.apply(moveBlocks(state, range(state), -1) ?? state.tr);
    expect(outline(moved.doc)).toEqual(["paragraph:two", "paragraph:three", "paragraph:four", "paragraph:five", "paragraph:one"]);
  });

  it("duplicateBlocks inserts copies without ids after the range and selects the copies", () => {
    const state = selected(doc, 1, 2);
    const next = state.apply(duplicateBlocks(state, range(state)) ?? state.tr);
    expect(outline(next.doc)).toEqual([
      "paragraph:one",
      "paragraph:two",
      "paragraph:three",
      "paragraph:two",
      "paragraph:three",
      "paragraph:four",
      "paragraph:five",
    ]);
    expect(idsOf(next.doc)).toEqual([pid(0), pid(1), pid(2), null, null, pid(3), pid(4)]); // UniqueID names the copies
    expect(span(next.selection)).toEqual([3, 4]);
  });

  it("turnBlocksInto converts each block that allows it and leaves the others untouched", () => {
    const page = schema.nodeFromJSON({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "one" }] },
        { type: "image", attrs: { id: pid(1), attachmentId: "a0000000-0000-4000-8000-000000000001" } },
        { type: "heading", attrs: { id: pid(2), level: 2 }, content: [{ type: "text", text: "two" }] },
        { type: "paragraph", attrs: { id: pid(3) }, content: [{ type: "text", text: "three" }] },
      ],
    });
    const state = selected(page, 0, 2);
    const kinds = turnIntoKindsOf(state, range(state));
    expect(kinds).toEqual(expect.arrayContaining(["heading1", "bulletList", "image", "file", "blockquote"]));

    const next = state.apply(turnBlocksInto(state, range(state), "heading1") ?? state.tr);
    expect(outline(next.doc)).toEqual(["heading1:one", "image:", "heading1:two", "paragraph:three"]); // the image stays
    expect(idsOf(next.doc)).toEqual([pid(0), pid(1), pid(2), pid(3)]); // same blocks, same ids
    expect(span(next.selection)).toEqual([0, 2]);
    next.doc.check();

    // Already a heading 2: only the paragraph changes; with every block already that kind, nothing does.
    expect(canTurnBlocksInto(state, range(state), "heading2")).toBe(true);
    const mixed = state.apply(turnBlocksInto(state, range(state), "heading2") ?? state.tr);
    expect(outline(mixed.doc)).toEqual(["heading2:one", "image:", "heading2:two", "paragraph:three"]);
    const again = selected(mixed.doc, 0, 2);
    expect(canTurnBlocksInto(again, range(again), "heading2")).toBe(false);
    expect(turnBlocksInto(again, range(again), "heading2")).toBeNull();
  });

  it("turnBlocksInto changes the number of blocks when one becomes several, and keeps them all selected", () => {
    const page = schema.nodeFromJSON({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "one" }] },
        {
          type: "callout",
          attrs: { id: pid(1), icon: "💡" },
          content: [
            { type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "a" }] },
            { type: "paragraph", attrs: { id: pid(3) }, content: [{ type: "text", text: "b" }] },
          ],
        },
        { type: "paragraph", attrs: { id: pid(4) }, content: [{ type: "text", text: "tail" }] },
      ],
    });
    const state = selected(page, 0, 1);
    const next = state.apply(turnBlocksInto(state, range(state), "paragraph") ?? state.tr);
    expect(outline(next.doc)).toEqual(["paragraph:one", "paragraph:a", "paragraph:b", "paragraph:tail"]);
    expect(span(next.selection)).toEqual([0, 2]);
    next.doc.check();
  });

  it("turnBlocksInto of one block is the single-block command", () => {
    const state = selected(doc, 1);
    const next = state.apply(turnBlocksInto(state, range(state), "heading3") ?? state.tr);
    expect(outline(next.doc)[1]).toBe("heading3:two");
    expect(next.selection).toBeInstanceOf(TextSelection); // as the single-block command leaves it
  });
});

describe("bulk actions in the editor", () => {
  it("duplicateBlocks gives the copies new ids from UniqueID and keeps the originals", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    const state = editor.state.apply(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1, 2)));
    editor.view.updateState(state);
    const tr = duplicateBlocks(editor.state, range(editor.state));
    expect(tr).not.toBeNull();
    editor.view.dispatch(tr ?? editor.state.tr);

    expect(outline(editor.state.doc)).toEqual([
      "paragraph:one",
      "paragraph:two",
      "paragraph:three",
      "paragraph:two",
      "paragraph:three",
      "paragraph:four",
    ]);
    const ids = idsOf(editor.state.doc);
    expect([ids[0], ids[1], ids[2], ids[5]]).toEqual([pid(0), pid(1), pid(2), pid(3)]);
    expect(ids[3]).toMatch(UUID);
    expect(ids[4]).toMatch(UUID);
    expect(new Set(ids).size).toBe(6);
    expect(span(editor.state.selection)).toEqual([3, 4]);
    expect(drawnSelected(editor)).toEqual(["two", "three"]);
  });
});

// ---------------------------------------------------------------------------------------------
// The block handle

describe("the block handle and a selection", () => {
  async function hover(editor: Editor, id: string): Promise<HTMLElement> {
    const block = editor.view.dom.querySelector(`[data-id='${id}']`);
    if (block === null) {
      throw new Error(`no block ${id}`);
    }
    fireEvent.mouseMove(block);
    return screen.findByRole("button", { name: t("block.actions") });
  }

  async function menuOf(editor: Editor, id: string): Promise<HTMLElement> {
    fireEvent.click(await hover(editor, id));
    return screen.findByRole("menu", { name: t("block.actions") });
  }

  async function closeMenu(): Promise<void> {
    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
  }

  function select(editor: Editor, anchor: number, head: number = anchor): void {
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, anchor, head)));
    });
  }

  it("selects the block on a click of its grip, and says how many blocks are selected", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] }]);
    const editor = await openPage(user, "Notes");
    expect(screen.queryByText(/blocks? selected/)).not.toBeInTheDocument();

    await menuOf(editor, pid(1));
    expect(span(editor.state.selection)).toEqual([1, 1]);
    // Announced politely, in a live region of its own (the block menu's "link copied" notice is the page's one status).
    expect(await screen.findByText("1 block selected")).toHaveAttribute("aria-live", "polite");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(editor.view.dom.querySelector(".is-block-selected")).toHaveAttribute("data-id", pid(1));
    await closeMenu();
    expect(span(editor.state.selection)).toEqual([1, 1]); // closing the menu keeps the block selected
    expect(editor.view.hasFocus()).toBe(true); // and the editor has the keyboard (Escape, Backspace, Ctrl+C)
  });

  it("Shift+click on another grip extends the range, in both directions, and the status follows", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    await menuOf(editor, pid(1));
    await closeMenu();

    fireEvent.click(await hover(editor, pid(3)), { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([1, 3]);
    expect(await screen.findByText("3 blocks selected")).toBeInTheDocument();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument(); // no menu for a Shift+click

    fireEvent.click(await hover(editor, pid(0)), { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 1]);
    expect(await screen.findByText("2 blocks selected")).toBeInTheDocument();
  });

  it("Shift+click on a grip with only a caret in the text selects from the caret's block", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] }]);
    const editor = await openPage(user, "Notes");
    caret(editor, 2); // in "One"
    fireEvent.click(await hover(editor, pid(2)), { shiftKey: true });
    expect(span(editor.state.selection)).toEqual([0, 2]);
  });

  it("acts on the whole range from the menu of a block inside it: delete", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    select(editor, 1, 2);

    const menu = await menuOf(editor, pid(2));
    expect(within(menu).queryByRole("menuitem", { name: t("block.copyLink") })).not.toBeInTheDocument(); // a link points at one block
    await user.click(within(menu).getByRole("menuitem", { name: t("block.delete") }));
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Four"]);
    expect(idsOf(editor.state.doc)).toEqual([pid(0), pid(3)]);
  });

  it("duplicates the range with new ids, in one go", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] }]);
    const editor = await openPage(user, "Notes");
    select(editor, 0, 1);

    await user.click(within(await menuOf(editor, pid(0))).getByRole("menuitem", { name: t("block.duplicate") }));
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Two", "paragraph:One", "paragraph:Two", "paragraph:Three"]);
    const ids = idsOf(editor.state.doc);
    expect([ids[0], ids[1], ids[4]]).toEqual([pid(0), pid(1), pid(2)]);
    expect(ids[2]).toMatch(UUID);
    expect(ids[3]).toMatch(UUID);
    expect(ids[2]).not.toBe(ids[3]);
    expect(span(editor.state.selection)).toEqual([2, 3]); // the copies are selected
  });

  it("moves the range up and down together, and disables a move past the edge", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    select(editor, 1, 2);

    await user.click(within(await menuOf(editor, pid(1))).getByRole("menuitem", { name: t("block.moveUp") }));
    expect(idsOf(editor.state.doc)).toEqual([pid(1), pid(2), pid(0), pid(3)]);
    expect(span(editor.state.selection)).toEqual([0, 1]);

    const top = await menuOf(editor, pid(2)); // the selected blocks are at the top now: Up is disabled
    expect(within(top).getByRole("menuitem", { name: t("block.moveUp") })).toBeDisabled();
    await user.click(within(top).getByRole("menuitem", { name: t("block.moveDown") }));
    expect(idsOf(editor.state.doc)).toEqual([pid(0), pid(1), pid(2), pid(3)]);
    expect(span(editor.state.selection)).toEqual([1, 2]);
  });

  it("turns each block of the range into the chosen kind", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] },
    ]);
    const editor = await openPage(user, "Notes");
    select(editor, 0, 1);

    await user.click(within(await menuOf(editor, pid(1))).getByRole("menuitem", { name: t("block.turnInto") }));
    const kinds = await screen.findByRole("menu", { name: t("block.turnInto") });
    expect(within(kinds).getByRole("menuitem", { name: "Text" })).toBeDisabled(); // both are text already
    await user.click(within(kinds).getByRole("menuitem", { name: "Heading 1" }));
    expect(outline(editor.state.doc)).toEqual(["heading1:One", "heading1:Two", "paragraph:Three"]);
    expect(idsOf(editor.state.doc)).toEqual([pid(0), pid(1), pid(2)]);
    expect(span(editor.state.selection)).toEqual([0, 1]);
  });

  it("clicking the grip of a block outside the range selects that block alone", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] }]);
    const editor = await openPage(user, "Notes");
    select(editor, 0, 1);

    const menu = await menuOf(editor, pid(2));
    expect(span(editor.state.selection)).toEqual([2, 2]);
    expect(within(menu).getByRole("menuitem", { name: t("block.copyLink") })).toBeEnabled();
  });

  it("Escape, then Backspace after a grip click works on the block (the editor has the keyboard)", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] }]);
    const editor = await openPage(user, "Notes");
    await menuOf(editor, pid(1));
    await closeMenu();

    expect(fireEvent.keyDown(editor.view.dom, { key: "Backspace" })).toBe(false);
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Three"]);
  });

  it("undoes a bulk action in one step", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    select(editor, 1, 2);
    await user.click(within(await menuOf(editor, pid(1))).getByRole("menuitem", { name: t("block.delete") }));
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Four"]);

    act(() => {
      expect(undo(editor.state, editor.view.dispatch)).toBe(true);
    });
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Two", "paragraph:Three", "paragraph:Four"]);
    expect(idsOf(editor.state.doc)).toEqual([pid(0), pid(1), pid(2), pid(3)]);
    act(() => {
      expect(redo(editor.state, editor.view.dispatch)).toBe(true);
    });
    expect(outline(editor.state.doc)).toEqual(["paragraph:One", "paragraph:Four"]);
  });
});

// ---------------------------------------------------------------------------------------------
// Clipboard

describe("copy, cut and paste of a range", () => {
  it("copies the whole blocks (HTML with their structure, plain text with a blank line between)", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1, 2)));
    const copied = copyOrCut(editor, "copy");

    expect(copied.text).toBe("two\n\nthree");
    expect(copied.html).toContain("<p");
    expect(copied.html).toContain("two");
    expect(copied.html).toContain("three");
    expect(copied.html).not.toContain("one");
    expect(copied.html).toMatch(/data-pm-slice="0 0/); // whole blocks: nothing open at either end
    expect(outline(editor.state.doc)).toEqual(["paragraph:one", "paragraph:two", "paragraph:three", "paragraph:four"]);
  });

  it("cut copies and removes the range", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1, 2)));
    const cut = copyOrCut(editor, "cut");

    expect(cut.text).toBe("two\n\nthree");
    await waitFor(() => {
      expect(outline(editor.state.doc)).toEqual(["paragraph:one", "paragraph:four"]);
    });
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
  });

  it("pastes a copied range back as the same blocks with new ids", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1, 2)));
    const copied = copyOrCut(editor, "copy");

    caret(editor, editor.state.doc.content.size - 1); // the end of "four"
    editor.view.dom.dispatchEvent(pasteEvent(copied.html, copied.text));

    expect(outline(editor.state.doc)).toEqual(["paragraph:one", "paragraph:two", "paragraph:three", "paragraph:four", "paragraph:two", "paragraph:three"]);
    const ids = idsOf(editor.state.doc);
    expect(ids.slice(0, 4)).toEqual([pid(0), pid(1), pid(2), pid(3)]); // the originals keep theirs
    expect(ids[4]).toMatch(UUID);
    expect(ids[5]).toMatch(UUID);
    expect(new Set(ids).size).toBe(6); // the copies have ids of their own
    editor.state.doc.check();
  });

  it("pastes over a selected range, replacing it", async () => {
    const { editor } = await mount("one", "two", "three", "four");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0, 0)));
    const copied = copyOrCut(editor, "copy");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 2, 3)));
    editor.view.dom.dispatchEvent(pasteEvent(copied.html, copied.text));

    expect(outline(editor.state.doc)).toEqual(["paragraph:one", "paragraph:two", "paragraph:one"]);
    expect(editor.state.doc.child(2).attrs.id).toMatch(UUID);
    expect(editor.state.doc.child(2).attrs.id).not.toBe(pid(0));
  });
});

// ---------------------------------------------------------------------------------------------
// Drag

describe("dragging a range", () => {
  async function gripOf(editor: Editor, id: string): Promise<HTMLElement> {
    const block = editor.view.dom.querySelector(`[data-id='${id}']`);
    if (block === null) {
      throw new Error(`no block ${id}`);
    }
    fireEvent.mouseMove(block);
    return screen.findByRole("button", { name: t("block.actions") });
  }

  it("carries the whole range as one slice, a move that keeps the ids, with text that says how many", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const selection = BlockRangeSelection.create(editor.state.doc, 1, 2);
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(selection));
    });

    const data = new FakeDataTransfer();
    const setDragImage = vi.spyOn(data, "setDragImage");
    fireEvent.dragStart(await gripOf(editor, pid(2)), { dataTransfer: data });

    const { dragging } = editor.view;
    expect(dragging?.move).toBe(true);
    expect(dragging?.slice.content.childCount).toBe(2);
    expect(dragging?.slice.content.child(0).attrs.id).toBe(pid(1));
    expect(dragging?.slice.content.child(1).attrs.id).toBe(pid(2));
    expect([dragging?.slice.openStart, dragging?.slice.openEnd]).toEqual([0, 0]);
    expect(editor.state.selection.eq(selection)).toBe(true); // the drop deletes exactly this range
    expect(data.effectAllowed).toBe("move");
    expect(data.getData("text/html")).toContain("Two");
    expect(data.getData("text/html")).toContain("Three");
    expect(data.getData("text/plain")).toBe("Two\n\nThree");
    expect(setDragImage).toHaveBeenCalledTimes(1);
    const [image] = setDragImage.mock.calls[0] as unknown as [HTMLElement];
    expect(image.textContent).toBe("2 blocks");
  });

  it("a drop moves the range to where it lands, keeping the ids, in one transaction", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three", "Four") }] },
    ]);
    const editor = await openPage(user, "Notes");
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0, 1)));
    });
    const data = new FakeDataTransfer();
    fireEvent.dragStart(await gripOf(editor, pid(1)), { dataTransfer: data });

    // jsdom has no layout: say where the pointer is (the end of the page).
    vi.spyOn(editor.view, "posAtCoords").mockReturnValue({ pos: editor.state.doc.content.size, inside: -1 });
    let transactions = 0;
    editor.on("transaction", () => {
      transactions += 1;
    });
    act(() => {
      fireEvent.drop(editor.view.dom, { dataTransfer: data, clientX: 4, clientY: 4 });
    });

    expect(outline(editor.state.doc)).toEqual(["paragraph:Three", "paragraph:Four", "paragraph:One", "paragraph:Two"]);
    expect(idsOf(editor.state.doc)).toEqual([pid(2), pid(3), pid(0), pid(1)]); // moved, not copied
    expect(transactions).toBeGreaterThan(0);
    expect(editor.view.dragging).toBeNull();
  });

  it("the handle of a block outside the range drags that block alone, as before", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] },
    ]);
    const editor = await openPage(user, "Notes");
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 0, 1)));
    });
    const data = new FakeDataTransfer();
    fireEvent.dragStart(await gripOf(editor, pid(2)), { dataTransfer: data });

    expect(editor.state.selection).toBeInstanceOf(NodeSelection);
    expect(editor.view.dragging?.slice.content.childCount).toBe(1);
    expect(editor.view.dragging?.slice.content.child(0).attrs.id).toBe(pid(2));
  });

  it("startBlockDrag labels one selected block \"1 block\"", async () => {
    const { editor } = await mount("one", "two");
    editor.view.dispatch(editor.state.tr.setSelection(BlockRangeSelection.create(editor.state.doc, 1)));
    const data = new FakeDataTransfer();
    const setDragImage = vi.spyOn(data, "setDragImage");
    startBlockDrag(editor.view, 5, data as unknown as DataTransfer);
    const [image] = setDragImage.mock.calls[0] as unknown as [HTMLElement];
    expect(image.textContent).toBe("1 block");
    expect(editor.view.dragging?.slice.content.childCount).toBe(1);
  });
});

// ---------------------------------------------------------------------------------------------
// The Loro binding

describe("a bulk action and the Loro binding", () => {
  /** A plain view bound to `doc`, with an undo manager that never merges two actions into one step. */
  async function bound(texts: string[]) {
    const doc = loroDocFrom(paragraphs(...texts));
    const undoManager = new UndoManager(doc, { mergeInterval: 0 });
    const state = EditorState.create({
      schema,
      plugins: [LoroSyncPlugin({ doc: doc as LoroDocType }), LoroUndoPlugin({ doc, undoManager })],
    });
    const view = new EditorView(document.createElement("div"), { state });
    await tick();
    const updates: Uint8Array[] = [];
    const unsubscribe = doc.subscribeLocalUpdates((bytes) => {
      updates.push(bytes);
    });
    return {
      doc,
      view,
      undoManager,
      updates,
      portable: (): JsonValue => loroToPortable(doc),
      destroy: () => {
        unsubscribe();
        view.destroy();
        undoManager.free();
      },
    };
  }

  type Action = (state: EditorState, selectionRange: ReturnType<typeof range>) => ReturnType<typeof deleteBlocks>;
  const actions: [string, Action][] = [
    ["delete", (state, r) => deleteBlocks(state, r)],
    ["duplicate", (state, r) => duplicateBlocks(state, r)],
    ["move up", (state, r) => moveBlocks(state, r, -1)],
    ["move down", (state, r) => moveBlocks(state, r, 1)],
    ["turn into", (state, r) => turnBlocksInto(state, r, "heading2")],
  ];

  for (const [name, action] of actions) {
    it(`records one update for "${name}" on a range, and one undo reverses it`, async () => {
      const handle = await bound(["one", "two", "three", "four", "five"]);
      try {
        const { view, doc } = handle;
        const before = handle.portable();
        view.dispatch(view.state.tr.setSelection(BlockRangeSelection.create(view.state.doc, 1, 2)));
        expect(handle.updates).toHaveLength(0); // a selection writes nothing

        const tr = action(view.state, range(view.state));
        expect(tr).not.toBeNull();
        view.dispatch(tr ?? view.state.tr);

        expect(handle.updates).toHaveLength(1); // one commit, however many blocks it touched
        expect(handle.portable()).not.toEqual(before);
        expect(loroToPortable(doc)).toEqual(handle.portable());

        expect(handle.undoManager.canUndo()).toBe(true);
        expect(undo(view.state, view.dispatch)).toBe(true);
        expect(handle.portable()).toEqual(before); // the whole action, in one step
        expect(handle.undoManager.canUndo()).toBe(false);

        expect(redo(view.state, view.dispatch)).toBe(true);
        expect(handle.portable()).not.toEqual(before);
      } finally {
        handle.destroy();
      }
    });
  }

  it("keeps two actions apart as two undo steps", async () => {
    const handle = await bound(["one", "two", "three", "four"]);
    try {
      const { view } = handle;
      const original = handle.portable();
      view.dispatch(view.state.tr.setSelection(BlockRangeSelection.create(view.state.doc, 0, 1)));
      view.dispatch(moveBlocks(view.state, range(view.state), 1) ?? view.state.tr);
      const afterMove = handle.portable();
      view.dispatch(deleteBlocks(view.state, range(view.state)) ?? view.state.tr);
      expect(handle.updates).toHaveLength(2);

      undo(view.state, view.dispatch);
      expect(handle.portable()).toEqual(afterMove);
      undo(view.state, view.dispatch);
      expect(handle.portable()).toEqual(original);
    } finally {
      handle.destroy();
    }
  });

  it("writes the same document the view shows, with a range selected while a remote change comes in", async () => {
    const handle = await bound(["one", "two", "three", "four"]);
    const remote = new LoroDoc();
    remote.import(handle.doc.export({ mode: "snapshot" }));
    const remoteHandle = new UndoManager(remote, {});
    const remoteState = EditorState.create({ schema, plugins: [LoroSyncPlugin({ doc: remote as LoroDocType })] });
    const remoteView = new EditorView(document.createElement("div"), { state: remoteState });
    await tick();
    try {
      const { view } = handle;
      view.dispatch(view.state.tr.setSelection(BlockRangeSelection.create(view.state.doc, 1, 2)));

      // The other replica types in the first block and deletes the last one.
      remoteView.dispatch(remoteView.state.tr.insertText("!", 4));
      remoteView.dispatch(remoteView.state.tr.delete(remoteView.state.doc.content.size - 6, remoteView.state.doc.content.size));
      handle.doc.import(remote.export({ mode: "update", from: handle.doc.oplogVersion() }));

      expect(outline(view.state.doc)).toEqual(["paragraph:one!", "paragraph:two", "paragraph:three"]);
      expect(span(view.state.selection)).toEqual([1, 2]); // the range stays on its blocks
    } finally {
      remoteView.destroy();
      remoteHandle.free();
      handle.destroy();
    }
  });
});
