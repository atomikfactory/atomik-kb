// The table of contents block (ADR-0016, amendment of 2026-10-06): the schema against the
// fixture, the pure reading of headings, `/toc`, the list in the page editor (it follows the
// headings as they are added, renamed and removed, a click moves the caret to the heading), the
// hint of a page without headings, read-only editors, deleting the block, and that the view
// writes nothing to the document.

import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { duplicateBlock, turnInto, turnIntoKinds } from "./blocks";
import { loroSyncPluginKey } from "./loro-binding";
import { createEditorSchema, ID_BEARING_NODES } from "./schema";
import { filterSlashCommands, SLASH_COMMANDS } from "./slash-commands";
import { collectHeadings, findHeading } from "./table-of-contents";

afterEach(() => {
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// ----- documents ---------------------------------------------------------------------------

function heading(n: number, level: number, value: string): JsonValue {
  return {
    type: "heading",
    attrs: { id: pid(n), level },
    ...(value === "" ? {} : { content: [{ type: "text", text: value }] }),
  };
}

function paragraph(n: number, value: string): JsonValue {
  return { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

function toc(n: number): JsonValue {
  return { type: "tableOfContents", attrs: { id: pid(n) } };
}

function page(...blocks: JsonValue[]): JsonValue {
  return { type: "doc", content: blocks };
}

/** The table of contents on top, an introduction, a section with two subsections, and a second section. */
function guide(): JsonValue {
  return page(
    toc(1),
    heading(2, 1, "Introduction"),
    paragraph(3, "Some words."),
    heading(4, 2, "Setup"),
    heading(5, 3, "Install"),
    paragraph(6, "More words."),
    heading(7, 2, "Usage"),
  );
}

// ----- looking at the list -----------------------------------------------------------------

function list(): HTMLElement {
  return screen.getByRole("navigation", { name: t("toc.label") });
}

/** The lines of the list as `level:text`. */
function lines(): string[] {
  return within(list())
    .queryAllByRole("button")
    .map((button) => `${button.closest("li")?.getAttribute("data-level") ?? "?"}:${button.textContent ?? ""}`);
}

/** Where the heading with `value` starts in the editor's document. */
function headingStart(editor: Editor, value: string): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found === -1 && node.type.name === "heading" && node.textContent === value) {
      found = pos;
    }
    return found === -1;
  });
  if (found === -1) {
    throw new Error(`no heading ${value}`);
  }
  return found;
}

/** Deletes the block at `pos`. */
function deleteAt(editor: Editor, pos: number): void {
  const size = editor.state.doc.nodeAt(pos)?.nodeSize ?? 0;
  editor.view.dispatch(editor.state.tr.delete(pos, pos + size));
}

/** Runs an editor change the way the app does and lets React settle. */
function change(run: () => void): void {
  act(() => {
    run();
  });
}

function button(name: string, index = 0): HTMLElement {
  const found = within(list()).getAllByRole("button", { name })[index];
  if (found === undefined) {
    throw new Error(`no line ${name}`);
  }
  return found;
}

async function openGuide(content: JsonValue = guide()) {
  const app = await renderPages([{ name: "Personal", pages: [{ title: "Guide", content }] }]);
  const editor = await openPage(app.user, "Guide");
  return { ...app, editor };
}

// ----- the schema --------------------------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("has the table of contents of the fixture: a leaf atom block with a block id and nothing else", () => {
    expect(live.nodes.tableOfContents).toEqual({ content: "", group: "block", inline: false, atom: true, attrs: ["id"] });
    expect(fixture.nodes.tableOfContents).toEqual(live.nodes.tableOfContents);
    expect(fixture.idBearing).toContain("tableOfContents");
    expect(ID_BEARING_NODES).toContain("tableOfContents");
  });

  it("is a leaf and an atom, and HTML <div data-type=tableOfContents>", () => {
    const type = createEditorSchema().nodes.tableOfContents;
    expect(type).toBeDefined();
    const node = type?.create({ id: pid(9) });
    expect(node?.isLeaf).toBe(true);
    expect(node?.isAtom).toBe(true);
    expect(type?.spec.parseDOM).toMatchObject([{ tag: 'div[data-type="tableOfContents"]' }]);
  });
});

