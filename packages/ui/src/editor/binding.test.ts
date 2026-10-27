// The vendored Loro binding (loro-binding/, see VENDORED.md): the derived mark table and
// the local patches, each against the behaviour it fixes.

import { loroToPortable, normalizePortable, PAGE_MARKS } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { createTable } from "@tiptap/extension-table";
import type { NodeType } from "@tiptap/pm/model";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import type { Transaction } from "@tiptap/pm/state";
import { addRowAfter, deleteColumn } from "@tiptap/pm/tables";
import { findWrapping } from "@tiptap/pm/transform";
import type { EditorView } from "@tiptap/pm/view";
import { LoroDoc } from "loro-crdt";
import type { LoroList, LoroMap, LoroText, MapOp, VersionVector } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { bindView, loroDocFrom, PAGE_FIXTURE, paragraphs, pid, sync, tick, withoutHardBreakIds } from "../test/editor";
import type { BoundView } from "../test/editor";
import { configLoroTextStyle, getLoroTextStyle, loroSyncPluginKey, redo, undo } from "./loro-binding";
import { createEditorSchema } from "./schema";

const schema = createEditorSchema();

const views: BoundView[] = [];

function bound(doc: LoroDoc, options: Parameters<typeof bindView>[2] = {}): EditorView {
  const handle = bindView(schema, doc, options);
  views.push(handle);
  return handle.view;
}

afterEach(() => {
  for (const handle of views.splice(0)) {
    handle.destroy();
  }
  vi.restoreAllMocks();
});

/** Every op `doc` recorded since `since`. */
function opsSince(doc: LoroDoc, since: VersionVector) {
  return doc
    .exportJsonUpdates(since)
    .changes.flatMap((change) => change.ops)
    .map((op) => op.content);
}

/** Position just inside the start of the `index`-th top-level block. */
function startOfBlock(view: EditorView, index: number): number {
  let pos = 0;
  for (let i = 0; i < index; i += 1) {
    pos += view.state.doc.child(i).nodeSize;
  }
  return pos + 1;
}

describe("mark expansion (patch f)", () => {
  it("derives the shape v1 table (and the colour marks of 2026-10-07) from the schema's inclusive flags", () => {
    expect(getLoroTextStyle(schema)).toEqual({
      bold: { expand: "after" },
      italic: { expand: "after" },
      underline: { expand: "after" },
      strike: { expand: "after" },
      code: { expand: "none" },
      link: { expand: "none" },
      textColor: { expand: "after" },
      highlight: { expand: "after" },
    });
  });

  it("is the table the core and core-client configure", () => {
    const derived = getLoroTextStyle(schema);
    expect(Object.fromEntries(PAGE_MARKS.map(([name, expand]) => [name, { expand }]))).toEqual(derived);
  });

  it("makes Loro extend bold over text typed at its end, but not code or a link", () => {
    const doc = new LoroDoc();
    configLoroTextStyle(doc, schema);
    const text = doc.getText("t");
    text.insert(0, "ab");
    text.mark({ start: 0, end: 1 }, "bold", {});
    text.mark({ start: 1, end: 2 }, "link", { href: "https://example.com" });
    text.insert(2, "!");
    text.insert(1, "?");
    expect(text.toDelta()).toEqual([
      { insert: "a?", attributes: { bold: {} } },
      { insert: "b", attributes: { link: { href: "https://example.com" } } },
      { insert: "!" },
    ]);
  });
});

describe("initialisation (patch e, upstream issue #77)", () => {
  it("never writes into a document without a root node, and stays read-only", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const doc = new LoroDoc();
    const view = bound(doc);
    await tick();

    expect(loroSyncPluginKey.getState(view.state)?.readOnlyReason).toEqual({ reason: "empty-document" });
    expect(view.editable).toBe(false);
    view.dispatch(view.state.tr.insertText("typed anyway", 1));
    expect(doc.oplogVersion().length()).toBe(0);
    expect(doc.toJSON()).toEqual({ doc: {} }); // the root container exists, empty
    expect(errors).toHaveBeenCalledWith(expect.stringContaining("no root node"));
  });

  it("writes nothing before the view shows the document, then shows the document", async () => {
    const doc = loroDocFrom(PAGE_FIXTURE);
    const before = doc.oplogVersion();
    const view = bound(doc); // starts with ProseMirror's empty default document
    view.dispatch(view.state.tr.insertText("too early", 1));
    expect(doc.oplogVersion().compare(before)).toBe(0);

    await tick();
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(
      normalizePortable(withoutHardBreakIds(loroToPortable(doc))),
    );
    expect(view.editable).toBe(true);
    expect(doc.oplogVersion().compare(before)).toBe(0);
  });

  it("writes local edits once initialised", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const view = bound(doc);
    await tick();
    view.dispatch(view.state.tr.insertText(" world", 6));
    expect(loroToPortable(doc)).toEqual(paragraphs("Hello world"));
  });
});

describe("attributes (patch d, upstream PR #84)", () => {
  it("emits no delete op for null attributes that the document does not hold", async () => {
    const doc = loroDocFrom({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: "c" }, content: [{ type: "text", text: "let x" }] }],
    });
    const view = bound(doc);
    await tick();
    const before = doc.oplogVersion();
    view.dispatch(view.state.tr.insertText(" = 1", 6)); // `language` is null in ProseMirror

    const ops = opsSince(doc, before);
    expect(ops).toEqual([expect.objectContaining({ type: "insert", text: " = 1" })]);
    expect(ops.filter((op): op is MapOp => op.type === "delete" && "key" in op)).toEqual([]);
  });

  it("still deletes an attribute that becomes null", async () => {
    const doc = loroDocFrom({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: "c", language: "rust" }, content: [{ type: "text", text: "fn" }] }],
    });
    const view = bound(doc);
    await tick();
    view.dispatch(view.state.tr.setNodeAttribute(0, "language", null));
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: "c" }, content: [{ type: "text", text: "fn" }] }],
    });
  });
});

