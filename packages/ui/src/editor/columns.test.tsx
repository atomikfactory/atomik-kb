// The columns layout in the editor (ADR-0016, amendment of 2026-10-06): the schema against
// the fixture, the commands on block schema v1 (every node they create gets a block id from
// UniqueID, every block they move keeps its id and the caret stays where it was), the
// no-nesting rule (the commands, the `/` entries and the transaction filter), what the block
// menu does with a layout, the `/columns` entries and the toolbar in the page editor, and
// two bound replicas editing different columns.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { loroDocFrom, paragraphs, pid, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { duplicateBlock, isColumnList, turnInto, turnIntoKinds } from "./blocks";
import { columnsUnavailable, MAX_COLUMNS } from "./columns";
import { LoroBinding } from "./extensions";
import { loroSyncPluginKey, redo, undo } from "./loro-binding";
import { createEditorSchema, createSchemaExtensions, ID_BEARING_NODES } from "./schema";
import { SLASH_COMMANDS } from "./slash-commands";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/**
 * A Tiptap editor over the schema extensions (no React): ids come from UniqueID. With `doc`
 * it is bound to that Loro document (and shows it once `tick()` has passed).
 */
function mount(content: JsonValue, doc: LoroDoc | null = null): Editor {
  const element = document.createElement("div");
  document.body.append(element);
  const extensions = createSchemaExtensions();
  if (doc !== null) {
    extensions.push(LoroBinding.configure({ doc, readOnly: false }));
  }
  const editor = new Editor({ element, extensions, content: content as JSONContent });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
  vi.restoreAllMocks();
});

// ----- documents ---------------------------------------------------------------------------

let nextId = 100;

/** A paragraph with `text` (none when empty) and an id of its own. */
function p(text: string): JsonValue {
  return {
    type: "paragraph",
    attrs: { id: pid(nextId++) },
    ...(text === "" ? {} : { content: [{ type: "text", text }] }),
  };
}

function col(...blocks: JsonValue[]): JsonValue {
  return { type: "column", attrs: { id: pid(nextId++) }, content: blocks };
}

function cols(...columns: JsonValue[]): JsonValue {
  return { type: "columnList", attrs: { id: pid(nextId++) }, content: columns };
}

function wrap(type: string, ...blocks: JsonValue[]): JsonValue {
  return { type, attrs: { id: pid(nextId++) }, content: blocks };
}

function page(...blocks: JsonValue[]): JsonValue {
  return { type: "doc", content: blocks };
}

/** `Before`, a layout of three columns (`A1 A2`, `B1`, `C1`) and `After`. */
function layout(): JsonValue {
  return page(p("Before"), cols(col(p("A1"), p("A2")), col(p("B1")), col(p("C1"))), p("After"));
}

/** The same with two columns (`A1`, `B1`). */
function twoColumns(): JsonValue {
  return page(p("Before"), cols(col(p("A1")), col(p("B1"))), p("After"));
}

// ----- looking at an editor --------------------------------------------------------------------

const SHORT: Record<string, string> = { paragraph: "p", columnList: "cols", column: "col" };

function shape(node: PmNode): string {
  const name = SHORT[node.type.name] ?? node.type.name;
  if (node.isTextblock) {
    return node.textContent === "" ? name : `${name}:${node.textContent}`;
  }
  const children: string[] = [];
  node.forEach((child) => {
    children.push(shape(child));
  });
  return `${name}(${children.join(" ")})`;
}

/** The top-level blocks of the page as a compact string: `p:Before cols(col(p:A1) col(p)) p:After`. */
function outline(editor: Editor): string {
  const blocks: string[] = [];
  editor.state.doc.forEach((node) => {
    blocks.push(shape(node));
  });
  return blocks.join(" ");
}

/** The ids of every node that carries one, in document order. */
function blockIds(editor: Editor): string[] {
  const ids: string[] = [];
  editor.state.doc.descendants((node) => {
    if (ID_BEARING_NODES.includes(node.type.name)) {
      ids.push(String(node.attrs.id));
    }
  });
  return ids;
}

/** Every id-bearing node has a UUID and no two share one. */
function expectUniqueIds(editor: Editor): void {
  const ids = blockIds(editor);
  for (const id of ids) {
    expect(id).toMatch(UUID);
  }
  expect(new Set(ids).size).toBe(ids.length);
}

/** The id of the first node named `name`. */
function idOfFirst(editor: Editor, name: string): string {
  let found = "";
  editor.state.doc.descendants((node) => {
    if (found === "" && node.type.name === name) {
      found = String(node.attrs.id);
    }
    return found === "";
  });
  return found;
}

/** Where the text of the first textblock holding exactly `text` starts. */
function textStart(editor: Editor, text: string): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found === -1 && node.isTextblock && node.textContent === text) {
      found = pos + 1;
    }
    return found === -1 && !node.isTextblock;
  });
  if (found === -1) {
    throw new Error(`no block holding ${text}`);
  }
  return found;
}

/** Puts the caret in the block holding `text`, `offset` characters in. */
function caretAt(editor: Editor, text: string, offset = 0): void {
  editor.view.dispatch(
    editor.state.tr.setSelection(TextSelection.create(editor.state.doc, textStart(editor, text) + offset)),
  );
}