// ----- reading headings --------------------------------------------------------------------

describe("collectHeadings and findHeading", () => {
  const schema = createEditorSchema();
  const pm = (...blocks: JsonValue[]): PmNode => schema.nodeFromJSON(page(...blocks));

  it("lists the headings of levels 1 to 3 in reading order, with their text and block id", () => {
    const doc = pm(heading(1, 1, "One"), paragraph(2, "text"), heading(3, 3, "Three"), heading(4, 2, "Two"));
    expect(collectHeadings(doc)).toEqual([
      { id: pid(1), level: 1, text: "One" },
      { id: pid(3), level: 3, text: "Three" },
      { id: pid(4), level: 2, text: "Two" },
    ]);
  });

  it("reads a heading as one line of plain text: marks dropped, a line break a space, a page reference nothing", () => {
    const doc = pm({
      type: "heading",
      attrs: { id: pid(1), level: 2 },
      content: [
        { type: "text", text: "Plan " },
        { type: "text", marks: [{ type: "bold" }], text: "A" },
        { type: "hardBreak" },
        { type: "text", marks: [{ type: "code" }], text: "b" },
        { type: "pageRef", attrs: { id: pid(2), pageId: "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6" } },
      ],
    });
    expect(collectHeadings(doc).map((entry) => entry.text)).toEqual(["Plan A b"]);
    expect(collectHeadings(pm(heading(3, 1, ""))).map((entry) => entry.text)).toEqual([""]);
  });

  it("finds the headings inside columns, callouts, toggles and table cells too", () => {
    const column = (n: number, ...blocks: JsonValue[]): JsonValue => ({ type: "column", attrs: { id: pid(n) }, content: blocks });
    const doc = pm(
      heading(1, 1, "Top"),
      {
        type: "columnList",
        attrs: { id: pid(2) },
        content: [column(3, heading(4, 2, "Left")), column(5, paragraph(6, "x"), heading(7, 2, "Right"))],
      },
      { type: "callout", attrs: { id: pid(8) }, content: [heading(9, 3, "Noted")] },
      {
        type: "details",
        attrs: { id: pid(10), open: true },
        content: [
          { type: "detailsSummary", attrs: { id: pid(11) }, content: [{ type: "text", text: "Fold" }] },
          { type: "detailsContent", attrs: { id: pid(12) }, content: [heading(13, 2, "Hidden")] },
        ],
      },
      {
        type: "table",
        attrs: { id: pid(14) },
        content: [
          {
            type: "tableRow",
            attrs: { id: pid(15) },
            content: [{ type: "tableCell", attrs: { id: pid(16) }, content: [heading(17, 3, "Cell")] }],
          },
        ],
      },
    );
    expect(collectHeadings(doc).map((entry) => entry.text)).toEqual(["Top", "Left", "Right", "Noted", "Hidden", "Cell"]);
  });

  it("leaves out the levels the editor has no heading for (4 and deeper)", () => {
    const doc = pm(heading(1, 1, "One"), heading(2, 4, "Deep"), heading(3, 6, "Deeper"), heading(4, 3, "Three"));
    expect(collectHeadings(doc).map((entry) => entry.text)).toEqual(["One", "Three"]);
  });

  it("finds a heading by its block id", () => {
    const doc = pm(heading(1, 1, "One"), paragraph(2, "text"), heading(3, 2, "Two"));
    const two = collectHeadings(doc)[1];
    if (two === undefined) {
      throw new Error("no second line");
    }
    expect(doc.nodeAt(findHeading(doc, two, 1) ?? -1)?.textContent).toBe("Two");
    // The id wins over the place in the list.
    expect(doc.nodeAt(findHeading(doc, two, 0) ?? -1)?.textContent).toBe("Two");
    expect(findHeading(doc, { id: pid(99), level: 2, text: "Gone" }, 1)).toBeNull();
  });

  it("finds a heading without an id by its place in the list", () => {
    // An editor never holds a heading without an id (UniqueID gives it one), a reader may.
    const bare = (level: number, value: string): JsonValue => ({
      type: "heading",
      attrs: { level },
      content: [{ type: "text", text: value }],
    });
    const doc = pm(bare(1, "A"), paragraph(2, "text"), bare(2, "B"));
    const entries = collectHeadings(doc);
    expect(entries.map((entry) => entry.id)).toEqual([null, null]);
    const second = entries[1];
    if (second === undefined) {
      throw new Error("no second line");
    }
    expect(doc.nodeAt(findHeading(doc, second, 1) ?? -1)?.textContent).toBe("B");
    expect(findHeading(doc, second, 5)).toBeNull();
  });
});

