// "Sort list A-Z" (ADR-0052): the innermost list, every kind of item, the subtree that travels, the ids that stay,
// the caret that follows, nothing to do giving no transaction, and one undo step through the Loro binding.

import type { JsonValue } from "@atomik-kb/core-client";
import type { Node as PmNode } from "@tiptap/pm/model";
import { EditorState, TextSelection } from "@tiptap/pm/state";
import { afterEach, describe, expect, it } from "vitest";

import { bindView, loroDocFrom, pid, tick } from "../test/editor";
import type { BoundView } from "../test/editor";
import { innermostListPos, isListNode, itemKey, sortListIn, sortListTransaction, sortedItemOrder } from "./list-sort";
import { redo, undo } from "./loro-binding";
import { createEditorSchema } from "./schema";

const schema = createEditorSchema();

type Json = { [key: string]: JsonValue };

let ids = 0;
/** A fresh block id. */
function id(): string {
  ids += 1;
  return pid(1000 + ids);
}

function text(value: string): JsonValue[] {
  return value === "" ? [] : [{ type: "text", text: value }];
}

function paragraph(value: string): Json {
  return { type: "paragraph", attrs: { id: id() }, content: text(value) };
}

/** A list item (or a task item when `checked` is given) with its text and what is nested in it. */
function item(value: string, nested: Json[] = [], checked?: boolean): Json {
  return checked === undefined
    ? { type: "listItem", attrs: { id: id() }, content: [paragraph(value), ...nested] }
    : { type: "taskItem", attrs: { id: id(), checked }, content: [paragraph(value), ...nested] };
}

function list(type: "bulletList" | "orderedList" | "taskList", items: Json[]): Json {
  return { type, attrs: { id: id() }, content: items };
}

function bullets(...values: string[]): Json {
  return list("bulletList", values.map((value) => item(value)));
}

function docOf(...blocks: Json[]): PmNode {
  return schema.nodeFromJSON({ type: "doc", content: blocks });
}

/** The texts of the items of the list that starts at `pos`. */
function itemTexts(doc: PmNode, pos: number): string[] {
  const out: string[] = [];
  doc.nodeAt(pos)?.forEach((child) => {
    out.push(itemKey(child));
  });
  return out;
}

/** The position inside the text `value` (its first paragraph in the document), `offset` characters in. */
function posIn(doc: PmNode, value: string, offset = 0): number {
  let found = -1;
  doc.descendants((node, pos) => {
    if (found === -1 && node.type.name === "paragraph" && node.textContent === value) {
      found = pos + 1 + offset;
    }
    return found === -1;
  });
  if (found === -1) {
    throw new Error(`no paragraph "${value}"`);
  }
  return found;
}

function stateAt(doc: PmNode, pos: number): EditorState {
  return EditorState.create({ schema, doc, selection: TextSelection.create(doc, pos) });
}

/** Sorts the list around the caret and answers the new state (`null` for no transaction). */
function sorted(doc: PmNode, pos: number): EditorState | null {
  const state = stateAt(doc, pos);
  const tr = sortListTransaction(state);
  return tr === null ? null : state.apply(tr);
}

describe("which list and what order", () => {
  it("finds the innermost list around a position, and none outside a list", () => {
    const doc = docOf(
      paragraph("before"),
      list("bulletList", [item("a", [bullets("a1", "a2")]), item("b")]),
      paragraph("after"),
    );
    const outer = innermostListPos(doc, posIn(doc, "b"));
    const inner = innermostListPos(doc, posIn(doc, "a2"));
    expect(outer).not.toBeNull();
    expect(inner).not.toBeNull();
    expect(inner).not.toBe(outer);
    expect(isListNode(doc.nodeAt(outer ?? 0))).toBe(true);
    // The caret in the text of an item that has a nested list is in the outer list.
    expect(innermostListPos(doc, posIn(doc, "a"))).toBe(outer);
    expect(innermostListPos(doc, posIn(doc, "before"))).toBeNull();
    expect(innermostListPos(doc, posIn(doc, "after"))).toBeNull();
  });

  it("sorts by the text of the first paragraph: numbers by value, case and accents ignored, empty last", () => {
    const doc = docOf(bullets("Item 10", "item 2", "école", "Zoe", "", "Dog", "Item 1"));
    const state = sorted(doc, posIn(doc, "Zoe"));
    expect(state).not.toBeNull();
    expect(itemTexts(state?.doc ?? doc, 0)).toEqual(["Dog", "école", "Item 1", "item 2", "Item 10", "Zoe", ""]);
  });

  it("is stable: items with the same key keep the order they had", () => {
    const order = sortedItemOrder(docOf(bullets("b", "a", "B", "A")).child(0));
    expect(order).toEqual([1, 3, 0, 2]);
  });

  it("gives nothing to do for a list that is in order, has one item, or none", () => {
    expect(sortedItemOrder(docOf(bullets("a", "B", "c")).child(0))).toBeNull();
    expect(sortedItemOrder(docOf(bullets("only")).child(0))).toBeNull();
    expect(sortedItemOrder(docOf(bullets("x", "")).child(0))).toBeNull(); // the empty item is last already
    const doc = docOf(bullets("a", "b"), paragraph("text"));
    expect(sortListTransaction(stateAt(doc, posIn(doc, "a")))).toBeNull();
    expect(sortListTransaction(stateAt(doc, posIn(doc, "text")))).toBeNull(); // not in a list
  });
});

