// The inline equation (ADR-0016, amendment of 2026-10-07): the `mathInline` node in the schema (shape,
// HTML in and out, text), how it is made (`$x$`, which must not fight the `$$ ` block rule; `/inline
// equation`; the command), the chip in the page editor (drawn by the stand-in renderer, a sigma when
// empty, the source and KaTeX message when it does not parse), its popover (live preview, Enter
// applies, Escape cancels, a press outside applies, Backspace in an empty field removes the equation),
// read-only editors, and what the Loro binding does with all of it (one update per insert or apply,
// two editors converge, a portable round trip).

import { loroToPortable, portableToLoro } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { loroDocFrom, paragraphs, pid, sync, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, loroSyncPluginKey, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { loadMath, renderMath, renderMathInline, setMathRenderer } from "./math";
import { MATH_INLINE_NODE } from "./math-inline";
import { createEditorSchema, createSchemaExtensions, ID_BEARING_NODES } from "./schema";

const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;

// ----- helpers ------------------------------------------------------------------------------

function equation(n: number, latex: JsonValue): JsonValue {
  return { type: "mathInline", attrs: { id: pid(n), latex } };
}

function text(value: string): JsonValue {
  return { type: "text", text: value };
}

/** One paragraph (id `pid(0)`) of the given inline content, then an empty one (`pid(1)`). */
function page(...content: JsonValue[]): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content },
      { type: "paragraph", attrs: { id: pid(1) } },
    ],
  };
}

/** Every op `doc` recorded since `since`, grouped by change. */
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

/** The sources of the equations of the editor's document, in order. */
function equations(editor: Editor): string[] {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === MATH_INLINE_NODE) {
      found.push(String(node.attrs.latex));
    }
    return true;
  });
  return found;
}

/** Where the `n`-th equation starts. */
function positionOf(editor: Editor, n = 0): number {
  let seen = -1;
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === MATH_INLINE_NODE) {
      seen += 1;
      if (seen === n) {
        found = pos;
      }
    }
    return found === -1;
  });
  if (found === -1) {
    throw new Error("no such equation");
  }
  return found;
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** Types `text` the way the browser does (input rules only react to typed text). */
function typeText(editor: Editor, value: string): void {
  for (const character of value) {
    const { from, to } = editor.state.selection;
    const handled = editor.view.someProp("handleTextInput", (handle) =>
      handle(editor.view, from, to, character, () => editor.state.tr.insertText(character, from, to)),
    );
    if (handled !== true) {
      editor.view.dispatch(editor.state.tr.insertText(character, from, to));
    }
  }
}

/** Presses `key` the way the browser delivers it to the editor; whether the editor handled it. */
function press(editor: Editor, key: string): boolean {
  return (
    editor.view.someProp("handleKeyDown", (handle) => handle(editor.view, new KeyboardEvent("keydown", { key }))) === true
  );
}

