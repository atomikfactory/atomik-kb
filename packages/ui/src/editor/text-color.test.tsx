// Text colour and highlight (ADR-0016, amendment of 2026-10-07): the two marks in the schema
// (names, attribute, expansion, HTML in and out, commands), what the Loro binding does with
// them (stored with their `color`, one update per pick, two editors converge, a colour this
// version does not know is left alone), the colour button and its popover in the bubble menu
// and what pasting does with colours.

import { loroToPortable, normalizePortable, PAGE_MARKS } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import type { LoroList, LoroMap, LoroText } from "loro-crdt";
import { afterEach, describe, expect, it } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { loroDocFrom, PAGE_FIXTURE, paragraphs, pid, sync, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { CALLOUT_COLORS } from "./callout";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, loroSyncPluginKey, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { MarkdownPaste } from "./markdown-paste";
import { isPaletteColor, PALETTE_COLORS } from "./palette";
import type { PaletteColor } from "./palette";
import { createEditorSchema, createSchemaExtensions } from "./schema";
import { FOREIGN_HIGHLIGHT_COLOR, selectionColor } from "./text-color";

const NAMES = ["Gray", "Brown", "Orange", "Yellow", "Green", "Blue", "Purple", "Pink", "Red"];

const schemaFixture = blockSchemaFixture as unknown as BlockSchemaFixture;

// ----- helpers ------------------------------------------------------------------------------

function mark(type: "textColor" | "highlight", color: string): JsonValue {
  return { type, attrs: { color } };
}

function run(text: string, ...marks: JsonValue[]): JsonValue {
  return marks.length === 0 ? { type: "text", text } : { type: "text", text, marks };
}

/** A page of one paragraph (id `pid(0)`) made of the given runs. */
function page(...runs: JsonValue[]): JsonValue {
  return { type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) }, content: runs }] };
}

/** `Hello world`, with the marks on `Hello` and on `world` as given. */
function hello(first: JsonValue[] = [], second: JsonValue[] = []): JsonValue {
  return page(run("Hello", ...first), run(" "), run("world", ...second));
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A Tiptap editor over the schema extensions (and the Loro binding when given a document, the paste handler when asked). */
function mount(content: JSONContent, options: { doc?: LoroDoc; paste?: boolean } = {}): Editor {
  const extensions = createSchemaExtensions();
  if (options.paste === true) {
    extensions.push(MarkdownPaste);
  }
  if (options.doc !== undefined) {
    extensions.push(LoroBinding.configure({ doc: options.doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions, content, editable: true });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

/** The editor's initial content, read from the document like the page editor does. */
function contentOfLoro(doc: LoroDoc): JSONContent {
  const root = doc.getMap(ROOT_DOC_KEY);
  return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
}

/** An editor bound to `doc`, after the binding showed the document. */
async function mountBound(doc: LoroDoc): Promise<Editor> {
  const editor = mount(contentOfLoro(doc), { doc });
  await tick();
  await tick();
  return editor;
}

function select(editor: Editor, from: number, to = from): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, from, to)));
}

/** The runs of the first paragraph as `text` or `text:mark=color,mark=color`, in the editor. */
function runs(editor: Editor): string[] {
  const out: string[] = [];
  editor.state.doc.child(0).forEach((node) => {
    const marks = node.marks.map((m) => `${m.type.name}=${String(m.attrs.color)}`);
    out.push(marks.length === 0 ? (node.text ?? "") : `${node.text ?? ""}:${marks.join(",")}`);
  });
  return out;
}

/** Every change `doc` recorded since `since`, as its ops. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

/** The portable form of the editor's document, with the ids and the order the core uses. */
function portableOf(editor: Editor): JsonValue {
  return normalizePortable(editor.getJSON() as JsonValue);
}

// ----- the marks ----------------------------------------------------------------------------