describe("what moves with an item", () => {
  it("sorts a bullet, a numbered and a to-do list, and keeps the checked state with its item", () => {
    const bullet = docOf(bullets("c", "a", "b"));
    expect(itemTexts(sorted(bullet, posIn(bullet, "a"))?.doc ?? bullet, 0)).toEqual(["a", "b", "c"]);

    const ordered = docOf(list("orderedList", [item("pear"), item("apple"), item("fig")]));
    expect(itemTexts(sorted(ordered, posIn(ordered, "fig"))?.doc ?? ordered, 0)).toEqual(["apple", "fig", "pear"]);

    const tasks = docOf(
      list("taskList", [item("write", [], true), item("buy", [], false), item("call", [], true), item("dream", [], false)]),
    );
    const done = sorted(tasks, posIn(tasks, "buy"))?.doc ?? tasks;
    const rows: [string, boolean][] = [];
    done.child(0).forEach((child) => {
      rows.push([itemKey(child), child.attrs.checked === true]);
    });
    expect(rows).toEqual([
      ["buy", false],
      ["call", true],
      ["dream", false],
      ["write", true],
    ]);
    expect(done.child(0).type.name).toBe("taskList");
  });

  it("moves an item with its nested lists, formatting and ids, and nested lists keep their own order", () => {
    const bold: Json = {
      type: "paragraph",
      attrs: { id: id() },
      content: [{ type: "text", text: "beta", marks: [{ type: "bold" }] }, { type: "text", text: " rest" }],
    };
    const beta: Json = { type: "listItem", attrs: { id: id() }, content: [bold, bullets("z", "y", "x")] };
    const alpha = item("alpha", [bullets("n2", "n1")]);
    const doc = docOf(list("bulletList", [beta, alpha]));
    const state = sorted(doc, posIn(doc, "alpha"));
    expect(state).not.toBeNull();
    const result = (state?.doc ?? doc).child(0);

    // The items are the very nodes they were, only in the other order: ids, marks, nested lists and all.
    expect(result.child(0)).toBe(doc.child(0).child(1));
    expect(result.child(1)).toBe(doc.child(0).child(0));
    expect(result.child(0).attrs.id).toBe(doc.child(0).child(1).attrs.id);
    expect(result.child(1).firstChild?.textContent).toBe("beta rest");
    expect(result.child(1).firstChild?.firstChild?.marks.map((mark) => mark.type.name)).toEqual(["bold"]);

    // The nested lists were not sorted: z, y, x stays z, y, x.
    const nestedTexts = (index: number): string[] => {
      const out: string[] = [];
      result.child(index).child(1).forEach((entry) => {
        out.push(itemKey(entry));
      });
      return out;
    };
    expect(nestedTexts(0)).toEqual(["n2", "n1"]);
    expect(nestedTexts(1)).toEqual(["z", "y", "x"]);

    // Every block id of the page is still there once.
    const idsOf = (node: PmNode): string[] => {
      const found: string[] = [];
      node.descendants((child) => {
        if (typeof child.attrs.id === "string") {
          found.push(child.attrs.id);
        }
        return true;
      });
      return found.sort();
    };
    expect(idsOf(state?.doc ?? doc)).toEqual(idsOf(doc));
  });

  it("sorts only the innermost list: the caret in a nested list leaves the outer one alone", () => {
    const doc = docOf(list("bulletList", [item("b", [bullets("z", "y")]), item("a")]));
    const state = sorted(doc, posIn(doc, "y"));
    const outer = (state?.doc ?? doc).child(0);
    expect([itemKey(outer.child(0)), itemKey(outer.child(1))]).toEqual(["b", "a"]);
    const nested: string[] = [];
    outer.child(0).child(1).forEach((entry) => {
      nested.push(itemKey(entry));
    });
    expect(nested).toEqual(["y", "z"]);
  });

  it("leaves other lists and the rest of the page untouched", () => {
    const first = bullets("b", "a");
    const second = bullets("d", "c");
    const doc = docOf(paragraph("intro"), first, paragraph("between"), second, paragraph("outro"));
    const state = sorted(doc, posIn(doc, "a"));
    expect(state).not.toBeNull();
    const result = state?.doc ?? doc;
    expect(result.childCount).toBe(5);
    expect(itemTexts(result, result.child(0).nodeSize)).toEqual(["a", "b"]);
    // The nodes of everything else are the very objects they were.
    expect(result.child(0)).toBe(doc.child(0));
    expect(result.child(2)).toBe(doc.child(2));
    expect(result.child(3)).toBe(doc.child(3));
    expect(result.child(4)).toBe(doc.child(4));
  });
});