// ----- the / menu --------------------------------------------------------------------------

describe("the / menu", () => {
  it("has the entry, found by its name and by toc, contents, headings and outline", () => {
    const entry = SLASH_COMMANDS.find((command) => command.id === "toc");
    expect(entry).toBeDefined();
    expect(entry === undefined ? null : t(entry.label)).toBe("Table of contents");
    for (const word of ["toc", "contents", "headings", "outline", "table of"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("toc");
    }
    expect(filterSlashCommands("path").map((command) => command.id)).not.toContain("toc");
  });

  it("inserts the block where /toc was typed, with an id and a paragraph to type into below it", async () => {
    const empty: JsonValue = { type: "paragraph", attrs: { id: pid(1) } };
    const { user, memory, editor } = await openGuide(page(paragraph(0, "Intro"), empty));
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize + 1)),
    );
    editor.commands.insertContent("/toc");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual([
        "Table of contents",
      ]);
    });
    await user.click(within(menu).getByText("Table of contents"));

    expect(editor.state.doc.childCount).toBe(3);
    expect(editor.state.doc.child(0).textContent).toBe("Intro");
    const block = editor.state.doc.child(1);
    const blockId: unknown = block.attrs.id;
    expect(block.type.name).toBe("tableOfContents");
    expect(blockId).toMatch(UUID);
    // The empty paragraph the `/toc` was typed in became the block; one follows it for the caret.
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
    expect(await screen.findByRole("navigation", { name: t("toc.label") })).toBeInTheDocument();
    expect(screen.getByText(t("toc.empty"))).toBeInTheDocument();

    // What reaches the core is the block with its id, and nothing of the list it shows.
    const { page: pageId } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, pageId("Guide")))).toContain('"tableOfContents"');
    });
    const stored = (await contentOf(memory, pageId("Guide"))) as { content: { type: string }[] };
    expect(stored.content.find((node) => node.type === "tableOfContents")).toEqual({
      type: "tableOfContents",
      attrs: { id: blockId },
    });
  });

  it("splits a paragraph it is typed in, and goes below a selected block", async () => {
    const { editor } = await openGuide(page(paragraph(0, "ab")));
    const names = (): string[] => {
      const result: string[] = [];
      editor.state.doc.forEach((node) => {
        result.push(node.type.name);
      });
      return result;
    };
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, 2)));
    expect(editor.commands.insertTableOfContents()).toBe(true);
    expect(names()).toEqual(["paragraph", "tableOfContents", "paragraph"]);
    // The caret lands in the paragraph after the block, and a selected block gets one below it.
    expect(editor.state.selection.$from.parent.type.name).toBe("paragraph");
    editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize)));
    expect(editor.commands.insertTableOfContents()).toBe(true);
    expect(names()).toEqual(["paragraph", "tableOfContents", "tableOfContents", "paragraph"]);
  });
});

// ----- the list in the page editor ----------------------------------------------------------

