// Simple tables in the editor (ADR-0016, amendment of 2026-10-05): Tiptap's table commands
// on block schema v1 (every node they create gets a block id from UniqueID, every node they
// keep keeps its id), the keyboard, what the block menu does with a table, and the table
// in the page editor: the `/table` command and the toolbar.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { loroDocFrom, paragraphs, pid, sync, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { duplicateBlock, isTable, turnInto, turnIntoKinds } from "./blocks";
import { LoroBinding } from "./extensions";
import { redo, undo } from "./loro-binding";
import { ID_BEARING_NODES, createSchemaExtensions } from "./schema";
import { SLASH_COMMANDS } from "./slash-commands";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/**
 * A Tiptap editor over the schema extensions (no React): ids come from UniqueID. With `doc`
 * it is bound to that Loro document (and shows it once `tick()` has passed).
 */
function mount(content: JSONContent, doc: LoroDoc | null = null): Editor {
  const element = document.createElement("div");
  document.body.append(element);
  const extensions = createSchemaExtensions();
  if (doc !== null) {
    extensions.push(LoroBinding.configure({ doc, readOnly: false }));
  }
  const editor = new Editor({ element, extensions, content });
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

/** A page of `Before`, a table of three rows (a header row, `1 2`, `3 4`) and `After`. */
function tableDoc(): JsonValue {
  let next = 100;
  const id = (): string => pid(next++);
  const cell = (type: string, text: string): JsonValue => ({
    type,
    attrs: { id: id(), colspan: 1, rowspan: 1 },
    content: [{ type: "paragraph", attrs: { id: id() }, content: [{ type: "text", text }] }],
  });
  const row = (type: string, ...texts: string[]): JsonValue => ({
    type: "tableRow",
    attrs: { id: id() },
    content: texts.map((text) => cell(type, text)),
  });
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
      {
        type: "table",
        attrs: { id: id() },
        content: [row("tableHeader", "A", "B"), row("tableCell", "1", "2"), row("tableCell", "3", "4")],
      },
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "After" }] },
    ],
  };
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

/** The first table of the page. */
function findTable(editor: Editor): { pos: number; rows: number } {
  let found: { pos: number; rows: number } | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (found === null && node.type.name === "table") {
      found = { pos, rows: node.childCount };
    }
    return found === null;
  });
  if (found === null) {
    throw new Error("no table");
  }
  return found;
}

/** The first table as rows of cells, each `h:text` (header cell) or `d:text` (plain cell). */
function grid(editor: Editor): string[][] {
  const { pos } = findTable(editor);
  const rows: string[][] = [];
  editor.state.doc.nodeAt(pos)?.forEach((row) => {
    const cells: string[] = [];
    row.forEach((cell) => {
      cells.push(`${cell.type.name === "tableHeader" ? "h" : "d"}:${cell.textContent}`);
    });
    rows.push(cells);
  });
  return rows;
}

/** Puts the caret in the paragraph of the cell at `row`, `col` of the first table. */
function inCell(editor: Editor, row: number, col: number): void {
  const { pos } = findTable(editor);
  const table = editor.state.doc.nodeAt(pos);
  const rowNode = table?.child(row);
  if (table === null || table === undefined || rowNode === undefined) {
    throw new Error("no such row");
  }
  let rowOffset = 0;
  for (let i = 0; i < row; i += 1) {
    rowOffset += table.child(i).nodeSize;
  }
  let cellOffset = 0;
  for (let i = 0; i < col; i += 1) {
    cellOffset += rowNode.child(i).nodeSize;
  }
  // table > row > cell > paragraph: the start of the paragraph's text is four levels in.
  const at = pos + 4 + rowOffset + cellOffset;
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
}

/** Selects the cells from `anchor` to `head` (each `[row, col]`): a cell selection, not text. */
function selectCells(editor: Editor, anchor: [number, number], head: [number, number]): void {
  const startOf = (cell: [number, number]): number => {
    inCell(editor, cell[0], cell[1]);
    const { $from } = editor.state.selection;
    return $from.before($from.depth - 1); // the paragraph's parent is the cell
  };
  editor.commands.setCellSelection({ anchorCell: startOf(anchor), headCell: startOf(head) });
}