/** The editor's initial content, read from the document like the page editor does. */
function contentOfLoro(doc: LoroDoc): JSONContent {
  const root = doc.getMap(ROOT_DOC_KEY);
  return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

/** A Tiptap editor over the schema extensions, and the Loro binding when given a document. */
function mount(content: JSONContent, doc: LoroDoc | null = null): Editor {
  const extensions = createSchemaExtensions();
  if (doc !== null) {
    extensions.push(LoroBinding.configure({ doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions, content });
  mounted.push({ editor, element });
  return editor;
}

async function mountBound(doc: LoroDoc): Promise<Editor> {
  const editor = mount(contentOfLoro(doc), doc);
  await tick();
  await tick();
  return editor;
}

/** The fake renderer notes what it was asked for, and puts the source in the markup so a test can read it. */
const asked: { latex: string; inline: boolean }[] = [];

function installFake(): void {
  setMathRenderer((latex, options) => {
    asked.push({ latex, inline: options?.inline === true });
    if (latex.includes("@@")) {
      throw new Error(`Undefined control sequence: ${latex}`);
    }
    return `<span class="katex" data-inline="${String(options?.inline === true)}" data-source="${encodeURIComponent(latex)}"></span>`;
  });
}

beforeEach(() => {
  asked.length = 0;
  installFake();
});

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function openNotes(content: JsonValue) {
  const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
  const editor = await openPage(app.user, "Notes");
  await waitFor(() => {
    expect(editor.view.dom.querySelector(".math-inline")).not.toBeNull();
  });
  return { ...app, editor };
}

function chip(editor: Editor, n = 0): HTMLElement {
  const found = editor.view.dom.querySelectorAll<HTMLElement>(".math-inline")[n];
  if (found === undefined) {
    throw new Error("no such chip");
  }
  return found;
}

/** What each chip draws: the source the stand-in was given, or the source as written. */
function drawn(editor: Editor): string[] {
  return Array.from(editor.view.dom.querySelectorAll(".math-inline")).map((element) => {
    const source = element.querySelector("[data-source]")?.getAttribute("data-source");
    return source === null || source === undefined ? (element.textContent ?? "") : decodeURIComponent(source);
  });
}

function popover(): HTMLElement | null {
  return screen.queryByRole("dialog", { name: t("mathInline.label") });
}

async function findPopover(): Promise<HTMLElement> {
  return screen.findByRole("dialog", { name: t("mathInline.label") });
}

function field(): HTMLInputElement {
  return within(screen.getByRole("dialog", { name: t("mathInline.label") })).getByRole("textbox", {
    name: t("mathInline.field"),
  });
}

// ----- the node -----------------------------------------------------------------------------

describe("the node in the schema", () => {
  it("is an inline atom with id and latex, and the schema matches the regenerated fixture", () => {
    const schema = createEditorSchema();
    const node = schema.nodes.mathInline;
    expect(node).toBeDefined();
    expect(node?.isInline).toBe(true);
    expect(node?.isAtom).toBe(true);
    expect(node?.isLeaf).toBe(true);
    expect(node?.spec.group).toBe("inline");
    expect(node?.spec.selectable).toBe(true);
    expect(Object.keys(node?.spec.attrs ?? {})).toEqual(["id", "latex"]);
    expect(node?.spec.attrs?.latex?.default).toBe("");
    expect(ID_BEARING_NODES).toContain("mathInline");

    expect(fixture.nodes.mathInline).toEqual({
      content: "",
      group: "inline",
      inline: true,
      atom: true,
      attrs: ["id", "latex"],
    });
    expect(describeSchema(schema).nodes.mathInline).toEqual(fixture.nodes.mathInline);
    expect(fixture.idBearing).toContain("mathInline");
    expect([...ID_BEARING_NODES].sort()).toEqual([...fixture.idBearing].sort());
  });

  it("can sit wherever inline content can, and nowhere else", () => {
    const schema = createEditorSchema();
    const type = schema.nodes.mathInline;
    if (type === undefined) {
      throw new Error("no mathInline in the schema");
    }
    for (const name of ["paragraph", "heading"]) {
      expect(schema.nodes[name]?.contentMatch.matchType(type), name).not.toBeNull();
    }
    // A code block, a math block and a toggle's summary hold text only.
    for (const name of ["codeBlock", "mathBlock", "detailsSummary"]) {
      expect(schema.nodes[name]?.contentMatch.matchType(type), name).toBeNull();
    }
    expect(schema.nodes.doc?.contentMatch.matchType(type)).toBeNull();
    expect(schema.nodes.callout?.contentMatch.matchType(type)).toBeNull();
  });

  it("reads as `$latex$` as plain text, and as a chip carrying its source in HTML", () => {
    const editor = mount(page(text("Energy "), equation(2, "E=mc^2"), text(" and "), equation(3, "a_i^2")) as JSONContent);
    expect(editor.getText()).toBe("Energy $E=mc^2$ and $a_i^2$\n\n");
    const html = editor.getHTML();
    expect(html).toContain('data-type="mathInline"');
    expect(html).toContain('data-latex="E=mc^2"');
    expect(html).toContain(">$E=mc^2$</span>");
    expect(html).toContain(`data-id="${pid(2)}"`);
  });

  it("has no text while the source is empty, and keeps characters HTML must escape", () => {
    const editor = mount(page(text("a"), equation(2, ""), equation(3, 'x < "y" & z')) as JSONContent);
    expect(editor.getText()).toBe('a$x < "y" & z$\n\n');
    // Read back from the HTML it writes (a copy and a paste), the sources are the same.
    const copy = mount({ type: "doc", content: [{ type: "paragraph" }] });
    copy.commands.setContent(editor.getHTML());
    expect(equations(copy)).toEqual(["", 'x < "y" & z']);
  });

  it("is read from HTML: a chip makes an equation, with no source when it has none", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      '<p>a <span data-type="mathInline" data-latex="x^2">$x^2$</span> b ' +
        '<span data-type="mathInline">no source</span> c ' +
        '<span data-type="mathInline" data-latex="">$$</span></p>',
    );
    expect(equations(editor)).toEqual(["x^2", "", ""]);
    expect(editor.getText()).toBe("a $x^2$ b  c ");
  });

  it("is inserted by a command that gives it an id and a source, and refuses where inline atoms cannot go", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "Hi" }] }] });
    placeCursor(editor, 3);
    expect(editor.commands.insertMathInline({ latex: "x^2" })).toBe(true);
    expect(equations(editor)).toEqual(["x^2"]);
    const node = editor.state.doc.firstChild?.child(1);
    expect(node?.type.name).toBe("mathInline");
    expect(node?.attrs.id).toMatch(UUID);
    // The caret ends up right after it, and the command takes no argument either.
    expect(editor.state.selection.from).toBe(editor.state.doc.firstChild!.nodeSize - 1);
    expect(editor.commands.insertMathInline()).toBe(true);
    expect(equations(editor)).toEqual(["x^2", ""]);

    const code = mount({ type: "doc", content: [{ type: "codeBlock", content: [{ type: "text", text: "x" }] }] });
    placeCursor(code, 1);
    expect(code.commands.insertMathInline({ latex: "x" })).toBe(false);
    expect(equations(code)).toEqual([]);
  });
});

