// The math block: schema parity with the fixture, how it is made (`$$ `, `/math`, the
// commands), the equation drawn in its view (with a fake renderer: the editor tests never
// load KaTeX) and how its source collapses and expands, what happens on errors, that drawing
// writes nothing to a bound Loro document, the keys, the block menu's conversions, and one
// test with the real KaTeX.

import { loroToPortable } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { fireEvent, waitFor } from "@testing-library/react";
import { AllSelection, TextSelection } from "@tiptap/pm/state";
import { LoroDoc } from "loro-crdt";
import type { LoroList, LoroMap, LoroText } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { loroDocFrom, pid, tick } from "../test/editor";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { kindOf, turnInto, turnIntoKinds } from "./blocks";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { loadMath, renderMath, setMathRenderer } from "./math";
import type { MathRenderer } from "./math";
import { ID_BEARING_NODES, createEditorSchema, createSchemaExtensions } from "./schema";
import { SLASH_COMMANDS, filterSlashCommands } from "./slash-commands";

const LATEX = "E = mc^2";

/** A page of a paragraph and a math block holding `latex` (none when empty). */
function mathDocument(latex = LATEX): JSONContent {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
      {
        type: "mathBlock",
        attrs: { id: pid(1) },
        ...(latex === "" ? {} : { content: [{ type: "text", text: latex }] }),
      },
    ],
  };
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

interface MountOptions {
  doc?: LoroDoc;
  editable?: boolean;
}