/** Where the first block of column `index` of the first column list starts (inside the block). */
function columnStart(editor: Editor, index: number): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found === -1 && node.type.name === "columnList") {
      let at = pos + 1;
      for (let i = 0; i < index; i += 1) {
        at += node.child(i).nodeSize;
      }
      found = at + 2; // inside the column, inside its first block
    }
    return found === -1;
  });
  if (found === -1) {
    throw new Error("no column list");
  }
  return found;
}

/** Puts the caret at the start of the first block of column `index`. */
function caretInColumn(editor: Editor, index: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, columnStart(editor, index))));
}

/** Where the caret is: the text of its block and how far in (`+` when something is selected). */
function caret(editor: Editor): string {
  const { $from, empty } = editor.state.selection;
  return `${$from.parent.textContent}@${String($from.parentOffset)}${empty ? "" : "+"}`;
}

// ----- the schema ------------------------------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("has the column list and the column of the fixture, each with a block id and nothing else", () => {
    expect(live.nodes.columnList).toEqual({
      content: "column{2,5}",
      group: "block",
      inline: false,
      atom: false,
      attrs: ["id"],
    });
    expect(live.nodes.column).toEqual({ content: "block+", group: null, inline: false, atom: false, attrs: ["id"] });
    expect(fixture.nodes.columnList).toEqual(live.nodes.columnList);
    expect(fixture.nodes.column).toEqual(live.nodes.column);
    expect(fixture.idBearing).toEqual(expect.arrayContaining(["columnList", "column"]));
    expect(ID_BEARING_NODES).toEqual(expect.arrayContaining(["columnList", "column"]));
  });

  it("holds two to five columns, and a column only inside a column list", () => {
    const schema = createEditorSchema();
    const list = schema.nodes.columnList;
    const column = schema.nodes.column;
    const paragraph = schema.nodes.paragraph;
    if (list === undefined || column === undefined || paragraph === undefined) {
      throw new Error("the schema lacks the column nodes");
    }
    const one = (): PmNode => column.create(null, paragraph.create());
    expect(list.validContent(list.createAndFill()?.content ?? list.create().content)).toBe(true);
    expect(list.createAndFill()?.childCount).toBe(2); // the default is the smallest valid list
    for (const count of [2, 3, 4, 5]) {
      expect(() => {
        list.createChecked(null, Array.from({ length: count }, one));
      }).not.toThrow();
    }
    for (const count of [0, 1, 6]) {
      expect(() => {
        list.createChecked(null, Array.from({ length: count }, one));
      }).toThrow();
    }
    // A column belongs to a column list only: not to the page, not to a quote.
    expect(schema.nodes.doc?.contentMatch.matchType(column)).toBeNull();
    expect(schema.nodes.blockquote?.contentMatch.matchType(column)).toBeNull();
  });
});

// ----- the commands -------------------------------------------------------------------------

describe("setColumns", () => {
  it("wraps the block with the caret into 2 columns: the block in the first, an empty paragraph in the other", () => {
    const editor = mount(page(p("One"), p("Two"), p("Three")));
    caretAt(editor, "Two", 2);
    const before = blockIds(editor);
    expect(editor.commands.setColumns(2)).toBe(true);
    expect(outline(editor)).toBe("p:One cols(col(p:Two) col(p)) p:Three");
    // The caret is still in the block it was in, at the same place.
    expect(caret(editor)).toBe("Two@2");
    // The block is the same block; the layout, its columns and the new paragraph have ids of their own.
    const after = blockIds(editor);
    expect(after).toHaveLength(before.length + 4);
    expect(before.every((id) => after.includes(id))).toBe(true);
    expectUniqueIds(editor);
  });

  it("wraps into 3 columns, and into 5", () => {
    const three = mount(page(p("One"), p("Two")));
    caretAt(three, "Two");
    expect(three.commands.setColumns(3)).toBe(true);
    expect(outline(three)).toBe("p:One cols(col(p:Two) col(p) col(p))");
    expectUniqueIds(three);
    expect(blockIds(three)).toHaveLength(2 + 1 + 3 + 2);

    const five = mount(page(p("One")));
    expect(five.commands.setColumns(MAX_COLUMNS)).toBe(true);
    expect(outline(five)).toBe("cols(col(p:One) col(p) col(p) col(p) col(p))");
    expectUniqueIds(five);
  });

  it("refuses any other count, and leaves the page as it is", () => {
    const editor = mount(page(p("One"), p("Two")));
    caretAt(editor, "Two");
    for (const count of [0, 1, 6, 2.5, Number.NaN]) {
      expect(editor.commands.setColumns(count), String(count)).toBe(false);
      expect(editor.can().setColumns(count), String(count)).toBe(false);
    }
    expect(outline(editor)).toBe("p:One p:Two");
    expect(editor.can().setColumns(2)).toBe(true);
    expect(outline(editor)).toBe("p:One p:Two"); // `can` changes nothing
  });

  it("wraps the whole top-level block the caret is in: a list, a callout", () => {
    const editor = mount(
      page(wrap("bulletList", wrap("listItem", p("x"), p("y"))), wrap("callout", p("note")), p("tail")),
    );
    caretAt(editor, "y", 1);
    const listId = idOfFirst(editor, "bulletList");
    expect(editor.commands.setColumns(2)).toBe(true);
    expect(outline(editor)).toBe("cols(col(bulletList(listItem(p:x p:y))) col(p)) callout(p:note) p:tail");
    expect(caret(editor)).toBe("y@1");
    expect(blockIds(editor)).toContain(listId);
    caretAt(editor, "note");
    expect(editor.commands.setColumns(3)).toBe(true);
    expect(outline(editor)).toBe(
      "cols(col(bulletList(listItem(p:x p:y))) col(p)) cols(col(callout(p:note)) col(p) col(p)) p:tail",
    );
    expectUniqueIds(editor);
  });

  it("keeps a selected block selected, and a selection inside one block", () => {
    const editor = mount(page(p("One"), { type: "horizontalRule", attrs: { id: pid(nextId++) } }, p("Two")));
    const rule = editor.state.doc.child(1);
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize)));
    expect(editor.commands.setColumns(2)).toBe(true);
    expect(outline(editor)).toBe("p:One cols(col(horizontalRule()) col(p)) p:Two");
    const { selection } = editor.state;
    expect(selection instanceof NodeSelection && selection.node.attrs.id === rule.attrs.id).toBe(true);

    caretAt(editor, "Two", 0);
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, textStart(editor, "Two"), textStart(editor, "Two") + 2)),
    );
    expect(editor.commands.setColumns(2)).toBe(true);
    expect(caret(editor)).toBe("Two@0+");
    expect(editor.state.selection.to - editor.state.selection.from).toBe(2);
  });
});

