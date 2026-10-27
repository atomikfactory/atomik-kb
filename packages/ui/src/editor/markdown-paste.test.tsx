// Pasting Markdown into the page editor: the text becomes blocks, in one transaction, in the
// live schema; everything that is not Markdown, and every place that wants text as it is,
// pastes the way it always did.

import { loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { waitFor } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";

import { loroDocFrom, paragraphs, pid, tick } from "../test/editor";
import { contentOf, idsOf, openPage, pasteEvent, renderPages } from "../test/page";
import { LoroBinding } from "./extensions";
import { undo } from "./loro-binding";
import { MarkdownPaste, nodesFromPortable } from "./markdown-paste";
import { createEditorSchema, createSchemaExtensions } from "./schema";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const FENCE = "```";
const IMAGE = "5f0c1d2e-3a4b-4c5d-8e6f-708192a3b4c5";

const mounted: { editor: Editor; element: HTMLElement }[] = [];

interface MountOptions {
  doc?: LoroDoc;
}

/** A Tiptap editor over the schema extensions and the paste handler (and the Loro binding when given a document). */
function mount(content: JSONContent, options: MountOptions = {}): Editor {
  const extensions = [...createSchemaExtensions(), MarkdownPaste];
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

function block(type: string, text: string, attrs: Record<string, unknown> = {}): JSONContent {
  return { type, attrs: { id: pid(1), ...attrs }, ...(text === "" ? {} : { content: [{ type: "text", text }] }) };
}

/** A page holding one empty paragraph. */
function emptyPage(): JSONContent {
  return { type: "doc", content: [block("paragraph", "")] };
}

/** Pastes `text` (and `html`, when the clipboard carries some) the way the webview delivers it. */
function paste(editor: Editor, text: string, html = ""): void {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data = {
    types: [...(html === "" ? [] : ["text/html"]), "text/plain"],
    files: [],
    getData: (type: string) => (type === "text/html" ? html : type === "text/plain" ? text : ""),
  };
  Object.defineProperty(event, "clipboardData", { value: data });
  editor.view.dom.dispatchEvent(event);
}

/** The top-level blocks as `type:text`, with the level of a heading (`heading2:Title`). */
function outline(editor: Editor): string[] {
  const lines: string[] = [];
  editor.state.doc.forEach((node) => {
    const level = node.type.name === "heading" ? String(node.attrs.level) : "";
    lines.push(`${node.type.name}${level}:${node.textContent}`);
  });
  return lines;
}

/** Puts the cursor (or the selection) at positions of the document. */
function select(editor: Editor, from: number, to = from): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, from, to)));
}

/** The names of every node of the document, in order. */
function kinds(editor: Editor): string[] {
  const names: string[] = [];
  editor.state.doc.descendants((node) => {
    names.push(node.type.name);
  });
  return names;
}