describe("the list", () => {
  it("shows the headings in order, indented by level, with no hint", async () => {
    await openGuide();
    expect(lines()).toEqual(["1:Introduction", "2:Setup", "3:Install", "2:Usage"]);
    expect(screen.queryByText(t("toc.empty"))).not.toBeInTheDocument();
    expect(within(list()).getByText(t("toc.title"))).toBeInTheDocument();
    for (const item of within(list()).getAllByRole("listitem")) {
      expect(item.querySelector("button")).toHaveAttribute("type", "button");
    }
  });

  it("follows the headings as they are added, renamed and removed", async () => {
    const { editor } = await openGuide();
    expect(lines()).toEqual(["1:Introduction", "2:Setup", "3:Install", "2:Usage"]);

    // Added: a heading at the end of the page, and a new first heading above the table of contents.
    change(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, {
        type: "heading",
        attrs: { level: 2 },
        content: [{ type: "text", text: "Appendix" }],
      });
    });
    expect(lines()).toEqual(["1:Introduction", "2:Setup", "3:Install", "2:Usage", "2:Appendix"]);
    change(() => {
      editor.commands.insertContentAt(0, { type: "heading", attrs: { level: 1 }, content: [{ type: "text", text: "Title" }] });
    });
    expect(lines()).toEqual(["1:Title", "1:Introduction", "2:Setup", "3:Install", "2:Usage", "2:Appendix"]);

    // Renamed: text typed into a heading, and a heading given another level.
    const setup = headingStart(editor, "Setup");
    change(() => {
      editor.view.dispatch(editor.state.tr.insertText("ing up", setup + 1 + "Setup".length));
    });
    expect(lines()).toEqual(["1:Title", "1:Introduction", "2:Setuping up", "3:Install", "2:Usage", "2:Appendix"]);
    change(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(headingStart(editor, "Install"), "level", 2));
    });
    expect(lines()).toEqual(["1:Title", "1:Introduction", "2:Setuping up", "2:Install", "2:Usage", "2:Appendix"]);

    // Removed: a heading deleted, and one turned into a paragraph.
    change(() => {
      deleteAt(editor, headingStart(editor, "Usage"));
    });
    expect(lines()).toEqual(["1:Title", "1:Introduction", "2:Setuping up", "2:Install", "2:Appendix"]);
    change(() => {
      const at = headingStart(editor, "Title");
      editor.view.dispatch(editor.state.tr.setNodeMarkup(at, editor.schema.nodes.paragraph, { id: pid(40) }));
    });
    expect(lines()).toEqual(["1:Introduction", "2:Setuping up", "2:Install", "2:Appendix"]);
  });

  it("shows an empty heading as an untitled line, and its text once it is typed", async () => {
    const { editor } = await openGuide(page(toc(1), heading(2, 1, "First"), heading(3, 2, "")));
    expect(lines()).toEqual(["1:First", `2:${t("toc.untitled")}`]);
    change(() => {
      editor.view.dispatch(editor.state.tr.insertText("Second", headingStart(editor, "") + 1));
    });
    expect(lines()).toEqual(["1:First", "2:Second"]);
  });

  it("shows a hint while the page has no headings, and the list once it has one", async () => {
    const { editor } = await openGuide(page(toc(1), paragraph(2, "Only text.")));
    expect(screen.getByText(t("toc.empty"))).toBeInTheDocument();
    expect(within(list()).queryAllByRole("button")).toHaveLength(0);

    change(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, {
        type: "heading",
        attrs: { level: 1 },
        content: [{ type: "text", text: "Now there is one" }],
      });
    });
    expect(screen.queryByText(t("toc.empty"))).not.toBeInTheDocument();
    expect(lines()).toEqual(["1:Now there is one"]);

    change(() => {
      deleteAt(editor, headingStart(editor, "Now there is one"));
    });
    expect(screen.getByText(t("toc.empty"))).toBeInTheDocument();
  });

  it("is the same in every table of contents of the page", async () => {
    await openGuide(page(toc(1), heading(2, 1, "Only"), toc(3)));
    const lists = screen.getAllByRole("navigation", { name: t("toc.label") });
    expect(lists).toHaveLength(2);
    for (const one of lists) {
      expect(within(one).getAllByRole("button").map((item) => item.textContent)).toEqual(["Only"]);
    }
  });
});

// ----- clicking a line ---------------------------------------------------------------------