describe("addColumn", () => {
  it("appends an empty column, up to five, and leaves the caret where it was", () => {
    const editor = mount(twoColumns());
    caretAt(editor, "A1", 1);
    const before = blockIds(editor);
    expect(editor.commands.addColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1) col(p:B1) col(p)) p:After");
    expect(caret(editor)).toBe("A1@1");
    expect(blockIds(editor)).toHaveLength(before.length + 2); // the column and its paragraph
    expectUniqueIds(editor);
    expect(editor.commands.addColumn()).toBe(true);
    expect(editor.commands.addColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1) col(p:B1) col(p) col(p) col(p)) p:After");
    // Five is the most.
    expect(editor.can().addColumn()).toBe(false);
    expect(editor.commands.addColumn()).toBe(false);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1) col(p:B1) col(p) col(p) col(p)) p:After");
    expectUniqueIds(editor);
  });

  it("works on a selected layout too, and not outside one", () => {
    const editor = mount(twoColumns());
    caretAt(editor, "Before");
    expect(editor.can().addColumn()).toBe(false);
    expect(editor.commands.addColumn()).toBe(false);

    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize)));
    expect(editor.commands.addColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1) col(p:B1) col(p)) p:After");
    expect(editor.state.selection instanceof NodeSelection).toBe(true);
  });
});

describe("removeColumn", () => {
  it("moves the blocks of a column to the end of the previous one", () => {
    const editor = mount(layout());
    caretAt(editor, "B1", 1);
    const before = blockIds(editor);
    const previous = String(editor.state.doc.child(1).child(0).attrs.id);
    expect(editor.commands.removeColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p:C1)) p:After");
    expect(caret(editor)).toBe("B1@1");
    // Only the removed column's own id is gone: the blocks and the receiving column keep theirs.
    const after = blockIds(editor);
    expect(after).toHaveLength(before.length - 1);
    expect(before.filter((id) => !after.includes(id))).toHaveLength(1);
    expect(String(editor.state.doc.child(1).child(0).attrs.id)).toBe(previous);
    expectUniqueIds(editor);
  });

  it("moves the blocks of the last column to the end of the one before it", () => {
    const editor = mount(layout());
    caretAt(editor, "C1");
    expect(editor.commands.removeColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2) col(p:B1 p:C1)) p:After");
    expect(caret(editor)).toBe("C1@0");
  });

  it("moves the blocks of the first column to the start of the next one, keeping the reading order", () => {
    const editor = mount(layout());
    caretAt(editor, "A2", 1);
    const next = String(editor.state.doc.child(1).child(1).attrs.id);
    expect(editor.commands.removeColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p:C1)) p:After");
    expect(caret(editor)).toBe("A2@1");
    expect(String(editor.state.doc.child(1).child(0).attrs.id)).toBe(next);
    expectUniqueIds(editor);
  });

  it("unwraps the layout instead when only one column would remain", () => {
    const editor = mount(twoColumns());
    caretAt(editor, "B1", 1);
    const ids = blockIds(editor);
    expect(editor.commands.removeColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before p:A1 p:B1 p:After");
    expect(caret(editor)).toBe("B1@1");
    // The layout and its columns are gone, every block kept its id.
    expect(blockIds(editor)).toHaveLength(ids.length - 3);
    expectUniqueIds(editor);
  });

  it("works in a block inside a list inside a column, and not outside a column", () => {
    const editor = mount(
      page(p("Before"), cols(col(p("A1")), col(wrap("bulletList", wrap("listItem", p("deep")))), col(p("C1")))),
    );
    caretAt(editor, "deep", 2);
    expect(editor.commands.removeColumn()).toBe(true);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 bulletList(listItem(p:deep))) col(p:C1))");
    expect(caret(editor)).toBe("deep@2");
    caretAt(editor, "Before");
    expect(editor.can().removeColumn()).toBe(false);
    expect(editor.commands.removeColumn()).toBe(false);
  });
});