// ----- typing `$x$` --------------------------------------------------------------------------

describe("typing $x$", () => {
  /** An editor with one empty paragraph and the caret in it. */
  function emptyEditor(): Editor {
    const editor = mount({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] });
    placeCursor(editor, 1);
    return editor;
  }

  it("turns the text between two dollars into an equation when the closing one is typed, the caret after it", () => {
    const editor = emptyEditor();
    typeText(editor, "$x");
    expect(equations(editor)).toEqual([]); // not before the closing dollar
    typeText(editor, "$");
    expect(equations(editor)).toEqual(["x"]);
    const paragraph = editor.state.doc.firstChild;
    expect(paragraph?.childCount).toBe(1);
    expect(paragraph?.firstChild?.type.name).toBe("mathInline");
    expect(paragraph?.firstChild?.attrs.id).toMatch(UUID);
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe(2); // right after the node

    // Typing goes on after it.
    typeText(editor, " is");
    expect(editor.getText()).toBe("$x$ is");
  });

  it("works after text, with or without a space before the opening dollar, and for a longer source", () => {
    const editor = emptyEditor();
    typeText(editor, String.raw`Energy $E=mc^2$ and a$\frac{a}{b}$`);
    expect(equations(editor)).toEqual(["E=mc^2", String.raw`\frac{a}{b}`]);
    expect(editor.state.doc.firstChild?.child(0).text).toBe("Energy ");
    expect(editor.state.doc.firstChild?.child(2).text).toBe(" and a");
    expect(editor.getText()).toBe(String.raw`Energy $E=mc^2$ and a$\frac{a}{b}$`);
  });

  it("works in a heading and a list item", () => {
    const heading = mount({ type: "doc", content: [{ type: "heading", attrs: { level: 2 } }] });
    placeCursor(heading, 1);
    typeText(heading, "Proof of $a^2$");
    expect(equations(heading)).toEqual(["a^2"]);

    const list = mount({
      type: "doc",
      content: [{ type: "bulletList", content: [{ type: "listItem", content: [{ type: "paragraph" }] }] }],
    });
    placeCursor(list, 3);
    typeText(list, "Let $n$");
    expect(equations(list)).toEqual(["n"]);
  });

  it("does nothing with whitespace right after the opening dollar or before the closing one, or with no source", () => {
    for (const typed of ["$ x$", "$x $", "$$", "$ $", "costs $5 and $", String.raw`$x\$`]) {
      const editor = emptyEditor();
      typeText(editor, typed);
      expect(equations(editor), typed).toEqual([]);
      expect(editor.getText(), typed).toBe(typed);
      editor.destroy();
    }
  });

  it("leaves `$$` to the block: `$$ ` still makes a math block, and `$$x$` is text", () => {
    const block = emptyEditor();
    typeText(block, "$$ ");
    expect(block.state.doc.firstChild?.type.name).toBe("mathBlock");
    expect(equations(block)).toEqual([]);

    const plain = emptyEditor();
    typeText(plain, "$$x$");
    expect(plain.state.doc.firstChild?.type.name).toBe("paragraph");
    expect(equations(plain)).toEqual([]);
    expect(plain.getText()).toBe("$$x$");

    // `$$` after other text is text, as for the block rule, and a `$$` run is never an equation's dollar.
    const inline = emptyEditor();
    typeText(inline, "a $$b$$");
    expect(equations(inline)).toEqual([]);
    expect(inline.getText()).toBe("a $$b$$");
  });

  it("does not convert after a backslash (an escaped dollar)", () => {
    const editor = emptyEditor();
    typeText(editor, String.raw`\$x$`);
    expect(equations(editor)).toEqual([]);
    expect(editor.getText()).toBe(String.raw`\$x$`);
  });

  it("stays text in a code block, a math block, inline code and a toggle's summary", () => {
    const code = mount({ type: "doc", content: [{ type: "codeBlock" }] });
    placeCursor(code, 1);
    typeText(code, "$x$");
    expect(equations(code)).toEqual([]);
    expect(code.getText()).toBe("$x$");

    const math = mount({ type: "doc", content: [{ type: "mathBlock", attrs: { id: pid(0) } }] });
    placeCursor(math, 1);
    typeText(math, "a$x$");
    expect(equations(math)).toEqual([]);
    expect(math.state.doc.firstChild?.textContent).toBe("a$x$");

    const inlineCode = mount({
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "$x", marks: [{ type: "code" }] }] }],
    });
    placeCursor(inlineCode, 3);
    typeText(inlineCode, "$");
    expect(equations(inlineCode)).toEqual([]);

    const toggle = mount({
      type: "doc",
      content: [
        {
          type: "details",
          attrs: { open: true },
          content: [
            { type: "detailsSummary" },
            { type: "detailsContent", content: [{ type: "paragraph" }] },
          ],
        },
      ],
    });
    placeCursor(toggle, 2);
    typeText(toggle, "$x$");
    expect(equations(toggle)).toEqual([]);
    expect(toggle.state.doc.firstChild?.firstChild?.textContent).toBe("$x$");
  });

  it("is taken back by Backspace right after it, which leaves the typed text", () => {
    const editor = emptyEditor();
    typeText(editor, "$x$");
    expect(equations(editor)).toEqual(["x"]);
    expect(press(editor, "Backspace")).toBe(true);
    expect(equations(editor)).toEqual([]);
    expect(editor.getText()).toBe("$x$");
  });

  it("makes one equation per pair", () => {
    const editor = emptyEditor();
    typeText(editor, "$x$ and $y$");
    expect(equations(editor)).toEqual(["x", "y"]);
    expect(editor.getText()).toBe("$x$ and $y$");
  });
});