describe("mark attributes (patch g)", () => {
  it("stores a link as { href } only, like the core writes it", async () => {
    const doc = loroDocFrom(paragraphs("see docs"));
    const view = bound(doc);
    await tick();
    const link = schema.marks.link?.create({ href: "https://example.com/docs" });
    if (link === undefined) {
      throw new Error("no link mark");
    }
    view.dispatch(view.state.tr.addMark(5, 9, link));

    const text = ((doc.getMap("doc").get("children") as LoroList).get(0) as LoroMap).get("children") as LoroList;
    expect((text.get(0) as LoroText).toDelta()).toEqual([
      { insert: "see " },
      { insert: "docs", attributes: { link: { href: "https://example.com/docs" } } },
    ]);
  });

  it("does not rewrite a stored link on the next edit of its paragraph", async () => {
    const doc = loroDocFrom({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "p" },
          content: [
            { type: "text", text: "a " },
            { type: "text", marks: [{ type: "link", attrs: { href: "https://example.com" } }], text: "link" },
          ],
        },
      ],
    });
    const view = bound(doc);
    await tick();
    const before = doc.oplogVersion();
    view.dispatch(view.state.tr.insertText("!", 1));
    expect(opsSince(doc, before)).toEqual([expect.objectContaining({ type: "insert", text: "!" })]);
  });
});

describe("remote changes (patch d, upstream PR #85)", () => {
  it("keeps the selection on its text, set in the same transaction as the change", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const view = bound(doc);
    await tick();
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 6))); // after "Hello"

    const dispatched: Transaction[] = [];
    const dispatch = view.dispatch.bind(view);
    view.dispatch = (tr: Transaction) => {
      dispatched.push(tr);
      dispatch(tr);
    };
    const remoteText = ((remote.getMap("doc").get("children") as LoroList).get(0) as LoroMap).get(
      "children",
    ) as LoroList;
    (remoteText.get(0) as LoroText).insert(0, "Hey! ");
    remote.commit();
    sync(remote, doc);

    expect(view.state.doc.textContent).toBe("Hey! Hello world");
    expect(view.state.selection.from).toBe(11);
    expect(dispatched).toHaveLength(1);
    expect(dispatched[0]?.selectionSet).toBe(true);
    await tick();
    expect(dispatched).toHaveLength(1); // no late selection fix-up
    expect(view.state.selection.from).toBe(11);
  });

  it("does not put remote changes on the undo stack", async () => {
    const doc = loroDocFrom(paragraphs("one"));
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const handle = bindView(schema, doc);
    views.push(handle);
    const { view } = handle;
    await tick();

    view.dispatch(view.state.tr.insertText(" two", 4));
    const remoteText = ((remote.getMap("doc").get("children") as LoroList).get(0) as LoroMap).get(
      "children",
    ) as LoroList;
    (remoteText.get(0) as LoroText).insert(0, ">");
    remote.commit();
    sync(remote, doc);
    expect(view.state.doc.textContent).toBe(">one two");

    expect(undo(view.state, view.dispatch)).toBe(true);
    expect(view.state.doc.textContent).toBe(">one");
    expect(handle.undoManager.canUndo()).toBe(false);
    expect(redo(view.state, view.dispatch)).toBe(true);
    expect(view.state.doc.textContent).toBe(">one two");
  });
});

describe("selections the cursors cannot carry (patch x)", () => {
  function nodeType(name: string): NodeType {
    const type = schema.nodes[name];
    if (type === undefined) {
      throw new Error(`no node type ${name}`);
    }
    return type;
  }

  /** The page in `view`, and another replica of it in `remoteView`. */
  async function replicas(json: JsonValue) {
    const doc = loroDocFrom(json);
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const view = bound(doc);
    const remoteView = bound(remote);
    await tick();
    return { view, remoteView, receive: () => sync(remote, doc) };
  }

  it("keeps the caret in its text when another replica wraps the block", async () => {
    const errors = vi.spyOn(console, "error");
    const { view, remoteView, receive } = await replicas(paragraphs("one", "two", "three"));
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, startOfBlock(view, 1) + 2)));

    // The binding writes the wrapped paragraph as a new container: the caret's text is gone.
    const range = remoteView.state.doc.resolve(startOfBlock(remoteView, 1)).blockRange();
    const wrapping = range === null ? null : findWrapping(range, nodeType("blockquote"));
    if (range === null || wrapping === null) {
      throw new Error("cannot wrap the paragraph");
    }
    remoteView.dispatch(remoteView.state.tr.wrap(range, wrapping));
    receive();

    expect(view.state.doc.child(1).type.name).toBe("blockquote");
    const { $head, empty } = view.state.selection;
    expect(empty).toBe(true);
    expect($head.parent.attrs.id).toBe(pid(1));
    expect($head.parentOffset).toBe(2);
    expect(errors).not.toHaveBeenCalled();
  });

  it("puts the caret where its block was when another replica deletes the block", async () => {
    const errors = vi.spyOn(console, "error");
    const { view, remoteView, receive } = await replicas(paragraphs("one", "two", "three", "four"));
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, startOfBlock(view, 1) + 2)));

    remoteView.dispatch(remoteView.state.tr.delete(startOfBlock(remoteView, 1) - 1, startOfBlock(remoteView, 2) - 1));
    receive();

    expect(view.state.doc.textContent).toBe("onethreefour");
    expect(view.state.selection.empty).toBe(true);
    expect(view.state.selection.from).toBe(startOfBlock(view, 1)); // the start of "three"
    expect(errors).not.toHaveBeenCalled();
  });

  it("keeps a node selection on its node when another replica types before it", async () => {
    const { view, remoteView, receive } = await replicas({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "one" }] },
        { type: "horizontalRule", attrs: { id: pid(1) } },
        { type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "two" }] },
      ],
    });
    view.dispatch(view.state.tr.setSelection(NodeSelection.create(view.state.doc, startOfBlock(view, 1) - 1)));

    remoteView.dispatch(remoteView.state.tr.insertText("!!", 4));
    receive();

    expect(view.state.doc.child(0).textContent).toBe("one!!");
    const { selection } = view.state;
    expect(selection).toBeInstanceOf(NodeSelection);
    expect(selection instanceof NodeSelection ? selection.node.type.name : null).toBe("horizontalRule");
    expect(selection.from).toBe(startOfBlock(view, 1) - 1);
  });
});