describe("unwrapColumns", () => {
  it("lifts every block out in column order, keeping their ids and the caret", () => {
    const editor = mount(layout());
    caretAt(editor, "B1", 1);
    const ids = blockIds(editor);
    expect(editor.commands.unwrapColumns()).toBe(true);
    expect(outline(editor)).toBe("p:Before p:A1 p:A2 p:B1 p:C1 p:After");
    expect(caret(editor)).toBe("B1@1");
    expect(blockIds(editor)).toHaveLength(ids.length - 4); // the list and its three columns
    expectUniqueIds(editor);
  });

  it("lifts the empty paragraphs of a fresh layout too, and works from a selected layout", () => {
    const editor = mount(page(p("One")));
    editor.commands.setColumns(3);
    expect(outline(editor)).toBe("cols(col(p:One) col(p) col(p))");
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    expect(editor.commands.unwrapColumns()).toBe(true);
    expect(outline(editor)).toBe("p:One p p");
    expectUniqueIds(editor);
    expect(editor.can().unwrapColumns()).toBe(false); // nothing to unwrap now
    expect(editor.commands.unwrapColumns()).toBe(false);
  });

  it("is undone and redone by the binding, ids and layout back", async () => {
    const errors = vi.spyOn(console, "error");
    const doc = loroDocFrom(layout());
    const editor = mount(page(p("")), doc);
    await tick();
    const original = outline(editor);
    const ids = blockIds(editor);
    caretAt(editor, "B1");
    expect(editor.commands.unwrapColumns()).toBe(true);
    expect(outline(editor)).toBe("p:Before p:A1 p:A2 p:B1 p:C1 p:After");
    expect(undo(editor.state, editor.view.dispatch)).toBe(true);
    expect(outline(editor)).toBe(original);
    expect(blockIds(editor)).toEqual(ids);
    expect(redo(editor.state, editor.view.dispatch)).toBe(true);
    expect(outline(editor)).toBe("p:Before p:A1 p:A2 p:B1 p:C1 p:After");
    expect(errors).not.toHaveBeenCalled();
  });
});

describe("blocks that stay what they are", () => {
  it("keeps the id of an empty block it wraps, which UniqueID would otherwise hand to the next column", () => {
    // UniqueID reads an empty block followed by a new one in a changed range as a split and
    // gives the id to the second: wrapping must leave the block out of the range.
    const empty = mount(page(p("One"), p("")));
    const rule = mount(page(p("One"), { type: "horizontalRule", attrs: { id: pid(nextId++) } }));
    for (const [editor, name] of [
      [empty, "paragraph"],
      [rule, "horizontalRule"],
    ] as const) {
      let pos = 0;
      editor.state.doc.forEach((node, offset) => {
        pos = node.type.name === name ? offset : pos;
      });
      const id = String(editor.state.doc.nodeAt(pos)?.attrs.id);
      editor.view.dispatch(
        editor.state.tr.setSelection(
          name === "paragraph"
            ? TextSelection.create(editor.state.doc, pos + 1)
            : NodeSelection.create(editor.state.doc, pos),
        ),
      );
      expect(editor.commands.setColumns(3), name).toBe(true);
      const wrapped = editor.state.doc.child(1).child(0).child(0);
      expect(wrapped.type.name, name).toBe(name);
      expect(String(wrapped.attrs.id), name).toBe(id);
      expectUniqueIds(editor);
    }
  });
});

