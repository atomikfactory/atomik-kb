// The page editor inside the app, over the in-memory core (`MemoryTransport`, which keeps
// page documents with loro-crdt like the real core).

import { MemoryTransport, normalizePortable } from "@atomik-kb/core-client";
import type { DocTransport, JsonValue, PageId, Transport } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroList, LoroMap } from "loro-crdt";
import type { LoroText } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { PAGE_FIXTURE, paragraphs, pid, withoutHardBreakIds } from "../test/editor";
import { contentOf, findEditor, idsOf, openPage, pasteEvent, renderPages } from "../test/page";
import { renderWith } from "../test/render";
import { row } from "../test/sidebar";
import { canUndo } from "./loro-binding";
import { FLUSH_INTERVAL_MS } from "./page-session";

afterEach(() => {
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** Ids of the top-level blocks, in order. */
function topIds(editor: Editor): unknown[] {
  const ids: unknown[] = [];
  editor.state.doc.forEach((node) => {
    ids.push(node.attrs.id);
  });
  return ids;
}

/** Puts the cursor `offset` characters into the `index`-th top-level block. */
function cursorIn(editor: Editor, index: number, offset: number): void {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos + offset)));
}

/** The first text of the first paragraph of a core-side page document. */
function firstText(doc: Parameters<Parameters<MemoryTransport["changePage"]>[1]>[0]): LoroText {
  const paragraph = (doc.getMap("doc").get("children") as LoroList).get(0) as LoroMap;
  return (paragraph.get("children") as LoroList).get(0) as LoroText;
}