/** The cell the caret (or the start of the selection) is in, as `[row, col]`. */
function caretCell(editor: Editor): [number, number] | null {
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth -= 1) {
    if ($from.node(depth).type.name === "table") {
      return [$from.index(depth), $from.index(depth + 1)];
    }
  }
  return null;
}

describe("the table commands", () => {
  it("insert a table of the asked size, every node with an id of its own", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.insertTable({ rows: 3, cols: 3, withHeaderRow: true });
    expect(grid(editor)).toEqual([
      ["h:", "h:", "h:"],
      ["d:", "d:", "d:"],
      ["d:", "d:", "d:"],
    ]);
    // table + 3 rows + 9 cells + 9 paragraphs
    expect(blockIds(editor)).toHaveLength(1 + 3 + 9 + 9);
    expectUniqueIds(editor);
  });

  const cases: [string, (editor: Editor) => boolean, string[][], "grows" | "shrinks" | "same"][] = [
    [
      "add a row above",
      (editor) => editor.commands.addRowBefore(),
      [["h:A", "h:B"], ["d:", "d:"], ["d:1", "d:2"], ["d:3", "d:4"]],
      "grows",
    ],
    [
      "add a row below",
      (editor) => editor.commands.addRowAfter(),
      [["h:A", "h:B"], ["d:1", "d:2"], ["d:", "d:"], ["d:3", "d:4"]],
      "grows",
    ],
    [
      "add a column before",
      (editor) => editor.commands.addColumnBefore(),
      [["h:", "h:A", "h:B"], ["d:", "d:1", "d:2"], ["d:", "d:3", "d:4"]],
      "grows",
    ],
    [
      "add a column after",
      (editor) => editor.commands.addColumnAfter(),
      [["h:A", "h:", "h:B"], ["d:1", "d:", "d:2"], ["d:3", "d:", "d:4"]],
      "grows",
    ],
    ["delete the row", (editor) => editor.commands.deleteRow(), [["h:A", "h:B"], ["d:3", "d:4"]], "shrinks"],
    ["delete the column", (editor) => editor.commands.deleteColumn(), [["h:B"], ["d:2"], ["d:4"]], "shrinks"],
    [
      "turn the header row off",
      (editor) => editor.commands.toggleHeaderRow(),
      [["d:A", "d:B"], ["d:1", "d:2"], ["d:3", "d:4"]],
      "same",
    ],
  ];

  it.each(cases)("%s from the cell `1`: new nodes get ids, kept nodes keep theirs", (_name, run, expected, change) => {
    const editor = mount(tableDoc() as JSONContent);
    inCell(editor, 1, 0);
    const before = blockIds(editor);
    expect(run(editor)).toBe(true);
    expect(grid(editor)).toEqual(expected);
    expectUniqueIds(editor);
    const after = blockIds(editor);
    if (change === "grows") {
      expect(after).toHaveLength(before.length + (expected.length === 4 ? 1 + 2 * 2 : 3 * 2));
      expect(before.every((id) => after.includes(id))).toBe(true);
    } else if (change === "shrinks") {
      expect(after.length).toBeLessThan(before.length);
      expect(after.every((id) => before.includes(id))).toBe(true);
    } else {
      expect([...after].sort()).toEqual([...before].sort()); // header cells became plain cells, same ids
    }
  });

  it("turn the header row on again after it was turned off", () => {
    const editor = mount(tableDoc() as JSONContent);
    inCell(editor, 0, 0);
    editor.commands.toggleHeaderRow();
    editor.commands.toggleHeaderRow();
    expect(grid(editor)[0]).toEqual(["h:A", "h:B"]);
    expectUniqueIds(editor);
  });

  it("delete the table, and the only table of a page leaves a page that can still be typed in", () => {
    const editor = mount(tableDoc() as JSONContent);
    inCell(editor, 2, 1);
    editor.commands.deleteTable();
    expect(editor.state.doc.childCount).toBe(2);
    expect(editor.getText({ blockSeparator: "|" })).toBe("Before|After");

    const alone = mount({
      type: "doc",
      content: [
        {
          type: "table",
          content: [{ type: "tableRow", content: [{ type: "tableCell", content: [{ type: "paragraph" }] }] }],
        },
      ],
    });
    inCell(alone, 0, 0);
    alone.commands.deleteTable();
    expect(alone.state.doc.childCount).toBeGreaterThan(0);
    expect(alone.state.doc.child(0).type.name).not.toBe("table");
    expectUniqueIds(alone);
  });
});