describe("no column list inside a column", () => {
  /** Where a column list could be inserted at the end of column `index` of the first layout. */
  function endOfColumn(editor: Editor, index: number): number {
    let found = -1;
    editor.state.doc.descendants((node, pos) => {
      if (found === -1 && node.type.name === "columnList") {
        let at = pos + 1;
        for (let i = 0; i < index; i += 1) {
          at += node.child(i).nodeSize;
        }
        found = at + 1 + node.child(index).content.size;
      }
      return found === -1;
    });
    return found;
  }

  function newList(editor: Editor): PmNode {
    const list = editor.schema.nodes.columnList?.createAndFill();
    if (list === null || list === undefined) {
      throw new Error("no column list");
    }
    return list;
  }

  const fromTheBinding = { type: "non-local-updates" };

  it("makes the commands refuse inside a column and for a block that holds a layout", () => {
    const editor = mount(page(p("Top"), cols(col(p("A1")), col(p("B1"))), wrap("callout", p("note"))));
    caretAt(editor, "A1");
    expect(editor.can().setColumns(2)).toBe(false);
    expect(editor.commands.setColumns(2)).toBe(false);
    expect(editor.commands.setColumns(3)).toBe(false);
    expect(outline(editor)).toBe("p:Top cols(col(p:A1) col(p:B1)) callout(p:note)");

    // A callout around a layout is a block that holds one: it cannot go into a column either.
    const around = mount(page(p("Top"), wrap("callout", cols(col(p("X")), col(p("Y"))))));
    caretAt(around, "Y");
    expect(around.commands.setColumns(2)).toBe(false);
    expect(columnsUnavailable(around)).toBe("slash.columns.inColumn");
    // The layout in the callout can still be edited with the other commands.
    expect(around.commands.addColumn()).toBe(true);
    expect(outline(around)).toBe("p:Top callout(cols(col(p:X) col(p:Y) col(p)))");
    expect(around.commands.unwrapColumns()).toBe(true);
    expect(outline(around)).toBe("p:Top callout(p:X p:Y p)");
  });

  it("makes /columns unavailable inside a column and inside a table cell", () => {
    const entries = SLASH_COMMANDS.filter((command) => command.id.startsWith("columns"));
    expect(entries.map((command) => command.id)).toEqual(["columns2", "columns3"]);
    const cell = wrap("tableCell", p("cell"));
    const editor = mount(
      page(p("Top"), cols(col(p("A1")), col(p("B1"))), wrap("table", wrap("tableRow", cell)), wrap("callout", p("note"))),
    );
    for (const command of entries) {
      caretAt(editor, "Top");
      expect(command.unavailable?.(editor), command.id).toBeNull();
      caretAt(editor, "note");
      expect(command.unavailable?.(editor), command.id).toBeNull();
      caretAt(editor, "B1");
      expect(command.unavailable?.(editor), command.id).toBe("slash.columns.inColumn");
      caretAt(editor, "cell");
      expect(command.unavailable?.(editor), command.id).toBe("slash.columns.inCell");
    }
    expect(t("slash.columns.inColumn")).not.toBe("");
    expect(t("slash.columns.inCell")).not.toBe(t("slash.columns.inColumn"));
  });

  it("rejects a local transaction that puts a column list into a column", () => {
    const editor = mount(twoColumns());
    const before = outline(editor);
    editor.view.dispatch(editor.state.tr.insert(endOfColumn(editor, 0), newList(editor)));
    expect(outline(editor)).toBe(before);
    // Pasted or inserted as content, the same.
    caretAt(editor, "A1", 2);
    editor.commands.insertContent({ type: "columnList", content: [{ type: "column", content: [{ type: "paragraph" }] }, { type: "column", content: [{ type: "paragraph" }] }] });
    expect(outline(editor)).toBe(before);
    // Edits that do not nest go through.
    editor.commands.insertContent("x");
    expect(outline(editor)).toBe("p:Before cols(col(p:A1x) col(p:B1)) p:After");
  });

  it("never filters what the binding dispatches, and still edits a document that holds a nested layout", () => {
    const editor = mount(twoColumns());
    const tr = editor.state.tr.insert(endOfColumn(editor, 0), newList(editor)).setMeta(loroSyncPluginKey, fromTheBinding);
    editor.view.dispatch(tr);
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 cols(col(p) col(p))) col(p:B1)) p:After");

    // The document is what it is: typing in the nested layout (or beside it) is not refused,
    // only adding one more nested layout is.
    caretAt(editor, "B1", 2);
    editor.commands.insertContent("!");
    const nested = editor.view.dom.querySelector('[data-type="column"] [data-type="columnList"]');
    expect(nested).not.toBeNull();
    const before = outline(editor);
    expect(before).toBe("p:Before cols(col(p:A1 cols(col(p) col(p))) col(p:B1!)) p:After");
    editor.view.dispatch(editor.state.tr.insert(endOfColumn(editor, 1), newList(editor)));
    expect(outline(editor)).toBe(before);
    // The first column still takes the commands of a column.
    caretAt(editor, "A1");
    expect(editor.commands.addColumn()).toBe(true);
  });
});

describe("the block menu on a layout", () => {
  it("offers a quote or a callout around the layout, never text that would flatten it", () => {
    const editor = mount(layout());
    const list = editor.state.doc.child(1);
    expect(isColumnList(list)).toBe(true);
    expect(isColumnList(editor.state.doc.child(0))).toBe(false);
    expect(turnIntoKinds(list)).toEqual(["blockquote", "callout"]);
    const pos = editor.state.doc.child(0).nodeSize;
    for (const kind of ["paragraph", "heading1", "bulletList", "orderedList", "taskList", "codeBlock"] as const) {
      expect(turnInto(editor.state, pos, kind), kind).toBeNull();
    }
    const tr = turnInto(editor.state, pos, "callout");
    expect(tr).not.toBeNull();
    if (tr !== null) {
      editor.view.dispatch(tr);
    }
    expect(outline(editor)).toBe(
      "p:Before callout(cols(col(p:A1 p:A2) col(p:B1) col(p:C1))) p:After",
    );
    expectUniqueIds(editor);
  });

  it("keeps a layout whole when the container around it is turned into text", () => {
    const wrapped = page(wrap("callout", p("Intro"), cols(col(p("A1")), col(p("B1")))));
    for (const kind of ["paragraph", "bulletList"] as const) {
      const editor = mount(wrapped);
      const tr = turnInto(editor.state, 0, kind);
      expect(tr, kind).not.toBeNull();
      if (tr !== null) {
        editor.view.dispatch(tr);
      }
      expect(editor.state.doc.textContent, kind).toBe("IntroA1B1");
      let layouts = 0;
      editor.state.doc.descendants((node) => {
        layouts += node.type.name === "columnList" ? 1 : 0;
      });
      expect(layouts, kind).toBe(1);
      expectUniqueIds(editor);
    }
  });

  it("duplicates a layout with ids of its own", () => {
    const editor = mount(layout());
    const pos = editor.state.doc.child(0).nodeSize;
    const tr = duplicateBlock(editor.state, pos);
    expect(tr).not.toBeNull();
    if (tr !== null) {
      editor.view.dispatch(tr);
    }
    expect(outline(editor)).toBe(
      "p:Before cols(col(p:A1 p:A2) col(p:B1) col(p:C1)) cols(col(p:A1 p:A2) col(p:B1) col(p:C1)) p:After",
    );
    // Two paragraphs, two layouts of one list, three columns and four paragraphs each.
    expect(blockIds(editor)).toHaveLength(2 + 2 * (1 + 3 + 4));
    expectUniqueIds(editor);
  });
});