describe("the marks", () => {
  it("share the nine names of the callout colours, exported once", () => {
    expect([...PALETTE_COLORS]).toEqual(["gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red"]);
    expect(CALLOUT_COLORS).toBe(PALETTE_COLORS);
    expect(PALETTE_COLORS.every((color) => isPaletteColor(color))).toBe(true);
    for (const other of ["", "Red", "chartreuse", null, undefined, 3]) {
      expect(isPaletteColor(other), String(other)).toBe(false);
    }
  });

  it("are `textColor` and `highlight` with one `color` attribute, expanding after, and match the regenerated fixture", () => {
    const schema = createEditorSchema();
    const live = describeSchema(schema).marks;
    expect(Object.keys(live)).toEqual(["bold", "italic", "underline", "strike", "code", "link", "textColor", "highlight"]);
    for (const name of ["textColor", "highlight"]) {
      expect(live[name], name).toEqual({ attrs: ["color"], expand: "after" });
      expect(schemaFixture.marks[name], name).toEqual(live[name]);
      expect(schema.marks[name]?.spec.inclusive, name).not.toBe(false);
    }
    // The core-client table (and so the core's, which a Rust test compares with the fixture) is the same.
    expect(PAGE_MARKS.slice(-2)).toEqual([
      ["textColor", "after"],
      ["highlight", "after"],
    ]);
  });

  it("have no colour by default, and exclude themselves: one colour at a time, a text colour and a highlight together", () => {
    const { marks } = createEditorSchema();
    const { textColor, highlight } = marks;
    if (textColor === undefined || highlight === undefined) {
      throw new Error("the schema has no colour marks");
    }
    expect(textColor.create().attrs).toEqual({ color: null });
    expect(highlight.create().attrs).toEqual({ color: null });
    expect(textColor.excludes(textColor)).toBe(true);
    expect(highlight.excludes(highlight)).toBe(true);
    expect(textColor.excludes(highlight)).toBe(false);
    expect(highlight.excludes(textColor)).toBe(false);
    expect(marks.bold?.excludes(textColor)).toBe(false);
  });

  it("render as `<span data-text-color>` and `<mark data-highlight>`, and nothing for a mark with no colour", () => {
    const editor = mount(
      page(
        run("red", mark("textColor", "red")),
        run(" "),
        run("marked", mark("highlight", "yellow")),
        run(" "),
        run("both", mark("textColor", "blue"), mark("highlight", "pink")),
        run(" "),
        run("bare", { type: "textColor" }),
      ) as JSONContent,
    );
    const dom = editor.view.dom;
    expect(dom.querySelector("span[data-text-color='red']")).toHaveTextContent("red");
    expect(dom.querySelector("mark[data-highlight='yellow']")).toHaveTextContent("marked");
    // Both on one run: both elements, the text inside the inner one.
    const both = dom.querySelector("span[data-text-color='blue'] mark[data-highlight='pink']");
    expect(both).toHaveTextContent("both");
    const bare = [...dom.querySelectorAll("span")].find((element) => element.textContent === "bare");
    expect(bare?.getAttributeNames()).toEqual([]);
    // What is copied out of the editor carries the colours.
    expect(editor.getHTML()).toContain('data-text-color="red"');
    expect(editor.getHTML()).toContain('data-highlight="yellow"');
  });

  it("parse their own HTML back, keep a name they do not know, and make no mark of an empty one", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      '<p><span data-text-color="red">a</span> <mark data-highlight="pink">b</mark> ' +
        '<span data-text-color="chartreuse">c</span> <span data-text-color="">d</span> ' +
        '<span data-text-color="blue"><mark data-highlight="green">e</mark></span></p>',
    );
    expect(runs(editor)).toEqual([
      "a:textColor=red",
      " ",
      "b:highlight=pink",
      " ",
      "c:textColor=chartreuse",
      " d ",
      "e:textColor=blue,highlight=green",
    ]);
  });

  it("make a yellow highlight of a `<mark>` from foreign HTML, and ignore inline colour styles", () => {
    expect(FOREIGN_HIGHLIGHT_COLOR).toBe("yellow");
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      "<p><mark>a</mark> <mark data-highlight=''>b</mark> " +
        '<span style="color: red">c</span> <span style="background-color: #ff0">d</span> ' +
        '<font color="red">e</font> <mark style="background-color: pink" class="x">f</mark></p>',
    );
    expect(runs(editor)).toEqual([
      "a:highlight=yellow",
      " ",
      "b:highlight=yellow",
      " c d e ",
      "f:highlight=yellow",
    ]);
  });

  it("are in the golden sample of the page fixture, a coloured run, a highlighted run and one that is both", () => {
    const found: string[] = [];
    const visit = (node: JsonValue): void => {
      if (Array.isArray(node)) {
        node.forEach(visit);
      } else if (typeof node === "object" && node !== null) {
        if (Array.isArray(node.marks)) {
          const names = node.marks.flatMap((m) =>
            typeof m === "object" && m !== null && !Array.isArray(m) && typeof m.type === "string" ? [m.type] : [],
          );
          if (names.some((name) => name === "textColor" || name === "highlight")) {
            found.push(names.join("+"));
          }
        }
        Object.values(node).forEach(visit);
      }
    };
    visit(PAGE_FIXTURE);
    expect(found).toEqual(["textColor", "highlight", "textColor+highlight"]);
  });
});