describe("opening a page", () => {
  it("shows every block kind of the fixture page", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Fixture", content: PAGE_FIXTURE }] }]);
    const editor = await openPage(user, "Fixture");
    const dom = editor.view.dom;

    expect(dom.querySelector("h1")).toHaveTextContent("Welcome to atomik-kb");
    expect(dom.querySelector("h1 strong")).toHaveTextContent("atomik-kb");
    expect(dom.querySelector("em")).toHaveTextContent("italic");
    expect(dom.querySelector("u")).toHaveTextContent("underline");
    expect(dom.querySelector("s")).toHaveTextContent("strike");
    expect(dom.querySelector("p code")).toHaveTextContent("code");
    expect(dom.querySelector("a[href='https://example.com/docs']")).toHaveTextContent("link");
    expect(dom.querySelector("p br")).not.toBeNull();
    // The references point at pages this workspace does not have.
    await waitFor(() => {
      expect(dom.querySelectorAll("[data-page-ref]")).toHaveLength(2);
      expect(within(dom.querySelector("[data-page-ref]") as HTMLElement).getByText(t("pageRef.deleted"))).toBeTruthy();
    });
    expect(dom.querySelector("ul:not([data-type]) > li ul > li")).toHaveTextContent("Nested item");
    expect(dom.querySelector("ol")).toHaveAttribute("start", "3");
    const tasks = dom.querySelectorAll("ul[data-type='taskList'] > li");
    expect(tasks).toHaveLength(2);
    expect(tasks[0]).toHaveAttribute("data-checked", "true");
    expect(tasks[0]?.querySelector("input[type='checkbox']")).toBeChecked();
    expect(tasks[1]?.querySelector("input[type='checkbox']")).not.toBeChecked();
    expect(dom.querySelector("blockquote")).toHaveTextContent("A quoted line with");
    // The inline equation of the quote's paragraph is a chip in the line, drawn from its latex.
    expect(dom.querySelector("blockquote p [data-type='mathInline'] .math-inline")).not.toBeNull();
    expect(dom.querySelector("blockquote [data-type='mathInline']")).toHaveAttribute("data-latex", "E=mc^2");
    expect(dom.querySelector("pre code.language-rust")).toHaveTextContent('fn main() { println!("hi"); }');
    expect(dom.querySelector("hr")).not.toBeNull();
    expect(dom.querySelector("[data-type='details'] summary")).toHaveTextContent("Toggle summary");
    // The sample's toggle is a toggle heading of level 2 (ADR-0040): the summary has the type of a heading.
    expect(dom.querySelector("[data-type='details'] summary")).toHaveAttribute("data-level", "2");
    expect(dom.querySelector("[data-type='details'] summary")).toHaveClass("toggle-heading");
    expect(dom.querySelector("[data-type='details'] [data-type='detailsContent']")).toHaveTextContent(
      "Hidden until opened.",
    );
    await waitFor(() => {
      expect(dom.querySelector("[data-type='callout'] .callout-icon")).toHaveTextContent("💡");
    });
    expect(dom.querySelector("[data-type='callout'] .callout-content")).toHaveTextContent("Callouts carry an icon.");
    // The table: a header row of three th, two body rows of td, one cell with two paragraphs.
    expect(dom.querySelectorAll("table > tbody > tr")).toHaveLength(3);
    expect(dom.querySelectorAll("table th")).toHaveLength(3);
    expect(dom.querySelectorAll("table td")).toHaveLength(6);
    expect(dom.querySelector("table th")).toHaveTextContent("Feature");
    expect(dom.querySelector("table td strong")).toHaveTextContent("Done");
    expect(dom.querySelectorAll("table td")[2]?.querySelectorAll("p")).toHaveLength(2);
    expect(dom.querySelectorAll("table td")[4]).toHaveTextContent("a | b");
    expect(dom).toHaveTextContent("Unicode: café, 日本語, 👩‍💻 and 𝔘𝔫𝔦𝔠𝔬𝔡𝔢.");
    // Blocks carry their ids into the DOM (search results and backlinks scroll to them).
    expect(dom.querySelector("[data-id='1a9e4c3f-8d2b-4f1c-8b6e-2c3d4e5f6071']")?.tagName).toBe("P");
  });

  it("holds exactly the fixture's content (round trip through the editor)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Fixture", content: PAGE_FIXTURE }] }]);
    const editor = await openPage(user, "Fixture");
    // The fixture's hardBreak carries an id that block schema v1 does not give it.
    expect(normalizePortable(editor.getJSON() as JsonValue)).toEqual(normalizePortable(withoutHardBreakIds(PAGE_FIXTURE)));
  });

  it("mounts only once the core's snapshot is in, and never on default content", async () => {
    const memory = new MemoryTransport({
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Slow", content: paragraphs("From the core") }] }],
    });
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: { ...memory.docs, open: async (pageId) => gate.then(() => memory.docs.open(pageId)) },
    };
    const { user } = renderWith(transport);
    await user.click(await screen.findByRole("treeitem", { name: "Slow" }));
    expect(await screen.findByText(t("editor.loading"))).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: t("editor.label") })).not.toBeInTheDocument();
    release();
    const editor = await findEditor();
    expect(editor.getText()).toBe("From the core");
  });
});