describe("undo (patch v)", () => {
  it("writes a local edit made right after an undo, in the same task", async () => {
    const doc = loroDocFrom(paragraphs("one"));
    const view = bound(doc);
    await tick();
    view.dispatch(view.state.tr.insertText(" two", 4));
    expect(undo(view.state, view.dispatch)).toBe(true);
    view.dispatch(view.state.tr.insertText("!", 4));
    expect(loroToPortable(doc)).toEqual(paragraphs("one!"));
    expect(view.state.doc.textContent).toBe("one!");
  });
});

describe("content outside the schema (patch e)", () => {
  it("opens read-only and writes nothing when the document holds an unknown node", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const doc = loroDocFrom({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p" }, content: [{ type: "text", text: "keep me" }] },
        { type: "kanban", attrs: { id: "k" }, content: [{ type: "text", text: "from a newer version" }] },
      ],
    });
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();

    expect(loroSyncPluginKey.getState(view.state)?.readOnlyReason).toEqual({
      reason: "unknown-content",
      nodes: ["kanban"],
      marks: [],
    });
    expect(view.editable).toBe(false);
    view.dispatch(view.state.tr.insertText("x", 1));
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(errors).toHaveBeenCalled(); // the binding cannot create the node
  });

  it("turns read-only before writing when a remote change brings an unknown mark", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const doc = loroDocFrom(paragraphs("plain text"));
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const view = bound(doc);
    await tick();
    expect(view.editable).toBe(true);

    remote.configTextStyle({ spoiler: { expand: "none" } });
    const remoteText = ((remote.getMap("doc").get("children") as LoroList).get(0) as LoroMap).get(
      "children",
    ) as LoroList;
    (remoteText.get(0) as LoroText).mark({ start: 0, end: 5 }, "spoiler", { color: "yellow" });
    remote.commit();
    sync(remote, doc);
    const after = doc.oplogVersion();

    expect(loroSyncPluginKey.getState(view.state)?.readOnlyReason).toMatchObject({ marks: ["spoiler"] });
    expect(view.editable).toBe(false);
    view.dispatch(view.state.tr.insertText("x", startOfBlock(view, 0)));
    expect(doc.oplogVersion().compare(after)).toBe(0);
    expect(loroToPortable(doc)).toEqual(loroToPortable(remote));
  });

  it("can be asked to stay read-only", async () => {
    const doc = loroDocFrom(paragraphs("text"));
    const before = doc.oplogVersion();
    const view = bound(doc, { readOnly: true });
    await tick();
    expect(view.editable).toBe(false);
    view.dispatch(view.state.tr.insertText("x", 1));
    expect(doc.oplogVersion().compare(before)).toBe(0);
  });
});

describe("merged content outside the schema (patch r)", () => {
  // Concurrent edits can leave structures the schema does not accept; upstream dropped
  // them from the view and the next write deleted them.
  const merged: JsonValue = {
    type: "doc",
    content: [
      {
        type: "bulletList",
        attrs: { id: "l" },
        content: [
          {
            type: "listItem",
            attrs: { id: "i" },
            content: [
              {
                type: "callout",
                attrs: { id: "c", icon: "💡" },
                content: [{ type: "paragraph", attrs: { id: "k" }, content: [{ type: "text", text: "kept" }] }],
              },
            ],
          },
        ],
      },
      { type: "paragraph", attrs: { id: "p" }, content: [{ type: "text", text: "elsewhere" }] },
    ],
  };

  it("shows the content, completed where ProseMirror can, without writing", async () => {
    const doc = loroDocFrom(merged);
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();
    expect(view.state.doc.textContent).toBe("keptelsewhere");
    // The list item gets the paragraph it must start with, in the view only.
    expect(view.state.doc.child(0).child(0).child(0).type.name).toBe("paragraph");
    expect(doc.oplogVersion().compare(before)).toBe(0);
  });

  it("does not delete it when the user edits elsewhere", async () => {
    const doc = loroDocFrom(merged);
    const view = bound(doc);
    await tick();
    const end = view.state.doc.content.size - 1;
    view.dispatch(view.state.tr.insertText("!", end));
    expect(JSON.stringify(loroToPortable(doc))).toContain("kept");
    expect(JSON.stringify(loroToPortable(doc))).toContain("elsewhere!");
  });
});