// ----- commands -----------------------------------------------------------------------------

describe("the commands", () => {
  it("set a text colour on the selection and replace it: one colour at a time", () => {
    const editor = mount(paragraphs("Hello world") as JSONContent);
    select(editor, 7, 12);
    expect(editor.commands.setTextColor("red")).toBe(true);
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=red"]);
    expect(editor.commands.setTextColor("blue")).toBe(true);
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=blue"]);
    // Part of the run: the rest keeps its colour, the part takes the new one.
    select(editor, 7, 9);
    editor.commands.setTextColor("green");
    expect(runs(editor)).toEqual(["Hello ", "wo:textColor=green", "rld:textColor=blue"]);
    expect(editor.isActive("textColor", { color: "green" })).toBe(true);
  });

  it("set a highlight the same way, independent of the text colour", () => {
    const editor = mount(paragraphs("Hello world") as JSONContent);
    select(editor, 7, 12);
    editor.chain().setTextColor("red").setHighlight("yellow").run();
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=red,highlight=yellow"]);
    editor.commands.setHighlight("pink");
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=red,highlight=pink"]);
    editor.commands.setTextColor("blue");
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=blue,highlight=pink"]);
  });

  it("remove one mark and leave the others", () => {
    const editor = mount(
      page(run("Hello world", { type: "bold" }, mark("textColor", "red"), mark("highlight", "yellow"))) as JSONContent,
    );
    select(editor, 1, 6);
    expect(editor.commands.unsetTextColor()).toBe(true);
    expect(runs(editor)).toEqual(["Hello:bold=undefined,highlight=yellow", " world:bold=undefined,textColor=red,highlight=yellow"]);
    editor.commands.unsetHighlight();
    expect(runs(editor)).toEqual(["Hello:bold=undefined", " world:bold=undefined,textColor=red,highlight=yellow"]);
  });

  it("write only names of the palette: a name they do not know, or none, changes nothing", () => {
    const editor = mount(paragraphs("Hello world") as JSONContent);
    select(editor, 7, 12);
    const before = editor.getJSON();
    expect(editor.commands.setTextColor("chartreuse" as PaletteColor)).toBe(false);
    expect(editor.commands.setHighlight("" as PaletteColor)).toBe(false);
    expect(editor.getJSON()).toEqual(before);
  });

  it("leave code alone: its mark takes no other mark", () => {
    const editor = mount(page(run("let x", { type: "code" }), run(" and text")) as JSONContent);
    select(editor, 1, 15);
    editor.commands.setTextColor("red");
    expect(runs(editor)).toEqual(["let x:code=undefined", " and text:textColor=red"]);
  });

  it("set stored marks on a cursor, so typing continues in colour, and take them off again", () => {
    const editor = mount(paragraphs("Hello world") as JSONContent);
    select(editor, 12);
    expect(editor.commands.setTextColor("green")).toBe(true);
    expect(editor.state.storedMarks?.map((m) => `${m.type.name}=${String(m.attrs.color)}`)).toEqual(["textColor=green"]);
    expect(selectionColor(editor, "textColor")).toBe("green");
    editor.chain().setHighlight("pink").run();
    editor.view.dispatch(editor.state.tr.insertText("!"));
    expect(runs(editor)).toEqual(["Hello world", "!:textColor=green,highlight=pink"]);

    // Typing goes on in the colour of the run it is in (the marks expand after), until Default.
    editor.view.dispatch(editor.state.tr.insertText("?"));
    expect(runs(editor)).toEqual(["Hello world", "!?:textColor=green,highlight=pink"]);
    expect(editor.commands.unsetTextColor()).toBe(true);
    expect(selectionColor(editor, "textColor")).toBeNull();
    editor.commands.unsetHighlight();
    editor.view.dispatch(editor.state.tr.insertText("."));
    expect(runs(editor)).toEqual(["Hello world", "!?:textColor=green,highlight=pink", "."]);
  });

  it("expand over text typed at the end of a coloured run, but not before it", () => {
    const editor = mount(hello([], [mark("textColor", "red")]) as JSONContent);
    select(editor, 12);
    editor.view.dispatch(editor.state.tr.insertText("!"));
    expect(runs(editor)).toEqual(["Hello ", "world!:textColor=red"]);
    select(editor, 7);
    editor.view.dispatch(editor.state.tr.insertText(">"));
    expect(runs(editor)).toEqual(["Hello >", "world!:textColor=red"]);
  });
});