describe("pasting Markdown", () => {
  it("pastes a heading and a list as blocks, with new ids", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "# Title\n\n- one\n- two");

    // The empty paragraph the cursor was in is replaced.
    expect(outline(editor)).toEqual(["heading1:Title", "bulletList:onetwo"]);
    expect(editor.state.doc.child(1).childCount).toBe(2);
    editor.state.doc.descendants((node) => {
      if (node.type.name !== "text") {
        expect(node.attrs.id, node.type.name).toMatch(UUID);
      }
    });
    editor.state.doc.check();
  });

  it("pastes a code fence as a code block with its language", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, `${FENCE}rust\nfn main() {\n    println!("hi");\n}\n${FENCE}\n`);

    expect(editor.getJSON().content).toMatchObject([
      { type: "codeBlock", attrs: { language: "rust" }, content: [{ type: "text", text: 'fn main() {\n    println!("hi");\n}' }] },
    ]);
  });

  it("pastes a GFM table as a table, its first row as header cells", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "| Feature | Status |\n| --- | --- |\n| Tables | **Done** |\n| Pipes | a \\| b |\n");

    expect(outline(editor)).toEqual(["table:FeatureStatusTablesDonePipesa | b"]);
    const dom = editor.view.dom;
    expect(dom.querySelectorAll("table tr")).toHaveLength(3);
    expect(dom.querySelectorAll("table th")).toHaveLength(2);
    expect(dom.querySelectorAll("table td")).toHaveLength(4);
    expect(dom.querySelector("table td strong")).toHaveTextContent("Done");
    editor.state.doc.check();
  });

  it("pastes quotes, callouts, task lists, math, dividers and toggles", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(
      editor,
      [
        "> A quote",
        "",
        "> ⚠️ Careful",
        "",
        "- [x] done",
        "- [ ] todo",
        "",
        "3. third",
        "",
        "$$",
        "E = mc^2",
        "$$",
        "",
        "---",
        "",
        "<details open>",
        "<summary>More</summary>",
        "",
        "Inside",
        "",
        "</details>",
      ].join("\n"),
    );

    expect(outline(editor)).toEqual([
      "blockquote:A quote",
      "callout:Careful",
      "taskList:donetodo",
      "orderedList:third",
      "mathBlock:E = mc^2",
      "horizontalRule:",
      "details:MoreInside",
    ]);
    expect(editor.state.doc.child(1).attrs.icon).toBe("⚠️");
    expect(editor.state.doc.child(2).child(0).attrs.checked).toBe(true);
    expect(editor.state.doc.child(3).attrs.start).toBe(3);
    expect(editor.state.doc.child(6).attrs.open).toBe(true);
    editor.state.doc.check();
  });

  it("pastes the Markdown of a toggle heading (a heading inside the summary) as a toggle heading", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(
      editor,
      [
        "<details>",
        "<summary><h2>Folded</h2></summary>",
        "",
        "Inside",
        "",
        "</details>",
        "",
        "<details>",
        "<summary>Plain</summary>",
        "",
        "Inside",
        "",
        "</details>",
      ].join("\n"),
    );

    expect(outline(editor)).toEqual(["details:FoldedInside", "details:PlainInside"]);
    expect(editor.state.doc.child(0).child(0).attrs.level).toBe(2);
    expect(editor.state.doc.child(0).child(0).textContent).toBe("Folded");
    expect(editor.state.doc.child(1).child(0).attrs.level).toBeNull();
    expect(editor.view.dom.querySelector("summary[data-level='2']")).toHaveTextContent("Folded");
    editor.state.doc.check();
  });

  it("pastes an attachment link on a line of its own as an image block", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, `![A picture](attachment://${IMAGE})\n\ntext`);

    expect(editor.getJSON().content).toMatchObject([
      { type: "image", attrs: { attachmentId: IMAGE, alt: "A picture" } },
      { type: "paragraph" },
    ]);
  });

  it("caps heading levels at 3", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "#### Deep\n\n## Two");
    expect(outline(editor)).toEqual(["heading3:Deep", "heading2:Two"]);
  });

  it("pastes the page-v1 Markdown fixture, every kind of block in it valid", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, readFileSync("../../fixtures/doc/page-v1.md", "utf8"));

    editor.state.doc.check();
    const names = new Set(kinds(editor));
    for (const name of [
      "heading",
      "bulletList",
      "orderedList",
      "taskList",
      "blockquote",
      "codeBlock",
      "mathBlock",
      "horizontalRule",
      "details",
      "detailsSummary",
      "detailsContent",
      "callout",
      "table",
      "tableHeader",
      "tableCell",
      "image",
      "file",
      "hardBreak",
    ]) {
      expect(names.has(name), name).toBe(true);
    }
    expect(editor.view.dom.querySelector("u")).toHaveTextContent("underline");
    expect(editor.view.dom.querySelector("a[href='https://example.com/docs']")).toHaveTextContent("link");
  });
});

