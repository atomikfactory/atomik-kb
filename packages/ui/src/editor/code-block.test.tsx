// The code block: offline syntax highlighting (decorations, never document content), the
// language picker (one attribute, the block id stays) and what both do to a bound Loro
// document (nothing on load, one attribute change per pick), and the drawn preview of
// `mermaid` blocks (with a fake renderer: the editor tests never load Mermaid).

import { loroToPortable } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { fireEvent } from "@testing-library/react";
import { LoroDoc } from "loro-crdt";
import type { LoroList, LoroMap } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { loroDocFrom, pid, tick } from "../test/editor";
import { pickerValueFor } from "./code-block";
import { LoroBinding } from "./extensions";
import { createNodeFromLoroObj, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode } from "./loro-binding";
import { renderDiagram, setDiagramRenderer } from "./mermaid";
import type { DiagramRenderer, DiagramTheme } from "./mermaid";
import { createEditorSchema, createSchemaExtensions } from "./schema";

const SOURCE = "const answer: number = 42;\nexport function greet(name: string) {\n  return `hi ${name}`; // done\n}";

function codeDocument(language: string | null, text = SOURCE): JSONContent {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
      {
        type: "codeBlock",
        attrs: { id: pid(1), language },
        content: [{ type: "text", text }],
      },
    ],
  };
}

const mounted: { editor: Editor; element: HTMLElement }[] = [];

interface MountOptions {
  doc?: LoroDoc;
  editable?: boolean;
  onMouseDown?: () => void;
}

/** A Tiptap editor over the schema extensions (and the Loro binding when given a document). */
function mount(content: JSONContent, options: MountOptions = {}): Editor {
  const extensions = createSchemaExtensions();
  if (options.doc !== undefined) {
    extensions.push(LoroBinding.configure({ doc: options.doc, readOnly: false }));
  }
  const element = document.createElement("div");
  document.body.append(element);
  const { onMouseDown } = options;
  const editor = new Editor({
    element,
    extensions,
    content,
    editable: options.editable ?? true,
    editorProps: {
      handleDOMEvents: {
        mousedown: () => {
          onMouseDown?.();
          return false;
        },
      },
    },
  });
  mounted.push({ editor, element });
  return editor;
}

afterEach(() => {
  for (const { editor, element } of mounted.splice(0)) {
    editor.destroy();
    element.remove();
  }
});

function pickerOf(editor: Editor): HTMLSelectElement {
  const select = editor.view.dom.querySelector("select.code-block-language");
  if (!(select instanceof HTMLSelectElement)) {
    throw new Error("no language picker");
  }
  return select;
}

function codeBlockAttrs(editor: Editor): Record<string, unknown> {
  return editor.state.doc.child(1).attrs;
}