/** Position just inside the `index`-th top-level block. */
function startOf(editor: Editor, index: number): number {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  return pos;
}

describe("the / menu", () => {
  /** The titles of the entries of the open menu. */
  function titles(menu: HTMLElement): (string | null | undefined)[] {
    return within(menu)
      .getAllByRole("option")
      .map((option) => option.querySelector(".suggestion-title")?.textContent);
  }

  it("finds the two entries by their name and by the words layout, split and side by side", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    for (const word of ["colum", "layout", "split", "sideb"]) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
      editor.commands.insertContent(`/${word}`);
      const menu = await screen.findByRole("listbox", { name: t("slash.label") });
      await waitFor(() => {
        expect(titles(menu)).toEqual(["2 columns", "3 columns"]);
      });
      // Close it, so that the next word opens a menu of its own (the old one is gone).
      fireEvent.keyDown(editor.view.dom, { key: "Escape" });
      await waitFor(() => {
        expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
      });
    }
  });

  it("keeps /table and /columns apart", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
    editor.commands.insertContent("/table");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(titles(menu)).toEqual(["Table", "Table view", "Table of contents"]); // no column entry
    });
  });

  it("makes the block the text was typed in the first column of a layout of the chosen size", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] },
    ]);
    const editor = await openPage(user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, startOf(editor, 1))));
    editor.commands.insertContent("/colum");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(titles(menu)).toEqual(["2 columns", "3 columns"]);
    });
    await user.click(within(menu).getByText("3 columns"));

    // The typed text is gone, the empty paragraph it was typed in is the first column, and the caret is in it.
    expect(outline(editor)).toBe("p:Intro cols(col(p) col(p) col(p))");
    expect(editor.state.selection.$from.node(2).type.name).toBe("column");
    expect(editor.state.selection.$from.index(1)).toBe(0);
    expect(editor.state.selection.empty).toBe(true);
    // The paragraph the text was typed in keeps its id (it is the one in the first column).
    expect(String(editor.state.doc.child(1).child(0).child(0).attrs.id)).toBe(pid(1));
    expectUniqueIds(editor);
    expect(editor.view.dom.querySelectorAll('[data-type="column"]')).toHaveLength(3);
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();

    const { page: pageId } = await idsOf(memory);
    await waitFor(async () => {
      const saved = JSON.stringify(await contentOf(memory, pageId("Notes")));
      expect(saved).toContain('"columnList"');
      expect(saved.match(/"column"/g)).toHaveLength(3);
    });
  });

  it("shows why it is unavailable inside a column, and does nothing there", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Board", content: twoColumns() }] },
    ]);
    const editor = await openPage(user, "Board");
    caretAt(editor, "A1", 2);
    editor.commands.insertContent(" /colum"); // after a space: that is where `/` opens the menu
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(titles(menu)).toEqual(["2 columns", "3 columns"]);
    });
    for (const option of within(menu).getAllByRole("option")) {
      expect(option).toHaveAttribute("aria-disabled", "true");
      expect(within(option).getByText(t("slash.columns.inColumn"))).toBeInTheDocument();
    }
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 /colum) col(p:B1)) p:After");
  });
});