describe("where the blocks go", () => {
  it("puts them after the block the cursor is in, which stays whole", () => {
    const editor = mount({ type: "doc", content: [block("paragraph", "Hello world"), block("paragraph", "End")] });
    select(editor, 6); // between "Hello" and " world"
    paste(editor, "# T\n\n- a");

    expect(outline(editor)).toEqual(["paragraph:Hello world", "heading1:T", "bulletList:a", "paragraph:End"]);
  });

  it("replaces an empty paragraph between other blocks", () => {
    const editor = mount({
      type: "doc",
      content: [block("paragraph", "Before"), block("paragraph", ""), block("paragraph", "After")],
    });
    select(editor, "Before".length + 3);
    paste(editor, "## Mid\n\n> quote");

    expect(outline(editor)).toEqual(["paragraph:Before", "heading2:Mid", "blockquote:quote", "paragraph:After"]);
  });

  it("puts the cursor at the end of what was pasted", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "# Title\n\nlast **line**");
    editor.commands.insertContent("!");

    expect(outline(editor)).toEqual(["heading1:Title", "paragraph:last line!"]);
  });

  it("pastes a single paragraph as text at the cursor, marks included", () => {
    const editor = mount({ type: "doc", content: [block("paragraph", "Hello world")] });
    select(editor, 6);
    paste(editor, "**bold** and `code`");

    expect(outline(editor)).toEqual(["paragraph:Hellobold and code world"]);
    expect(editor.getJSON().content?.[0]?.content).toMatchObject([
      { text: "Hello" },
      { text: "bold", marks: [{ type: "bold" }] },
      { text: " and " },
      { text: "code", marks: [{ type: "code" }] },
      { text: " world" },
    ]);
  });

  it("replaces the selected text", () => {
    const editor = mount({ type: "doc", content: [block("paragraph", "Hello world")] });
    select(editor, 7, 12); // "world"
    paste(editor, "*there*");

    expect(outline(editor)).toEqual(["paragraph:Hello there"]);
    expect(editor.getJSON().content?.[0]?.content?.[1]).toMatchObject({ text: "there", marks: [{ type: "italic" }] });
  });

  it("replaces a selection that is a whole paragraph with blocks", () => {
    const editor = mount({ type: "doc", content: [block("paragraph", "Replace me"), block("paragraph", "Stay")] });
    select(editor, 1, 11);
    paste(editor, "# New\n\n- item");

    expect(outline(editor)).toEqual(["heading1:New", "bulletList:item", "paragraph:Stay"]);
  });

  it("keeps the document valid in an empty list item, a table cell, a toggle's summary and a column", () => {
    const where: { name: string; doc: JSONContent; find: (editor: Editor) => number }[] = [
      {
        name: "list item",
        doc: {
          type: "doc",
          content: [
            {
              type: "bulletList",
              attrs: { id: pid(1) },
              content: [{ type: "listItem", attrs: { id: pid(2) }, content: [block("paragraph", "")] }],
            },
          ],
        },
        find: () => 3,
      },
      {
        name: "table cell",
        doc: {
          type: "doc",
          content: [
            {
              type: "table",
              attrs: { id: pid(1) },
              content: [
                {
                  type: "tableRow",
                  attrs: { id: pid(2) },
                  content: [{ type: "tableCell", attrs: { id: pid(3) }, content: [block("paragraph", "cell")] }],
                },
              ],
            },
          ],
        },
        find: () => 5,
      },
      {
        name: "toggle summary",
        doc: {
          type: "doc",
          content: [
            {
              type: "details",
              attrs: { id: pid(1) },
              content: [block("detailsSummary", "Summary"), { type: "detailsContent", attrs: { id: pid(2) }, content: [block("paragraph", "")] }],
            },
          ],
        },
        find: () => 4,
      },
      {
        name: "column",
        doc: {
          type: "doc",
          content: [
            {
              type: "columnList",
              attrs: { id: pid(1) },
              content: [
                { type: "column", attrs: { id: pid(2) }, content: [block("paragraph", "left")] },
                { type: "column", attrs: { id: pid(3) }, content: [block("paragraph", "right")] },
              ],
            },
          ],
        },
        find: () => 5,
      },
    ];
    for (const { name, doc, find } of where) {
      const editor = mount(doc);
      select(editor, find(editor));
      paste(editor, "# Pasted\n\n- a\n- b\n\n| x |\n| - |\n| 1 |");
      expect(() => {
        editor.state.doc.check();
      }, name).not.toThrow();
      expect(editor.state.doc.textContent, name).toContain("Pasted");
      expect(kinds(editor), name).toContain("table");
    }
  });
});