describe("views and documents", () => {
  it("stops listening to the document when the view is destroyed", async () => {
    const doc = loroDocFrom(paragraphs("text"));
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const handle = bindView(schema, doc);
    await tick();
    const dispatch = vi.fn();
    handle.view.dispatch = dispatch;
    handle.destroy();

    const remoteText = ((remote.getMap("doc").get("children") as LoroList).get(0) as LoroMap).get(
      "children",
    ) as LoroList;
    (remoteText.get(0) as LoroText).insert(0, "x");
    remote.commit();
    sync(remote, doc);
    expect(dispatch).not.toHaveBeenCalled();
  });
});

describe("math blocks (additive node, 2026-10-06)", () => {
  const SOURCE = "a &= b\nc &= d";

  function mathPage(): JsonValue {
    return {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: SOURCE }] },
        { type: "mathBlock", attrs: { id: pid(2) } },
      ],
    };
  }

  it("shows a math block, newlines and an empty one included, and loading writes nothing", async () => {
    const doc = loroDocFrom(mathPage());
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();

    expect(view.state.doc.child(1).type.name).toBe("mathBlock");
    expect(view.state.doc.child(1).textContent).toBe(SOURCE);
    expect(view.state.doc.child(2).type.name).toBe("mathBlock");
    expect(view.state.doc.child(2).content.size).toBe(0);
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(normalizePortable(mathPage()));
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(loroToPortable(doc)).toEqual(mathPage());
  });

  it("writes typed text as text, the way it writes a code block's", async () => {
    const doc = loroDocFrom(mathPage());
    const view = bound(doc);
    await tick();
    const before = doc.oplogVersion();

    view.dispatch(view.state.tr.insertText("!", startOfBlock(view, 1) + SOURCE.length));
    view.dispatch(view.state.tr.insertText("x^2", startOfBlock(view, 2)));
    // Text inserts, and the text container the empty block did not have yet: no attribute
    // or node is written or deleted.
    const ops = opsSince(doc, before);
    expect(ops.filter((op) => op.type === "insert" && "text" in op).map((op) => ("text" in op ? op.text : ""))).toEqual([
      "!",
      "x^2",
    ]);
    expect(ops.filter((op) => op.type === "delete")).toEqual([]);
    expect(ops.filter((op): op is MapOp => "key" in op)).toEqual([]);
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: `${SOURCE}!` }] },
        { type: "mathBlock", attrs: { id: pid(2) }, content: [{ type: "text", text: "x^2" }] },
      ],
    });
  });

  it("turns a paragraph into a math block and back, the block id and the text staying", async () => {
    const doc = loroDocFrom(mathPage());
    const view = bound(doc);
    await tick();
    const start = startOfBlock(view, 0);

    view.dispatch(view.state.tr.setBlockType(start, start, schema.nodes.mathBlock as NodeType, { id: pid(0) }));
    const [, ...rest] = (mathPage() as { content: JsonValue[] }).content;
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [{ type: "mathBlock", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] }, ...rest],
    });

    view.dispatch(view.state.tr.setBlockType(start, start, schema.nodes.paragraph as NodeType, { id: pid(0) }));
    expect(loroToPortable(doc)).toEqual(mathPage());
  });

  it("converges between two replicas that type into the same equation", async () => {
    const first = loroDocFrom(mathPage());
    const viewA = bound(first);
    const second = new LoroDoc();
    second.import(first.export({ mode: "snapshot" }));
    const viewB = bound(second);
    await tick();

    viewA.dispatch(viewA.state.tr.insertText("A", startOfBlock(viewA, 1)));
    viewB.dispatch(viewB.state.tr.insertText("B", startOfBlock(viewB, 1) + SOURCE.length));
    sync(first, second);
    sync(second, first);
    await tick();

    const expected = `A${SOURCE}B`;
    expect(loroToPortable(first)).toEqual(loroToPortable(second));
    expect(viewA.state.doc.child(1).textContent).toBe(expected);
    expect(viewB.state.doc.child(1).textContent).toBe(expected);
  });
});