describe("the columns toolbar", () => {
  const buttons = [t("columns.addLabel"), t("columns.removeLabel"), t("columns.unwrapLabel")];

  /** The page of `layout()` open in the editor, the caret in the first block of column `index`. */
  async function openLayout(index = 1) {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Board", content: layout() }] }]);
    const editor = await openPage(app.user, "Board");
    caretInColumn(editor, index);
    editor.commands.focus();
    return { ...app, editor };
  }

  it("shows while the selection is in a column, with the three commands", async () => {
    const { editor } = await openLayout();
    const toolbar = await screen.findByRole("toolbar", { name: t("columns.label") });
    expect(within(toolbar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(buttons);
    expect(toolbar.closest('[data-type="columnList"]')).toBeNull(); // a floating panel, not inside the page

    // Outside the layout it is gone, back in a column it is back.
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 2)));
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("columns.label") })).not.toBeInTheDocument();
    });
    caretInColumn(editor, 2);
    expect(await screen.findByRole("toolbar", { name: t("columns.label") })).toBeInTheDocument();
  });

  it("adds a column, removes the current one and unwraps the layout", async () => {
    const { user, editor, memory } = await openLayout();
    const press = async (name: string): Promise<void> => {
      await user.click(within(await screen.findByRole("toolbar", { name: t("columns.label") })).getByRole("button", { name }));
    };

    await press(t("columns.addLabel"));
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2) col(p:B1) col(p:C1) col(p)) p:After");
    expectUniqueIds(editor);
    await press(t("columns.removeLabel")); // the caret is in the second column: its block joins the first
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p:C1) col(p)) p:After");
    expect(editor.state.selection.$from.parent.textContent).toBe("B1");
    await press(t("columns.removeLabel")); // the caret is in the first column: its blocks join the next one
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2 p:B1 p:C1) col(p)) p:After");
    await press(t("columns.unwrapLabel"));
    expect(outline(editor)).toBe("p:Before p:A1 p:A2 p:B1 p:C1 p p:After");
    expectUniqueIds(editor);
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("columns.label") })).not.toBeInTheDocument();
    });
    const { page: pageId } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, pageId("Board")))).not.toContain('"columnList"');
    });
  });

  it("disables adding at five columns", async () => {
    const { user, editor } = await openLayout();
    const toolbar = await screen.findByRole("toolbar", { name: t("columns.label") });
    await user.click(within(toolbar).getByRole("button", { name: t("columns.addLabel") }));
    await user.click(within(toolbar).getByRole("button", { name: t("columns.addLabel") }));
    expect(outline(editor)).toBe("p:Before cols(col(p:A1 p:A2) col(p:B1) col(p:C1) col(p) col(p)) p:After");
    await waitFor(() => {
      expect(within(toolbar).getByRole("button", { name: t("columns.addLabel") })).toBeDisabled();
    });
    expect(within(toolbar).getByRole("button", { name: t("columns.removeLabel") })).toBeEnabled();
  });

  it("is not there in a read-only editor, which still shows the layout", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = layout() as { content: JsonValue[] };
    const withUnknown: JsonValue = {
      type: "doc",
      content: [...content.content, { type: "kanban", attrs: { id: "k0" } }],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content: withUnknown }] }]);
    const editor = await openPage(user, "Board");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(editor.view.dom.querySelectorAll('[data-type="column"]')).toHaveLength(3);
    vi.spyOn(editor, "isFocused", "get").mockReturnValue(true);
    caretInColumn(editor, 1);
    await Promise.resolve();
    expect(editor.isFocused).toBe(true);
    expect(screen.queryByRole("toolbar", { name: t("columns.label") })).not.toBeInTheDocument();
  });

  it("marks the column the selection is in, and no other", async () => {
    const { editor } = await openLayout(1);
    const active = (): string[] =>
      [...editor.view.dom.querySelectorAll('[data-type="column"]')]
        .filter((element) => element.classList.contains("is-active"))
        .map((element) => element.textContent);
    expect(active()).toEqual(["B1"]);
    caretInColumn(editor, 2);
    expect(active()).toEqual(["C1"]);
    caretAt(editor, "Before");
    expect(active()).toEqual([]);
  });
});