describe("saving", () => {
  it("sends typing to the core within the flush interval", async () => {
    const memory = new MemoryTransport({
      workspaceName: "Research",
      spaces: [{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Hello") }] }],
    });
    const imports = vi.fn((pageId: PageId, update: Uint8Array) => memory.docs.import(pageId, update));
    const docs: DocTransport = { ...memory.docs, import: imports };
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs,
    };
    const { user } = renderWith(transport);
    await user.click(await screen.findByRole("treeitem", { name: "Notes" }));
    const editor = await findEditor();
    const { page } = await idsOf(memory);

    cursorIn(editor, 0, 5);
    editor.commands.insertContent(" world");
    const typedAt = Date.now();
    await waitFor(() => {
      expect(imports).toHaveBeenCalled();
    });
    expect(Date.now() - typedAt).toBeLessThan(FLUSH_INTERVAL_MS + 200);
    expect(await contentOf(memory, page("Notes"))).toEqual(paragraphs("Hello world"));
  });
});

describe("changes from elsewhere", () => {
  it("shows them, and undo only takes back the user's own edit", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Hello") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);

    cursorIn(editor, 0, 5);
    editor.commands.insertContent("!");
    expect(canUndo(editor.state)).toBe(true);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toEqual(paragraphs("Hello!"));
    });

    act(() => {
      memory.changePage(page("Notes"), (doc) => {
        firstText(doc).insert(0, "Oh, ");
      });
    });
    await waitFor(() => {
      expect(editor.getText()).toBe("Oh, Hello!");
    });

    fireEvent.keyDown(editor.view.dom, { key: "z", ctrlKey: true });
    expect(editor.getText()).toBe("Oh, Hello");
    expect(canUndo(editor.state)).toBe(false); // the remote change is not on the stack
    fireEvent.keyDown(editor.view.dom, { key: "z", ctrlKey: true, shiftKey: true });
    expect(editor.getText()).toBe("Oh, Hello!");
    fireEvent.keyDown(editor.view.dom, { key: "z", ctrlKey: true });
    fireEvent.keyDown(editor.view.dom, { key: "y", ctrlKey: true });
    expect(editor.getText()).toBe("Oh, Hello!");
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toEqual(paragraphs("Oh, Hello!"));
    });
  });
});

describe("saving failures", () => {
  it("says that changes are not saved after three failed attempts, and stops once they are", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Hello") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    memory.failNextDocs({ code: "Storage", message: "disk full" }, 3);

    for (const character of ["a", "b", "c"]) {
      cursorIn(editor, 0, editor.state.doc.child(0).content.size);
      editor.commands.insertContent(character);
      await new Promise((resolve) => setTimeout(resolve, FLUSH_INTERVAL_MS + 20));
    }
    expect(await screen.findByText(t("editor.unsaved"))).toBeInTheDocument();

    cursorIn(editor, 0, editor.state.doc.child(0).content.size);
    editor.commands.insertContent("!");
    await waitFor(() => {
      expect(screen.queryByText(t("editor.unsaved"))).not.toBeInTheDocument();
    });
    expect(await contentOf(memory, page("Notes"))).toEqual(paragraphs("Helloabc!"));
  });
});

describe("markdown shortcuts", () => {
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

  it("turns '# ', '- ', '1. ', '> ', '[ ] ', three backticks and '---' into blocks", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    const cases: [string, string][] = [
      ["# ", "heading"],
      ["- ", "bulletList"],
      ["1. ", "orderedList"],
      ["> ", "blockquote"],
      ["[ ] ", "taskList"],
      ["``` ", "codeBlock"],
      ["---", "horizontalRule"],
    ];
    for (const [input, kind] of cases) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      cursorIn(editor, 0, 0);
      typeText(editor, input);
      expect(editor.state.doc.child(0).type.name, input).toBe(kind);
    }
  });
});

describe("ids from elsewhere", () => {
  it("never rewrites the ids of a remote change, even duplicated ones", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(page("Notes"), (doc) => {
        // Another writer (a merge, say) left a second block with the same id.
        const children = doc.getMap("doc").get("children") as LoroList;
        const twin = children.insertContainer(1, new LoroMap());
        twin.set("nodeName", "paragraph");
        twin.setContainer("attributes", new LoroMap()).set("id", pid(0));
        twin.setContainer("children", new LoroList());
      });
    });
    await waitFor(() => {
      expect(editor.state.doc.childCount).toBe(2);
    });
    expect(topIds(editor)).toEqual([pid(0), pid(0)]);
    await new Promise((resolve) => setTimeout(resolve, FLUSH_INTERVAL_MS + 50));
    expect(await contentOf(memory, page("Notes"))).toEqual({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "One" }] },
        { type: "paragraph", attrs: { id: pid(0) } },
      ],
    });
  });
});