/** A Tiptap editor over the schema extensions (and the Loro binding when given a document). */
function mount(content: JSONContent, options: MountOptions = {}): Editor {
  const extensions = createSchemaExtensions();
  if (options.doc !== undefined) {
    extensions.push(LoroBinding.configure({ doc: options.doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const editor = new Editor({ element, extensions, content, editable: options.editable ?? true });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

/** Every latex the fake renderer was asked for. */
const drawn: string[] = [];
let render: MathRenderer;

/** The fake's markup: it says how long the source was, so a test can tell which draw it shows. */
function markupOf(latex: string): string {
  return `<span class="katex" data-length="${String(latex.length)}"></span>`;
}

function installFake(): void {
  render = markupOf;
  setMathRenderer((latex) => {
    drawn.push(latex);
    return render(latex);
  });
}

beforeEach(() => {
  drawn.length = 0;
  installFake();
});

afterEach(() => {
  vi.useRealTimers();
});

function blockOf(editor: Editor): HTMLElement {
  const block = editor.view.dom.querySelector<HTMLElement>("div.math-block");
  if (block === null) {
    throw new Error("no math block");
  }
  return block;
}

function sourceOf(editor: Editor): HTMLElement {
  const source = editor.view.dom.querySelector<HTMLElement>("div.math-block > div.math-block-source");
  if (source === null) {
    throw new Error("no math block source");
  }
  return source;
}

function renderOf(editor: Editor): HTMLElement {
  const element = editor.view.dom.querySelector<HTMLElement>("div.math-block > div.math-block-render");
  if (element === null) {
    throw new Error("no math block render");
  }
  return element;
}

/** Where the text of the first math block of the page starts and ends. */
function mathSpan(editor: Editor): { start: number; end: number } {
  let span: { start: number; end: number } | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (span === null && node.type.name === "mathBlock") {
      span = { start: pos + 1, end: pos + 1 + node.content.size };
    }
    return span === null;
  });
  if (span === null) {
    throw new Error("no math block in the page");
  }
  return span;
}

/** The position just inside the math block (the start of its text). */
function startOfMath(editor: Editor): number {
  return mathSpan(editor).start;
}

/** The end of the text of the math block. */
function endOfMath(editor: Editor): number {
  return mathSpan(editor).end;
}

/** Throws what it is given, whatever that is (a renderer may throw anything). */
function fail(reason: unknown): never {
  throw reason;
}

/** Types `text` the way the browser does (input rules only react to typed text). */
function typeText(editor: Editor, text: string): void {
  for (const character of text) {
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

/** Every op `doc` recorded since `since`, grouped by change. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

describe("the node of block schema v1", () => {
  it("is in the fixture with the content, group and attributes the editor has", () => {
    const schema = createEditorSchema();
    const shape = describeSchema(schema).nodes.mathBlock;
    const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;

    expect(shape).toEqual(fixture.nodes.mathBlock);
    expect(shape).toEqual({ content: "text*", group: "block", inline: false, atom: false, attrs: ["id"] });
    expect(schema.nodes.mathBlock?.spec.marks).toBe("");
    expect(schema.nodes.mathBlock?.spec.defining).toBe(true);
    expect(ID_BEARING_NODES).toContain("mathBlock");
    expect(fixture.idBearing).toContain("mathBlock");
  });

  it("is `<div data-type=\"mathBlock\">` with the LaTeX as text, and reads that back", () => {
    const editor = mount(mathDocument("a^2 + b^2"));
    expect(editor.getHTML()).toContain(`<div data-type="mathBlock" data-id="${pid(1)}">a^2 + b^2</div>`);

    editor.commands.setContent(`<div data-type="mathBlock" data-id="${pid(5)}">x = 1\ny = 2</div>`);
    const block = editor.state.doc.child(0);
    expect(block.type.name).toBe("mathBlock");
    expect(block.attrs).toEqual({ id: pid(5) });
    expect(block.textContent).toBe("x = 1\ny = 2");
  });

  it("holds no marks: bold over its text is refused", () => {
    const editor = mount(mathDocument());
    editor.commands.setTextSelection({ from: startOfMath(editor), to: endOfMath(editor) });
    editor.commands.toggleBold();
    const marks: string[] = [];
    editor.state.doc.child(1).descendants((node) => {
      marks.push(...node.marks.map((mark) => mark.type.name));
    });
    expect(marks).toEqual([]);
  });
});

describe("making a math block", () => {
  it("turns '$$ ' typed in an empty paragraph into one, the block id and the caret kept", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph", attrs: { id: pid(0) } }] });
    editor.commands.setTextSelection(1);
    typeText(editor, "$$ ");

    expect(editor.state.doc.childCount).toBe(1);
    const block = editor.state.doc.child(0);
    expect(block.type.name).toBe("mathBlock");
    expect(block.attrs).toEqual({ id: pid(0) });
    expect(block.textContent).toBe("");
    expect(editor.state.selection.$from.parent).toBe(block);
    expect(editor.view.dom.querySelector("div.math-block")).toHaveAttribute("data-id", pid(0));
  });

  it("leaves '$$ ' alone anywhere else: after text, before text, or inside a math block", () => {
    const withText = mount({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "abc" }] }],
    });
    withText.commands.setTextSelection(1);
    typeText(withText, "$$ ");
    expect(withText.state.doc.child(0).type.name).toBe("paragraph");
    expect(withText.state.doc.child(0).textContent).toBe("$$ abc");

    const after = mount({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "a" }] }],
    });
    after.commands.setTextSelection(2);
    typeText(after, "$$ ");
    expect(after.state.doc.child(0).type.name).toBe("paragraph");
    expect(after.state.doc.child(0).textContent).toBe("a$$ ");

    const inside = mount(mathDocument(""));
    inside.commands.setTextSelection(startOfMath(inside));
    typeText(inside, "$$ ");
    expect(inside.state.doc.child(1).type.name).toBe("mathBlock");
    expect(inside.state.doc.child(1).textContent).toBe("$$ ");
  });

  it("is what `/math` does: the block keeps its id and the caret is inside it, empty", () => {
    const editor = mount({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "/math" }] }],
    });
    const command = SLASH_COMMANDS.find((candidate) => candidate.id === "math");
    expect(command).toBeDefined();
    expect(t(command?.label ?? "slash.math")).toBe("Equation");

    command?.run(editor, { from: 1, to: 6 });
    const block = editor.state.doc.child(0);
    expect(editor.state.doc.childCount).toBe(1);
    expect(block.type.name).toBe("mathBlock");
    expect(block.attrs).toEqual({ id: pid(0) });
    expect(block.content.size).toBe(0);
    expect(editor.state.selection.$from.parent).toBe(block);
  });

  it("is found in the `/` menu by its name and its keywords", () => {
    const command = SLASH_COMMANDS.find((candidate) => candidate.id === "math");
    for (const query of ["math", "equation", "latex", "formula", "tex", "equ"]) {
      expect(filterSlashCommands(query), query).toContain(command);
    }
    expect(filterSlashCommands("diagram")).not.toContain(command);
  });

  it("is a code node, so the `/` menu stays closed in it (LaTeX has slashes)", () => {
    const editor = mount(mathDocument("a "));
    expect(editor.state.doc.child(1).type.spec.code).toBe(true);
  });

  it("is a command, and the same command turns it back into a paragraph", () => {
    const editor = mount({
      type: "doc",
      content: [{ type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "x = 1" }] }],
    });
    editor.commands.setTextSelection(2);

    expect(editor.commands.setMathBlock()).toBe(true);
    expect(editor.state.doc.child(0).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(0) });
    expect(editor.state.doc.child(0).textContent).toBe("x = 1");

    expect(editor.commands.toggleMathBlock()).toBe(true);
    expect(editor.state.doc.child(0).type.name).toBe("paragraph");
    expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(0) });
    expect(editor.state.doc.child(0).textContent).toBe("x = 1");

    expect(editor.commands.toggleMathBlock()).toBe(true);
    expect(editor.state.doc.child(0).type.name).toBe("mathBlock");
  });
});

describe("the equation", () => {
  it("is drawn in the view, below the source, and is no part of the text", async () => {
    const editor = mount(mathDocument());
    await tick();

    const block = blockOf(editor);
    expect([...block.children].map((child) => child.className.split(" ")[0])).toEqual([
      "math-block-source",
      "math-block-render",
    ]);
    expect(block).toHaveAttribute("data-type", "mathBlock");
    expect(block).toHaveAttribute("data-id", pid(1));
    const equation = renderOf(editor);
    expect(equation).toHaveAttribute("contenteditable", "false");
    expect(equation).toHaveAttribute("data-state", "ready");
    expect(equation).toHaveAccessibleName(t("mathBlock.label"));
    expect(equation.querySelector(".math-block-equation > span.katex")).toHaveAttribute(
      "data-length",
      String(LATEX.length),
    );
    expect(drawn).toEqual([LATEX]);
    // The source is the editable text, and only the text is the document's.
    expect(sourceOf(editor).textContent).toBe(LATEX);
    expect(editor.state.doc.child(1).textContent).toBe(LATEX);
    expect(sourceOf(editor)).toHaveAttribute("spellcheck", "false");
  });

  it("draws not on every key but once the text rests for 200 ms (and at once on load)", async () => {
    vi.useFakeTimers();
    const editor = mount(mathDocument("x"));
    await vi.advanceTimersByTimeAsync(0);
    expect(drawn).toEqual(["x"]);

    editor.view.dispatch(editor.state.tr.insertText("^2", endOfMath(editor)));
    await vi.advanceTimersByTimeAsync(150);
    editor.view.dispatch(editor.state.tr.insertText("+1", endOfMath(editor)));
    await vi.advanceTimersByTimeAsync(199); // 199 ms since the last key
    expect(drawn).toEqual(["x"]);
    expect(renderOf(editor).querySelector("span.katex")).toHaveAttribute("data-length", "1");

    await vi.advanceTimersByTimeAsync(1);
    expect(drawn).toEqual(["x", "x^2+1"]);
    expect(renderOf(editor).querySelector("span.katex")).toHaveAttribute("data-length", "5");
  });

  it("does not draw again for what is no change of its text", async () => {
    vi.useFakeTimers();
    const editor = mount(mathDocument());
    await vi.advanceTimersByTimeAsync(0);

    editor.view.dispatch(editor.state.tr.insertText("changed", 2, 3)); // the paragraph before the block
    editor.view.dispatch(editor.state.tr.setNodeAttribute(editor.state.doc.child(0).nodeSize, "id", pid(7)));
    await vi.advanceTimersByTimeAsync(1000);
    expect(drawn).toHaveLength(1);
  });

  it("keeps the last equation on show until the next one is drawn", async () => {
    vi.useFakeTimers();
    const editor = mount(mathDocument("a"));
    await vi.advanceTimersByTimeAsync(0);
    const first = renderOf(editor).querySelector("span.katex");
    expect(first).not.toBeNull();

    editor.view.dispatch(editor.state.tr.insertText("b", endOfMath(editor)));
    await vi.advanceTimersByTimeAsync(100);
    expect(renderOf(editor).querySelector("span.katex")).toBe(first); // the old one stays until the new one is drawn
    await vi.advanceTimersByTimeAsync(100);
    expect(renderOf(editor).querySelector("span.katex")).not.toBe(first);
  });

  it("shows nothing for an empty source and draws it once there is some", async () => {
    vi.useFakeTimers();
    const editor = mount(mathDocument(""));
    await vi.advanceTimersByTimeAsync(0);
    expect(renderOf(editor)).toHaveAttribute("data-state", "empty");
    expect(renderOf(editor)).toBeEmptyDOMElement();
    expect(drawn).toEqual([]);

    editor.view.dispatch(editor.state.tr.insertText("x^2", startOfMath(editor)));
    await vi.advanceTimersByTimeAsync(200);
    expect(renderOf(editor)).toHaveAttribute("data-state", "ready");
    expect(drawn).toEqual(["x^2"]);

    editor.view.dispatch(editor.state.tr.delete(startOfMath(editor), startOfMath(editor) + 3));
    await vi.advanceTimersByTimeAsync(200);
    expect(renderOf(editor)).toHaveAttribute("data-state", "empty");
    expect(drawn).toHaveLength(1); // nothing to draw: the renderer is not asked
  });

  it("treats a source of only white space as empty", async () => {
    const editor = mount(mathDocument(" \n "));
    await tick();
    expect(renderOf(editor)).toHaveAttribute("data-state", "empty");
    expect(drawn).toEqual([]);
  });
});

describe("the source", () => {
  it("is collapsed while the selection is elsewhere and expands when it enters the block", async () => {
    const editor = mount(mathDocument());
    await tick();
    const source = sourceOf(editor);

    expect(source).toHaveClass("is-collapsed");
    expect(source).toHaveAttribute("aria-hidden", "true");
    expect(blockOf(editor)).not.toHaveClass("is-editing");

    editor.commands.setTextSelection(startOfMath(editor) + 2);
    expect(source).not.toHaveClass("is-collapsed");
    expect(source).not.toHaveAttribute("aria-hidden");
    expect(blockOf(editor)).toHaveClass("is-editing");
    expect(renderOf(editor)).toHaveAttribute("data-state", "ready"); // the equation stays under it

    editor.commands.setTextSelection(2);
    expect(source).toHaveClass("is-collapsed");
    expect(blockOf(editor)).not.toHaveClass("is-editing");

    // A selection of the whole page is not a selection in the block.
    editor.commands.setTextSelection(startOfMath(editor));
    expect(source).not.toHaveClass("is-collapsed");
    editor.view.dispatch(editor.state.tr.setSelection(new AllSelection(editor.state.doc)));
    expect(source).toHaveClass("is-collapsed");
  });

  it("stays in view while it is empty, with a hint", async () => {
    const editor = mount(mathDocument(""));
    await tick();
    const source = sourceOf(editor);

    expect(source).not.toHaveClass("is-collapsed");
    expect(source).toHaveClass("is-empty");
    expect(source).toHaveAttribute("data-placeholder", t("mathBlock.placeholder"));

    editor.view.dispatch(editor.state.tr.insertText("x", startOfMath(editor)));
    expect(source).not.toHaveClass("is-empty");
  });

  it("collapses when something is typed, once the selection has left", async () => {
    vi.useFakeTimers();
    const editor = mount(mathDocument(""));
    await vi.advanceTimersByTimeAsync(0);
    editor.commands.setTextSelection(startOfMath(editor));
    editor.view.dispatch(editor.state.tr.insertText("x^2", startOfMath(editor)));
    await vi.advanceTimersByTimeAsync(200);
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed"); // the caret is in it

    editor.commands.setTextSelection(2);
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
  });

  it("is where the caret goes when the equation is clicked", async () => {
    const editor = mount(mathDocument());
    await tick();
    expect(sourceOf(editor)).toHaveClass("is-collapsed");

    const notPrevented = fireEvent.mouseDown(renderOf(editor));
    expect(notPrevented).toBe(false); // the editor keeps the focus, nothing is selected natively
    expect(editor.state.selection).toBeInstanceOf(TextSelection);
    expect(editor.state.selection.empty).toBe(true);
    expect(editor.state.selection.from).toBe(endOfMath(editor));
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(1));
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed");
    expect(editor.state.doc.child(1).textContent).toBe(LATEX); // a click edits nothing
  });

  it("does not take other buttons than the main one", async () => {
    const editor = mount(mathDocument());
    await tick();
    editor.commands.setTextSelection(2);

    expect(fireEvent.mouseDown(renderOf(editor), { button: 2 })).toBe(true);
    expect(editor.state.selection.from).toBe(2);
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
  });

  it("is not part of the document: the editor ignores what is drawn and the classes set on the view", async () => {
    const editor = mount(mathDocument());
    const before = editor.state.doc;
    let updates = 0;
    editor.on("update", () => {
      updates += 1;
    });
    await tick();

    // A change of that DOM the editor would otherwise read back.
    const equation = renderOf(editor);
    equation.append(document.createElement("em"));
    editor.commands.setTextSelection(startOfMath(editor)); // expands: classes change on the view
    editor.commands.setTextSelection(2); // collapses again
    await tick();
    await tick();
    expect(renderOf(editor)).toBe(equation);
    expect(equation.querySelector("em")).not.toBeNull();
    expect(editor.state.doc.eq(before)).toBe(true);
    expect(updates).toBe(0);
  });
});

describe("when the equation cannot be typeset", () => {
  const MESSAGE = "KaTeX parse error: Expected 'EOF', got '}' at position 3";

  it("shows the message in place of the equation, keeps the source in view and the text as it is", async () => {
    render = () => {
      throw new Error(`  ${MESSAGE}\n\n`);
    };
    const editor = mount(mathDocument("a}b"));
    await tick();

    const equation = renderOf(editor);
    expect(equation).toHaveAttribute("data-state", "error");
    expect(equation).toHaveTextContent(t("mathBlock.error"));
    expect(equation.querySelector(".math-block-error-detail")?.textContent).toBe(MESSAGE); // trimmed
    expect(equation.querySelector("span.katex")).toBeNull();
    // The caret is elsewhere, and still the source can be read and edited.
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed");
    expect(editor.state.doc.child(1).textContent).toBe("a}b");
  });

  it("goes back to the equation when the text is fixed", async () => {
    vi.useFakeTimers();
    render = (latex) => {
      if (latex.includes("}")) {
        throw new Error(MESSAGE);
      }
      return markupOf(latex);
    };
    const editor = mount(mathDocument("a}b"));
    await vi.advanceTimersByTimeAsync(0);
    expect(renderOf(editor)).toHaveAttribute("data-state", "error");

    editor.view.dispatch(editor.state.tr.delete(startOfMath(editor) + 1, startOfMath(editor) + 2));
    await vi.advanceTimersByTimeAsync(200);
    const equation = renderOf(editor);
    expect(equation).toHaveAttribute("data-state", "ready");
    expect(equation).not.toHaveTextContent(t("mathBlock.error"));
    expect(equation.querySelector("span.katex")).not.toBeNull();
    expect(sourceOf(editor)).toHaveClass("is-collapsed"); // the equation is on show again
    expect(editor.state.doc.child(1).textContent).toBe("ab");
  });

  it("shows an error that is not an Error, trims a long message and never writes any of it", async () => {
    render = () => {
      throw new TypeError(`${"x".repeat(1000)}\n`);
    };
    const long = mount(mathDocument());
    await tick();
    const detail = renderOf(long).querySelector(".math-block-error-detail")?.textContent ?? "";
    expect(detail.startsWith("xxx")).toBe(true);
    expect(detail.length).toBeLessThan(400);
    expect(detail.endsWith("...")).toBe(true);

    render = () => fail("unusual");
    const other = mount(mathDocument());
    await tick();
    expect(renderOf(other)).toHaveTextContent("unusual");
    expect(other.state.doc.child(1).textContent).toBe(LATEX);
  });

  it("draws again with the next change, whatever went wrong the last time", async () => {
    let attempts = 0;
    render = (latex) => {
      attempts += 1;
      if (attempts === 1) {
        throw new Error("chunk missing");
      }
      return markupOf(latex);
    };
    const editor = mount(mathDocument());
    await tick();
    expect(renderOf(editor)).toHaveAttribute("data-state", "error");
    expect(renderOf(editor)).toHaveTextContent("chunk missing");

    editor.view.dispatch(editor.state.tr.insertText("!", endOfMath(editor)));
    await tick(250);
    expect(renderOf(editor)).toHaveAttribute("data-state", "ready");
  });
});

describe("a read-only editor", () => {
  it("shows the equation only, whatever the selection, and a click on it does nothing", async () => {
    const editor = mount(mathDocument(), { editable: false });
    await tick();

    expect(blockOf(editor)).toHaveClass("is-readonly");
    expect(renderOf(editor)).toHaveAttribute("data-state", "ready");
    expect(renderOf(editor).querySelector("span.katex")).not.toBeNull();
    expect(sourceOf(editor)).toHaveClass("is-collapsed");

    editor.commands.setTextSelection(startOfMath(editor) + 1);
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
    expect(blockOf(editor)).not.toHaveClass("is-editing");

    editor.commands.setTextSelection(2);
    expect(fireEvent.mouseDown(renderOf(editor))).toBe(true);
    expect(editor.state.selection.from).toBe(2);
  });

  it("keeps the source collapsed when the equation cannot be typeset, and says so", async () => {
    render = () => {
      throw new Error("bad");
    };
    const editor = mount(mathDocument(), { editable: false });
    await tick();
    expect(renderOf(editor)).toHaveTextContent(t("mathBlock.error"));
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
  });

  it("follows the editor when it becomes editable", async () => {
    const editor = mount(mathDocument(), { editable: false });
    await tick();
    editor.commands.setTextSelection(startOfMath(editor));
    expect(sourceOf(editor)).toHaveClass("is-collapsed");

    editor.setEditable(true);
    expect(blockOf(editor)).not.toHaveClass("is-readonly");
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed"); // the caret is in it
  });
});

describe("the keys", () => {
  it("Backspace in an empty math block makes it a paragraph again, same block, same id", () => {
    const editor = mount(mathDocument(""));
    editor.commands.setTextSelection(startOfMath(editor));

    expect(press(editor, "Backspace")).toBe(true);
    expect(editor.state.doc.childCount).toBe(2);
    expect(editor.state.doc.child(1).type.name).toBe("paragraph");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1) });
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(1));
  });

  it("Backspace in a math block with text is not taken (it deletes a character as usual)", () => {
    const editor = mount(mathDocument("ab"));
    editor.commands.setTextSelection(startOfMath(editor) + 2);
    expect(press(editor, "Backspace")).toBe(false);
    expect(editor.state.doc.child(1).type.name).toBe("mathBlock");
  });

  it("Enter writes a newline into the source: multi-line LaTeX, one block", () => {
    const editor = mount(mathDocument("a &= b"));
    editor.commands.setTextSelection(endOfMath(editor));

    expect(press(editor, "Enter")).toBe(true);
    expect(editor.state.doc.childCount).toBe(2);
    expect(editor.state.doc.child(1).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(1).textContent).toBe("a &= b\n");
    typeText(editor, "c &= d");
    expect(editor.state.doc.child(1).textContent).toBe("a &= b\nc &= d");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1) });
  });

  it("a third Enter at the end leaves the block and drops the two newlines", () => {
    const editor = mount(mathDocument("x\n\n"));
    editor.commands.setTextSelection(endOfMath(editor));

    expect(press(editor, "Enter")).toBe(true);
    expect(editor.state.doc.child(1).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(1).textContent).toBe("x");
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
  });

  it("Escape and ArrowDown at the end go to the next block, or to a new paragraph after the last", () => {
    const content = mathDocument();
    content.content?.push({ type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "After" }] });
    const editor = mount(content);

    editor.commands.setTextSelection(startOfMath(editor) + 1);
    expect(press(editor, "Escape")).toBe(true);
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));
    expect(editor.state.selection.from).toBe(editor.state.doc.child(0).nodeSize + editor.state.doc.child(1).nodeSize + 1);

    editor.commands.setTextSelection(endOfMath(editor));
    expect(press(editor, "ArrowDown")).toBe(true);
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));

    // The last block of the page: a paragraph is added after it.
    const last = mount(mathDocument());
    last.commands.setTextSelection(endOfMath(last));
    expect(press(last, "ArrowDown")).toBe(true);
    expect(last.state.doc.childCount).toBe(3);
    expect(last.state.doc.child(2).type.name).toBe("paragraph");
    expect(last.state.selection.$from.parent).toBe(last.state.doc.child(2));

    const again = mount(mathDocument());
    again.commands.setTextSelection(endOfMath(again));
    expect(press(again, "Escape")).toBe(true);
    expect(again.state.doc.childCount).toBe(3);
  });

  it("ArrowDown elsewhere in the source, and Escape outside a math block, are not taken", () => {
    const editor = mount(mathDocument("ab"));
    editor.commands.setTextSelection(startOfMath(editor) + 1);
    expect(press(editor, "ArrowDown")).toBe(false);

    editor.commands.setTextSelection(2);
    expect(press(editor, "Escape")).toBe(false);
  });

  it("ArrowUp at the start of a math block that starts the page makes room above it", () => {
    const editor = mount({
      type: "doc",
      content: [{ type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: LATEX }] }],
    });
    editor.commands.setTextSelection(1);

    expect(press(editor, "ArrowUp")).toBe(true);
    expect(editor.state.doc.childCount).toBe(2);
    expect(editor.state.doc.child(0).type.name).toBe("paragraph");
    expect(editor.state.doc.child(1).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(1).textContent).toBe(LATEX);

    const second = mount(mathDocument());
    second.commands.setTextSelection(startOfMath(second));
    expect(press(second, "ArrowUp")).toBe(false); // there is a block above: the browser moves the caret
  });

  it("arrow keys step into a math block from the block above and below, the source expanding", async () => {
    const content = mathDocument();
    content.content?.push({ type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "After" }] });
    const editor = mount(content);
    await tick();
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
    // jsdom has no layout: say that the caret is at the edge of its line.
    vi.spyOn(editor.view, "endOfTextblock").mockReturnValue(true);

    editor.commands.setTextSelection(2); // in "Before"
    expect(press(editor, "ArrowDown")).toBe(true);
    expect(editor.state.selection.from).toBe(startOfMath(editor)); // the start of the source
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed");

    editor.commands.setTextSelection(endOfMath(editor) + 2); // in "After"
    expect(press(editor, "ArrowUp")).toBe(true);
    expect(editor.state.selection.from).toBe(endOfMath(editor)); // the end of the source

    editor.commands.setTextSelection(7); // end of "Before"
    expect(press(editor, "ArrowRight")).toBe(true);
    expect(editor.state.selection.from).toBe(startOfMath(editor));

    editor.commands.setTextSelection(endOfMath(editor) + 2); // start of "After"
    expect(press(editor, "ArrowLeft")).toBe(true);
    expect(editor.state.selection.from).toBe(endOfMath(editor));
  });

  it("leaves arrow keys alone when the caret is not at the edge of its line, or a modifier is held", () => {
    const editor = mount(mathDocument());
    const edge = vi.spyOn(editor.view, "endOfTextblock").mockReturnValue(false);
    editor.commands.setTextSelection(2);
    expect(press(editor, "ArrowDown")).toBe(false);
    expect(editor.state.selection.from).toBe(2);

    edge.mockReturnValue(true);
    expect(
      editor.view.someProp("handleKeyDown", (handle) =>
        handle(editor.view, new KeyboardEvent("keydown", { key: "ArrowDown", shiftKey: true })),
      ),
    ).not.toBe(true);
    expect(editor.state.selection.from).toBe(2);
  });
});

describe("the block menu's conversions", () => {
  it("offers it among the kinds a block can become, and knows it as the current kind", () => {
    const editor = mount(mathDocument());
    expect(kindOf(editor.state.doc.child(1))).toBe("mathBlock");
    expect(turnIntoKinds(editor.state.doc.child(1))).toContain("mathBlock");
    expect(turnIntoKinds(editor.state.doc.child(0))).toContain("mathBlock");
    const pos = editor.state.doc.child(0).nodeSize;
    expect(turnInto(editor.state, pos, "mathBlock")).toBeNull(); // already one
  });

  it("turns text into an equation and back, keeping the id and the text", () => {
    const editor = mount(mathDocument());
    const pos = editor.state.doc.child(0).nodeSize;

    const toParagraph = turnInto(editor.state, pos, "paragraph");
    expect(toParagraph).not.toBeNull();
    if (toParagraph !== null) {
      editor.view.dispatch(toParagraph);
    }
    expect(editor.state.doc.child(1).type.name).toBe("paragraph");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1) });
    expect(editor.state.doc.child(1).textContent).toBe(LATEX);

    const toMath = turnInto(editor.state, 0, "mathBlock"); // the first paragraph, "Before"
    expect(toMath).not.toBeNull();
    if (toMath !== null) {
      editor.view.dispatch(toMath);
    }
    expect(editor.state.doc.child(0).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(0) });
    expect(editor.state.doc.child(0).textContent).toBe("Before");
    expect(editor.view.dom.querySelectorAll("div.math-block")).toHaveLength(1); // the other one is a paragraph now
  });

  it("treats it like a code block: the text becomes code and an equation again, as plain text", () => {
    const editor = mount(mathDocument("a\nb"));
    const pos = editor.state.doc.child(0).nodeSize;

    const toCode = turnInto(editor.state, pos, "codeBlock");
    if (toCode !== null) {
      editor.view.dispatch(toCode);
    }
    expect(editor.state.doc.child(1).type.name).toBe("codeBlock");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1), language: null });
    expect(editor.state.doc.child(1).textContent).toBe("a\nb");

    const back = turnInto(editor.state, pos, "mathBlock");
    if (back !== null) {
      editor.view.dispatch(back);
    }
    expect(editor.state.doc.child(1).type.name).toBe("mathBlock");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1) });
    expect(editor.state.doc.child(1).textContent).toBe("a\nb");

    // Inside a quote: the quote's text becomes one equation per text block.
    const quoted = mount({
      type: "doc",
      content: [
        {
          type: "blockquote",
          attrs: { id: pid(3) },
          content: [{ type: "paragraph", attrs: { id: pid(4) }, content: [{ type: "text", text: "q = 1" }] }],
        },
      ],
    });
    const wrapped = turnInto(quoted.state, 0, "mathBlock");
    if (wrapped !== null) {
      quoted.view.dispatch(wrapped);
    }
    expect(quoted.state.doc.child(0).type.name).toBe("mathBlock");
    expect(quoted.state.doc.child(0).attrs).toEqual({ id: pid(4) });
    expect(quoted.state.doc.child(0).textContent).toBe("q = 1");
  });
});

describe("with the Loro binding", () => {
  /** A math block document as the core would hold it. */
  function mathLoroDoc(latex = LATEX): LoroDoc {
    return loroDocFrom({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: latex }] },
      ],
    });
  }

  /** The editor's initial content, read from the document like the page editor does. */
  function contentOfLoro(doc: LoroDoc): JSONContent {
    const root = doc.getMap(ROOT_DOC_KEY);
    return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
  }

  it("writes nothing when a document is loaded and the equation is drawn", async () => {
    const doc = mathLoroDoc();
    const before = doc.oplogVersion();
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    expect(renderOf(editor)).toHaveAttribute("data-state", "ready");
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1) });
    expect(doc.oplogVersion().toJSON()).toEqual(before.toJSON());
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("writes nothing for the view: not for a click, a collapse, an expansion or a change of its DOM", async () => {
    const doc = mathLoroDoc();
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();
    const before = doc.oplogVersion();

    fireEvent.mouseDown(renderOf(editor)); // the caret goes into the source: the block expands
    expect(sourceOf(editor)).not.toHaveClass("is-collapsed");
    editor.commands.setTextSelection(2); // and collapses
    expect(sourceOf(editor)).toHaveClass("is-collapsed");
    renderOf(editor).append(document.createElement("em"));
    await tick();
    await tick();

    expect(changesSince(doc, before)).toEqual([]);
    expect(doc.oplogVersion().toJSON()).toEqual(before.toJSON());
  });

  it("writes the typed text and nothing else when the source is edited", async () => {
    const doc = mathLoroDoc("x");
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    editor.commands.setTextSelection(endOfMath(editor));
    typeText(editor, "^2");
    await tick(250); // the equation is drawn again
    expect(drawn).toEqual(["x", "x^2"]);
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: "x^2" }] },
      ],
    });
  });

  it("writes an Enter as a newline in the same text, and the multi-line source reads back as it was", async () => {
    const doc = mathLoroDoc("a &= b");
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    editor.commands.setTextSelection(endOfMath(editor));
    expect(press(editor, "Enter")).toBe(true);
    typeText(editor, "c &= d");
    const written = loroToPortable(doc);
    expect(written).toMatchObject({
      content: [{ type: "paragraph" }, { type: "mathBlock", content: [{ type: "text", text: "a &= b\nc &= d" }] }],
    });

    const reopened = mount(contentOfLoro(doc));
    expect(reopened.state.doc.child(1).textContent).toBe("a &= b\nc &= d");
  });

  it("follows a change the document made itself, writing nothing back", async () => {
    const doc = mathLoroDoc("x");
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    // Another replica types into the equation.
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const block = (remote.getMap("doc").get("children") as LoroList).get(1) as LoroMap;
    ((block.get("children") as LoroList).get(0) as LoroText).insert(1, "^2");
    remote.commit();
    const before = doc.oplogVersion();
    doc.import(remote.export({ mode: "update", from: doc.oplogVersion() }));
    await tick(300);

    expect(editor.state.doc.child(1).textContent).toBe("x^2");
    expect(drawn.at(-1)).toBe("x^2");
    expect(doc.exportJsonUpdates(before).changes.every((change) => !change.id.endsWith(`@${doc.peerIdStr}`))).toBe(true);
  });
});