/** A document of one list with an item for each text, between two paragraphs. */
function listPage(type: "bulletList" | "orderedList" | "taskList", ...items: string[]): JSONContent {
  const itemType = type === "taskList" ? "taskItem" : "listItem";
  return {
    type: "doc",
    content: [
      block("paragraph", "before"),
      {
        type,
        attrs: { id: pid(1) },
        content: items.map((text, index) => ({
          type: itemType,
          attrs: { id: pid(2 + index), ...(itemType === "taskItem" ? { checked: false } : {}) },
          content: [block("paragraph", text)],
        })),
      },
      block("paragraph", "after"),
    ],
  };
}

/** The position of the paragraph of the `index`-th list item, at its end. */
function endOfItem(editor: Editor, index: number): number {
  let found = 0;
  let seen = -1;
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === "listItem" || node.type.name === "taskItem") {
      seen += 1;
      if (seen === index) {
        found = pos + 1 + (node.firstChild?.nodeSize ?? 2) - 1;
      }
    }
  });
  return found;
}

describe("pasting into lists", () => {
  it("adds the items of a pasted list to the list the cursor is in, after the current item", () => {
    const editor = mount(listPage("bulletList", "one", "two"));
    select(editor, endOfItem(editor, 0));
    paste(editor, "- a\n- b");

    expect(editor.state.doc.toString()).toBe(
      'doc(paragraph("before"), bulletList(listItem(paragraph("one")), listItem(paragraph("a")), listItem(paragraph("b")), listItem(paragraph("two"))), paragraph("after"))',
    );
  });

  it("puts the items in place of an empty item", () => {
    const editor = mount(listPage("bulletList", "", "two"));
    select(editor, endOfItem(editor, 0));
    paste(editor, "- a\n- b");

    expect(editor.state.doc.toString()).toBe(
      'doc(paragraph("before"), bulletList(listItem(paragraph("a")), listItem(paragraph("b")), listItem(paragraph("two"))), paragraph("after"))',
    );
  });

  it("does the same for numbered lists and task lists", () => {
    const numbers = mount(listPage("orderedList", "one", ""));
    select(numbers, endOfItem(numbers, 1));
    paste(numbers, "1. a\n2. b");
    expect(numbers.state.doc.child(1).childCount).toBe(3);
    expect(numbers.state.doc.child(1).textContent).toBe("oneab");

    const tasks = mount(listPage("taskList", "one", ""));
    select(tasks, endOfItem(tasks, 1));
    paste(tasks, "- [x] done\n- [ ] todo");
    expect(tasks.state.doc.child(1).type.name).toBe("taskList");
    expect(tasks.state.doc.child(1).childCount).toBe(3);
    expect(tasks.state.doc.child(1).child(1).attrs.checked).toBe(true);
    expect(tasks.state.doc.toString()).not.toContain("bulletList");
  });

  it("nests a list of another kind under the item", () => {
    const editor = mount(listPage("orderedList", "one"));
    select(editor, endOfItem(editor, 0));
    paste(editor, "- a\n- b");

    expect(editor.state.doc.toString()).toBe(
      'doc(paragraph("before"), orderedList(listItem(paragraph("one"), bulletList(listItem(paragraph("a")), listItem(paragraph("b"))))), paragraph("after"))',
    );
  });

  it("replaces an empty item that is the whole list with the blocks, and splits a list around them", () => {
    const sole = mount(listPage("bulletList", ""));
    select(sole, endOfItem(sole, 0));
    paste(sole, "# T\n\nbody");
    expect(outline(sole)).toEqual(["paragraph:before", "heading1:T", "paragraph:body", "paragraph:after"]);

    const middle = mount(listPage("bulletList", "a", "", "c"));
    select(middle, endOfItem(middle, 1));
    paste(middle, "# T\n\nbody");
    expect(outline(middle)).toEqual([
      "paragraph:before",
      "bulletList:a",
      "heading1:T",
      "paragraph:body",
      "bulletList:c",
      "paragraph:after",
    ]);
  });

  it("puts blocks after a non-empty item inside it, valid, when they are not a list of its kind", () => {
    const editor = mount(listPage("bulletList", "one"));
    select(editor, endOfItem(editor, 0));
    paste(editor, "# T\n\nbody");

    expect(editor.state.doc.toString()).toBe(
      'doc(paragraph("before"), bulletList(listItem(paragraph("one"), heading("T"), paragraph("body"))), paragraph("after"))',
    );
  });

  it("puts blocks pasted in a toggle's summary after the whole toggle", () => {
    const editor = mount({
      type: "doc",
      content: [
        {
          type: "details",
          attrs: { id: pid(1) },
          content: [block("detailsSummary", "Sum"), { type: "detailsContent", attrs: { id: pid(2) }, content: [block("paragraph", "inside")] }],
        },
      ],
    });
    select(editor, 3);
    paste(editor, "# T\n\nbody");

    expect(editor.state.doc.toString()).toBe(
      'doc(details(detailsSummary("Sum"), detailsContent(paragraph("inside"))), heading("T"), paragraph("body"))',
    );
  });
});