describe("selectionColor", () => {
  it("is the colour of the whole selection, none for none, and undefined for a mixed one", () => {
    const editor = mount(
      page(
        run("aa", mark("textColor", "red")),
        run("bb", mark("textColor", "blue")),
        run("cc", mark("textColor", "red")),
        run("dd"),
        run("ee", mark("textColor", "chartreuse")),
        run("ff", { type: "textColor" }),
      ) as JSONContent,
    );
    select(editor, 1, 3);
    expect(selectionColor(editor, "textColor")).toBe("red");
    select(editor, 1, 5); // two colours
    expect(selectionColor(editor, "textColor")).toBeUndefined();
    select(editor, 5, 9); // one colour, part of the selection plain
    expect(selectionColor(editor, "textColor")).toBeUndefined();
    select(editor, 7, 9);
    expect(selectionColor(editor, "textColor")).toBeNull();
    select(editor, 9, 11);
    expect(selectionColor(editor, "textColor")).toBe("chartreuse"); // not in the palette, still its name
    select(editor, 11, 13);
    expect(selectionColor(editor, "textColor")).toBeNull(); // a mark that holds no colour
    select(editor, 1, 3);
    expect(selectionColor(editor, "highlight")).toBeNull();
  });
});

// ----- portable form and the Loro binding ---------------------------------------------------