/** Every op `doc` recorded since `since`, grouped by change. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

describe("highlighting", () => {
  it("styles the code of a block with a language, as decorations on `pre > code`", () => {
    const editor = mount(codeDocument("typescript"));
    const code = editor.view.dom.querySelector("div.code-block > pre > code.language-typescript");

    expect(code).not.toBeNull();
    expect(code?.querySelector("span.hljs-keyword")).toHaveTextContent("const");
    expect(code?.querySelector("span.hljs-number")).toHaveTextContent("42");
    const titles = [...(code?.querySelectorAll("span.hljs-title") ?? [])].map((title) => title.textContent);
    expect(titles).toContain("greet");
    expect(code?.querySelector("span.hljs-comment")).toHaveTextContent("// done");
    // Decorations wrap the text; the text itself is untouched.
    expect(code?.textContent).toBe(SOURCE);
    expect(editor.state.doc.child(1).textContent).toBe(SOURCE);
  });

  it("knows the aliases Markdown fences use", () => {
    const editor = mount(codeDocument("ts"));
    expect(editor.view.dom.querySelector("code.language-ts span.hljs-keyword")).toHaveTextContent("const");
  });

  it("leaves plain text and unknown languages unstyled (no guessing)", () => {
    for (const language of [null, "no-such-language"]) {
      const editor = mount(codeDocument(language));
      expect(editor.view.dom.querySelector("pre code")?.textContent).toBe(SOURCE);
      expect(editor.view.dom.querySelector("pre code [class*='hljs-']"), String(language)).toBeNull();
    }
  });

  it("follows the text as it is edited", () => {
    const editor = mount(codeDocument("python", "x = 1"));
    expect(editor.view.dom.querySelector("pre code span.hljs-keyword")).toBeNull();
    const end = editor.state.doc.child(0).nodeSize + 1 + "x = 1".length;
    // The caret is not in the block: the highlighting follows the document, not the selection.
    editor.view.dispatch(editor.state.tr.insertText("\ndef f(): pass", end));
    expect(editor.view.dom.querySelector("pre code span.hljs-keyword")).toHaveTextContent("def");
  });
});

describe("the language picker", () => {
  it("lists plain text first and then the languages of the common grammars, sorted", () => {
    const select = pickerOf(mount(codeDocument(null)));
    const options = [...select.options];

    expect(select).toHaveAccessibleName(t("codeBlock.language"));
    expect(options[0]).toHaveValue("");
    expect(options[0]).toHaveTextContent(t("codeBlock.plainText"));
    const labels = options.slice(1).map((option) => option.textContent ?? "");
    expect(labels).toEqual([...labels].sort((a, b) => a.localeCompare(b, "en", { sensitivity: "base" })));
    const values = options.map((option) => option.value);
    for (const language of ["javascript", "typescript", "python", "rust", "bash", "json", "sql", "css", "xml"]) {
      expect(values, language).toContain(language);
    }
    expect(values).not.toContain("plaintext");
    expect(options.find((option) => option.value === "typescript")).toHaveTextContent("TypeScript");
    expect(options.find((option) => option.value === "cpp")).toHaveTextContent("C++");
  });

  it("shows the stored language: a listed one, an alias of one, or the raw name", () => {
    expect(pickerOf(mount(codeDocument(null))).value).toBe("");
    expect(pickerOf(mount(codeDocument("typescript"))).value).toBe("typescript");
    expect(pickerOf(mount(codeDocument("ts"))).value).toBe("typescript");
    expect(pickerOf(mount(codeDocument("plaintext"))).value).toBe("");

    const unlisted = mount(codeDocument("zig"));
    expect(pickerOf(unlisted).value).toBe("zig");
    expect(pickerOf(unlisted).selectedOptions[0]).toHaveTextContent("zig");
    expect(pickerValueFor(undefined)).toBe("");
  });

  it("sets `language` and keeps the block id", () => {
    const editor = mount(codeDocument("typescript"));
    const select = pickerOf(editor);

    fireEvent.change(select, { target: { value: "python" } });
    expect(codeBlockAttrs(editor)).toEqual({ id: pid(1), language: "python" });
    expect(editor.view.dom.querySelector("pre code")).toHaveClass("language-python");
    expect(editor.view.dom.querySelector("div.code-block")).toHaveAttribute("data-id", pid(1));
    // Restyled at once, though the caret is not in the block: `const` is no Python keyword.
    const keywords = [...editor.view.dom.querySelectorAll("pre code span.hljs-keyword")].map((span) => span.textContent);
    expect(keywords).toContain("return");
    expect(keywords).not.toContain("const");
    expect(editor.state.doc.child(1).textContent).toBe(SOURCE);

    fireEvent.change(pickerOf(editor), { target: { value: "" } });
    expect(codeBlockAttrs(editor)).toEqual({ id: pid(1), language: null });
    expect(editor.view.dom.querySelector("pre code")).not.toHaveClass("language-python");
    expect(editor.view.dom.querySelector("pre code [class*='hljs-']")).toBeNull();
    expect(editor.view.dom.querySelector("div.code-block")).toHaveAttribute("data-id", pid(1));
  });

  it("changes the right block when there are several", () => {
    const content = codeDocument("rust");
    content.content?.push({
      type: "codeBlock",
      attrs: { id: pid(2), language: null },
      content: [{ type: "text", text: "echo hi" }],
    });
    const editor = mount(content);
    const second = editor.view.dom.querySelectorAll("select.code-block-language")[1];
    expect(second).toBeInstanceOf(HTMLSelectElement);

    fireEvent.change(second as HTMLSelectElement, { target: { value: "bash" } });
    expect(editor.state.doc.child(1).attrs).toEqual({ id: pid(1), language: "rust" });
    expect(editor.state.doc.child(2).attrs).toEqual({ id: pid(2), language: "bash" });
  });

  it("keeps the picker right when the language changes from elsewhere", () => {
    const editor = mount(codeDocument("rust"));
    editor.view.dispatch(editor.state.tr.setNodeAttribute(editor.state.doc.child(0).nodeSize, "language", "go"));
    expect(pickerOf(editor).value).toBe("go");
    expect(editor.view.dom.querySelector("pre code")).toHaveClass("language-go");
  });

  it("is disabled in a read-only editor and changes nothing", () => {
    const editor = mount(codeDocument("rust"), { editable: false });
    const select = pickerOf(editor);

    expect(select).toBeDisabled();
    fireEvent.change(select, { target: { value: "go" } });
    expect(codeBlockAttrs(editor)).toEqual({ id: pid(1), language: "rust" });
    expect(select.value).toBe("rust");
  });

  it("is not part of the text: the caret stays where it was and ProseMirror ignores the control's events", () => {
    let mouseDowns = 0;
    const editor = mount(codeDocument("rust"), {
      onMouseDown: () => {
        mouseDowns += 1;
      },
    });
    editor.commands.setTextSelection(2);

    fireEvent.mouseDown(pickerOf(editor));
    expect(mouseDowns).toBe(0);
    expect(editor.state.selection.from).toBe(2);

    // The same event on the code is ProseMirror's.
    fireEvent.mouseDown(editor.view.dom.querySelector("pre code") as Element);
    expect(mouseDowns).toBe(1);
  });
});

describe("the code block keeps its behaviour", () => {
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

  it("turns '```lang ' into a code block of that language, and '``` ' into one without", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setTextSelection(1);
    typeText(editor, "```ts ");
    expect(editor.state.doc.child(0).type.name).toBe("codeBlock");
    expect(editor.state.doc.child(0).attrs.language).toBe("ts");
    expect(pickerOf(editor).value).toBe("typescript");

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setTextSelection(1);
    typeText(editor, "``` ");
    expect(editor.state.doc.child(0).type.name).toBe("codeBlock");
    expect(editor.state.doc.child(0).attrs.language).toBeNull();
  });

  it("makes a block a code block with the command behind `/code`, and back", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text: "x = 1" }] }] });
    editor.commands.setTextSelection(2);
    expect(editor.commands.setCodeBlock()).toBe(true);
    expect(editor.state.doc.child(0).type.name).toBe("codeBlock");
    expect(editor.state.doc.child(0).attrs.language).toBeNull();
    expect(editor.view.dom.querySelector("div.code-block pre code")).toHaveTextContent("x = 1");
    expect(editor.commands.toggleCodeBlock()).toBe(true);
    expect(editor.state.doc.child(0).type.name).toBe("paragraph");
  });

  it("leaves the block on a third Enter, like StarterKit's code block did", () => {
    const editor = mount({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: pid(1), language: "rust" }, content: [{ type: "text", text: "fn a() {}\n\n" }] }],
    });
    editor.commands.setTextSelection(1 + "fn a() {}\n\n".length);
    const handled = editor.view.someProp("handleKeyDown", (handle) =>
      handle(editor.view, new KeyboardEvent("keydown", { key: "Enter" })),
    );
    expect(handled).toBe(true);

    expect(editor.state.doc.child(0).type.name).toBe("codeBlock");
    expect(editor.state.doc.child(0).textContent).toBe("fn a() {}");
    expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(1), language: "rust" });
    expect(editor.state.doc.child(1).type.name).toBe("paragraph");
  });

  it("is the same node of block schema v1: content, group and attributes as in the fixture", () => {
    const schema = createEditorSchema();
    const shape = describeSchema(schema).nodes.codeBlock;
    const fixture = (blockSchemaFixture as unknown as BlockSchemaFixture).nodes.codeBlock;

    expect(shape).toEqual(fixture);
    expect(shape).toEqual({ content: "text*", group: "block", inline: false, atom: false, attrs: ["id", "language"] });
    expect(schema.nodes.codeBlock?.spec.code).toBe(true);
    expect(schema.nodes.codeBlock?.spec.marks).toBe("");
    expect(schema.nodes.doc?.contentMatch.defaultType?.name).toBe("paragraph");
  });

  it("leaves a ```-fence for Markdown export as it was: the language is the info string", () => {
    const editor = mount(codeDocument("rust", "fn main() {}"));
    const node = editor.state.doc.child(1);
    expect(node.attrs.language).toBe("rust");
    expect(node.textContent).toBe("fn main() {}");
  });
});

describe("with the Loro binding", () => {
  /** A code block document as the core would hold it. */
  function codeLoroDoc(language: string | null): LoroDoc {
    return loroDocFrom({
      type: "doc",
      content: [
        {
          type: "codeBlock",
          attrs: { id: pid(1), ...(language === null ? {} : { language }) },
          content: [{ type: "text", text: SOURCE }],
        },
      ],
    });
  }

  /** The editor's initial content, read from the document like the page editor does. */
  function contentOfLoro(doc: LoroDoc): JSONContent {
    const root = doc.getMap(ROOT_DOC_KEY);
    return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
  }

  it("shows the highlighting and writes nothing when a document is loaded", async () => {
    const doc = codeLoroDoc("typescript");
    const before = doc.oplogVersion();
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    expect(editor.view.dom.querySelector("pre code.language-typescript span.hljs-keyword")).toHaveTextContent("const");
    expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(1), language: "typescript" });
    expect(doc.oplogVersion().toJSON()).toEqual(before.toJSON());
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("writes nothing for a block without language either (no detection result is stored)", async () => {
    const doc = codeLoroDoc(null);
    const before = doc.oplogVersion();
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    expect(editor.view.dom.querySelector("pre code [class*='hljs-']")).toBeNull();
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("makes exactly one attribute change per pick, and the block id stays", async () => {
    const doc = codeLoroDoc("typescript");
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    const beforePick = doc.oplogVersion();
    fireEvent.change(pickerOf(editor), { target: { value: "python" } });
    expect(changesSince(doc, beforePick)).toEqual([
      [expect.objectContaining({ type: "insert", key: "language", value: "python" })],
    ]);
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [
        {
          type: "codeBlock",
          attrs: { id: pid(1), language: "python" },
          content: [{ type: "text", text: SOURCE }],
        },
      ],
    });

    const beforePlain = doc.oplogVersion();
    fireEvent.change(pickerOf(editor), { target: { value: "" } });
    expect(changesSince(doc, beforePlain)).toEqual([[expect.objectContaining({ type: "delete", key: "language" })]]);
    expect(loroToPortable(doc)).toEqual({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: SOURCE }] }],
    });
  });

  it("does not turn a change the document made itself into a write", async () => {
    const doc = codeLoroDoc("rust");
    const editor = mount(contentOfLoro(doc), { doc });
    await tick();
    await tick();

    // Another replica changes the language: the picker follows, nothing comes back out.
    const remote = new LoroDoc();
    remote.import(doc.export({ mode: "snapshot" }));
    const block = (remote.getMap("doc").get("children") as LoroList).get(0) as LoroMap;
    const attributes = block.get("attributes") as LoroMap;
    attributes.set("language", "go");
    remote.commit();
    const before = doc.oplogVersion();
    doc.import(remote.export({ mode: "update", from: doc.oplogVersion() }));
    await tick();

    expect(pickerOf(editor).value).toBe("go");
    expect(editor.view.dom.querySelector("pre code")).toHaveClass("language-go");
    expect(doc.exportJsonUpdates(before).changes.every((change) => !change.id.endsWith(`@${doc.peerIdStr}`))).toBe(true);
  });
});