describe("a click on a line", () => {
  it("scrolls the heading into view and puts the caret at its start", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user, editor } = await openGuide();
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.content.size - 2)),
    );
    expect(editor.state.selection.$from.parent.textContent).toBe("Usage");

    await user.click(button("Setup"));

    const { selection } = editor.state;
    expect(selection).toBeInstanceOf(TextSelection);
    expect(selection.empty).toBe(true);
    expect(selection.$from.parent.type.name).toBe("heading");
    expect(selection.$from.parent.textContent).toBe("Setup");
    expect(selection.$from.parentOffset).toBe(0);
    expect(selection.from).toBe(headingStart(editor, "Setup") + 1);
    expect(scroll).toHaveBeenCalledWith({ block: "center" });
    expect(scroll.mock.contexts.at(-1)).toBe(editor.view.nodeDOM(headingStart(editor, "Setup")));

    await user.click(button("Introduction"));
    expect(editor.state.selection.$from.parent.textContent).toBe("Introduction");
    expect(scroll.mock.contexts.at(-1)).toBe(editor.view.nodeDOM(headingStart(editor, "Introduction")));
  });

  it("goes to the right heading when two have the same text", async () => {
    const { user, editor } = await openGuide(page(toc(1), heading(2, 2, "Notes"), paragraph(3, "x"), heading(4, 2, "Notes")));
    await user.click(button("Notes", 1));
    expect(editor.state.selection.$from.parent.attrs.id).toBe(pid(4));
    await user.click(button("Notes", 0));
    expect(editor.state.selection.$from.parent.attrs.id).toBe(pid(2));
  });

  it("follows a heading that moved since the list was drawn, and does nothing for one that is gone", async () => {
    const { user, editor } = await openGuide();
    const usage = button("Usage");
    const before = headingStart(editor, "Usage");
    change(() => {
      editor.view.dispatch(editor.state.tr.insertText("Words in front. ", editor.state.doc.child(0).nodeSize + 1));
    });
    expect(headingStart(editor, "Usage")).toBeGreaterThan(before);
    await user.click(usage);
    expect(editor.state.selection.from).toBe(headingStart(editor, "Usage") + 1);

    // A line whose heading was just deleted (the list redraws next): the selection stays put.
    const stale = button("Setup");
    deleteAt(editor, headingStart(editor, "Setup"));
    const selection = editor.state.selection.from;
    fireEvent.click(stale);
    expect(editor.state.selection.from).toBe(selection);
  });
});

// ----- read-only editors -------------------------------------------------------------------

describe("in a read-only editor", () => {
  it("shows the same list, and a click moves the caret without writing", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = page(toc(1), heading(2, 1, "Kept"), heading(3, 2, "As it is"), {
      type: "kanban",
      attrs: { id: "k0", columns: 3 },
    });
    const { memory, editor } = await openGuide(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(lines()).toEqual(["1:Kept", "2:As it is"]);

    // A bare click event: user-event would also put the DOM selection in the button, which
    // ProseMirror then reads back into the editor's selection (a browser keeps it out of buttons).
    fireEvent.click(button("As it is"));
    expect(editor.state.selection.$from.parent.textContent).toBe("As it is");

    const { page: pageId } = await idsOf(memory);
    expect(await contentOf(memory, pageId("Guide"))).toEqual(content);
  });
});

// ----- the block itself --------------------------------------------------------------------

describe("the block", () => {
  it("is selected as a whole, outlined, and deleted with Backspace or Delete", async () => {
    const { editor } = await openGuide();
    expect(editor.state.doc.child(0).type.name).toBe("tableOfContents");
    change(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    });
    const dom = editor.view.nodeDOM(0);
    expect(dom).toBeInstanceOf(HTMLElement);
    expect(dom).toHaveClass("ProseMirror-selectednode");
    expect(dom).toHaveAttribute("data-type", "tableOfContents");

    change(() => {
      expect(editor.commands.keyboardShortcut("Backspace")).toBe(true);
    });
    expect(editor.state.doc.firstChild?.type.name).toBe("heading");
    expect(screen.queryByRole("navigation", { name: t("toc.label") })).not.toBeInTheDocument();
    expect(JSON.stringify(editor.getJSON())).not.toContain("tableOfContents");

    // The same with Delete, on a block in the middle of the page.
    const middle = editor.state.doc.child(0).nodeSize;
    change(() => {
      editor.commands.insertContentAt(middle, { type: "tableOfContents" });
    });
    expect(editor.state.doc.nodeAt(middle)?.type.name).toBe("tableOfContents");
    change(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, middle)));
      expect(editor.commands.keyboardShortcut("Delete")).toBe(true);
    });
    expect(JSON.stringify(editor.getJSON())).not.toContain("tableOfContents");
  });

  it("gets a block id of its own when it is inserted, and keeps it", async () => {
    const { editor } = await openGuide(page(paragraph(0, "x")));
    change(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, { type: "tableOfContents" });
    });
    const id: unknown = editor.state.doc.lastChild?.attrs.id;
    expect(id).toMatch(UUID);
    change(() => {
      editor.view.dispatch(editor.state.tr.insertText("y", 2));
    });
    expect(editor.state.doc.lastChild?.attrs.id).toBe(id);
  });
});

