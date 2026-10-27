// Toggle headings (ADR-0016, amendment of 2026-10-14; ADR-0040): the optional `level` of
// `detailsSummary` against the fixture, how the summary looks, the binding (the level reaches the
// document, an older editor shows a plain toggle), the `/toggle heading` entries, "turn into" both
// ways between headings, toggles and toggle headings with no content lost, and Markdown pasted
// back.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor, getSchema } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { DetailsSummary } from "@tiptap/extension-details";
import type { Node as PmNode } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { loroDocFrom, bindView, pid, tick } from "../test/editor";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { rangeOfBlocks } from "./block-selection";
import { canTurnBlocksInto, turnBlocksInto, turnIntoKindsOf } from "./block-range-commands";
import { kindOf, turnInto, turnIntoKinds } from "./blocks";
import type { BlockKind } from "./blocks";
import { LoroBinding } from "./extensions";
import { createEditorSchema, createSchemaExtensions } from "./schema";
import { filterSlashCommands, SLASH_COMMANDS } from "./slash-commands";
import { toggleHeadingLevel } from "./toggle-heading";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// ----- documents ---------------------------------------------------------------------------

function text(value: string, ...marks: string[]): JSONContent {
  return { type: "text", text: value, ...(marks.length === 0 ? {} : { marks: marks.map((type) => ({ type })) }) };
}

function paragraph(n: number, value: string): JSONContent {
  return { type: "paragraph", attrs: { id: pid(n) }, ...(value === "" ? {} : { content: [text(value)] }) };
}

function heading(n: number, level: number, ...content: JSONContent[]): JSONContent {
  return { type: "heading", attrs: { id: pid(n), level }, content };
}

function bullets(n: number, value: string): JSONContent {
  return {
    type: "bulletList",
    attrs: { id: pid(n) },
    content: [{ type: "listItem", attrs: { id: pid(n + 1) }, content: [paragraph(n + 2, value)] }],
  };
}

/** A toggle: the details block `n`, its summary `n + 1`, its content `n + 2`, then `body` (default one paragraph). */
function toggle(n: number, level: number | null, title: string, body?: JSONContent[]): JSONContent {
  return {
    type: "details",
    attrs: { id: pid(n), open: true },
    content: [
      {
        type: "detailsSummary",
        attrs: { id: pid(n + 1), ...(level === null ? {} : { level }) },
        ...(title === "" ? {} : { content: [text(title)] }),
      },
      { type: "detailsContent", attrs: { id: pid(n + 2) }, content: body ?? [paragraph(n + 3, "inside")] },
    ],
  };
}

function page(...blocks: JSONContent[]): JSONContent {
  return { type: "doc", content: blocks };
}