describe("the portable form and the Loro binding", () => {
  const colourful = page(
    run("Hello", mark("textColor", "red")),
    run(" "),
    run("wor", mark("highlight", "yellow")),
    run("ld", { type: "bold" }, mark("textColor", "blue"), mark("highlight", "pink")),
  );

  it("keeps the attributes through the document: stored as Loro text marks, read back as the same portable JSON", async () => {
    const doc = loroDocFrom(colourful);
    expect(loroToPortable(doc)).toEqual(normalizePortable(colourful));
    const paragraph = (doc.getMap("doc").get("children") as LoroList).get(0) as LoroMap;
    const text = (paragraph.get("children") as LoroList).get(0) as LoroText;
    expect(text.toDelta()).toEqual([
      { insert: "Hello", attributes: { textColor: { color: "red" } } },
      { insert: " " },
      { insert: "wor", attributes: { highlight: { color: "yellow" } } },
      { insert: "ld", attributes: { bold: {}, textColor: { color: "blue" }, highlight: { color: "pink" } } },
    ]);

    const before = doc.oplogVersion();
    const editor = await mountBound(doc);
    expect(runs(editor)).toEqual([
      "Hello:textColor=red",
      " ",
      "wor:highlight=yellow",
      "ld:bold=undefined,textColor=blue,highlight=pink",
    ]);
    expect(portableOf(editor)).toEqual(normalizePortable(colourful));
    // Showing the document wrote nothing.
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("writes one update for a pick: the mark with its colour, then one for the next colour, one for Default", async () => {
    const doc = loroDocFrom(paragraphs("Hello world"));
    const editor = await mountBound(doc);
    select(editor, 7, 12);

    let before = doc.oplogVersion();
    editor.chain().setTextColor("red").run();
    let changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(JSON.stringify(changes)).toContain('"textColor"');
    expect(JSON.stringify(changes)).toContain('"red"');
    expect(loroToPortable(doc)).toEqual(normalizePortable(hello([], [mark("textColor", "red")])));

    before = doc.oplogVersion();
    editor.chain().setTextColor("blue").run();
    changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(JSON.stringify(changes)).toContain('"blue"');
    expect(loroToPortable(doc)).toEqual(normalizePortable(hello([], [mark("textColor", "blue")])));

    before = doc.oplogVersion();
    editor.chain().setHighlight("pink").run();
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(loroToPortable(doc)).toEqual(normalizePortable(hello([], [mark("textColor", "blue"), mark("highlight", "pink")])));

    before = doc.oplogVersion();
    editor.chain().unsetTextColor().run();
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(loroToPortable(doc)).toEqual(normalizePortable(hello([], [mark("highlight", "pink")])));
    editor.chain().unsetHighlight().run();
    expect(loroToPortable(doc)).toEqual(normalizePortable(hello()));
    // Nothing of the colours is left in the stored text.
    expect(JSON.stringify(loroToPortable(doc))).not.toContain("Color");
  });

  it("converges between two bound editors: a pick shows on the other with its colour, and concurrent picks agree", async () => {
    const docA = loroDocFrom(paragraphs("Hello world"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const editorA = await mountBound(docA);
    const editorB = await mountBound(docB);

    select(editorA, 1, 6);
    editorA.chain().setHighlight("pink").run();
    select(editorB, 7, 12);
    editorB.chain().setTextColor("green").run();
    sync(docA, docB);
    sync(docB, docA);
    await tick();

    const expected = ["Hello:highlight=pink", " ", "world:textColor=green"];
    expect(runs(editorA)).toEqual(expected);
    expect(runs(editorB)).toEqual(expected);
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));

    // The same text coloured two ways at once: both replicas end with the same one colour.
    select(editorA, 7, 12);
    editorA.chain().setTextColor("red").run();
    select(editorB, 7, 12);
    editorB.chain().setTextColor("blue").run();
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(runs(editorA)).toEqual(runs(editorB));
    expect(runs(editorA)).toHaveLength(3);
    expect(runs(editorA)[2]).toMatch(/^world:textColor=(red|blue)$/);
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(portableOf(editorA)).toEqual(loroToPortable(docA));
  });

  it("types in colour: stored marks write the text with the mark, one update", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    select(editor, 6);
    editor.chain().setTextColor("orange").run();
    const before = doc.oplogVersion();
    editor.view.dispatch(editor.state.tr.insertText("!"));
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(loroToPortable(doc)).toEqual(normalizePortable(page(run("Hello"), run("!", mark("textColor", "orange")))));
    // Typing goes on in the colour (the mark expands after, in Loro too).
    editor.view.dispatch(editor.state.tr.insertText("?"));
    expect(loroToPortable(doc)).toEqual(normalizePortable(page(run("Hello"), run("!?", mark("textColor", "orange")))));
  });

  it("keeps a colour this version does not know through an unrelated edit, shown as plain text, and replaces it on a pick", async () => {
    const stored = hello([], [mark("textColor", "chartreuse"), mark("highlight", "mauve")]);
    const doc = loroDocFrom(stored);
    const before = doc.oplogVersion();
    const editor = await mountBound(doc);
    expect(changesSince(doc, before)).toEqual([]);
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=chartreuse,highlight=mauve"]);
    // The element keeps the name; no style rule knows it, so it shows as plain text.
    expect(editor.view.dom.querySelector("[data-text-color='chartreuse']")).toHaveTextContent("world");
    expect(editor.view.dom.querySelector("mark[data-highlight='mauve']")).toHaveTextContent("world");

    // Typing elsewhere and editing inside the run write no mark operation.
    const beforeEdit = doc.oplogVersion();
    editor.view.dispatch(editor.state.tr.insertText("!", 1));
    editor.view.dispatch(editor.state.tr.insertText("o", 9));
    const ops = JSON.stringify(changesSince(doc, beforeEdit));
    expect(ops).not.toContain("textColor");
    expect(ops).not.toContain("chartreuse");
    expect(loroToPortable(doc)).toEqual(
      normalizePortable(page(run("!Hello "), run("woorld", mark("textColor", "chartreuse"), mark("highlight", "mauve")))),
    );

    // A pick replaces it.
    select(editor, 8, 14);
    editor.chain().setTextColor("red").run();
    expect(JSON.stringify(loroToPortable(doc))).not.toContain("chartreuse");
    expect(JSON.stringify(loroToPortable(doc))).toContain("mauve");
  });
});

// ----- the colour button and its popover ----------------------------------------------------

describe("the colour button and the popover in the bubble menu", () => {
  async function openSelected(content: JsonValue = paragraphs("Hello world"), from = 7, to = 12) {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(app.user, "Notes");
    editor.view.dom.tabIndex = 0; // jsdom only focuses a contenteditable that has one
    act(() => {
      editor.chain().focus().setTextSelection({ from, to }).run();
    });
    const toolbar = await screen.findByRole("toolbar", { name: t("format.label") });
    return { ...app, editor, toolbar };
  }

  const colorButton = (toolbar: HTMLElement): HTMLElement => within(toolbar).getByRole("button", { name: t("format.color") });
  const findPopover = (): Promise<HTMLElement> => screen.findByRole("dialog", { name: t("format.colorTitle") });
  const popoverIsOpen = (): boolean => screen.queryByRole("dialog", { name: t("format.colorTitle") }) !== null;
  const textRow = (popover: HTMLElement): HTMLElement => within(popover).getByRole("group", { name: t("format.textColor") });
  const highlightRow = (popover: HTMLElement): HTMLElement => within(popover).getByRole("group", { name: t("format.highlight") });
  const pressedIn = (row: HTMLElement): (string | null)[] =>
    within(row)
      .getAllByRole("button")
      .filter((button) => button.getAttribute("aria-pressed") === "true")
      .map((button) => button.getAttribute("aria-label") ?? button.textContent);

  it("is a button of the menu, named, with a popover of two rows: nine text colours and Default, nine highlights and Default", async () => {
    const { user, toolbar } = await openSelected();
    const button = colorButton(toolbar);
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(button.querySelector(".format-color-glyph")).not.toHaveAttribute("data-text");
    expect(button.querySelector(".format-color-glyph")).not.toHaveAttribute("data-fill");
    expect(popoverIsOpen()).toBe(false);

    await user.click(button);
    const popover = await findPopover();
    expect(colorButton(toolbar)).toHaveAttribute("aria-expanded", "true");
    expect(within(textRow(popover)).getAllByRole("button").map((b) => b.getAttribute("aria-label") ?? b.textContent)).toEqual([
      ...NAMES.map((name) => `${name} text`),
      t("format.colorDefault"),
    ]);
    expect(
      within(highlightRow(popover)).getAllByRole("button").map((b) => b.getAttribute("aria-label") ?? b.textContent),
    ).toEqual([...NAMES.map((name) => `${name} highlight`), t("format.colorDefault")]);
    // A swatch per name of the palette, in order; text swatches show an "A", highlights are filled.
    const textSwatches = [...textRow(popover).querySelectorAll(".color-swatch-text")];
    expect(textSwatches.map((swatch) => swatch.getAttribute("data-color"))).toEqual([...PALETTE_COLORS]);
    expect(textSwatches[0]).toHaveTextContent("A");
    const fillSwatches = [...highlightRow(popover).querySelectorAll(".color-swatch-fill")];
    expect(fillSwatches.map((swatch) => swatch.getAttribute("data-color"))).toEqual([...PALETTE_COLORS]);
    // Nothing is coloured: Default is the marked one in both rows.
    expect(pressedIn(textRow(popover))).toEqual([t("format.colorDefault")]);
    expect(pressedIn(highlightRow(popover))).toEqual([t("format.colorDefault")]);
  });

  it("colours the selection with a pick, closes, keeps the selection and writes one update to the core", async () => {
    const { user, editor, toolbar, memory } = await openSelected();
    const doc = docOf(editor);

    const before = doc.oplogVersion();
    await user.click(colorButton(toolbar));
    await user.click(within(textRow(await findPopover())).getByRole("button", { name: "Red text" }));
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=red"]);
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(popoverIsOpen()).toBe(false);
    expect(editor.state.selection.from).toBe(7);
    expect(editor.state.selection.to).toBe(12);
    // The bubble menu is still there, and its button shows the colour.
    const after = await screen.findByRole("toolbar", { name: t("format.label") });
    expect(colorButton(after).querySelector(".format-color-glyph")).toHaveAttribute("data-text", "red");

    await user.click(colorButton(after));
    const popover = await findPopover();
    expect(pressedIn(textRow(popover))).toEqual(["Red text"]);
    expect(pressedIn(highlightRow(popover))).toEqual([t("format.colorDefault")]);
    await user.click(within(highlightRow(popover)).getByRole("button", { name: "Yellow highlight" }));
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=red,highlight=yellow"]);
    await waitFor(() => {
      expect(colorButton(screen.getByRole("toolbar", { name: t("format.label") })).querySelector(".format-color-glyph")).toHaveAttribute(
        "data-fill",
        "yellow",
      );
    });

    // It reaches the core as the portable form, with the block id the page had.
    const { page: pageId } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, pageId("Notes"))).toEqual(
        normalizePortable(hello([], [mark("textColor", "red"), mark("highlight", "yellow")])),
      );
    });
  });

  it("replaces a colour, and Default takes it off", async () => {
    const { user, editor, toolbar } = await openSelected(hello([], [mark("textColor", "pink"), mark("highlight", "green")]));
    await user.click(colorButton(toolbar));
    const popover = await findPopover();
    expect(pressedIn(textRow(popover))).toEqual(["Pink text"]);
    expect(pressedIn(highlightRow(popover))).toEqual(["Green highlight"]);

    await user.click(within(textRow(popover)).getByRole("button", { name: "Blue text" }));
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=blue,highlight=green"]);

    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    await user.click(within(textRow(await findPopover())).getByRole("button", { name: t("format.colorDefault") }));
    expect(runs(editor)).toEqual(["Hello ", "world:highlight=green"]);

    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    const again = await findPopover();
    expect(pressedIn(textRow(again))).toEqual([t("format.colorDefault")]);
    await user.click(within(highlightRow(again)).getByRole("button", { name: t("format.colorDefault") }));
    expect(runs(editor)).toEqual(["Hello world"]);
  });

  it("marks nothing for a mixed selection, and nothing for a colour it does not know, which a pick replaces", async () => {
    const mixed = page(run("aaa", mark("textColor", "red")), run("bbb", mark("textColor", "blue")), run("ccc", mark("textColor", "chartreuse")));
    const { user, editor, toolbar } = await openSelected(mixed, 1, 7);
    await user.click(colorButton(toolbar));
    let popover = await findPopover();
    expect(pressedIn(textRow(popover))).toEqual([]);
    expect(pressedIn(highlightRow(popover))).toEqual([t("format.colorDefault")]);
    await user.keyboard("{Escape}");
    expect(popoverIsOpen()).toBe(false);

    act(() => {
      editor.chain().focus().setTextSelection({ from: 7, to: 10 }).run();
    });
    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    popover = await findPopover();
    expect(pressedIn(textRow(popover))).toEqual([]); // `chartreuse` is not a swatch, and not the default either
    await user.click(within(textRow(popover)).getByRole("button", { name: "Green text" }));
    expect(runs(editor)).toEqual(["aaa:textColor=red", "bbb:textColor=blue", "ccc:textColor=green"]);
  });

  it("closes on Escape, on a press outside and after a pick, and the button toggles it", async () => {
    const { user, editor, toolbar } = await openSelected();

    await user.click(colorButton(toolbar));
    await findPopover();
    await user.keyboard("{Escape}");
    expect(popoverIsOpen()).toBe(false);
    expect(runs(editor)).toEqual(["Hello world"]);

    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    await findPopover();
    fireEvent.pointerDown(document.body);
    await waitFor(() => {
      expect(popoverIsOpen()).toBe(false);
    });

    // The button is not "outside": pressing it again closes the popover.
    const again = await screen.findByRole("toolbar", { name: t("format.label") });
    await user.click(colorButton(again));
    await findPopover();
    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    expect(popoverIsOpen()).toBe(false);

    await user.click(colorButton(await screen.findByRole("toolbar", { name: t("format.label") })));
    await user.click(within(highlightRow(await findPopover())).getByRole("button", { name: "Orange highlight" }));
    expect(popoverIsOpen()).toBe(false);
    expect(colorButton(await screen.findByRole("toolbar", { name: t("format.label") }))).toHaveAttribute("aria-expanded", "false");
    expect(runs(editor)).toEqual(["Hello ", "world:highlight=orange"]);
    // None of it changed the text.
    expect(editor.state.doc.textContent).toBe("Hello world");
  });

  it("can be used from the keyboard: Enter opens it on the first colour, Tab reaches every choice, Enter picks", async () => {
    const { user, editor, toolbar } = await openSelected();

    colorButton(toolbar).focus();
    await user.keyboard("{Enter}");
    const popover = await findPopover();
    const buttons = within(popover).getAllByRole("button");
    expect(buttons).toHaveLength(20);
    expect(buttons[0]).toHaveFocus();
    for (const button of buttons.slice(1)) {
      await user.tab();
      expect(button).toHaveFocus();
    }

    // Enter on a focused swatch picks it.
    buttons[4]?.focus();
    await user.keyboard("{Enter}");
    expect(runs(editor)).toEqual(["Hello ", "world:textColor=green"]);
    expect(popoverIsOpen()).toBe(false);
  });
});