describe("two bound replicas", () => {
  /** Two replicas of the page `json`, each with an editor over it. */
  async function twoEditors(json: JsonValue) {
    const docA = loroDocFrom(json);
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const empty: JsonValue = { type: "doc", content: [{ type: "paragraph" }] };
    const a = mount(empty, docA);
    const b = mount(empty, docB);
    await tick();
    // Both replicas receive the other's change at the same moment, as two devices that are
    // online together do.
    const exchange = async (): Promise<void> => {
      const forB = docA.export({ mode: "update", from: docB.oplogVersion() });
      const forA = docB.export({ mode: "update", from: docA.oplogVersion() });
      docB.import(forB);
      docA.import(forA);
      await tick();
    };
    return { docA, docB, a, b, exchange };
  }

  /** How many columns the first column list of the document holds (not what the view shows). */
  function columnsHeld(doc: LoroDoc): number {
    const content = (loroToPortable(doc) as { content?: { type?: string; content?: unknown[] }[] }).content ?? [];
    return content.find((node) => node.type === "columnList")?.content?.length ?? 0;
  }

  /** The two replicas hold the same document and each view shows what its replica holds. */
  function expectAgreement(a: Editor, docA: LoroDoc, b: Editor, docB: LoroDoc): void {
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expect(outline(b)).toBe(outline(a));
    expectUniqueIds(a);
    expectUniqueIds(b);
  }

  it("converge when two replicas type in different columns at the same time", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(layout());
    caretAt(a, "A1", 2);
    a.commands.insertContent("a");
    caretAt(a, "A2");
    a.commands.insertContent(">");
    caretAt(b, "C1", 0);
    b.commands.insertContent("c");
    caretAt(b, "B1", 2);
    b.commands.insertContent("b");
    await exchange();
    const merged = "p:Before cols(col(p:A1a p:>A2) col(p:B1b) col(p:cC1)) p:After";
    expect(outline(a)).toBe(merged);
    expectAgreement(a, docA, b, docB);
    // Another round changes nothing.
    const settled = docA.oplogVersion();
    await exchange();
    await exchange();
    expect(outline(a)).toBe(merged);
    expect(docA.oplogVersion().compare(settled)).toBe(0);
    expect(errors).not.toHaveBeenCalled();
  });

  it("converge when one replica adds a column while the other types in a column", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(layout());
    caretAt(a, "A1");
    expect(a.commands.addColumn()).toBe(true);
    caretAt(b, "B1", 2);
    b.commands.insertContent("!");
    await exchange();
    expect(outline(a)).toBe("p:Before cols(col(p:A1 p:A2) col(p:B1!) col(p:C1) col(p)) p:After");
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });

  it("converge when one replica removes a column while the other types in another one", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(layout());
    caretAt(a, "B1"); // the blocks of the middle column join the first column
    expect(a.commands.removeColumn()).toBe(true);
    caretAt(b, "C1", 2);
    b.commands.insertContent("!");
    await exchange();
    expect(outline(a)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p:C1!)) p:After");
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });

  it("lets the block that was moved win: text typed in a block another replica moved into a column is not kept", async () => {
    // ADR-0011, "moved blocks win": the binding writes a move as a delete and an insert, so the
    // other replica's typing goes into the copy that was deleted. Nothing new for columns.
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(layout());
    caretAt(a, "B1"); // removing the middle column moves its block into the first column
    expect(a.commands.removeColumn()).toBe(true);
    caretAt(b, "B1", 2);
    b.commands.insertContent("!");
    await exchange();
    expect(outline(a)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p:C1)) p:After");
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });

  it("converge on a layout with fewer than two columns when two replicas each remove one of three", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(layout());
    caretAt(a, "B1");
    expect(a.commands.removeColumn()).toBe(true);
    caretAt(b, "C1");
    expect(b.commands.removeColumn()).toBe(true);
    await exchange();
    // Both removed a different column, and each moved its blocks into the column that the other
    // removed (ADR-0011, "moved blocks win"): the merged list holds the one column neither
    // removed, with what the first replica moved into it. The schema wants two columns at least:
    // the editors show the list completed with an empty column, as the binding completes any
    // merged content (patch r), and agree with each other. The document holds one column.
    expect(outline(a)).toBe("p:Before cols(col(p:A1 p:A2 p:B1) col(p)) p:After");
    expect(outline(b)).toBe(outline(a));
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA)); // (the view's filler column has no id: it is not in the document)
    expect(columnsHeld(docA)).toBe(1);
    expect(JSON.stringify(loroToPortable(docA))).not.toContain("C1"); // the second replica's move was lost
    // Exchanging more changes nothing, and the document is not rewritten to complete the list.
    const settled = docA.oplogVersion();
    await exchange();
    expect(docA.oplogVersion().compare(settled)).toBe(0);
    expect(columnsHeld(docA)).toBe(1);
    // The next local edit in that list writes the view's completion: the empty column becomes
    // part of the document. Two replicas that both edit it before they exchange write one each,
    // so the list ends up with two empty columns, the same on both (like the empty items the
    // binding completes in a merged list, patch r): untidy, but nothing is lost.
    caretAt(a, "A1", 2);
    a.commands.insertContent("a");
    caretAt(b, "A2", 0);
    b.commands.insertContent("b");
    await exchange();
    await exchange();
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expect(columnsHeld(docA)).toBe(3);
    expect(outline(a)).toBe("p:Before cols(col(p:A1a p:bA2 p:B1) col(p) col(p)) p:After");
    expect(outline(b)).toBe(outline(a));
    // The completions have no ids (UniqueID does not touch what the binding shows, and a local
    // edit elsewhere in the list is not a change to them): the two empty columns and their
    // paragraphs. Every other node has its own.
    const held = blockIds(a);
    const withIds = held.filter((id) => id !== "null");
    expect(held.length - withIds.length).toBe(4);
    expect(new Set(withIds).size).toBe(withIds.length);
    for (const id of withIds) {
      expect(id).toMatch(UUID);
    }
    expect(errors).not.toHaveBeenCalled();
  });

  it("converge on six columns when two replicas each add the fifth, and the editor takes it from there", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b, exchange } = await twoEditors(
      page(p("Before"), cols(col(p("1")), col(p("2")), col(p("3")), col(p("4")))),
    );
    caretAt(a, "1");
    expect(a.commands.addColumn()).toBe(true);
    caretAt(b, "1");
    expect(b.commands.addColumn()).toBe(true);
    await exchange();
    expect(outline(a)).toBe("p:Before cols(col(p:1) col(p:2) col(p:3) col(p:4) col(p) col(p))");
    expectAgreement(a, docA, b, docB);
    // Six is outside the schema: the commands refuse to add, and removing works.
    caretAt(a, "1");
    expect(a.can().addColumn()).toBe(false);
    expect(a.commands.removeColumn()).toBe(true);
    await exchange();
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });
});