// ----- nothing is written ------------------------------------------------------------------

describe("the document", () => {
  it("records no change when the page is shown, the list drawn and its lines clicked", async () => {
    const { user, editor } = await openGuide();
    const doc = loroSyncPluginKey.getState(editor.state)?.doc as LoroDoc;
    // This replica wrote nothing: it only imported what the core held.
    const own = (): number => doc.oplogVersion().get(doc.peerIdStr) ?? 0;
    expect(own()).toBe(0);

    for (const name of ["Introduction", "Install", "Usage", "Setup"]) {
      await user.click(button(name));
    }
    expect(own()).toBe(0);
    expect(editor.state.selection.$from.parent.textContent).toBe("Setup");
  });
});

describe("the block menu", () => {
  it("offers a quote or a callout around the block, never text that would replace it with an empty block", async () => {
    const { editor } = await openGuide();
    const block = editor.state.doc.child(0);
    expect(turnIntoKinds(block)).toEqual(["blockquote", "callout"]);
    expect(turnInto(editor.state, 0, "heading1")).toBeNull();
    expect(turnInto(editor.state, 0, "paragraph")).toBeNull();

    const wrap = turnInto(editor.state, 0, "callout");
    expect(wrap).not.toBeNull();
    change(() => {
      if (wrap !== null) {
        editor.view.dispatch(wrap);
      }
    });
    const callout = editor.state.doc.child(0);
    expect(callout.type.name).toBe("callout");
    expect(callout.child(0).type.name).toBe("tableOfContents");
    expect(callout.child(0).attrs.id).toBe(pid(1)); // the block is the same block, inside the callout
    // Its list is still the page's, from inside the callout (a new view: drawn a moment later).
    await waitFor(() => {
      expect(lines()).toEqual(["1:Introduction", "2:Setup", "3:Install", "2:Usage"]);
    });

    // Turning the callout into text keeps the block it holds, as it keeps an image or a table.
    const unwrap = turnInto(editor.state, 0, "paragraph");
    expect(unwrap).not.toBeNull();
    change(() => {
      if (unwrap !== null) {
        editor.view.dispatch(unwrap);
      }
    });
    expect(editor.state.doc.child(0).type.name).toBe("tableOfContents");
  });

  it("duplicates the block with an id of its own", async () => {
    const { editor } = await openGuide();
    const copy = duplicateBlock(editor.state, 0);
    expect(copy).not.toBeNull();
    change(() => {
      if (copy !== null) {
        editor.view.dispatch(copy);
      }
    });
    expect(editor.state.doc.child(0).type.name).toBe("tableOfContents");
    expect(editor.state.doc.child(1).type.name).toBe("tableOfContents");
    const ids = [editor.state.doc.child(0).attrs.id, editor.state.doc.child(1).attrs.id] as unknown[];
    expect(ids[0]).toBe(pid(1));
    expect(ids[1]).toMatch(UUID);
    expect(ids[1]).not.toBe(ids[0]);
    await waitFor(() => {
      expect(screen.getAllByRole("navigation", { name: t("toc.label") })).toHaveLength(2);
    });
  });
});

// ----- toggle headings (ADR-0040) ----------------------------------------------------------