// ----- paste --------------------------------------------------------------------------------

describe("pasting", () => {
  /** Pastes `html` (and its `text`) the way the webview delivers them. */
  function paste(editor: Editor, text: string, html: string): void {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    const data = {
      types: ["text/html", "text/plain"],
      files: [],
      getData: (type: string) => (type === "text/html" ? html : type === "text/plain" ? text : ""),
    };
    Object.defineProperty(event, "clipboardData", { value: data });
    editor.view.dom.dispatchEvent(event);
  }

  function emptyPage(): JSONContent {
    return { type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] };
  }

  it("keeps the colours of our own HTML, a name this version does not know included", () => {
    const editor = mount(emptyPage(), { paste: true });
    select(editor, 1);
    paste(
      editor,
      "red marked grey",
      '<p><span data-text-color="red">red</span> <mark data-highlight="pink">marked</mark> <span data-text-color="chartreuse">grey</span></p>',
    );
    expect(runs(editor)).toEqual(["red:textColor=red", " ", "marked:highlight=pink", " ", "grey:textColor=chartreuse"]);
  });

  it("makes a yellow highlight of a foreign `<mark>` and ignores inline colours", () => {
    const editor = mount(emptyPage(), { paste: true });
    select(editor, 1);
    paste(
      editor,
      "plain marked styled",
      '<p>plain <mark style="background: lime">marked</mark> <span style="color: red; background-color: yellow">styled</span></p>',
    );
    expect(runs(editor)).toEqual(["plain ", "marked:highlight=yellow", " styled"]);
  });

  it("does not turn coloured text that looks like Markdown into blocks: the colour is formatting, not a wrapper", () => {
    // Without the colour, HTML that only wraps the text is taken for the Markdown it is written as.
    const plain = mount(emptyPage(), { paste: true });
    select(plain, 1);
    paste(plain, "# Title", "<p># Title</p>");
    expect(plain.state.doc.child(0).type.name).toBe("heading");

    const editor = mount(emptyPage(), { paste: true });
    select(editor, 1);
    paste(editor, "# Title", '<p><span data-text-color="red"># Title</span></p>');
    expect(editor.state.doc.child(0).type.name).toBe("paragraph");
    expect(runs(editor)).toEqual(["# Title:textColor=red"]);
  });
});