describe("mermaid diagrams", () => {
  const SOURCE_OF_DIAGRAM = "flowchart TD\n  A[Start] --> B[End]";

  /** Every draw the fake renderer was asked for. */
  const drawn: { source: string; theme: DiagramTheme }[] = [];
  let render: DiagramRenderer;

  /** The fake's SVG: it says how long the source was, so a test can tell which draw it shows. */
  function svgOf(source: string): string {
    return `<svg xmlns="http://www.w3.org/2000/svg" data-length="${String(source.length)}"></svg>`;
  }

  function installFake(): void {
    render = (source) => Promise.resolve(svgOf(source));
    setDiagramRenderer((source, theme) => {
      drawn.push({ source, theme });
      return render(source, theme);
    });
  }

  beforeEach(() => {
    drawn.length = 0;
    installFake();
  });

  afterEach(() => {
    vi.useRealTimers();
    delete document.documentElement.dataset.theme;
  });

  function previewOf(editor: Editor): HTMLElement | null {
    return editor.view.dom.querySelector<HTMLElement>("div.code-block-preview");
  }

  function svgIn(editor: Editor): Element | null {
    return previewOf(editor)?.querySelector("svg") ?? null;
  }

  /** The end of the text of the document's last block (the code block of `codeDocument`). */
  function endOfCode(editor: Editor): number {
    return editor.state.doc.content.size - 1;
  }

  /** What the code block shows, child by child of its view. */
  function parts(editor: Editor): string[] {
    const block = editor.view.dom.querySelector("div.code-block");
    return [...(block?.children ?? [])].map(
      (child) => child.tagName.toLowerCase() + (child.className === "" ? "" : `.${child.className}`),
    );
  }

  it("lists Mermaid in the picker: plain text, no highlighting, any casing of the name", () => {
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    const option = [...pickerOf(editor).options].find((candidate) => candidate.value === "mermaid");

    expect(option).toHaveTextContent("Mermaid");
    expect(pickerOf(editor).value).toBe("mermaid");
    expect(pickerValueFor("Mermaid")).toBe("mermaid");
    expect(editor.view.dom.querySelector("pre code.language-mermaid [class*='hljs-']")).toBeNull();
    expect(editor.view.dom.querySelector("pre code")?.textContent).toBe(SOURCE_OF_DIAGRAM);
  });

  it("draws the source below it, inside the block: the text stays the editable part", async () => {
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await tick();

    expect(parts(editor)).toEqual(["select.code-block-language", "pre", "div.code-block-preview"]);
    const preview = previewOf(editor);
    expect(preview).toHaveAttribute("contenteditable", "false");
    expect(preview).toHaveAttribute("data-state", "ready");
    expect(preview).toHaveAccessibleName(t("codeBlock.diagramLabel"));
    // The preview holds the renderer's SVG and is no part of the text.
    expect(svgIn(editor)).toHaveAttribute("data-length", String(SOURCE_OF_DIAGRAM.length));
    const code = editor.view.dom.querySelector("pre code");
    expect(code?.textContent).toBe(SOURCE_OF_DIAGRAM);
    expect(code?.querySelector("svg")).toBeNull();
    expect(editor.state.doc.child(1).textContent).toBe(SOURCE_OF_DIAGRAM);
    expect(drawn).toEqual([{ source: SOURCE_OF_DIAGRAM, theme: "light" }]);
  });

  it("has no preview in other languages, and follows the picker both ways", async () => {
    const editor = mount(codeDocument("typescript"));
    await tick();
    expect(previewOf(editor)).toBeNull();
    expect(drawn).toEqual([]);

    fireEvent.change(pickerOf(editor), { target: { value: "mermaid" } });
    await tick();
    expect(codeBlockAttrs(editor)).toEqual({ id: pid(1), language: "mermaid" });
    expect(parts(editor)).toEqual(["select.code-block-language", "pre", "div.code-block-preview"]);
    expect(svgIn(editor)).toHaveAttribute("data-length", String(SOURCE.length));
    expect(drawn).toEqual([{ source: SOURCE, theme: "light" }]);

    fireEvent.change(pickerOf(editor), { target: { value: "python" } });
    await tick();
    expect(previewOf(editor)).toBeNull();
    expect(parts(editor)).toEqual(["select.code-block-language", "pre"]);
    expect(editor.state.doc.child(1).textContent).toBe(SOURCE);
    expect(drawn).toHaveLength(1);
  });

  it("shows nothing late for a block that stopped being a diagram", async () => {
    let finish: (svg: string) => void = () => undefined;
    render = () =>
      new Promise((resolve) => {
        finish = resolve;
      });
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await tick();
    expect(previewOf(editor)).toHaveAttribute("data-state", "loading");
    expect(previewOf(editor)).toHaveTextContent(t("codeBlock.diagramLoading"));

    fireEvent.change(pickerOf(editor), { target: { value: "" } });
    finish(svgOf(SOURCE_OF_DIAGRAM));
    await tick();
    expect(previewOf(editor)).toBeNull();
    expect(editor.view.dom.querySelector("svg")).toBeNull();
  });

  it("draws a block that is already a diagram when the page opens, read-only too", async () => {
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM), { editable: false });
    await tick();

    expect(svgIn(editor)).toHaveAttribute("data-length", String(SOURCE_OF_DIAGRAM.length));
    expect(pickerOf(editor)).toBeDisabled();
  });

  it("draws again, not on every key but once the text rests for 300 ms", async () => {
    vi.useFakeTimers();
    const editor = mount(codeDocument("mermaid", "flowchart TD"));
    await vi.advanceTimersByTimeAsync(0);
    expect(drawn.map((draw) => draw.source)).toEqual(["flowchart TD"]); // at once on load

    editor.view.dispatch(editor.state.tr.insertText("\n  A-->B", endOfCode(editor)));
    await vi.advanceTimersByTimeAsync(200);
    editor.view.dispatch(editor.state.tr.insertText("\n  B-->C", endOfCode(editor)));
    await vi.advanceTimersByTimeAsync(299); // 299 ms since the last key
    expect(drawn).toHaveLength(1);
    expect(svgIn(editor)).toHaveAttribute("data-length", "12");

    await vi.advanceTimersByTimeAsync(1);
    const edited = "flowchart TD\n  A-->B\n  B-->C";
    expect(drawn.map((draw) => draw.source)).toEqual(["flowchart TD", edited]);
    expect(svgIn(editor)).toHaveAttribute("data-length", String(edited.length));
  });

  it("does not draw again for what is no change of its text", async () => {
    vi.useFakeTimers();
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await vi.advanceTimersByTimeAsync(0);

    editor.view.dispatch(editor.state.tr.insertText("changed", 2, 3)); // the paragraph before the block
    editor.view.dispatch(editor.state.tr.setNodeAttribute(editor.state.doc.child(0).nodeSize, "id", pid(7)));
    await vi.advanceTimersByTimeAsync(1000);
    expect(drawn).toHaveLength(1);
  });

  it("keeps the last diagram on show while the next one is drawn", async () => {
    vi.useFakeTimers();
    const editor = mount(codeDocument("mermaid", "flowchart TD"));
    await vi.advanceTimersByTimeAsync(0);
    const first = svgIn(editor);
    expect(first).toHaveAttribute("data-length", "12");

    let finish: (svg: string) => void = () => undefined;
    render = () =>
      new Promise((resolve) => {
        finish = resolve;
      });
    editor.view.dispatch(editor.state.tr.insertText("\n  A-->B", endOfCode(editor)));
    await vi.advanceTimersByTimeAsync(300);
    expect(drawn).toHaveLength(2); // the second draw is under way
    expect(svgIn(editor)).toBe(first); // and the first diagram is still there

    finish(svgOf("the second"));
    await vi.advanceTimersByTimeAsync(0);
    expect(svgIn(editor)).not.toBe(first);
    expect(svgIn(editor)).toHaveAttribute("data-length", "10");
  });

  it("shows what the renderer says when the text does not parse, and the diagram again when it does", async () => {
    vi.useFakeTimers();
    render = () => Promise.reject(new Error("Parse error on line 2:\n  A--\nExpecting 'SEMI'"));
    const editor = mount(codeDocument("mermaid", "flowchart TD\n  A--"));
    await vi.advanceTimersByTimeAsync(0);

    const preview = previewOf(editor);
    expect(preview).toHaveAttribute("data-state", "error");
    expect(preview).toHaveTextContent(t("codeBlock.diagramError"));
    expect(preview).toHaveTextContent("Parse error on line 2:");
    expect(preview).toHaveTextContent("Expecting 'SEMI'");
    expect(preview?.querySelector("svg")).toBeNull();
    expect(editor.state.doc.child(1).textContent).toBe("flowchart TD\n  A--"); // the text is left alone

    render = (source) => Promise.resolve(svgOf(source));
    editor.view.dispatch(editor.state.tr.insertText(">B", endOfCode(editor)));
    await vi.advanceTimersByTimeAsync(300);
    expect(previewOf(editor)).toHaveAttribute("data-state", "ready");
    expect(previewOf(editor)).not.toHaveTextContent(t("codeBlock.diagramError"));
    expect(svgIn(editor)).not.toBeNull();
  });

  it("trims a long message, and shows a failure that is not a plain Error too", async () => {
    render = () => Promise.reject(new Error(`  ${"x".repeat(1000)}\n\n`));
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await tick();
    const detail = previewOf(editor)?.querySelector(".code-block-preview-detail")?.textContent ?? "";
    expect(detail.startsWith("xxx")).toBe(true);
    expect(detail.length).toBeLessThan(500);
    expect(detail.endsWith("...")).toBe(true);

    render = () => Promise.reject(new TypeError("unusual"));
    const other = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await tick();
    expect(previewOf(other)).toHaveTextContent("unusual");
  });

  it("shows a hint for an empty source and draws it once there is some", async () => {
    vi.useFakeTimers();
    const editor = mount({
      type: "doc",
      content: [{ type: "codeBlock", attrs: { id: pid(1), language: "mermaid" } }],
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(previewOf(editor)).toHaveAttribute("data-state", "empty");
    expect(previewOf(editor)).toHaveTextContent(t("codeBlock.diagramEmpty"));
    expect(drawn).toEqual([]);

    editor.view.dispatch(editor.state.tr.insertText("flowchart TD", 1));
    await vi.advanceTimersByTimeAsync(300);
    expect(previewOf(editor)).toHaveAttribute("data-state", "ready");
    expect(drawn.map((draw) => draw.source)).toEqual(["flowchart TD"]);

    editor.view.dispatch(editor.state.tr.delete(1, 1 + "flowchart TD".length));
    await vi.advanceTimersByTimeAsync(300);
    expect(previewOf(editor)).toHaveAttribute("data-state", "empty");
    expect(drawn).toHaveLength(1); // nothing to draw: the renderer is not asked
  });

  it("draws in the theme the page shows and again when it changes", async () => {
    document.documentElement.dataset.theme = "dark";
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    await tick();
    expect(drawn).toEqual([{ source: SOURCE_OF_DIAGRAM, theme: "dark" }]);

    document.documentElement.dataset.theme = "light";
    await tick();
    expect(drawn.map((draw) => draw.theme)).toEqual(["dark", "light"]);
    expect(previewOf(editor)).toHaveAttribute("data-state", "ready");

    // "system" resolves to the same theme (jsdom has no dark colour scheme): nothing to redraw.
    document.documentElement.dataset.theme = "system";
    await tick();
    expect(drawn).toHaveLength(2);
  });

  it("follows the system colour scheme when the theme is system", async () => {
    const listeners = new Set<() => void>();
    const media = {
      matches: true,
      addEventListener: (_type: string, listener: () => void) => {
        listeners.add(listener);
      },
      removeEventListener: (_type: string, listener: () => void) => {
        listeners.delete(listener);
      },
    };
    Object.defineProperty(window, "matchMedia", { configurable: true, writable: true, value: () => media });
    try {
      document.documentElement.dataset.theme = "system";
      mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
      await tick();
      expect(drawn.map((draw) => draw.theme)).toEqual(["dark"]);

      media.matches = false;
      for (const listener of [...listeners]) {
        listener();
      }
      await tick();
      expect(drawn.map((draw) => draw.theme)).toEqual(["dark", "light"]);
    } finally {
      for (const { editor, element } of mounted.splice(0)) {
        editor.destroy();
        element.remove();
      }
      Object.defineProperty(window, "matchMedia", { configurable: true, writable: true, value: undefined });
    }
    expect(listeners.size).toBe(0); // the last preview let go of the system setting
  });

  it("is not part of the document: the editor ignores what is drawn in it", async () => {
    const editor = mount(codeDocument("mermaid", SOURCE_OF_DIAGRAM));
    const before = editor.state.doc;
    let updates = 0;
    editor.on("update", () => {
      updates += 1;
    });
    await tick();

    const preview = previewOf(editor);
    preview?.append(document.createElement("em")); // a change of that DOM the editor would otherwise read back
    await tick();
    await tick();
    expect(previewOf(editor)).toBe(preview);
    expect(preview?.querySelector("em")).not.toBeNull();
    expect(editor.state.doc).toBe(before);
    expect(updates).toBe(0);
  });

  describe("with the Loro binding", () => {
    function diagramLoroDoc(): LoroDoc {
      return loroDocFrom({
        type: "doc",
        content: [
          {
            type: "codeBlock",
            attrs: { id: pid(1), language: "mermaid" },
            content: [{ type: "text", text: SOURCE_OF_DIAGRAM }],
          },
        ],
      });
    }

    function contentOf(doc: LoroDoc): JSONContent {
      const root = doc.getMap(ROOT_DOC_KEY);
      return createNodeFromLoroObj(createEditorSchema(), root as unknown as LoroNode, new Map()).toJSON() as JSONContent;
    }

    it("writes nothing for the preview: not on load, not when it is drawn, not when its DOM changes", async () => {
      const doc = diagramLoroDoc();
      const before = doc.oplogVersion();
      const editor = mount(contentOf(doc), { doc });
      await tick();
      await tick();

      expect(svgIn(editor)).not.toBeNull();
      expect(editor.state.doc.child(0).attrs).toEqual({ id: pid(1), language: "mermaid" });
      expect(changesSince(doc, before)).toEqual([]);

      previewOf(editor)?.append(document.createElement("em"));
      await tick();
      await tick();
      expect(changesSince(doc, before)).toEqual([]);
      expect(doc.oplogVersion().toJSON()).toEqual(before.toJSON());
    });

    it("writes the typed text and nothing else when the source is edited", async () => {
      const doc = diagramLoroDoc();
      const editor = mount(contentOf(doc), { doc });
      await tick();
      await tick();

      editor.view.dispatch(editor.state.tr.insertText("\n  B --> C", editor.state.doc.content.size - 1));
      await tick(350); // the preview draws again
      expect(drawn).toHaveLength(2);
      expect(loroToPortable(doc)).toEqual({
        type: "doc",
        content: [
          {
            type: "codeBlock",
            attrs: { id: pid(1), language: "mermaid" },
            content: [{ type: "text", text: `${SOURCE_OF_DIAGRAM}\n  B --> C` }],
          },
        ],
      });
    });
  });

  describe("the real Mermaid", () => {
    it("loads under the test environment and parses a flowchart", async () => {
      const { default: mermaid } = await import("mermaid");

      await expect(mermaid.parse("flowchart TD\n A-->B")).resolves.toMatchObject({
        diagramType: expect.stringContaining("flowchart") as unknown,
      });
      await expect(mermaid.parse("flowchart TD\n A--")).rejects.toThrow();
    }, 60_000);

    it("draws through the renderer module: text that does not parse is rejected and leaves nothing in the page", async () => {
      setDiagramRenderer(null);
      try {
        await expect(renderDiagram("flowchart TD\n  A--", "light")).rejects.toThrow();
        await expect(renderDiagram("flowchart TD\n  A--", "dark")).rejects.toThrow();
        expect(document.body.querySelector("[id*='atomik-diagram']")).toBeNull();
      } finally {
        installFake();
      }
    }, 60_000);
  });
});