describe("what is left to the default paste", () => {
  it("pastes plain text without Markdown as text, as before", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "just some words");
    expect(outline(editor)).toEqual(["paragraph:just some words"]);

    const lines = mount(emptyPage());
    select(lines, 1);
    paste(lines, "line one\nline two");
    expect(outline(lines)).toEqual(["paragraph:line one", "paragraph:line two"]);
  });

  it("does not take text that only looks like Markdown for it", () => {
    // (One star only: Tiptap's own paste rules, which the default paste still applies, would
    // make italics of two.)
    const text = "a * b and snake_case_name, a - b, #hashtag, costs $5 or $10, C:\\temp, and 1.5";
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, text);

    expect(outline(editor)).toEqual([`paragraph:${text}`]);
    expect(editor.getJSON().content?.[0]?.content).toEqual([{ type: "text", text }]);
  });

  it("does not take a link of the app for Markdown (that paste is the page reference handler's)", () => {
    const link = `atomik-kb://page/${pid(7)}#${pid(8)}`;
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, link);

    expect(outline(editor)).toEqual([`paragraph:${link}`]);
  });

  it("does not make a list or a heading of a lone marker", () => {
    for (const text of ["-", "#", "> ", "1."]) {
      const editor = mount(emptyPage());
      select(editor, 1);
      paste(editor, text);
      expect(outline(editor), text).toEqual([`paragraph:${text}`]);
    }
  });

  it("leaves a clipboard with HTML to the default handler", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "# Not a heading\n\n- not a list", "<p><strong>Rich</strong> text</p>");

    expect(outline(editor)).toEqual(["paragraph:Rich text"]);
    expect(editor.getJSON().content?.[0]?.content?.[0]).toMatchObject({ text: "Rich", marks: [{ type: "bold" }] });
  });

  it("treats HTML that only wraps the text (a terminal, a code editor) as no HTML", () => {
    const text = "# Heading\n- a\n- b";
    for (const html of [
      `<pre>${text}</pre>`,
      `<meta charset='utf-8'><div style="font-family: monospace; white-space: pre;"><div><span># Heading</span></div><div><span>- a</span></div><div><span>- b</span></div></div>`,
      `<p><span>${text.replaceAll("\n", "<br>")}</span></p>`,
    ]) {
      const editor = mount(emptyPage());
      select(editor, 1);
      paste(editor, text, html);
      expect(outline(editor), html).toEqual(["heading1:Heading", "bulletList:ab"]);
    }
  });

  it("pastes plain text inside a code block, whatever it holds", () => {
    const editor = mount({ type: "doc", content: [block("codeBlock", "x", { language: "js" })] });
    select(editor, 2);
    paste(editor, "# T\n\n- a\n**b**");

    expect(outline(editor)).toEqual(["codeBlock:x# T\n\n- a\n**b**"]);
  });

  it("pastes plain text inside a math block", () => {
    const editor = mount({ type: "doc", content: [block("mathBlock", "a")] });
    select(editor, 2);
    paste(editor, "# T\n- a");

    expect(outline(editor)).toEqual(["mathBlock:a# T\n- a"]);
  });

  it("pastes plain text inside inline code", () => {
    const editor = mount({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: pid(0) },
          content: [{ type: "text", text: "code", marks: [{ type: "code" }] }],
        },
      ],
    });
    select(editor, 3);
    paste(editor, "# T");

    expect(outline(editor)).toEqual(["paragraph:co# Tde"]);
  });

  it("pastes the Markdown itself when Shift is held (paste as plain text)", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    editor.view.dom.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Shift", keyCode: 16, shiftKey: true, bubbles: true, cancelable: true }),
    );
    paste(editor, "# Title\n\n- one");

    expect(outline(editor)).toEqual(["paragraph:# Title", "paragraph:- one"]);
  });
});