/** A toggle: the block `n`, its summary `n + 1` (a toggle heading of `level`, or a plain toggle), its body `n + 2`. */
function toggle(n: number, level: number | null, title: string, open: boolean, ...body: JsonValue[]): JsonValue {
  return {
    type: "details",
    attrs: { id: pid(n), open },
    content: [
      {
        type: "detailsSummary",
        attrs: { id: pid(n + 1), ...(level === null ? {} : { level }) },
        content: [{ type: "text", text: title }],
      },
      { type: "detailsContent", attrs: { id: pid(n + 2) }, content: body.length === 0 ? [paragraph(n + 3, "inside")] : body },
    ],
  };
}

describe("toggle headings", () => {
  const schema = createEditorSchema();
  const pm = (...blocks: JsonValue[]): PmNode => schema.nodeFromJSON(page(...blocks));

  it("are listed with the headings at their level, with the id of their summary; plain toggles are not", () => {
    const doc = pm(
      heading(1, 1, "One"),
      toggle(10, 2, "Folded", false, heading(14, 3, "Inside"), toggle(20, 3, "Deeper", true)),
      toggle(30, null, "Plain", false),
      toggle(40, 7, "Newer", false), // a level this version does not know is a plain toggle
      heading(50, 2, "Two"),
    );
    expect(collectHeadings(doc)).toEqual([
      { id: pid(1), level: 1, text: "One" },
      { id: pid(11), level: 2, text: "Folded" },
      { id: pid(14), level: 3, text: "Inside" },
      { id: pid(21), level: 3, text: "Deeper" },
      { id: pid(50), level: 2, text: "Two" },
    ]);
  });

  it("are found by the id of their summary, or by their place in the list", () => {
    const doc = pm(heading(1, 1, "One"), toggle(10, 2, "Folded", false));
    const entry = collectHeadings(doc)[1];
    if (entry === undefined) {
      throw new Error("no second line");
    }
    const at = findHeading(doc, entry, 1);
    expect(doc.nodeAt(at ?? -1)?.type.name).toBe("detailsSummary");
    expect(doc.nodeAt(at ?? -1)?.textContent).toBe("Folded");
    expect(findHeading(doc, { ...entry, id: null }, 1)).toBe(at);
    expect(findHeading(doc, { ...entry, id: pid(99) }, 1)).toBeNull();
  });

  it("are lines of the list, in order and indented by level, and a click goes to the summary of a closed toggle", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user, editor } = await openGuide(
      page(toc(1), heading(2, 1, "Start"), toggle(10, 2, "Folded", false), toggle(20, null, "Plain", false), heading(30, 2, "End")),
    );
    expect(lines()).toEqual(["1:Start", "2:Folded", "2:End"]);

    await user.click(button("Folded"));
    const { selection } = editor.state;
    expect(selection.empty).toBe(true);
    expect(selection.$from.parent.type.name).toBe("detailsSummary");
    expect(selection.$from.parent.attrs.id).toBe(pid(11));
    expect(selection.$from.parentOffset).toBe(0);
    // The summary stays on the page while the toggle is closed: it is what scrolls into view.
    expect(editor.state.doc.child(2).attrs.open).toBe(false);
    const summary = editor.view.dom.querySelector("summary[data-level='2']");
    expect(summary).not.toBeNull();
    expect(scroll).toHaveBeenCalledWith({ block: "center" });
    expect(scroll.mock.contexts.at(-1)).toBe(summary);
  });

  it("follow a heading turned into a toggle heading, and back", async () => {
    const { editor } = await openGuide(page(toc(1), heading(2, 1, "Start"), heading(3, 2, "Setup")));
    expect(lines()).toEqual(["1:Start", "2:Setup"]);
    const start = editor.state.doc.child(0).nodeSize + editor.state.doc.child(1).nodeSize;
    const toToggle = turnInto(editor.state, start, "toggleHeading3");
    expect(toToggle).not.toBeNull();
    change(() => {
      if (toToggle !== null) {
        editor.view.dispatch(toToggle);
      }
    });
    expect(lines()).toEqual(["1:Start", "3:Setup"]);
    const toHeading = turnInto(editor.state, start, "heading2");
    change(() => {
      if (toHeading !== null) {
        editor.view.dispatch(toHeading);
      }
    });
    expect(lines()).toEqual(["1:Start", "2:Setup"]);
  });
});