/** Portable JSON for a Loro document (the same plain objects). */
function portable(content: JSONContent): JsonValue {
  return content;
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A Tiptap editor over the schema extensions (and the Loro binding when given a document). */
function mount(content: JSONContent, doc?: LoroDoc): Editor {
  const extensions = createSchemaExtensions();
  if (doc !== undefined) {
    extensions.push(LoroBinding.configure({ doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions, content });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

/** The top-level blocks of the editor's document as plain JSON. */
function blocks(editor: Editor): JSONContent[] {
  const content = editor.state.doc.toJSON() as JSONContent;
  return content.content ?? [];
}

/** Position of the top-level block `index`. */
function startOf(editor: Editor, index: number): number {
  let pos = 0;
  for (let at = 0; at < index; at += 1) {
    pos += editor.state.doc.child(at).nodeSize;
  }
  return pos;
}

/** Turns top-level block `index` into `kind` and returns what the document is afterwards. */
function turn(editor: Editor, index: number, kind: BlockKind): JSONContent[] {
  const tr = turnInto(editor.state, startOf(editor, index), kind);
  if (tr === null) {
    throw new Error(`block ${String(index)} does not turn into ${kind}`);
  }
  editor.view.dispatch(tr);
  return blocks(editor);
}

/** What a block holds that matters here: its type and `level` (a heading's or its summary's), the end of its id and its text. */
function outline(editor: Editor): string[] {
  return blocks(editor).map((block) => {
    const summary = block.type === "details" ? block.content?.[0] : block;
    const level = block.type === "details" || block.type === "heading" ? (summary?.attrs as { level?: unknown } | undefined)?.level : undefined;
    const id = (block.attrs as { id?: unknown } | undefined)?.id;
    const own = typeof id === "string" ? id.slice(-2) : "?";
    const label = summary?.content?.map((part) => part.text ?? "").join("") ?? "";
    return `${block.type ?? "?"}${typeof level === "number" ? String(level) : ""}#${own}:${label}`;
  });
}

function nodeAt(editor: Editor, index: number): PmNode {
  return editor.state.doc.child(index);
}

// ----- the schema --------------------------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("gives the summary the optional attribute level after its id, in the fixture and in the editor", () => {
    expect(live.nodes.detailsSummary).toEqual({ content: "text*", group: null, inline: false, atom: false, attrs: ["id", "level"] });
    expect(fixture.nodes.detailsSummary).toEqual(live.nodes.detailsSummary);
    // No new node: the names of the fixture are the editor's.
    expect(Object.keys(fixture.nodes).sort()).toEqual(Object.keys(live.nodes).sort());
  });

  it("is a plain toggle by default: the level is null and not stored", () => {
    const summary = createEditorSchema().nodes.detailsSummary?.createAndFill();
    expect(summary?.attrs).toMatchObject({ level: null });
    const editor = mount(page(toggle(1, null, "Plain")));
    const loro = loroDocFrom(portable(page(toggle(1, null, "Plain"))));
    expect(JSON.stringify(loroToPortable(loro))).not.toContain('"level"');
    expect(nodeAt(editor, 0).child(0).attrs.level).toBeNull();
  });

  it("reads the level from the data-level of pasted HTML, and drops anything that is not 1 to 3", () => {
    const editor = mount(page(paragraph(1, "x")));
    /** The level of the summary of the toggle that `html` becomes. */
    const parsed = (html: string): unknown => {
      editor.commands.setContent(html);
      const block = nodeAt(editor, 0);
      return block.type.name === "details" ? block.child(0).attrs.level : "no toggle";
    };
    const wrap = (summary: string): string =>
      `<div data-type="details"><summary${summary}>T</summary><div data-type="detailsContent"><p>b</p></div></div>`;
    expect(parsed(wrap(' data-level="2"'))).toBe(2);
    expect(parsed(wrap(' data-level="3"'))).toBe(3);
    expect(parsed(wrap(""))).toBeNull();
    expect(parsed(wrap(' data-level="7"'))).toBeNull();
    expect(parsed(wrap(' data-level="x"'))).toBeNull();
  });
});

describe("the summary of a toggle heading", () => {
  it("is a summary with data-level and the class of its level, and a plain one has neither", () => {
    const editor = mount(page(toggle(1, 1, "One"), toggle(5, 2, "Two"), toggle(9, 3, "Three"), toggle(13, null, "Plain")));
    const summaries = [...editor.view.dom.querySelectorAll("summary")];
    expect(summaries.map((summary) => summary.getAttribute("data-level"))).toEqual(["1", "2", "3", null]);
    expect(summaries.map((summary) => summary.classList.contains("toggle-heading"))).toEqual([true, true, true, false]);
    expect(summaries.map((summary) => summary.textContent)).toEqual(["One", "Two", "Three", "Plain"]);
    expect(editor.view.dom.querySelectorAll("[data-type='details'] > button")).toHaveLength(4); // still toggles
  });

  it("keeps a level this version does not know as it is, and shows a plain toggle", () => {
    const editor = mount(page(toggle(1, 7, "Newer")));
    expect(nodeAt(editor, 0).child(0).attrs.level).toBe(7);
    const summary = editor.view.dom.querySelector("summary");
    expect(summary).not.toHaveAttribute("data-level");
    expect(summary).not.toHaveClass("toggle-heading");
  });

  it("is read from the document as a number: 1 to 3 only", () => {
    expect([1, 2, 3].map((level) => toggleHeadingLevel(level))).toEqual([1, 2, 3]);
    for (const other of [0, 4, 1.5, "2", null, undefined, true, NaN]) {
      expect(toggleHeadingLevel(other), String(other)).toBeNull();
    }
  });
});