// ----- the chip -----------------------------------------------------------------------------

describe("the chip", () => {
  it("draws the equation through the renderer in inline mode, as a button named after its source", async () => {
    const { editor } = await openNotes(page(text("Energy "), equation(2, "E=mc^2"), text(" is")));
    expect(drawn(editor)).toEqual(["E=mc^2"]);
    expect(asked.length).toBeGreaterThan(0);
    expect(asked.every((call) => call.inline && call.latex === "E=mc^2")).toBe(true);
    expect(chip(editor).querySelector(".katex")).toHaveAttribute("data-inline", "true");
    expect(chip(editor)).toHaveClass("is-ready");

    const view = editor.view.dom.querySelector("[data-type='mathInline']");
    expect(view).toHaveAttribute("data-latex", "E=mc^2");
    const button = screen.getByRole("button", { name: t("mathInline.edit", { latex: "E=mc^2" }) });
    expect(button).toBe(chip(editor));
    expect(button).toHaveAttribute("aria-haspopup", "dialog");
    expect(button).toHaveAttribute("aria-expanded", "false");
    expect(editor.getText()).toContain("Energy $E=mc^2$ is");
  });

  it("shows a sigma and asks nothing of the renderer while the source is empty", async () => {
    const { editor } = await openNotes(page(text("a "), equation(2, ""), text(" b"), equation(3, "   ")));
    expect(asked).toEqual([]);
    expect(chip(editor, 0)).toHaveClass("is-empty");
    expect(chip(editor, 1)).toHaveClass("is-empty"); // blank is empty
    expect(chip(editor, 0).querySelector(".math-inline-placeholder svg")).toHaveAttribute("aria-hidden", "true");
    expect(chip(editor, 0)).toHaveAccessibleName(t("mathInline.editEmpty"));
    expect(chip(editor, 0).querySelector(".katex")).toBeNull();
  });

  it("shows the source and the message of the error when it does not parse", async () => {
    const { editor } = await openNotes(page(equation(2, "a@@b"), text(" and "), equation(3, "ok")));
    expect(chip(editor, 0)).toHaveClass("is-error");
    expect(chip(editor, 0)).toHaveAttribute("title", "Undefined control sequence: a@@b");
    expect(chip(editor, 0).querySelector(".math-inline-source")).toHaveTextContent("a@@b");
    expect(chip(editor, 0).querySelector(".katex")).toBeNull();
    // The one next to it is drawn.
    expect(chip(editor, 1)).toHaveClass("is-ready");
    expect(chip(editor, 1)).not.toHaveAttribute("title");
  });

  it("is selected like any atom, outlined as such, and Backspace or Delete removes it, in the page too", async () => {
    const { editor, memory } = await openNotes(page(text("a "), equation(2, "x"), text(" b")));
    const { page: pageId } = await idsOf(memory);
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".math-inline-node.ProseMirror-selectednode")).not.toBeNull();
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Backspace")).toBe(true);
    });
    expect(equations(editor)).toEqual([]);
    expect(editor.getText()).toBe("a  b\n\n");
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, pageId("Notes")))).not.toContain("mathInline");
    });

    // Delete, on an equation the caret is in front of.
    act(() => {
      editor.commands.insertContentAt(3, { type: "mathInline", attrs: { latex: "y" } });
    });
    expect(equations(editor)).toEqual(["y"]);
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Delete")).toBe(true);
    });
    expect(equations(editor)).toEqual([]);
  });

  it("opens nothing in a read-only editor: no button, no popover on a click or on Enter", async () => {
    const { editor, user } = await openNotes(page(equation(2, "x")));
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(chip(editor)).not.toHaveAttribute("role");
    });
    expect(chip(editor)).not.toHaveAttribute("aria-haspopup");
    expect(screen.queryByRole("button", { name: /Edit equation/u })).not.toBeInTheDocument();
    expect(drawn(editor)).toEqual(["x"]); // still drawn
    await user.click(chip(editor));
    expect(popover()).toBeNull();
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      editor.commands.keyboardShortcut("Enter");
    });
    expect(popover()).toBeNull();
  });

  it("draws with the real KaTeX, inline: no display-mode wrapper, MathML for readers, and rejects what does not parse", async () => {
    setMathRenderer(null);
    try {
      await loadMath();
      const inline = renderMathInline("E=mc^2");
      expect(inline).toContain("katex");
      expect(inline).toContain("<math");
      expect(inline).not.toContain("katex-display");
      expect(renderMath("E=mc^2")).toContain("katex-display"); // the block is unchanged
      expect(() => renderMathInline(String.raw`\frac{1}{`)).toThrow();
      expect(renderMathInline(String.raw`\href{javascript:alert(1)}{x}`)).not.toMatch(/<a[\s>]/u);
    } finally {
      installFake();
    }
  }, 60_000);
});