describe("what reaches the document", () => {
  it("writes one update to the Loro document for a paste, and one undo takes it back", async () => {
    const doc = loroDocFrom(paragraphs("", "After"));
    const editor = mount(emptyPage(), { doc });
    await tick();
    select(editor, 1);
    const before = doc.oplogVersion();

    paste(editor, "# Title\n\n- one\n- two\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n```js\nlet x;\n```");

    expect(outline(editor)).toEqual(["heading1:Title", "bulletList:onetwo", "table:ab12", "codeBlock:let x;", "paragraph:After"]);
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);

    undo(editor.state, editor.view.dispatch);
    expect(outline(editor)).toEqual(["paragraph:", "paragraph:After"]);
  });

  it("never lets a node or a mark the parser does not write into the schema", () => {
    const schema = createEditorSchema();
    const nodes = nodesFromPortable(schema, [
      { type: "columnList", content: [{ type: "column", content: [{ type: "paragraph" }] }] },
      { type: "script", content: [{ type: "text", text: "alert(1)" }] },
      { type: "pageRef", attrs: { pageId: pid(5) } },
      {
        type: "paragraph",
        content: [
          { type: "text", text: "a", marks: [{ type: "highlight" }, { type: "bold" }] },
          { type: "pageRef", attrs: { pageId: pid(5) } },
          { type: "text", text: "b" },
        ],
      },
    ]);

    expect(nodes.map((node) => node.type.name)).toEqual(["paragraph"]);
    expect(nodes[0]?.textContent).toBe("ab");
    expect(nodes[0]?.child(0).marks.map((mark) => mark.type.name)).toEqual(["bold"]);
  });

  it("keeps a link only for a scheme the app opens", () => {
    const schema = createEditorSchema();
    const hrefs = ["https://example.com/a", "http://x.org", "mailto:me@example.com", "relative/page", "#anchor", `attachment://${IMAGE}`];
    const dropped = ["javascript:alert(1)", " JavaScript:alert(1)", "data:text/html;base64,AAAA", "file:///etc/passwd", "vbscript:x", ""];
    const link = (href: string): JsonValue => ({ type: "text", text: "t", marks: [{ type: "link", attrs: { href } }] });
    const nodes = nodesFromPortable(schema, [{ type: "paragraph", content: [...hrefs, ...dropped].map(link) }]);

    const marks: unknown[][] =
      nodes[0]?.content.content.map((text) => text.marks.map((mark): unknown => mark.attrs.href as unknown)) ?? [];
    expect(marks.slice(0, hrefs.length)).toEqual(hrefs.map((href) => [href]));
    // Adjacent runs without a link are one text node: the dropped links are all that is left.
    expect(marks.slice(hrefs.length)).toEqual([[]]);
  });

  it("repairs what the schema asks for: an item starts with a paragraph, a toggle holds a block", () => {
    const schema = createEditorSchema();
    const [list, details] = nodesFromPortable(schema, [
      {
        type: "bulletList",
        content: [{ type: "listItem", content: [{ type: "heading", attrs: { level: 2 }, content: [{ type: "text", text: "H" }] }] }],
      },
      { type: "details", content: [{ type: "detailsSummary" }, { type: "detailsContent" }] },
    ]);

    expect(list?.child(0).child(0).type.name).toBe("paragraph");
    expect(list?.child(0).child(1).type.name).toBe("heading");
    expect(details?.child(1).childCount).toBe(1);
    list?.check();
    details?.check();
  });
});