// ----- the binding -------------------------------------------------------------------------

describe("the binding", () => {
  const stored = (doc: LoroDoc): { type: string; attrs?: { level?: unknown }; content?: { attrs?: { level?: unknown } }[] } => {
    const root = loroToPortable(doc) as { content: { type: string; content?: { attrs?: { level?: unknown } }[] }[] };
    const [first] = root.content;
    if (first === undefined) {
      throw new Error("no block");
    }
    return first;
  };

  it("reads the level from the document and writes a change of it back, and clearing it deletes the key", async () => {
    const doc = loroDocFrom(portable(page(toggle(1, 2, "Title"))));
    const editor = mount(page(paragraph(0, "")), doc);
    await tick();
    expect(nodeAt(editor, 0).child(0).attrs.level).toBe(2);
    expect(stored(doc).content?.[0]?.attrs?.level).toBe(2);

    editor.view.dispatch(editor.state.tr.setNodeAttribute(1, "level", 3));
    expect(stored(doc).content?.[0]?.attrs?.level).toBe(3);
    editor.view.dispatch(editor.state.tr.setNodeAttribute(1, "level", null));
    expect(stored(doc).content?.[0]?.attrs).not.toHaveProperty("level");
  });

  it("is a plain toggle in an editor from before this amendment, which keeps the level until its summary is edited", async () => {
    // An older editor's schema: the stock summary, no `level`.
    const older = getSchema(createSchemaExtensions().map((extension) => (extension.name === "detailsSummary" ? DetailsSummary : extension)));
    expect(older.nodes.detailsSummary?.spec.attrs).not.toHaveProperty("level");

    const doc = loroDocFrom(portable(page(toggle(1, 2, "Title"), paragraph(5, "after"))));
    const view = bindView(older, doc);
    await tick();
    const summary = view.view.state.doc.child(0).child(0);
    expect(summary.type.name).toBe("detailsSummary");
    expect(summary.attrs).not.toHaveProperty("level"); // a plain toggle
    expect(summary.textContent).toBe("Title");
    expect(stored(doc).content?.[0]?.attrs?.level).toBe(2); // the document still has it

    // Closing the toggle, typing in its body and typing elsewhere leave the summary alone.
    view.view.dispatch(view.view.state.tr.setNodeAttribute(0, "open", false));
    let body = -1;
    view.view.state.doc.descendants((node, pos) => {
      if (node.isText && node.text === "inside") {
        body = pos + node.nodeSize;
      }
      return body === -1;
    });
    expect(body).toBeGreaterThan(0);
    view.view.dispatch(view.view.state.tr.insertText("!", body));
    expect(view.view.state.doc.child(0).child(1).textContent).toBe("inside!");
    view.view.dispatch(view.view.state.tr.insertText("?", view.view.state.doc.content.size - 1));
    expect(stored(doc).content?.[0]?.attrs?.level).toBe(2);

    // Typing in the summary rewrites it without the attribute it does not hold, on every replica.
    view.view.dispatch(view.view.state.tr.insertText("x", 3));
    expect(stored(doc).content?.[0]?.attrs).not.toHaveProperty("level");
    view.destroy();
  });
});

// ----- /toggle heading ----------------------------------------------------------------------