describe("tables (additive nodes, 2026-10-05)", () => {
  /** Where the text of the cell at `row`, `col` of the first table starts: table > row > cell > paragraph. */
  function cellStart(view: EditorView, row: number, col: number): number {
    let found = -1;
    view.state.doc.descendants((node, pos) => {
      if (found === -1 && node.type.name === "table") {
        let at = pos + 4;
        for (let i = 0; i < row; i += 1) {
          at += node.child(i).nodeSize;
        }
        for (let i = 0; i < col; i += 1) {
          at += node.child(row).child(i).nodeSize;
        }
        found = at;
      }
      return found === -1;
    });
    return found;
  }

  /** The text of every cell of the first table, by row. */
  function cellTexts(view: EditorView): string[][] {
    const rows: string[][] = [];
    view.state.doc.descendants((node) => {
      if (rows.length === 0 && node.type.name === "table") {
        node.forEach((row) => {
          const cells: string[] = [];
          row.forEach((cell) => {
            cells.push(cell.textContent);
          });
          rows.push(cells);
        });
      }
      return rows.length === 0;
    });
    return rows;
  }

  /** The node names of a portable tree: `doc(paragraph(text) table(...))`. */
  function shape(json: JsonValue): string {
    if (typeof json !== "object" || json === null || Array.isArray(json)) {
      return "?";
    }
    const { content, type } = json;
    const name = typeof type === "string" ? type : "?";
    return Array.isArray(content) && content.length > 0 ? `${name}(${content.map(shape).join(" ")})` : name;
  }

  /** Both replicas hold the same document and each view shows exactly its replica. */
  function expectAgreement(a: EditorView, docA: LoroDoc, b: EditorView, docB: LoroDoc): void {
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expect(b.state.doc.toJSON()).toEqual(a.state.doc.toJSON());
    expect(normalizePortable(a.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docA));
    expect(normalizePortable(b.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docB));
  }

  it("round-trips a table through Loro into a second editor, edited on both sides", async () => {
    const errors = vi.spyOn(console, "error");
    const docA = loroDocFrom(paragraphs("Before"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const a = bound(docA);
    const b = bound(docB);
    await tick();

    // Editor A writes a table of two rows and two columns, with a header row, and fills two cells.
    a.dispatch(a.state.tr.insert(a.state.doc.content.size, createTable(schema, 2, 2, true)));
    a.dispatch(a.state.tr.insertText("Name", cellStart(a, 0, 0)));
    a.dispatch(a.state.tr.insertText("x | y", cellStart(a, 1, 1)));
    expect(cellTexts(a)).toEqual([["Name", ""], ["", "x | y"]]);
    sync(docA, docB);

    expect(cellTexts(b)).toEqual([["Name", ""], ["", "x | y"]]);
    expectAgreement(a, docA, b, docB);
    // Stored as table > tableRow > tableHeader | tableCell > paragraph, the spans at their defaults.
    expect(shape(loroToPortable(docA))).toBe(
      "doc(paragraph(text) table(tableRow(tableHeader(paragraph(text)) tableHeader(paragraph)) " +
        "tableRow(tableCell(paragraph) tableCell(paragraph(text)))))",
    );
    expect(JSON.stringify(loroToPortable(docA))).toContain('"attrs":{"colspan":1,"rowspan":1}');

    // B adds a row below the first body row and deletes the first column while A types at the
    // end of the last cell: the three edits merge in both editors.
    b.dispatch(b.state.tr.setSelection(TextSelection.create(b.state.doc, cellStart(b, 1, 0))));
    expect(
      addRowAfter(b.state, (tr) => {
        b.dispatch(tr);
      }),
    ).toBe(true);
    b.dispatch(b.state.tr.setSelection(TextSelection.create(b.state.doc, cellStart(b, 0, 0))));
    expect(
      deleteColumn(b.state, (tr) => {
        b.dispatch(tr);
      }),
    ).toBe(true);
    expect(cellTexts(b)).toEqual([[""], ["x | y"], [""]]);
    a.dispatch(a.state.tr.insertText("!", cellStart(a, 1, 1) + "x | y".length));

    sync(docB, docA);
    sync(docA, docB);
    expect(cellTexts(a)).toEqual([[""], ["x | y!"], [""]]);
    expect(cellTexts(b)).toEqual([[""], ["x | y!"], [""]]);
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });

  it("shows a table the core wrote and writes nothing", async () => {
    const doc = loroDocFrom({
      type: "doc",
      content: [
        {
          type: "table",
          attrs: { id: pid(0) },
          content: [
            {
              type: "tableRow",
              attrs: { id: pid(1) },
              content: [
                {
                  type: "tableHeader",
                  attrs: { id: pid(2), colspan: 1, rowspan: 1 },
                  content: [{ type: "paragraph", attrs: { id: pid(3) }, content: [{ type: "text", text: "Head" }] }],
                },
                {
                  type: "tableCell",
                  attrs: { id: pid(4), colspan: 1, rowspan: 1 },
                  content: [{ type: "paragraph", attrs: { id: pid(5) }, content: [{ type: "text", text: "Cell" }] }],
                },
              ],
            },
          ],
        },
      ],
    });
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();
    expect(view.editable).toBe(true);
    expect(cellTexts(view)).toEqual([["Head", "Cell"]]);
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(doc));
    expect(doc.oplogVersion().compare(before)).toBe(0);
  });
});

describe("columns (additive nodes, 2026-10-06)", () => {
  /** A column list of columns holding one paragraph per text (`[]` is an empty column's paragraph). */
  function layout(...columns: string[][]): JsonValue {
    return {
      type: "columnList",
      content: columns.map((texts) => ({
        type: "column",
        content: (texts.length === 0 ? [""] : texts).map((text) => ({
          type: "paragraph",
          ...(text === "" ? {} : { content: [{ type: "text", text }] }),
        })),
      })),
    };
  }

  /** The texts of the blocks of every column of the first column list: `[["Left"], ["Right", "More"]]`. */
  function columnTexts(view: EditorView): string[][] {
    const columns: string[][] = [];
    view.state.doc.descendants((node) => {
      if (columns.length === 0 && node.type.name === "columnList") {
        node.forEach((column) => {
          const texts: string[] = [];
          column.forEach((block) => {
            texts.push(block.textContent);
          });
          columns.push(texts);
        });
      }
      return columns.length === 0;
    });
    return columns;
  }

  /** Where the text of the first block holding exactly `text` starts. */
  function textStart(view: EditorView, text: string): number {
    let found = -1;
    view.state.doc.descendants((node, pos) => {
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

  /** The node names of a portable tree: `doc(paragraph(text) columnList(column(...)))`. */
  function shape(json: JsonValue): string {
    if (typeof json !== "object" || json === null || Array.isArray(json)) {
      return "?";
    }
    const { content, type } = json;
    const name = typeof type === "string" ? type : "?";
    return Array.isArray(content) && content.length > 0 ? `${name}(${content.map(shape).join(" ")})` : name;
  }

  /** Both replicas hold the same document and each view shows exactly its replica. */
  function expectAgreement(a: EditorView, docA: LoroDoc, b: EditorView, docB: LoroDoc): void {
    expect(loroToPortable(docB)).toEqual(loroToPortable(docA));
    expect(b.state.doc.toJSON()).toEqual(a.state.doc.toJSON());
    expect(normalizePortable(a.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docA));
    expect(normalizePortable(b.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docB));
  }

  it("round-trips a column list through Loro into a second editor, edited on both sides", async () => {
    const errors = vi.spyOn(console, "error");
    const docA = loroDocFrom(paragraphs("Before"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const a = bound(docA);
    const b = bound(docB);
    await tick();

    // Editor A writes a layout of two columns (the right one with two blocks).
    a.dispatch(a.state.tr.insert(a.state.doc.content.size, schema.nodeFromJSON(layout(["Left"], ["Right", "More"]))));
    sync(docA, docB);

    expect(columnTexts(b)).toEqual([["Left"], ["Right", "More"]]);
    expectAgreement(a, docA, b, docB);
    // Stored as columnList > column > paragraph, each an element triple like any other block.
    expect(shape(loroToPortable(docA))).toBe(
      "doc(paragraph(text) columnList(column(paragraph(text)) column(paragraph(text) paragraph(text))))",
    );

    // A types in the left column while B adds a third column and types in the right one.
    a.dispatch(a.state.tr.insertText("!", textStart(a, "Left") + "Left".length));
    b.dispatch(b.state.tr.insertText("?", textStart(b, "More") + "More".length));
    let end = -1;
    b.state.doc.descendants((node, pos) => {
      if (node.type.name === "columnList") {
        end = pos + node.nodeSize - 1; // inside the list, after its last column
      }
      return end === -1;
    });
    b.dispatch(b.state.tr.insert(end, schema.nodeFromJSON((layout([""]) as { content: JsonValue[] }).content[0])));
    sync(docB, docA);
    sync(docA, docB);
    expect(columnTexts(a)).toEqual([["Left!"], ["Right", "More?"], [""]]);
    expect(columnTexts(b)).toEqual(columnTexts(a));
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });

  it("shows a layout the core wrote and writes nothing", async () => {
    const doc = loroDocFrom({
      type: "doc",
      content: [
        {
          type: "columnList",
          attrs: { id: pid(0) },
          content: [
            {
              type: "column",
              attrs: { id: pid(1) },
              content: [{ type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "Left" }] }],
            },
            {
              type: "column",
              attrs: { id: pid(3) },
              content: [{ type: "paragraph", attrs: { id: pid(4) }, content: [{ type: "text", text: "Right" }] }],
            },
          ],
        },
      ],
    });
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();
    expect(view.editable).toBe(true);
    expect(columnTexts(view)).toEqual([["Left"], ["Right"]]);
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(doc));
    expect(doc.oplogVersion().compare(before)).toBe(0);
  });

  it("merges edits in different columns, and a column added next to them", async () => {
    const errors = vi.spyOn(console, "error");
    const docA = loroDocFrom({ type: "doc", content: [layout(["One"], ["Two"], ["Three"])] });
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const a = bound(docA);
    const b = bound(docB);
    await tick();
    expect(columnTexts(a)).toEqual([["One"], ["Two"], ["Three"]]);

    // Typing in the first and the last column, concurrently with a new block in the middle one.
    a.dispatch(a.state.tr.insertText(">", textStart(a, "One")));
    a.dispatch(a.state.tr.insertText("<", textStart(a, "Three") + "Three".length));
    const middle = textStart(b, "Two") + "Two".length + 1; // after the paragraph, inside the column
    b.dispatch(b.state.tr.insert(middle, schema.nodeFromJSON({ type: "paragraph", content: [{ type: "text", text: "Extra" }] })));
    sync(docA, docB);
    sync(docB, docA);
    expect(columnTexts(a)).toEqual([[">One"], ["Two", "Extra"], ["Three<"]]);
    expectAgreement(a, docA, b, docB);
    expect(errors).not.toHaveBeenCalled();
  });
});

describe("derived blocks (additive nodes, 2026-10-06)", () => {
  function derivedPage(): JsonValue {
    return {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "tableOfContents", attrs: { id: pid(1) } },
        { type: "breadcrumb", attrs: { id: pid(2) } },
        { type: "paragraph", attrs: { id: pid(3) }, content: [{ type: "text", text: "After" }] },
      ],
    };
  }

  /** The names of the top-level blocks of a view. */
  function names(view: EditorView): string[] {
    const result: string[] = [];
    view.state.doc.forEach((node) => {
      result.push(node.type.name);
    });
    return result;
  }

  /** Where the `index`-th top-level block starts (just before it). */
  function blockAt(view: EditorView, index: number): number {
    return startOfBlock(view, index) - 1;
  }

  it("shows both blocks as leaves with their ids, and loading writes nothing", async () => {
    const doc = loroDocFrom(derivedPage());
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();

    expect(names(view)).toEqual(["paragraph", "tableOfContents", "breadcrumb", "paragraph"]);
    for (const index of [1, 2]) {
      const block = view.state.doc.child(index);
      expect(block.isLeaf).toBe(true);
      expect(block.attrs.id).toBe(pid(index));
    }
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(normalizePortable(derivedPage()));
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(loroToPortable(doc)).toEqual(derivedPage());
  });

  it("writes an inserted block as a leaf holding its id and nothing else", async () => {
    const doc = loroDocFrom(paragraphs("Before"));
    const view = bound(doc);
    await tick();
    const end = view.state.doc.content.size;

    view.dispatch(view.state.tr.insert(end, (schema.nodes.tableOfContents as NodeType).create({ id: pid(5) })));
    view.dispatch(view.state.tr.insert(view.state.doc.content.size, (schema.nodes.breadcrumb as NodeType).create({ id: pid(6) })));
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "tableOfContents", attrs: { id: pid(5) } },
        { type: "breadcrumb", attrs: { id: pid(6) } },
      ],
    });
    // Each is an element triple (name, attributes, children) with an empty children list.
    const children = doc.getMap("doc").get("children") as LoroList;
    for (const [index, name] of [
      [1, "tableOfContents"],
      [2, "breadcrumb"],
    ] as const) {
      const element = children.get(index) as LoroMap;
      expect(element.get("nodeName")).toBe(name);
      expect((element.get("attributes") as LoroMap).toJSON()).toEqual({ id: pid(index === 1 ? 5 : 6) });
      expect((element.get("children") as LoroList).length).toBe(0);
    }
  });

  it("round-trips through a second replica: inserted, moved and deleted on either side", async () => {
    const errors = vi.spyOn(console, "error");
    const docA = loroDocFrom(derivedPage());
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const a = bound(docA);
    const b = bound(docB);
    await tick();
    expect(names(b)).toEqual(["paragraph", "tableOfContents", "breadcrumb", "paragraph"]);

    // B deletes the table of contents while A types into the last paragraph and adds another one.
    b.dispatch(b.state.tr.delete(blockAt(b, 1), blockAt(b, 2)));
    a.dispatch(a.state.tr.insertText("!", startOfBlock(a, 3) + "After".length));
    a.dispatch(a.state.tr.insert(a.state.doc.child(0).nodeSize, (schema.nodes.tableOfContents as NodeType).create({ id: pid(7) })));
    sync(docA, docB);
    sync(docB, docA);
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(names(a)).toEqual(["paragraph", "tableOfContents", "breadcrumb", "paragraph"]);
    expect(a.state.doc.toJSON()).toEqual(b.state.doc.toJSON());
    expect(names(b)).toEqual(names(a));
    // The deleted block is gone on both sides and the one A added is there on both.
    const ids = (view: EditorView): unknown[] => {
      const found: unknown[] = [];
      view.state.doc.forEach((node) => {
        found.push(node.attrs.id);
      });
      return found;
    };
    expect(ids(a)).toEqual(ids(b));
    expect(ids(a)).not.toContain(pid(1));
    expect(ids(a)).toContain(pid(7));
    expect(a.state.doc.lastChild?.textContent).toBe("After!");

    // A moves the breadcrumb to the top (delete and insert, the id kept): B sees it there.
    const crumbs = a.state.doc.child(2);
    a.dispatch(a.state.tr.delete(blockAt(a, 2), blockAt(a, 3)));
    a.dispatch(a.state.tr.insert(0, crumbs));
    sync(docA, docB);
    expect(names(b)).toEqual(["breadcrumb", "paragraph", "tableOfContents", "paragraph"]);
    expect(b.state.doc.child(0).attrs.id).toBe(pid(2));
    expect(normalizePortable(b.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docB));
    expect(errors).not.toHaveBeenCalled();
  });
});