describe("in the page editor of the app", () => {
  it("pastes Markdown as blocks, and the core ends up holding them", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);

    editor.commands.setTextSelection(1);
    editor.view.dom.dispatchEvent(pasteEvent("", "# Title\n\n- one\n- two\n\n> 💡 Tip"));

    expect(outline(editor)).toEqual(["heading1:Title", "bulletList:onetwo", "callout:Tip"]);
    await waitFor(async () => {
      const content = (await contentOf(memory, page("Notes"))) as { content?: { type: string }[] };
      expect(content.content?.map((node) => node.type)).toEqual(["heading", "bulletList", "callout"]);
    });
  });
});


describe("pasting an inline equation ($x$, 2026-10-07)", () => {
  /** The inline content of the first block as `text` or `$latex$`, in order. */
  function inlineOf(editor: Editor): string[] {
    const found: string[] = [];
    editor.state.doc.firstChild?.forEach((node) => {
      found.push(node.type.name === "mathInline" ? `$${String(node.attrs.latex)}$` : (node.text ?? ""));
    });
    return found;
  }

  it("makes equations of the dollars in a line of text, each with an id of its own", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "Let $a^2$ and $b$ be real");

    expect(inlineOf(editor)).toEqual(["Let ", "$a^2$", " and ", "$b$", " be real"]);
    const ids: unknown[] = [];
    editor.state.doc.descendants((node) => {
      if (node.type.name === "mathInline") {
        ids.push(node.attrs.id);
      }
    });
    expect(ids).toHaveLength(2);
    for (const id of ids) {
      expect(id).toMatch(UUID);
    }
    expect(ids[0]).not.toBe(ids[1]);
    editor.state.doc.check();
  });

  it("makes an equation of a lone $x$, which is no longer plain text", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "$E=mc^2$");
    expect(inlineOf(editor)).toEqual(["$E=mc^2$"]);
    expect(editor.getText()).toBe("$E=mc^2$");
  });

  it("goes in at the cursor, in the middle of a sentence, and replaces a selection", () => {
    const editor = mount({ type: "doc", content: [block("paragraph", "Hello world")] });
    select(editor, 6);
    paste(editor, "$x$");
    expect(inlineOf(editor)).toEqual(["Hello", "$x$", " world"]);

    const selected = mount({ type: "doc", content: [block("paragraph", "Hello world")] });
    select(selected, 7, 12);
    paste(selected, "$y$");
    expect(inlineOf(selected)).toEqual(["Hello ", "$y$"]);
  });

  it("keeps the marks of the text around it, and reads it inside a list item and a heading", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, "# Proof of $a$\n\n- **bold $b$ text**");
    expect(outline(editor)).toEqual(["heading1:Proof of ", "bulletList:bold  text"]);
    const kindsOf = kinds(editor);
    expect(kindsOf.filter((name) => name === "mathInline")).toHaveLength(2);
    const item: string[] = [];
    editor.state.doc.child(1).child(0).child(0).forEach((node) => {
      item.push(node.type.name);
    });
    expect(item).toEqual(["text", "mathInline", "text"]);
  });

  it("is left alone in code, in a math block and in inline code, and in text that only has prices", () => {
    const code = mount({ type: "doc", content: [block("codeBlock", "x", { language: "js" })] });
    select(code, 2);
    paste(code, "a $y$ b");
    expect(outline(code)).toEqual(["codeBlock:xa $y$ b"]);

    const math = mount({ type: "doc", content: [block("mathBlock", "a")] });
    select(math, 2);
    paste(math, "$y$ and **b**");
    expect(outline(math)).toEqual(["mathBlock:a$y$ and **b**"]);

    const prices = mount(emptyPage());
    select(prices, 1);
    paste(prices, "it costs $5 or $10, not $x");
    expect(kinds(prices)).not.toContain("mathInline");
    expect(outline(prices)).toEqual(["paragraph:it costs $5 or $10, not $x"]);
  });

  it("does not make one of an equation with no source, and builds only what the schema takes", () => {
    const schema = createEditorSchema();
    const nodes = nodesFromPortable(schema, [
      {
        type: "paragraph",
        content: [
          { type: "mathInline", attrs: { latex: "" } },
          { type: "mathInline", attrs: { latex: 5 } },
          { type: "mathInline" },
          { type: "mathInline", attrs: { latex: "x" } },
        ],
      },
    ]);
    expect(nodes).toHaveLength(1);
    expect(nodes[0]?.childCount).toBe(1);
    expect(nodes[0]?.child(0).type.name).toBe("mathInline");
    expect(nodes[0]?.child(0).attrs.latex).toBe("x");
    nodes[0]?.check();
  });

  it("writes one update to the Loro document, and one undo takes it back", async () => {
    const doc = loroDocFrom(paragraphs(""));
    const editor = mount(emptyPage(), { doc });
    await tick();
    select(editor, 1);
    const before = doc.oplogVersion();
    paste(editor, "$x$ and $y$");
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    expect(JSON.stringify(loroToPortable(doc))).toContain('"latex":"x"');

    undo(editor.state, editor.view.dispatch);
    expect(kinds(editor)).not.toContain("mathInline");
  });
});