describe("the slash entries", () => {
  const ids = ["toggleHeading1", "toggleHeading2", "toggleHeading3"];

  it("are three entries of the basic section, right after Toggle", () => {
    const all = SLASH_COMMANDS.map((command) => command.id);
    const at = all.indexOf("toggle");
    expect(all.slice(at, at + 5)).toEqual(["toggle", ...ids, "quote"]);
    for (const id of ids) {
      const command = SLASH_COMMANDS.find((entry) => entry.id === id);
      expect(command?.section).toBe("basic");
      expect(command?.keywords).toEqual(expect.arrayContaining(["toggle", "heading"]));
    }
    expect(SLASH_COMMANDS.find((entry) => entry.id === "toggleHeading1")?.keywords).toContain("h1");
    expect(SLASH_COMMANDS.find((entry) => entry.id === "toggleHeading2")?.keywords).toContain("h2");
    expect(SLASH_COMMANDS.find((entry) => entry.id === "toggleHeading3")?.keywords).toContain("h3");
  });

  it("are found by their name, by toggle, by heading and by h1 to h3", () => {
    const names = (query: string): string[] => filterSlashCommands(query).map((command) => command.id);
    expect(names("toggle heading 2")).toEqual(["toggleHeading2"]);
    expect(names("toggle")).toEqual(["toggle", ...ids]);
    expect(names("collapsible heading")).toEqual([]);
    expect(names("h2")).toEqual(["heading2", "toggleHeading2"]);
    expect(names("h3")).toEqual(["heading3", "toggleHeading3"]);
    expect(names("heading")).toEqual(expect.arrayContaining(ids));
    expect(names("fold")).toEqual(expect.arrayContaining(["toggle", ...ids]));
  });

  for (const level of [1, 2, 3]) {
    it(`makes a toggle heading of level ${String(level)} with the caret in its summary`, () => {
      const editor = mount(page(paragraph(0, "/toggle")));
      const command = SLASH_COMMANDS.find((entry) => entry.id === `toggleHeading${String(level)}`);
      expect(command).toBeDefined();
      command?.run(editor, { from: 1, to: 8 });

      const [block] = blocks(editor);
      expect(block?.type).toBe("details");
      expect(block?.content?.[0]).toMatchObject({ type: "detailsSummary", attrs: { level } });
      expect(editor.state.selection.$from.parent.type.name).toBe("detailsSummary");
      expect(editor.state.selection.$from.parent.attrs.level).toBe(level);
      // The summary and the block have ids, and the body is the (emptied) paragraph.
      expect((block?.attrs as { id: string }).id).toMatch(UUID);
      expect((block?.content?.[0]?.attrs as { id: string }).id).toMatch(UUID);
      expect(block?.content?.[1]?.content?.map((child) => child.type)).toEqual(["paragraph"]);
    });
  }

  it("leaves the plain toggle entry a plain toggle", () => {
    const editor = mount(page(paragraph(0, "/toggle")));
    SLASH_COMMANDS.find((entry) => entry.id === "toggle")?.run(editor, { from: 1, to: 8 });
    expect(nodeAt(editor, 0).child(0).attrs.level).toBeNull();
  });
});

// ----- turn into ---------------------------------------------------------------------------