describe("the keyboard", () => {
  it("moves between cells with Tab and Shift-Tab and adds a row after the last cell", () => {
    const editor = mount(tableDoc() as JSONContent);
    inCell(editor, 0, 0);
    const press = (shift = false): void => {
      fireEvent.keyDown(editor.view.dom, { key: "Tab", shiftKey: shift });
    };

    press();
    expect(caretCell(editor)).toEqual([0, 1]);
    press();
    expect(caretCell(editor)).toEqual([1, 0]);
    press(true);
    expect(caretCell(editor)).toEqual([0, 1]);

    inCell(editor, 2, 1);
    expect(findTable(editor).rows).toBe(3);
    press(); // the last cell: a row is added and the caret moves into it
    expect(grid(editor)).toHaveLength(4);
    expect(caretCell(editor)).toEqual([3, 0]);
    expectUniqueIds(editor);
  });
});

describe("tables merged from concurrent edits", () => {
  it("shows a short row as the document holds it, and repairs it on the next local edit only", async () => {
    const docA = loroDocFrom(tableDoc());
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const empty: JSONContent = { type: "doc", content: [{ type: "paragraph" }] };
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

    // A adds a column while B adds a row: after the merge the new row is a cell short.
    inCell(a, 1, 0);
    expect(a.commands.addColumnAfter()).toBe(true);
    inCell(b, 1, 0);
    expect(b.commands.addRowAfter()).toBe(true);
    await exchange();
    const merged = [["h:A", "h:", "h:B"], ["d:1", "d:", "d:2"], ["d:", "d:"], ["d:3", "d:", "d:4"]];
    expect(grid(a)).toEqual(merged);
    expect(grid(b)).toEqual(merged);

    // Neither editor repaired it in place: that would have been the same repair twice, and
    // the table would grow with every exchange. Exchanging more changes nothing.
    const settled = docA.oplogVersion();
    for (let i = 0; i < 3; i += 1) {
      await exchange();
    }
    expect(grid(a)).toEqual(merged);
    expect(grid(b)).toEqual(merged);
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expect(docA.oplogVersion().compare(settled)).toBe(0);

    // The next local edit in that table repairs it: one cell, written by that replica alone.
    inCell(a, 3, 1);
    a.commands.insertContent("x");
    expect(grid(a)).toEqual([["h:A", "h:", "h:B"], ["d:1", "d:", "d:2"], ["d:", "d:", "d:"], ["d:3", "d:x", "d:4"]]);
    await exchange();
    await exchange();
    expect(grid(b)).toEqual(grid(a));
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expectUniqueIds(a);
    expectUniqueIds(b);
  });
});