// ----- the popover --------------------------------------------------------------------------

describe("the popover", () => {
  async function openPopover(content: JsonValue = page(text("a "), equation(2, "E=mc^2"), text(" b"))) {
    const app = await openNotes(content);
    await app.user.click(chip(app.editor));
    const dialog = await findPopover();
    return { ...app, dialog };
  }

  /** What the preview of the open popover shows: its state and the source it was drawn from. */
  function preview(dialog: HTMLElement): { state: string | null; source: string | null } {
    const element = dialog.querySelector(".math-inline-preview");
    const markup = element?.querySelector("[data-source]")?.getAttribute("data-source");
    return {
      state: element?.getAttribute("data-state") ?? null,
      source: markup === null || markup === undefined ? null : decodeURIComponent(markup),
    };
  }

  it("opens on a click on the chip, with the source in a field that has the focus and the equation under it", async () => {
    const { editor, dialog } = await openPopover();
    expect(chip(editor)).toHaveAttribute("aria-expanded", "true");
    expect(field()).toHaveValue("E=mc^2");
    await waitFor(() => {
      expect(field()).toHaveFocus();
    });
    expect(preview(dialog)).toEqual({ state: "ready", source: "E=mc^2" });
    expect(dialog.querySelector(".katex")).toHaveAttribute("data-inline", "true");
    expect(field()).toHaveAttribute("aria-describedby", dialog.querySelector(".math-inline-preview")?.id);
  });

  it("draws the preview as the source is typed, without writing anything to the page", async () => {
    const { editor, dialog } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(field(), { target: { value: "x^2+1" } });
    expect(preview(dialog)).toEqual({ state: "ready", source: "x^2+1" });
    fireEvent.change(field(), { target: { value: "" } });
    expect(preview(dialog).state).toBe("empty");
    expect(within(dialog).getByText(t("mathInline.previewEmpty"))).toBeInTheDocument();
    fireEvent.change(field(), { target: { value: "a@@b" } });
    expect(preview(dialog).state).toBe("error");
    expect(within(dialog).getByText(t("mathInline.error"))).toHaveAttribute("title", "Undefined control sequence: a@@b");
    // Only the popover drew them: the page is as it was, and so is the document.
    expect(equations(editor)).toEqual(["E=mc^2"]);
    expect(drawn(editor)).toEqual(["E=mc^2"]);
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("applies with Enter: one update with the new source, the id kept, the popover closed, the caret after it", async () => {
    const { editor } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(field(), { target: { value: "x^2+1" } });
    fireEvent.keyDown(field(), { key: "Enter" });

    expect(equations(editor)).toEqual(["x^2+1"]);
    expect(popover()).toBeNull();
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(JSON.stringify(changesSince(doc, before))).toContain("x^2+1");
    const stored = loroToPortable(doc) as { content: { content: { type: string; attrs?: Record<string, unknown> }[] }[] };
    expect(stored.content[0]?.content[1]).toEqual({ type: "mathInline", attrs: { id: pid(2), latex: "x^2+1" } });
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe(positionOf(editor) + 1);
    await waitFor(() => {
      expect(drawn(editor)).toEqual(["x^2+1"]);
    });
    expect(chip(editor)).toHaveAttribute("aria-expanded", "false");
  });

  it("closes with Enter and no change when the source was not touched, or was typed back", async () => {
    const { editor } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.keyDown(field(), { key: "Enter" });
    expect(popover()).toBeNull();
    expect(changesSince(doc, before)).toEqual([]);

    await act(async () => {
      fireEvent.click(chip(editor));
      await tick();
    });
    fireEvent.change(field(), { target: { value: "other" } });
    fireEvent.change(field(), { target: { value: "E=mc^2" } });
    fireEvent.keyDown(field(), { key: "Enter" });
    expect(popover()).toBeNull();
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("cancels with Escape: what was typed is dropped, nothing is written, the focus goes back to the page", async () => {
    const { editor } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(field(), { target: { value: "dropped" } });
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => {
      expect(popover()).toBeNull();
    });
    expect(equations(editor)).toEqual(["E=mc^2"]);
    expect(changesSince(doc, before)).toEqual([]);
    await waitFor(() => {
      expect(editor.view.dom).toContainElement(document.activeElement as HTMLElement);
    });
    // Reopened, it shows the stored source again, not the dropped draft.
    await act(async () => {
      fireEvent.click(chip(editor));
      await tick();
    });
    expect(field()).toHaveValue("E=mc^2");
  });

  it("applies what was typed when the press is outside it, as Enter does", async () => {
    const { editor } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(field(), { target: { value: "y" } });
    fireEvent.pointerDown(document.body);
    await waitFor(() => {
      expect(popover()).toBeNull();
    });
    expect(equations(editor)).toEqual(["y"]);
    expect(changesSince(doc, before)).toHaveLength(1);
  });

  it("closes on a second click on the chip, without a change", async () => {
    const { editor, user } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(chip(editor));
    expect(popover()).toBeNull();
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("removes the equation with Backspace in an empty field, in one update", async () => {
    const { editor } = await openPopover();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    // Backspace in a field that holds something is left to the field.
    expect(fireEvent.keyDown(field(), { key: "Backspace" })).toBe(true);
    expect(equations(editor)).toEqual(["E=mc^2"]);

    fireEvent.change(field(), { target: { value: "" } });
    expect(fireEvent.keyDown(field(), { key: "Backspace" })).toBe(false); // taken
    expect(equations(editor)).toEqual([]);
    expect(popover()).toBeNull();
    expect(editor.getText()).toBe("a  b\n\n");
    expect(changesSince(doc, before)).toHaveLength(1);
    expect(JSON.stringify(loroToPortable(doc))).not.toContain("mathInline");
  });

  it("opens on an empty equation, and Backspace there removes it at once", async () => {
    const { editor, dialog } = await openPopover(page(text("a"), equation(2, ""), text("b")));
    expect(field()).toHaveValue("");
    expect(within(dialog).getByText(t("mathInline.previewEmpty"))).toBeInTheDocument();
    fireEvent.keyDown(field(), { key: "Backspace" });
    expect(equations(editor)).toEqual([]);
    expect(editor.getText()).toBe("ab\n\n");
  });

  it("stores a source that does not parse as it is, and the chip shows the error", async () => {
    const { editor } = await openPopover();
    fireEvent.change(field(), { target: { value: "a@@b" } });
    fireEvent.keyDown(field(), { key: "Enter" });
    expect(equations(editor)).toEqual(["a@@b"]);
    await waitFor(() => {
      expect(chip(editor)).toHaveClass("is-error");
    });
  });

  it("opens on Enter when the equation is selected with the arrow keys", async () => {
    const { editor } = await openNotes(page(text("a "), equation(2, "x"), text(" b")));
    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, positionOf(editor))));
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Enter")).toBe(true);
    });
    expect(await findPopover()).toBeInTheDocument();
    expect(field()).toHaveValue("x");
  });
});