describe("list view (additive node, 2026-10-07)", () => {
  const TAG_OPTION = "3c9d1e52-6a47-4b8e-9f20-5d1c7a8b4e61";
  const PROPERTY_A = "5b2a7c14-9e30-4d68-8a15-3f6e0c9d2b74";
  const PROPERTY_B = "0000000a-0000-4000-8000-000000000001";

  /** A list view of the pages that carry a tag, with two properties shown and a sort by the first. */
  const configured = (id: string): JsonValue => ({
    type: "pageList",
    attrs: {
      id,
      source: { kind: "tag", optionId: TAG_OPTION },
      layout: "list",
      properties: [PROPERTY_A, PROPERTY_B],
      sort: { by: PROPERTY_A, dir: "desc" },
    },
  });

  function listPage(): JsonValue {
    return {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        configured(pid(1)),
        {
          type: "pageList",
          attrs: {
            id: pid(2),
            source: { kind: "children" },
            layout: "list",
            properties: [],
            sort: { by: "title", dir: "asc" },
          },
        },
        { type: "paragraph", attrs: { id: pid(3) }, content: [{ type: "text", text: "After" }] },
      ],
    };
  }

  const names = (view: EditorView): string[] => {
    const result: string[] = [];
    view.state.doc.forEach((node) => {
      result.push(node.type.name);
    });
    return result;
  };

  /** Where the `index`-th top-level block starts (just before it). */
  const blockAt = (view: EditorView, index: number): number => startOfBlock(view, index) - 1;

  it("shows the block as a leaf with its configuration, and loading writes nothing", async () => {
    const doc = loroDocFrom(listPage());
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();

    expect(names(view)).toEqual(["paragraph", "pageList", "pageList", "paragraph"]);
    const list = view.state.doc.child(1);
    expect(list.isLeaf).toBe(true);
    expect(list.attrs).toEqual({
      id: pid(1),
      source: { kind: "tag", optionId: TAG_OPTION },
      layout: "list",
      properties: [PROPERTY_A, PROPERTY_B],
      sort: { by: PROPERTY_A, dir: "desc" },
      // Absent from the document (a block written before them): `null`, which is never written.
      filters: null,
      columnWidths: null,
      groupBy: null,
      hiddenGroups: null,
      calculations: null,
      views: null,
    });
    expect(normalizePortable(view.state.doc.toJSON() as JsonValue)).toEqual(normalizePortable(listPage()));
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(loroToPortable(doc)).toEqual(listPage());
  });

  it("writes an inserted block as a leaf with its configuration: objects and a list as plain values", async () => {
    const doc = loroDocFrom(paragraphs("Before"));
    const view = bound(doc);
    await tick();

    view.dispatch(view.state.tr.insert(view.state.doc.content.size, (schema.nodes.pageList as NodeType).create({ id: pid(5) })));
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        {
          type: "pageList",
          attrs: {
            id: pid(5),
            source: { kind: "children" },
            layout: "list",
            properties: [],
            sort: { by: "title", dir: "asc" },
          },
        },
      ],
    });
    const element = (doc.getMap("doc").get("children") as LoroList).get(1) as LoroMap;
    expect(element.get("nodeName")).toBe("pageList");
    expect((element.get("children") as LoroList).length).toBe(0);
    // The three structured attributes are values of the attributes map, not containers of their own.
    const attributes = element.get("attributes") as LoroMap;
    expect(attributes.get("sort")).toEqual({ by: "title", dir: "asc" });
    expect(attributes.get("properties")).toEqual([]);
    expect(attributes.get("source")).toEqual({ kind: "children" });
  });

  it("writes only the attribute that changed, and keeps the id", async () => {
    const doc = loroDocFrom(listPage());
    const view = bound(doc);
    await tick();

    let before = doc.oplogVersion();
    view.dispatch(view.state.tr.setNodeAttribute(blockAt(view, 1), "sort", { by: "updated", dir: "asc" }));
    let ops = opsSince(doc, before);
    expect(ops).toHaveLength(1);
    expect(JSON.stringify(ops)).toContain('"sort"');
    expect(JSON.stringify(ops)).not.toContain('"properties"');
    expect(view.state.doc.child(1).attrs.id).toBe(pid(1));

    before = doc.oplogVersion();
    view.dispatch(view.state.tr.setNodeAttribute(blockAt(view, 1), "properties", [PROPERTY_B]));
    ops = opsSince(doc, before);
    expect(ops).toHaveLength(1);
    expect(JSON.stringify(ops)).toContain('"properties"');

    before = doc.oplogVersion();
    view.dispatch(view.state.tr.setNodeAttribute(blockAt(view, 1), "source", { kind: "children" }));
    expect(opsSince(doc, before)).toHaveLength(1);

    // Setting what is already there records nothing.
    before = doc.oplogVersion();
    view.dispatch(view.state.tr.setNodeAttribute(blockAt(view, 1), "layout", "list"));
    view.dispatch(view.state.tr.setNodeAttribute(blockAt(view, 1), "properties", [PROPERTY_B]));
    expect(opsSince(doc, before)).toEqual([]);

    expect((loroToPortable(doc) as { content: JsonValue[] }).content[1]).toEqual({
      type: "pageList",
      attrs: { id: pid(1), source: { kind: "children" }, layout: "list", properties: [PROPERTY_B], sort: { by: "updated", dir: "asc" } },
    });
  });

  it("keeps a configuration this version does not understand as it is", async () => {
    const odd: JsonValue = {
      type: "doc",
      content: [
        {
          type: "pageList",
          attrs: {
            id: pid(1),
            source: { kind: "query", text: "status = done" },
            layout: "gallery",
            properties: "all",
            sort: { by: "", dir: "sideways" },
          },
        },
        { type: "paragraph", attrs: { id: pid(2) } },
      ],
    };
    const doc = loroDocFrom(odd);
    const before = doc.oplogVersion();
    const view = bound(doc);
    await tick();
    expect(doc.oplogVersion().compare(before)).toBe(0);
    expect(view.state.doc.child(0).attrs.source).toEqual({ kind: "query", text: "status = done" });
    expect(view.state.doc.child(0).attrs.layout).toBe("gallery");
    // Typing in the paragraph below does not touch the block's attributes.
    view.dispatch(view.state.tr.insertText("x", startOfBlock(view, 1)));
    expect(loroToPortable(doc)).toMatchObject({
      content: [{ type: "pageList", attrs: { source: { kind: "query", text: "status = done" }, layout: "gallery", properties: "all" } }, {}],
    });
  });

  it("round-trips through a second replica: attributes changed on both sides merge, a deletion and a move agree", async () => {
    const errors = vi.spyOn(console, "error");
    const docA = loroDocFrom(listPage());
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    docA.setPeerId(1);
    docB.setPeerId(2);
    const a = bound(docA);
    const b = bound(docB);
    await tick();
    expect(names(b)).toEqual(["paragraph", "pageList", "pageList", "paragraph"]);

    // A changes the sort of the first list while B changes its properties and deletes the second.
    a.dispatch(a.state.tr.setNodeAttribute(blockAt(a, 1), "sort", { by: "created", dir: "asc" }));
    b.dispatch(b.state.tr.setNodeAttribute(blockAt(b, 1), "properties", []));
    b.dispatch(b.state.tr.delete(blockAt(b, 2), blockAt(b, 3)));
    sync(docA, docB);
    sync(docB, docA);
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(names(a)).toEqual(["paragraph", "pageList", "paragraph"]);
    expect(a.state.doc.toJSON()).toEqual(b.state.doc.toJSON());
    expect(a.state.doc.child(1).attrs).toMatchObject({ id: pid(1), properties: [], sort: { by: "created", dir: "asc" } });

    // A moves the list to the top (delete and insert, the id kept): B sees it there with its settings.
    const list = a.state.doc.child(1);
    a.dispatch(a.state.tr.delete(blockAt(a, 1), blockAt(a, 2)));
    a.dispatch(a.state.tr.insert(0, list));
    sync(docA, docB);
    expect(names(b)).toEqual(["pageList", "paragraph", "paragraph"]);
    expect(b.state.doc.child(0).attrs).toEqual(list.attrs);
    expect(normalizePortable(b.state.doc.toJSON() as JsonValue)).toEqual(loroToPortable(docB));
    expect(errors).not.toHaveBeenCalled();
  });
});