describe("tables in a bound editor", () => {
  /** Two replicas of the page of `tableDoc()`, each with an editor over it. */
  async function twoEditors() {
    const docA = loroDocFrom(tableDoc());
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const empty: JSONContent = { type: "doc", content: [{ type: "paragraph" }] };
    const a = mount(empty, docA);
    const b = mount(empty, docB);
    await tick();
    return { docA, docB, a, b };
  }

  it("takes a remote change while cells are selected", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, docB, a, b } = await twoEditors();
    // A selects the cells `1` to `4` (a cell selection, not a text selection).
    selectCells(a, [1, 0], [2, 1]);
    expect(a.state.selection.constructor.name).toBe("CellSelection");

    // B types in a cell of the selection and in the paragraph after the table.
    inCell(b, 1, 1);
    b.commands.insertContent("remote");
    sync(docB, docA);
    await tick();

    expect(grid(a)[1]).toEqual(["d:1", "d:remote2"]);
    expect(grid(a)).toEqual(grid(b));
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(errors).not.toHaveBeenCalled();
  });

  it("undoes and redoes table edits, and keeps the ids", async () => {
    const errors = vi.spyOn(console, "error");
    const { docA, a } = await twoEditors();
    const original = grid(a);
    const ids = blockIds(a);
    inCell(a, 1, 0);
    expect(a.commands.addRowAfter()).toBe(true);
    expect(a.commands.addColumnBefore()).toBe(true);
    expect(grid(a)).toHaveLength(4);
    const edited = grid(a);

    // The binding's undo: a table edit is a change of the document like any other.
    while (grid(a).length > 3 || grid(a)[0]?.length !== 2) {
      expect(undo(a.state, a.view.dispatch)).toBe(true);
    }
    expect(grid(a)).toEqual(original);
    expect(blockIds(a)).toEqual(ids);
    expect(loroToPortable(docA)).toEqual(loroToPortable(loroDocFrom(tableDoc())));
    while (grid(a).length < 4 || grid(a)[0]?.length !== 3) {
      expect(redo(a.state, a.view.dispatch)).toBe(true);
    }
    expect(grid(a)).toEqual(edited);
    expectUniqueIds(a);
    expect(errors).not.toHaveBeenCalled();
  });
});