describe("turn into", () => {
  const TOGGLES: BlockKind[] = ["toggle", "toggleHeading1", "toggleHeading2", "toggleHeading3"];

  it("offers the toggle kinds for text blocks and toggles, and leaves them out of the rest", () => {
    const editor = mount(
      page(paragraph(1, "p"), heading(2, 2, text("h")), toggle(3, 2, "t"), bullets(7, "l"), {
        type: "blockquote",
        attrs: { id: pid(10) },
        content: [paragraph(11, "q")],
      }),
    );
    for (const index of [0, 1, 2]) {
      expect(turnIntoKinds(nodeAt(editor, index)), String(index)).toEqual(expect.arrayContaining(TOGGLES));
    }
    for (const index of [3, 4]) {
      for (const kind of TOGGLES) {
        expect(turnIntoKinds(nodeAt(editor, index)), String(index)).not.toContain(kind);
      }
    }
    expect(kindOf(nodeAt(editor, 2))).toBe("toggleHeading2");
    expect(kindOf(mount(page(toggle(1, null, "t"))).state.doc.child(0))).toBe("toggle");
    expect(kindOf(mount(page(toggle(1, 7, "t"))).state.doc.child(0))).toBe("toggle"); // an unknown level is plain
  });

  it("turns a heading into a toggle heading: the text is the summary, the body is empty, the block keeps its id", () => {
    const editor = mount(page(paragraph(0, "before"), heading(1, 2, text("Plan "), text("ahead", "bold")), paragraph(2, "after")));
    const after = turn(editor, 1, "toggleHeading3");

    expect(after).toHaveLength(3);
    expect(outline(editor)).toEqual(["paragraph#00:before", "details3#01:Plan ahead", "paragraph#02:after"]);
    const block = nodeAt(editor, 1);
    expect(block.attrs).toMatchObject({ id: pid(1), open: false });
    const summary = block.child(0);
    expect(summary.attrs.level).toBe(3);
    expect(summary.attrs.id).toMatch(UUID);
    expect(summary.child(1).marks.map((mark) => mark.type.name)).toEqual(["bold"]); // the text keeps its marks
    const body = block.child(1);
    expect(body.childCount).toBe(1);
    expect(body.child(0).type.name).toBe("paragraph");
    expect(body.child(0).childCount).toBe(0); // empty
  });

  it("turns a paragraph into a toggle heading or a plain toggle, in one step", () => {
    const editor = mount(page(paragraph(0, "Notes")));
    expect(outline(turnEditor(editor, 0, "toggleHeading1"))).toEqual(["details1#00:Notes"]);
    const plain = mount(page(paragraph(0, "Notes")));
    expect(outline(turnEditor(plain, 0, "toggle"))).toEqual(["details#00:Notes"]);
    expect(nodeAt(plain, 0).child(0).attrs.level).toBeNull();
  });

  it("keeps what a heading holds that a summary cannot: dates and equations as text, a line break as a space", () => {
    const editor = mount(
      page(
        heading(
          0,
          1,
          text("Due "),
          { type: "dateMention", attrs: { id: pid(5), date: "2026-10-14", time: "09:30" } },
          { type: "hardBreak" },
          { type: "mathInline", attrs: { id: pid(6), latex: "x^2" } },
        ),
      ),
    );
    turn(editor, 0, "toggleHeading2");
    expect(nodeAt(editor, 0).child(0).textContent).toBe("Due 2026-10-14 09:30 x^2");
  });

  it("turns a toggle heading into a heading: the summary's text, then the body's blocks, none lost", () => {
    const body = [paragraph(10, "one"), bullets(11, "two"), paragraph(14, "three")];
    const editor = mount(page(paragraph(0, "before"), toggle(1, 2, "Title", body), paragraph(20, "after")));
    const textBefore = editor.state.doc.textContent;
    turn(editor, 1, "heading1");

    expect(outline(editor)).toEqual([
      "paragraph#00:before",
      "heading1#01:Title", // the block's id
      "paragraph#10:one", // the body's blocks keep theirs
      "bulletList#11:",
      "paragraph#14:three",
      "paragraph#20:after",
    ]);
    expect(nodeAt(editor, 1).attrs).toMatchObject({ id: pid(1), level: 1 });
    expect(editor.state.doc.textContent).toBe(textBefore);
    // The list kept its items too.
    expect(nodeAt(editor, 3).textContent).toBe("two");
  });

  it("goes from a heading to a toggle heading and back to the same heading", () => {
    const original = page(paragraph(0, "a"), heading(1, 2, text("Same")), paragraph(2, "b"));
    const editor = mount(original);
    const before = blocks(editor);
    turn(editor, 1, "toggleHeading2");
    expect(nodeAt(editor, 1).type.name).toBe("details");
    turn(editor, 1, "heading2");
    expect(blocks(editor)).toEqual(before); // the empty body was not left behind
    expect(editor.state.doc.childCount).toBe(3);
  });

  it("turns a toggle into a toggle heading and back by setting and clearing the level, and nothing else", () => {
    const editor = mount(page(toggle(1, null, "Fold", [paragraph(10, "x"), paragraph(11, "y")])));
    const before = blocks(editor)[0] as JSONContent;

    turn(editor, 0, "toggleHeading2");
    const heading2 = blocks(editor)[0] as JSONContent;
    expect(heading2.content?.[0]?.attrs).toEqual({ id: pid(2), level: 2 });
    expect(heading2.attrs).toEqual(before.attrs); // same block, same id, same open state
    expect(heading2.content?.[1]).toEqual(before.content?.[1]); // the body is untouched

    turn(editor, 0, "toggleHeading3"); // level to level
    expect(nodeAt(editor, 0).child(0).attrs.level).toBe(3);
    expect(nodeAt(editor, 0).child(1).textContent).toBe("xy");

    turn(editor, 0, "toggle");
    expect(blocks(editor)[0]).toEqual(before);
    expect(nodeAt(editor, 0).child(0).attrs.level).toBeNull();
  });

  it("does nothing for a block that already is the kind", () => {
    const editor = mount(page(toggle(1, 2, "T"), toggle(5, null, "P"), heading(9, 1, text("H"))));
    expect(turnInto(editor.state, startOf(editor, 0), "toggleHeading2")).toBeNull();
    expect(turnInto(editor.state, startOf(editor, 1), "toggle")).toBeNull();
    expect(turnInto(editor.state, startOf(editor, 2), "heading1")).toBeNull();
  });

  it("lifts the body of a plain toggle too when it becomes a heading or a paragraph", () => {
    const editor = mount(page(toggle(1, null, "Fold", [paragraph(10, "x")]), toggle(5, 3, "Head", [paragraph(14, "y")])));
    turn(editor, 0, "heading2");
    expect(outline(editor)).toEqual(["heading2#01:Fold", "paragraph#10:x", "details3#05:Head"]);
    turn(editor, 2, "paragraph");
    expect(outline(editor)).toEqual(["heading2#01:Fold", "paragraph#10:x", "paragraph#05:Head", "paragraph#14:y"]);
  });

  it("turns a toggle heading into the other text kinds as a toggle did before (every text block)", () => {
    const editor = mount(page(toggle(1, 1, "Title", [paragraph(10, "x")])));
    turn(editor, 0, "bulletList");
    expect(nodeAt(editor, 0).type.name).toBe("bulletList");
    expect(nodeAt(editor, 0).textContent).toBe("Titlex");
  });

  it("writes the new block to the document with its level", async () => {
    const doc = loroDocFrom(portable(page(heading(1, 2, text("Plan")))));
    const editor = mount(page(paragraph(0, "")), doc);
    await tick();
    expect(nodeAt(editor, 0).type.name).toBe("heading");
    turn(editor, 0, "toggleHeading2");
    expect(nodeAt(editor, 0).type.name).toBe("details");
    expect(loroToPortable(doc)).toMatchObject({ content: [{ type: "details", content: [{ attrs: { level: 2 } }, {}] }] });
  });

  it("works on a range of blocks: the ones that can change do, the others stay", () => {
    const editor = mount(page(paragraph(0, "a"), heading(1, 3, text("b")), bullets(2, "c"), toggle(6, null, "d")));
    const range = rangeOfBlocks(editor.state.doc, 0, 3);
    expect(turnIntoKindsOf(editor.state, range)).toEqual(expect.arrayContaining(["toggleHeading1"]));
    expect(canTurnBlocksInto(editor.state, range, "toggleHeading1")).toBe(true);
    const tr = turnBlocksInto(editor.state, range, "toggleHeading1");
    expect(tr).not.toBeNull();
    if (tr !== null) {
      editor.view.dispatch(tr);
    }
    expect(outline(editor)).toEqual(["details1#00:a", "details1#01:b", "bulletList#02:", "details1#06:d"]);
    // A range of lists alone has nothing to change.
    expect(canTurnBlocksInto(editor.state, rangeOfBlocks(editor.state.doc, 2, 2), "toggleHeading1")).toBe(false);
  });

  it("puts the caret in the new block", () => {
    const editor = mount(page(heading(0, 1, text("Caret"))));
    turn(editor, 0, "toggleHeading1");
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
    expect(editor.state.selection.$from.parent.type.name).toBe("detailsSummary");
  });
});

/** `turn` for tests that read the editor afterwards. */
function turnEditor(editor: Editor, index: number, kind: BlockKind): Editor {
  turn(editor, index, kind);
  return editor;
}