describe("the caret", () => {
  it("goes back into the item it was in, at the same place in its text", () => {
    const doc = docOf(bullets("cherry", "banana", "apple"));
    const state = sorted(doc, posIn(doc, "banana", 3));
    expect(state).not.toBeNull();
    const { selection } = state ?? stateAt(doc, 1);
    expect(selection.empty).toBe(true);
    expect(selection.$from.parent.textContent).toBe("banana");
    expect(selection.$from.parentOffset).toBe(3);
  });

  it("keeps a selection inside one item, and the caret in a nested item that moved", () => {
    const doc = docOf(list("bulletList", [item("b", [bullets("y", "x")]), item("a")]));
    const wanted = TextSelection.create(doc, posIn(doc, "b", 0), posIn(doc, "b", 1));
    const outerState = EditorState.create({ schema, doc, selection: wanted });
    const next = outerState.apply(sortListTransaction(outerState) ?? outerState.tr);
    expect(next.selection.$from.parent.textContent).toBe("b");
    expect(next.selection.from).toBeLessThan(next.selection.to);
    expect(next.doc.textBetween(next.selection.from, next.selection.to)).toBe("b");

    // The caret in "y", which is inside the item "b": after the outer list is sorted it is still in "y".
    const inner = stateAt(doc, posIn(doc, "y", 1));
    const outerTr = inner.tr;
    const outerPos = innermostListPos(inner.doc, 1);
    expect(outerPos).not.toBeNull();
    expect(sortListIn(outerTr, outerPos ?? 0)).toBe(true);
    const after = inner.apply(outerTr);
    expect(after.selection.$from.parent.textContent).toBe("y");
    expect(after.selection.$from.parentOffset).toBe(1);
  });
});

describe("through the Loro binding", () => {
  const views: BoundView[] = [];
  afterEach(() => {
    for (const handle of views.splice(0)) {
      handle.destroy();
    }
  });

  it("is one transaction, and one undo restores the order and one redo sorts again", async () => {
    const json: JsonValue = {
      type: "doc",
      content: [
        paragraph("intro"),
        list("taskList", [item("cherry", [], true), item("apple", [], false), item("banana", [], true)]),
      ],
    };
    const handle = bindView(schema, loroDocFrom(json));
    views.push(handle);
    const { view } = handle;
    await tick();
    const original = JSON.stringify(view.state.doc.toJSON());
    const pos = posIn(view.state.doc, "apple", 2);
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, pos)));

    const tr = sortListTransaction(view.state);
    expect(tr).not.toBeNull();
    expect(tr?.steps.length).toBe(1); // the list's content replaced once
    view.dispatch(tr ?? view.state.tr);
    const sortedJson = JSON.stringify(view.state.doc.toJSON());
    expect(sortedJson).not.toBe(original);
    expect(itemTexts(view.state.doc, view.state.doc.child(0).nodeSize)).toEqual(["apple", "banana", "cherry"]);

    expect(undo(view.state, view.dispatch)).toBe(true);
    expect(JSON.stringify(view.state.doc.toJSON())).toBe(original);
    expect(redo(view.state, view.dispatch)).toBe(true);
    expect(JSON.stringify(view.state.doc.toJSON())).toBe(sortedJson);
  });
});