describe("the block menu on a table", () => {
  it("offers a quote or a callout around the table, never text that would flatten it", () => {
    const editor = mount(tableDoc() as JSONContent);
    const { pos } = findTable(editor);
    const table = editor.state.doc.nodeAt(pos);
    expect(table === null ? false : isTable(table)).toBe(true);
    expect(table === null ? [] : turnIntoKinds(table)).toEqual(["blockquote", "callout"]);
    for (const kind of ["paragraph", "heading1", "bulletList", "orderedList", "taskList", "codeBlock"] as const) {
      expect(turnInto(editor.state, pos, kind), kind).toBeNull();
    }

    const tr = turnInto(editor.state, pos, "callout");
    expect(tr).not.toBeNull();
    if (tr !== null) {
      editor.view.dispatch(tr);
    }
    const callout = editor.state.doc.child(1);
    expect(callout.type.name).toBe("callout");
    expect(callout.child(0).type.name).toBe("table");
    expect(grid(editor)).toEqual([["h:A", "h:B"], ["d:1", "d:2"], ["d:3", "d:4"]]);
    expectUniqueIds(editor);
  });

  it("keeps a table whole when the container around it is turned into text", () => {
    const content = tableDoc() as { content: JSONContent[] };
    const table = content.content[1];
    const wrapped: JSONContent = {
      type: "doc",
      content: [
        {
          type: "callout",
          attrs: { id: pid(50), icon: "💡" },
          content: [{ type: "paragraph", attrs: { id: pid(51) }, content: [{ type: "text", text: "Intro" }] }, table ?? {}],
        },
      ],
    };
    for (const [kind, holder] of [
      ["paragraph", null],
      ["bulletList", "listItem"],
    ] as const) {
      const editor = mount(wrapped);
      const tr = turnInto(editor.state, 0, kind);
      expect(tr, kind).not.toBeNull();
      if (tr !== null) {
        editor.view.dispatch(tr);
      }
      expect(grid(editor), kind).toEqual([["h:A", "h:B"], ["d:1", "d:2"], ["d:3", "d:4"]]);
      if (holder === null) {
        expect(editor.state.doc.child(0).type.name).toBe("paragraph");
        expect(editor.state.doc.child(1).type.name).toBe("table");
      } else {
        // The table joins the item that starts with the paragraph before it.
        expect(editor.state.doc.child(0).child(0).childCount).toBe(2);
        expect(editor.state.doc.child(0).child(0).child(1).type.name).toBe("table");
      }
      expectUniqueIds(editor);
    }
  });

  it("duplicates a table with ids of its own", () => {
    const editor = mount(tableDoc() as JSONContent);
    const { pos } = findTable(editor);
    const tr = duplicateBlock(editor.state, pos);
    expect(tr).not.toBeNull();
    if (tr !== null) {
      editor.view.dispatch(tr);
    }
    expect(editor.state.doc.childCount).toBe(4);
    expect(editor.state.doc.child(1).textContent).toBe(editor.state.doc.child(2).textContent);
    expect(blockIds(editor)).toHaveLength(2 + 2 * (1 + 3 + 6 + 6) + 0); // two paragraphs, two tables
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
  it("inserts a table of 3 x 3 with a header row and the caret in its first cell", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] },
    ]);
    const editor = await openPage(user, "Notes");
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, startOf(editor, 1))));
    editor.commands.insertContent("/tab");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual([
        "Table",
        "Table view", // its label starts with the same word, and is shorter
        "Table of contents",
        // The other database views, by their keyword `database` (it contains "tab"), in list order.
        "List view",
        "Board view",
        "Calendar view",
        "Gallery view",
        "Dashboard view",
      ]);
    });
    await user.click(within(menu).getByText("Table"));

    expect(grid(editor)).toEqual([
      ["h:", "h:", "h:"],
      ["d:", "d:", "d:"],
      ["d:", "d:", "d:"],
    ]);
    expect(caretCell(editor)).toEqual([0, 0]);
    expect(editor.state.doc.child(0).textContent).toBe("Intro");
    // A block to type into below the table, even though the table came last.
    expect(editor.state.doc.lastChild?.type.name).toBe("paragraph");
    expectUniqueIds(editor);
    expect(editor.view.dom.querySelectorAll("table th")).toHaveLength(3);

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Notes")))).toContain('"tableHeader"');
    });
  });

  it("puts the table after the text it was typed behind, and inside a cell when typed there", () => {
    const command = SLASH_COMMANDS.find((candidate) => candidate.id === "table");
    expect(command).toBeDefined();
    const editor = mount({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "Hello /table" }] }],
    });
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    command?.run(editor, { from: 7, to: 13 });
    expect(editor.state.doc.child(0).textContent).toBe("Hello ");
    expect(editor.state.doc.child(1).type.name).toBe("table");
    expect(editor.state.doc.lastChild?.type.name).toBe("paragraph");
    expect(caretCell(editor)).toEqual([0, 0]);
    expectUniqueIds(editor);

    // Typed in a cell, it makes a table of its own there (the schema allows blocks in cells).
    inCell(editor, 1, 1);
    editor.commands.insertContent("/table");
    const { from } = editor.state.selection;
    command?.run(editor, { from: from - 6, to: from });
    const cell = editor.state.doc.child(1).child(1).child(1);
    expect(cell.type.name).toBe("tableCell");
    // It replaced the cell's empty paragraph, and a paragraph follows it to type in.
    expect(cell.childCount).toBe(2);
    expect(cell.child(0).type.name).toBe("table");
    expect(cell.child(1).type.name).toBe("paragraph");
    expect(caretCell(editor)).toEqual([0, 0]);
    let tables = 0;
    editor.state.doc.descendants((node) => {
      tables += node.type.name === "table" ? 1 : 0;
    });
    expect(tables).toBe(2);
    expectUniqueIds(editor);
  });

  it("finds the table by the words grid, rows and cells (`/columns` is the column layout)", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    for (const word of ["grid", "rows", "cells"]) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 1)));
      editor.commands.insertContent(`/${word}`);
      const menu = await screen.findByRole("listbox", { name: t("slash.label") });
      await waitFor(() => {
        expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual(
          ["Table"],
        );
      });
    }
  });
});