describe("pasting a <time> element of an exported file (ADR-0036, ADR-0056)", () => {
  /** The attributes of the date mentions of the document, in order. */
  function mentionsOf(editor: Editor): Record<string, unknown>[] {
    const found: Record<string, unknown>[] = [];
    editor.state.doc.descendants((node) => {
      if (node.type.name === "dateMention") {
        found.push({ ...node.attrs });
      }
      return true;
    });
    return found;
  }

  it("makes a date mention with an id of its own, its reminder and what it is filled in with, and keeps the text around it", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, 'Due <time datetime="2026-10-19T14:30" data-reminder="15" data-relative="now">2026-10-19 14:30</time> sharp');

    expect(editor.state.doc.firstChild?.textBetween(0, editor.state.doc.firstChild.content.size, "", (leaf) => `<${leaf.type.name}>`)).toBe(
      "Due <dateMention> sharp",
    );
    const [mention] = mentionsOf(editor);
    expect(mention).toMatchObject({ date: "2026-10-19", time: "14:30", reminder: 15, relative: "now" });
    expect(mention?.id).toMatch(UUID);
    editor.state.doc.check();
  });

  it("leaves an element that holds no date as the text it is", () => {
    const editor = mount(emptyPage());
    select(editor, 1);
    paste(editor, 'See <time datetime="2026-02-30">x</time> and **bold**');
    expect(mentionsOf(editor)).toEqual([]);
    expect(editor.getText()).toContain('<time datetime="2026-02-30">x</time>');
  });
});