describe("the real KaTeX", () => {
  it("typesets an equation in display mode, as HTML and MathML", async () => {
    setMathRenderer(null);
    try {
      await loadMath();
      const html = renderMath(LATEX);
      expect(html).toContain("katex");
      expect(html).toContain("katex-display");
      expect(html).toContain("<math");
      expect(html).toContain("katex-html");
    } finally {
      installFake();
    }
  }, 60_000);

  it("rejects what does not parse, and does not turn a link into one", async () => {
    setMathRenderer(null);
    try {
      await loadMath();
      expect(() => renderMath(String.raw`\frac{1}{`)).toThrow();
      expect(() => renderMath(String.raw`\unknowncommand`)).toThrow();
      const link = renderMath(String.raw`\href{javascript:alert(1)}{x}`);
      // `trust: false` shows the command as text (in the error colour) instead of making a link.
      expect(link).not.toMatch(/<a[\s>]/u);
      expect(link).not.toContain("href=");
    } finally {
      installFake();
    }
  }, 60_000);

  it("draws a math block of the editor", async () => {
    setMathRenderer(null);
    try {
      const editor = mount(mathDocument(String.raw`\int_0^1 x^2\,dx = \frac{1}{3}`));
      await waitFor(() => {
        expect(renderOf(editor)).toHaveAttribute("data-state", "ready");
      });
      expect(renderOf(editor).querySelector(".katex-display")).not.toBeNull();
      expect(renderOf(editor).querySelector("annotation")?.textContent).toBe(String.raw`\int_0^1 x^2\,dx = \frac{1}{3}`);
    } finally {
      installFake();
    }
  }, 60_000);
});