describe("block ids", () => {
  it("keeps the id on the first half of a split paragraph and gives the second a new one", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Hello world") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);

    cursorIn(editor, 0, 5);
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(editor.getText({ blockSeparator: "|" })).toBe("Hello| world");
    const [first, second] = topIds(editor);
    expect(first).toBe(pid(0));
    expect(second).toMatch(UUID);

    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toEqual({
        type: "doc",
        content: [
          { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Hello" }] },
          { type: "paragraph", attrs: { id: second }, content: [{ type: "text", text: " world" }] },
        ],
      });
    });
  });

  it("gives pasted copies of blocks new ids", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "End") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { view } = editor;
    // Copy the first two paragraphs, paste them at the end of the last one.
    const twoBlocks = view.state.doc.slice(0, view.state.doc.child(0).nodeSize + view.state.doc.child(1).nodeSize);
    const { dom, text } = view.serializeForClipboard(twoBlocks);
    editor.commands.setTextSelection(view.state.doc.content.size - 1);
    view.dom.dispatchEvent(pasteEvent(dom.innerHTML, text));

    const ids = topIds(editor);
    expect(editor.getText({ blockSeparator: "|" })).toBe("One|Two|End|One|Two");
    expect(ids.slice(0, 3)).toEqual([pid(0), pid(1), pid(2)]);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids.slice(3)) {
      expect(id).toMatch(UUID);
    }
  });
});

describe("pages from a newer version", () => {
  it("opens read-only with a notice when the page has a block this editor does not know", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "p0" }, content: [{ type: "text", text: "Kept as it is" }] },
        { type: "kanban", attrs: { id: "k0", columns: 3 } },
      ],
    };
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content }] }]);
    await user.click(row("Board"));
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    const box = await screen.findByRole("textbox", { name: t("editor.label") });
    expect(box).toHaveAttribute("contenteditable", "false");
    const { page } = await idsOf(memory);
    expect(await contentOf(memory, page("Board"))).toEqual(content);
  });
});

describe("links", () => {
  const linked: JsonValue = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p0" },
        content: [
          { type: "text", text: "see " },
          { type: "text", marks: [{ type: "link", attrs: { href: "https://example.com/a" } }], text: "docs" },
          { type: "text", text: " or " },
          { type: "text", marks: [{ type: "link", attrs: { href: "file:///etc/passwd" } }], text: "this" },
        ],
      },
    ],
  };

  it("opens http, https and mailto links through the shell and ignores other schemes", async () => {
    const openExternal = vi.fn(() => Promise.resolve());
    const opened = vi.spyOn(window, "open").mockImplementation(() => null);
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Links", content: linked }] }], {
      openExternal,
    });
    const editor = await openPage(user, "Links");
    fireEvent.click(editor.view.dom.querySelector("a[href='https://example.com/a']") as Element);
    await waitFor(() => {
      expect(openExternal).toHaveBeenCalledWith("https://example.com/a");
    });
    const other = editor.view.dom.querySelector("a[href^='file']");
    if (other !== null) {
      fireEvent.click(other);
    }
    expect(openExternal).toHaveBeenCalledTimes(1);
    expect(opened).not.toHaveBeenCalled();
  });

  it("falls back to a new tab without an opener where the shell cannot open links", async () => {
    const opened = vi.spyOn(window, "open").mockImplementation(() => null);
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Links", content: linked }] }]);
    const editor = await openPage(user, "Links");
    fireEvent.click(editor.view.dom.querySelector("a[href='https://example.com/a']") as Element);
    await waitFor(() => {
      expect(opened).toHaveBeenCalledWith("https://example.com/a", "_blank", "noopener,noreferrer");
    });
  });
});

describe("the editor without a document", () => {
  it("reports a page whose document cannot be opened and opens it on retry", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Back again") }] },
    ]);
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    memory.failNextDocs({ code: "Storage", message: "disk I/O error" });
    await user.click(row("Notes"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("editor.openError", { message: "disk I/O error" }));
    await user.click(within(alert).getByRole("button", { name: t("editor.retry") }));
    expect((await findEditor()).getText()).toBe("Back again");
  });
});