// ----- /inline equation ---------------------------------------------------------------------

describe("/inline equation", () => {
  it("inserts an empty equation where the command was typed and opens its popover, where the source is typed", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    const { page: pageId } = await idsOf(memory);
    placeCursor(editor, editor.state.doc.child(0).nodeSize + 1); // in the empty second paragraph
    editor.commands.insertContent("/inline");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    expect(within(menu).getAllByRole("option")[0]).toHaveTextContent(t("slash.mathInline"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // The command text is gone, an empty equation is there, and its popover is open with the focus in the field.
    expect(equations(editor)).toEqual([""]);
    expect(editor.getText()).toBe("Intro\n\n");
    await findPopover();
    await waitFor(() => {
      expect(field()).toHaveFocus();
    });
    expect(chip(editor)).toHaveClass("is-empty");
    expect(chip(editor)).toHaveAttribute("aria-expanded", "true");

    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.change(field(), { target: { value: "x^2" } });
    fireEvent.keyDown(field(), { key: "Enter" });
    expect(equations(editor)).toEqual(["x^2"]);
    expect(popover()).toBeNull();
    expect(changesSince(doc, before)).toHaveLength(1);
    await waitFor(async () => {
      expect(await contentOf(memory, pageId("Notes"))).toMatchObject({
        content: [
          {},
          {
            type: "paragraph",
            content: [{ type: "mathInline", attrs: { latex: "x^2", id: expect.stringMatching(UUID) as unknown } }],
          },
        ],
      });
    });
  });

  it("is found by its words, after the block equation for the words they share", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/latex");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(
        within(menu)
          .getAllByRole("option")
          .map((option) => option.querySelector(".suggestion-title")?.textContent),
      ).toEqual([t("slash.math"), t("slash.mathInline")]);
    });
    // The inline entry has the equation glyph's inline variant and a hint of its own.
    const option = within(menu).getAllByRole("option")[1] as HTMLElement;
    expect(option.querySelector(".slash-item-icon svg")).toHaveAttribute("aria-hidden", "true");
    expect(option).toHaveTextContent(t("slash.mathInline.hint"));
  });

  it("is unavailable where inline atoms cannot go (a toggle's summary), and does nothing there", async () => {
    const { user } = await renderPages([
      {
        name: "Personal",
        pages: [
          {
            title: "Notes",
            content: {
              type: "doc",
              content: [
                {
                  type: "details",
                  attrs: { id: pid(0), open: true },
                  content: [
                    { type: "detailsSummary", attrs: { id: pid(1) }, content: [text("Sum")] },
                    { type: "detailsContent", attrs: { id: pid(2) }, content: [{ type: "paragraph", attrs: { id: pid(3) } }] },
                  ],
                },
              ],
            },
          },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1 + 1 + 3);
    editor.commands.insertContent(" /inline");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(menu).getAllByRole("option")[0] as HTMLElement;
    expect(option).toHaveTextContent(t("slash.mathInline"));
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(within(option).getByText(t("slash.inline.notHere"))).toBeInTheDocument();
    fireEvent.click(option);
    expect(equations(editor)).toEqual([]);
    expect(popover()).toBeNull();
    expect(editor.getText()).toContain("/inline");
  });
});

// ----- the Loro binding ---------------------------------------------------------------------

type Stored = { content: { content: { type: string; attrs?: Record<string, unknown>; text?: string }[] }[] };

describe("the equation in a bound document", () => {
  it("is written as one update per insert and one per change, as a node with its attributes", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    placeCursor(editor, 6);

    const before = doc.oplogVersion();
    expect(editor.commands.insertMathInline({ latex: "E=mc^2" })).toBe(true);
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(JSON.stringify(changes)).toContain("mathInline");
    const stored = loroToPortable(doc) as Stored;
    const inline = stored.content[0]?.content ?? [];
    expect(inline.map((node) => node.type)).toEqual(["text", "mathInline"]);
    expect(inline[0]?.text).toBe("Hello");
    expect(inline[1]?.attrs).toMatchObject({ latex: "E=mc^2" });
    expect(typeof inline[1]?.attrs?.id).toBe("string");

    // A change of the source: one update, the id kept.
    const id = editor.state.doc.firstChild?.child(1).attrs.id as string;
    const beforeChange = doc.oplogVersion();
    editor.view.dispatch(editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { id, latex: "x" }));
    expect(changesSince(doc, beforeChange)).toHaveLength(1);
    expect((loroToPortable(doc) as Stored).content[0]?.content[1]?.attrs).toEqual({ id, latex: "x" });

    // An empty source is stored as an empty string: the attribute is required, and an empty
    // string is a value, not a null that leaves no trace.
    editor.view.dispatch(editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { id, latex: "" }));
    expect((loroToPortable(doc) as Stored).content[0]?.content[1]?.attrs).toEqual({ id, latex: "" });
    expect(JSON.stringify(loroToPortable(doc))).toContain('"latex":""');
  });

  it("is undone and redone with the editor's undo, an insert and a change each in a step of their own", async () => {
    const doc = loroDocFrom(paragraphs("Hello"));
    const editor = await mountBound(doc);
    placeCursor(editor, 6);
    editor.commands.insertMathInline({ latex: "a" });
    await tick(600); // past the time within which changes undo together
    const attrs = editor.state.doc.firstChild?.child(1).attrs ?? {};
    editor.view.dispatch(editor.state.tr.setNodeMarkup(positionOf(editor), undefined, { ...attrs, latex: "b" }));
    expect(equations(editor)).toEqual(["b"]);

    expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    await tick();
    expect(equations(editor)).toEqual(["a"]);
    expect(editor.state.doc.firstChild?.child(1).attrs.id).toBe(attrs.id);
    expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    await tick();
    expect(equations(editor)).toEqual([]);
    expect(editor.getText()).toBe("Hello");
    expect(editor.commands.keyboardShortcut("Mod-Shift-z")).toBe(true);
    await tick();
    expect(equations(editor)).toEqual(["a"]);
  });

  it("is read back from the document: a page that holds equations shows them and writes nothing on load", async () => {
    const doc = loroDocFrom(page(text("On "), equation(2, "x^2"), text(" and "), equation(3, ""), equation(4, String.raw`\frac{a}{b}`)));
    const before = doc.oplogVersion();
    const editor = await mountBound(doc);
    expect(equations(editor)).toEqual(["x^2", "", String.raw`\frac{a}{b}`]);
    expect(changesSince(doc, before)).toEqual([]);
    expect(editor.getText()).toContain(String.raw`On $x^2$ and $\frac{a}{b}$`);
  });

  it("converges between two editors: an insert and a change on one show on the other, concurrent changes agree", async () => {
    const docA = loroDocFrom(paragraphs("Hello"));
    const docB = new LoroDoc();
    docB.import(docA.export({ mode: "snapshot" }));
    const editorA = await mountBound(docA);
    const editorB = await mountBound(docB);

    placeCursor(editorA, 6);
    editorA.commands.insertMathInline({ latex: "x" });
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(equations(editorA)).toEqual(["x"]);
    expect(equations(editorB)).toEqual(["x"]);
    expect(editorB.getText()).toBe("Hello$x$");
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(editorB.state.doc.firstChild?.child(1).attrs.id).toBe(editorA.state.doc.firstChild?.child(1).attrs.id);

    // Both change the source at once: the last write wins, the same one on both.
    const attrs = (editor: Editor): Record<string, unknown> => editor.state.doc.firstChild?.child(1).attrs ?? {};
    editorA.view.dispatch(editorA.state.tr.setNodeMarkup(positionOf(editorA), undefined, { ...attrs(editorA), latex: "a+b" }));
    editorB.view.dispatch(editorB.state.tr.setNodeMarkup(positionOf(editorB), undefined, { ...attrs(editorB), latex: "c+d" }));
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(equations(editorA)).toEqual(equations(editorB));
    expect(equations(editorA)).toHaveLength(1);
    expect(["a+b", "c+d"]).toContain(equations(editorA)[0]);

    // One removes it while the other types next to it.
    editorA.view.dispatch(editorA.state.tr.delete(positionOf(editorA), positionOf(editorA) + 1));
    placeCursor(editorB, 3);
    editorB.view.dispatch(editorB.state.tr.insertText("!"));
    sync(docA, docB);
    sync(docB, docA);
    await tick();
    expect(loroToPortable(docA)).toEqual(loroToPortable(docB));
    expect(equations(editorA)).toEqual(equations(editorB));
  });

  it("round-trips through the portable JSON: the Loro document builds the same editor content, and back", async () => {
    const json = page(
      text("a "),
      equation(2, "E=mc^2"),
      text(" "),
      equation(3, ""),
      { type: "hardBreak" },
      equation(4, String.raw`a$b \ c`),
    );
    const doc = new LoroDoc();
    portableToLoro(doc, json);
    expect(loroToPortable(doc)).toEqual(json);

    const editor = await mountBound(doc);
    expect(equations(editor)).toEqual(["E=mc^2", "", String.raw`a$b \ c`]);
    expect(loroToPortable(doc)).toEqual(json);
    // And an edit next to them leaves them as they are.
    placeCursor(editor, 3);
    editor.view.dispatch(editor.state.tr.insertText("X"));
    expect(equations(editor)).toEqual(["E=mc^2", "", String.raw`a$b \ c`]);
    expect(loroToPortable(doc)).toEqual(
      page(text("a X"), equation(2, "E=mc^2"), text(" "), equation(3, ""), { type: "hardBreak" }, equation(4, String.raw`a$b \ c`)),
    );
  });
});