describe("the table toolbar", () => {
  const buttons = [
    t("table.addRowAbove"),
    t("table.addRowBelow"),
    t("table.deleteRow"),
    t("table.addColumnBefore"),
    t("table.addColumnAfter"),
    t("table.deleteColumn"),
    t("table.headerRow"),
    t("table.deleteTable"),
  ];

  /** The page of `tableDoc()` open in the editor, the caret in the cell `row`, `col`. */
  async function openTable(row = 1, col = 0) {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Grid", content: tableDoc() }] }]);
    const editor = await openPage(app.user, "Grid");
    inCell(editor, row, col);
    editor.commands.focus();
    return { ...app, editor };
  }

  it("shows while the selection is in a table, with the table commands", async () => {
    const { editor } = await openTable();
    const toolbar = await screen.findByRole("toolbar", { name: t("table.label") });
    expect(within(toolbar).getAllByRole("button").map((button) => button.getAttribute("aria-label"))).toEqual(buttons);
    expect(within(toolbar).getByRole("button", { name: t("table.headerRow") })).toHaveAttribute("aria-pressed", "true");
    expect(toolbar.closest("table")).toBeNull(); // a floating panel, not inside the page

    // A selection of cells (not text) is a selection in the table too, and the commands work on it.
    selectCells(editor, [1, 0], [2, 1]);
    expect(editor.state.selection.constructor.name).toBe("CellSelection");
    expect(await screen.findByRole("toolbar", { name: t("table.label") })).toBeInTheDocument();
    inCell(editor, 1, 0);

    // Outside the table it is gone, back in a cell it is back.
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 2)));
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("table.label") })).not.toBeInTheDocument();
    });
    inCell(editor, 0, 1);
    expect(await screen.findByRole("toolbar", { name: t("table.label") })).toBeInTheDocument();
  });

  it("adds and deletes rows and columns, with ids for what it adds", async () => {
    const { user, editor } = await openTable();
    const press = async (name: string): Promise<void> => {
      await user.click(within(await screen.findByRole("toolbar", { name: t("table.label") })).getByRole("button", { name }));
    };

    await press(t("table.addRowBelow"));
    expect(grid(editor)).toEqual([["h:A", "h:B"], ["d:1", "d:2"], ["d:", "d:"], ["d:3", "d:4"]]);
    await press(t("table.addRowAbove"));
    expect(grid(editor)).toHaveLength(5);
    await press(t("table.addColumnAfter"));
    await press(t("table.addColumnBefore"));
    expect(grid(editor).map((cells) => cells.length)).toEqual([4, 4, 4, 4, 4]);
    expectUniqueIds(editor);

    await press(t("table.deleteColumn"));
    await press(t("table.deleteRow"));
    expect(grid(editor).map((cells) => cells.length)).toEqual([3, 3, 3, 3]);
    expect(grid(editor)).toHaveLength(4);
    expectUniqueIds(editor);
    expect(editor.state.selection.$from.depth).toBeGreaterThan(0); // the caret stays in the table
    expect(caretCell(editor)).not.toBeNull();
  });

  it("turns the header row on and off, and deletes the table", async () => {
    const { user, editor, memory } = await openTable(0, 0);
    const toolbar = await screen.findByRole("toolbar", { name: t("table.label") });
    const header = within(toolbar).getByRole("button", { name: t("table.headerRow") });

    await user.click(header);
    expect(grid(editor)[0]).toEqual(["d:A", "d:B"]);
    expect(editor.view.dom.querySelectorAll("table th")).toHaveLength(0);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: t("table.headerRow") })).toHaveAttribute("aria-pressed", "false");
    });
    await user.click(screen.getByRole("button", { name: t("table.headerRow") }));
    expect(grid(editor)[0]).toEqual(["h:A", "h:B"]);

    await user.click(screen.getByRole("button", { name: t("table.deleteTable") }));
    expect(editor.view.dom.querySelector("table")).toBeNull();
    expect(editor.getText({ blockSeparator: "|" })).toBe("Before|After");
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("table.label") })).not.toBeInTheDocument();
    });
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Grid")))).not.toContain('"table"');
    });
  });

  it("is not there in a read-only editor, which still shows the table", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = tableDoc() as { content: JsonValue[] };
    const withUnknown: JsonValue = {
      type: "doc",
      content: [...content.content, { type: "kanban", attrs: { id: "k0" } }],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content: withUnknown }] }]);
    const editor = await openPage(user, "Board");
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(editor.view.dom.querySelectorAll("table tr")).toHaveLength(3);
    expect(editor.view.dom.querySelectorAll("table th")).toHaveLength(2);
    // Not even with the caret in a cell and the editor focused (a read-only view cannot be, so
    // say it is: the toolbar has to refuse on its own account).
    vi.spyOn(editor, "isFocused", "get").mockReturnValue(true);
    inCell(editor, 1, 1);
    await Promise.resolve();
    expect(editor.isFocused).toBe(true);
    expect(screen.queryByRole("toolbar", { name: t("table.label") })).not.toBeInTheDocument();
  });
});
