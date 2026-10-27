// Block UX in the page editor: the `/` menu, the `[[` page picker, the block handle, the
// bubble menu, and the controls inside blocks (toggles, to-dos, callout icons).

import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { FakeDataTransfer } from "../test/dnd";
import { paragraphs, pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";

afterEach(() => {
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

/** Position just inside the `index`-th top-level block. */
function startOf(editor: Editor, index: number): number {
  let pos = 1;
  for (let i = 0; i < index; i += 1) {
    pos += editor.state.doc.child(i).nodeSize;
  }
  return pos;
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** Top-level blocks as `type:text` (with the heading level), for compact assertions. */
function outline(editor: Editor): string[] {
  const lines: string[] = [];
  editor.state.doc.forEach((node) => {
    const level = node.type.name === "heading" ? String(node.attrs.level) : "";
    lines.push(`${node.type.name}${level}:${node.textContent}`);
  });
  return lines;
}

function ids(editor: Editor): unknown[] {
  const result: unknown[] = [];
  editor.state.doc.forEach((node) => {
    result.push(node.attrs.id);
  });
  return result;
}

describe("the / menu", () => {
  it("lists the block types and turns the block into the chosen one", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const labels = within(menu)
      .getAllByRole("option")
      .map((option) => option.querySelector(".suggestion-title")?.textContent);
    expect(labels).toEqual([
      "Text",
      "Heading 1",
      "Heading 2",
      "Heading 3",
      "Bulleted list",
      "Numbered list",
      "To-do list",
      "Toggle",
      "Toggle heading 1",
      "Toggle heading 2",
      "Toggle heading 3",
      "Quote",
      "Callout",
      "Divider",
      "Image",
      "Image from URL",
      "Photo gallery",
      "PDF",
      "Video",
      "Audio",
      "File",
      "Code",
      "Diagram",
      "Whiteboard",
      "Equation",
      "Table",
      "2 columns",
      "3 columns",
      "Template button",
      "Table of contents",
      "Breadcrumb",
      "List view",
      "Table view",
      "Board view",
      "Calendar view",
      "Gallery view",
      "Dashboard view",
      "Sort list A–Z",
      "Link to page",
      "New sub-page",
      "New page from template",
      "Sort subpages A–Z",
      "Sort subpages by date created",
      "Date",
      "Mention a page",
      "Emoji",
      "Inline equation",
    ]);
    await user.click(within(menu).getByText("Heading 1"));

    expect(outline(editor)).toEqual(["paragraph:Intro", "heading1:"]);
    expect(ids(editor)).toEqual([pid(0), pid(1)]); // the block keeps its id
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
  });

  it("filters as you type and chooses with Enter", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/to");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(
        within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent),
      ).toEqual([
        "Toggle",
        "To-do list", // as many letters match, but the label is longer
        "Toggle heading 1", // longer still
        "Toggle heading 2",
        "Toggle heading 3",
        "Table of contents", // its keyword `toc`
        "Date", // its keyword `today`
        "Link to page", // `to` inside its label
        "Photo gallery", // likewise, its label being longer
        "Template button", // likewise (bu-tt-on), longer still
        "Quote", // then the entries whose keywords or hints hold `to`, in the order of the list
        "Divider",
        "Image",
        "Image from URL", // the keyword `photo`
        "File",
        "Breadcrumb",
      ]);
    });
    expect(within(menu).getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(within(menu).getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" });
    expect(within(menu).getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    expect(editor.state.doc.child(0).type.name).toBe("details");
    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Notes")))).toContain('"details"');
    });
  });

  it("inserts each kind of block", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");
    const kinds: [string, (dom: HTMLElement) => Element | null][] = [
      ["Bulleted list", (dom) => dom.querySelector("ul:not([data-type]) > li")],
      ["Numbered list", (dom) => dom.querySelector("ol > li")],
      ["To-do list", (dom) => dom.querySelector("ul[data-type='taskList'] input[type='checkbox']")],
      ["Quote", (dom) => dom.querySelector("blockquote")],
      ["Callout", (dom) => dom.querySelector("[data-type='callout']")],
      ["Code", (dom) => dom.querySelector("pre code")],
      ["Diagram", (dom) => dom.querySelector("pre code.language-mermaid")],
      ["Equation", (dom) => dom.querySelector("div.math-block[data-type='mathBlock']")],
      ["Divider", (dom) => dom.querySelector("hr")],
      ["2 columns", (dom) => (dom.querySelectorAll("[data-type='columnList'] > [data-type='column']").length === 2 ? dom : null)],
      ["3 columns", (dom) => (dom.querySelectorAll("[data-type='columnList'] > [data-type='column']").length === 3 ? dom : null)],
      ["Template button", (dom) => dom.querySelector("[data-type='templateButton'] .template-button-trigger")],
      ["List view", (dom) => dom.querySelector("[data-type='pageList']")],
      ["Calendar view", (dom) => dom.querySelector(".page-list[data-layout='calendar']")],
      ["Gallery view", (dom) => dom.querySelector(".page-list[data-layout='gallery']")],
    ];
    for (const [label, find] of kinds) {
      editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
      placeCursor(editor, 1);
      editor.commands.insertContent("/");
      const menu = await screen.findByRole("listbox", { name: t("slash.label") });
      await user.click(within(menu).getByText(label));
      expect(find(editor.view.dom), label).not.toBeNull();
    }
  });

  it("/diagram makes a Mermaid code block with a starter flowchart, the caret inside, and shows its preview", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/mer");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(
        within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent),
      ).toEqual(["Diagram"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    const block = editor.state.doc.child(1);
    expect(block.type.name).toBe("codeBlock");
    expect(block.attrs).toEqual({ id: pid(1), language: "mermaid" }); // the block keeps its id
    expect(block.textContent).toBe("flowchart TD\n  A[Start] --> B[End]");
    expect(editor.state.selection.$from.parent).toBe(block);
    expect(editor.state.selection.empty).toBe(true);
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".code-block-preview svg")).not.toBeNull();
    });
  });
});

describe("the / menu and equations", () => {
  it("/math turns the empty block into an equation, the caret inside, and typing draws it", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/equ");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(
        within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent),
      ).toEqual(["Equation", "Inline equation", "Diagram"]); // the block first; `equ` is a keyword of the inline one and in `sequence`
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    const block = editor.state.doc.child(1);
    expect(block.type.name).toBe("mathBlock");
    expect(block.attrs).toEqual({ id: pid(1) }); // the block keeps its id
    expect(block.content.size).toBe(0);
    expect(editor.state.selection.$from.parent).toBe(block);
    expect(editor.view.dom.querySelector(".math-block-source")).not.toHaveClass("is-collapsed");

    editor.commands.insertContent("x^2");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".math-block-render")).toHaveAttribute("data-state", "ready");
    });
    expect(editor.state.doc.child(1).textContent).toBe("x^2");
  });
});

describe("the [[ page picker", () => {
  it("offers pages by title and inserts a chip that shows the live title", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("See ") }, { title: "Target page" }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    placeCursor(editor, 5);
    editor.commands.insertContent("[[targ");

    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(within(picker).getByRole("option")).toHaveTextContent("Target page");
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).toHaveTextContent("Target page");
    });
    expect(editor.getText()).toBe("See [[Target page]] ");
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "See " },
              { type: "pageRef", attrs: { pageId: page("Target page"), id: expect.stringMatching(UUID) as unknown } },
              { type: "text", text: " " },
            ],
          },
        ],
      });
    });

    // The chip follows the page's title.
    await act(async () => {
      await memory.dispatch({ type: "PageRename", data: { pageId: page("Target page"), title: "Renamed" } });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).toHaveTextContent("Renamed");
    });

    // A click on the chip opens the page.
    await user.click(within(editor.view.dom.querySelector("[data-page-ref]") as HTMLElement).getByRole("link"));
    expect(await screen.findByRole("heading", { level: 1, name: "Renamed" })).toBeInTheDocument();
  });

  it("offers the pages of the space before anything is typed", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }, { title: "Alpha" }, { title: "Beta" }] },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("[[");
    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(within(picker).getAllByRole("option").map((option) => option.textContent)).toEqual([
        "Notes", // the page glyph (an SVG) in front: no text
        "Alpha",
        "Beta",
      ]);
    });
  });

  it("shows a reference to a deleted page as such, and one to a page in another space by its title", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "p0" },
          content: [{ type: "pageRef", attrs: { id: "r0", pageId: "00000000-0000-4000-8000-000000000000" } }],
        },
      ],
    };
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content }] },
      { name: "Work", pages: [{ title: "Elsewhere", icon: "🧭" }] },
    ]);
    const editor = await openPage(user, "Notes");
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).toHaveTextContent(t("pageRef.deleted"));
    });

    const { page } = await idsOf(memory);
    act(() => {
      editor.commands.insertContentAt(1, { type: "pageRef", attrs: { pageId: page("Elsewhere") } });
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).toHaveTextContent("🧭Elsewhere");
    });
    // Opening it switches to its space.
    await user.click(within(editor.view.dom.querySelector("[data-page-ref]") as HTMLElement).getByRole("link"));
    expect(await screen.findByRole("heading", { level: 1, name: "Elsewhere" })).toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: "Elsewhere" })).toHaveAttribute("aria-selected", "true");
  });
});

describe("the block handle", () => {
  async function blockMenu(editor: Editor, id: string): Promise<HTMLElement> {
    const block = editor.view.dom.querySelector(`[data-id='${id}']`);
    if (block === null) {
      throw new Error(`no block ${id}`);
    }
    fireEvent.mouseMove(block);
    fireEvent.click(await screen.findByRole("button", { name: t("block.actions") }));
    return screen.findByRole("menu", { name: t("block.actions") });
  }

  it("duplicates, moves and deletes blocks", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two", "Three") }] },
    ]);
    const editor = await openPage(user, "Notes");

    await user.click(within(await blockMenu(editor, pid(1))).getByRole("menuitem", { name: t("block.duplicate") }));
    expect(outline(editor)).toEqual(["paragraph:One", "paragraph:Two", "paragraph:Two", "paragraph:Three"]);
    const copy = ids(editor)[2];
    expect(copy).toMatch(UUID);
    expect(copy).not.toBe(pid(1));

    await user.click(within(await blockMenu(editor, pid(2))).getByRole("menuitem", { name: t("block.moveUp") }));
    expect(outline(editor)).toEqual(["paragraph:One", "paragraph:Two", "paragraph:Three", "paragraph:Two"]);
    expect(ids(editor)).toEqual([pid(0), pid(1), pid(2), copy]); // moved, same id

    await user.click(within(await blockMenu(editor, pid(0))).getByRole("menuitem", { name: t("block.moveDown") }));
    expect(ids(editor)).toEqual([pid(1), pid(0), pid(2), copy]);

    const menu = await blockMenu(editor, pid(1));
    expect(within(menu).getByRole("menuitem", { name: t("block.moveUp") })).toBeDisabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("block.delete") }));
    expect(ids(editor)).toEqual([pid(0), pid(2), copy]);
  });

  it("sorts a list block A to Z from its menu, as one step, and offers it for lists only", async () => {
    const list = (...texts: string[]): JsonValue => ({
      type: "bulletList",
      attrs: { id: pid(10) },
      content: texts.map((value, index) => ({
        type: "listItem",
        attrs: { id: pid(11 + index * 2) },
        content: [{ type: "paragraph", attrs: { id: pid(12 + index * 2) }, content: [{ type: "text", text: value }] }],
      })),
    });
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Intro" }] },
        list("Cherry", "apple", "Banana"),
      ],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(user, "Notes");
    const listTexts = (): string[] => {
      const texts: string[] = [];
      editor.state.doc.child(1).forEach((item) => {
        texts.push(item.textContent);
      });
      return texts;
    };

    // A paragraph has no such command.
    expect(within(await blockMenu(editor, pid(0))).queryByRole("menuitem", { name: t("block.sortList") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");

    const menu = await blockMenu(editor, pid(10));
    expect(within(menu).getByRole("menuitem", { name: t("block.sortList") })).toBeEnabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("block.sortList") }));
    expect(listTexts()).toEqual(["apple", "Banana", "Cherry"]);
    // The items kept their ids (the list block and the intro too).
    const itemIds: unknown[] = [];
    editor.state.doc.child(1).forEach((item) => {
      itemIds.push(item.attrs.id);
    });
    expect(itemIds).toEqual([pid(13), pid(15), pid(11)]);
    expect(editor.state.doc.child(1).attrs.id).toBe(pid(10));
    expect(outline(editor)).toEqual(["paragraph:Intro", "bulletList:appleBananaCherry"]);

    // In order already: the command is there, and disabled.
    expect(within(await blockMenu(editor, pid(10))).getByRole("menuitem", { name: t("block.sortList") })).toBeDisabled();
  });

  it("turns a block into another kind, keeping the ids of what stays", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two") }] }]);
    const editor = await openPage(user, "Notes");

    await user.click(within(await blockMenu(editor, pid(0))).getByRole("menuitem", { name: t("block.turnInto") }));
    const kinds = await screen.findByRole("menu", { name: t("block.turnInto") });
    expect(within(kinds).getByRole("menuitem", { name: "Text" })).toBeDisabled();
    await user.click(within(kinds).getByRole("menuitem", { name: "Heading 2" }));
    expect(outline(editor)).toEqual(["heading2:One", "paragraph:Two"]);
    expect(ids(editor)).toEqual([pid(0), pid(1)]);

    await user.click(within(await blockMenu(editor, pid(1))).getByRole("menuitem", { name: t("block.turnInto") }));
    await user.click(within(await screen.findByRole("menu", { name: t("block.turnInto") })).getByRole("menuitem", { name: "Bulleted list" }));
    const list = editor.state.doc.child(1);
    expect(list.type.name).toBe("bulletList");
    expect(list.attrs.id).toMatch(UUID);
    expect(list.child(0).child(0).attrs.id).toBe(pid(1));

    const listId = String(list.attrs.id);
    await user.click(within(await blockMenu(editor, listId)).getByRole("menuitem", { name: t("block.turnInto") }));
    await user.click(within(await screen.findByRole("menu", { name: t("block.turnInto") })).getByRole("menuitem", { name: "To-do list" }));
    const tasks = editor.state.doc.child(1);
    expect(tasks.type.name).toBe("taskList");
    expect(tasks.attrs.id).toBe(listId); // the same list, other markers
    expect(tasks.child(0).child(0).attrs.id).toBe(pid(1));
  });

  it("turns a block into an equation and an equation back into text, keeping the id and the text", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("x^2", "Two") }] }]);
    const editor = await openPage(user, "Notes");

    await user.click(within(await blockMenu(editor, pid(0))).getByRole("menuitem", { name: t("block.turnInto") }));
    await user.click(within(await screen.findByRole("menu", { name: t("block.turnInto") })).getByRole("menuitem", { name: "Equation" }));
    expect(outline(editor)).toEqual(["mathBlock:x^2", "paragraph:Two"]);
    expect(ids(editor)).toEqual([pid(0), pid(1)]);
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".math-block-render")).toHaveAttribute("data-state", "ready");
    });

    await user.click(within(await blockMenu(editor, pid(0))).getByRole("menuitem", { name: t("block.turnInto") }));
    const kinds = await screen.findByRole("menu", { name: t("block.turnInto") });
    expect(within(kinds).getByRole("menuitem", { name: "Equation" })).toBeDisabled(); // it is one
    await user.click(within(kinds).getByRole("menuitem", { name: "Text" }));
    expect(outline(editor)).toEqual(["paragraph:x^2", "paragraph:Two"]);
    expect(ids(editor)).toEqual([pid(0), pid(1)]);
  });

  it("starts ProseMirror's own drag of the block from its grip (a move that keeps ids)", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two") }] }]);
    const editor = await openPage(user, "Notes");
    fireEvent.mouseMove(editor.view.dom.querySelector(`[data-id='${pid(1)}']`) as Element);
    const grip = await screen.findByRole("button", { name: t("block.actions") });
    const data = new FakeDataTransfer();
    fireEvent.dragStart(grip, { dataTransfer: data });

    const { selection } = editor.state;
    expect(selection).toBeInstanceOf(NodeSelection);
    expect((selection as NodeSelection).node.attrs.id).toBe(pid(1));
    expect(editor.view.dragging?.move).toBe(true);
    expect(editor.view.dragging?.slice.content.firstChild?.attrs.id).toBe(pid(1));
    expect(data.effectAllowed).toBe("move");
    expect(data.getData("text/html")).toContain("Two");
  });

  it("adds a block below and opens the / menu there", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("One") }] }]);
    const editor = await openPage(user, "Notes");
    fireEvent.mouseMove(editor.view.dom.querySelector(`[data-id='${pid(0)}']`) as Element);
    await user.click(await screen.findByRole("button", { name: t("block.add") }));
    expect(outline(editor)).toEqual(["paragraph:One", "paragraph:/"]);
    expect(await screen.findByRole("listbox", { name: t("slash.label") })).toBeInTheDocument();
  });
});

describe("the bubble menu", () => {
  it("formats selected text and links it", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Hello world") }] },
    ]);
    const editor = await openPage(user, "Notes");
    editor.view.dom.tabIndex = 0; // jsdom only focuses a contenteditable that has one
    act(() => {
      editor.chain().focus().setTextSelection({ from: 7, to: 12 }).run();
    });

    const toolbar = await screen.findByRole("toolbar", { name: t("format.label") });
    const bold = within(toolbar).getByRole("button", { name: t("format.bold") });
    expect(bold).toHaveAttribute("aria-pressed", "false");
    await user.click(bold);
    expect(editor.isActive("bold")).toBe(true);
    await waitFor(() => {
      expect(within(toolbar).getByRole("button", { name: t("format.bold") })).toHaveAttribute("aria-pressed", "true");
    });

    await user.click(within(toolbar).getByRole("button", { name: t("format.link") }));
    const address = within(await screen.findByRole("toolbar", { name: t("format.label") })).getByRole("textbox", {
      name: t("format.linkLabel"),
    });
    await user.type(address, "javascript:alert(1){Enter}");
    expect(await screen.findByText(t("format.linkInvalid"))).toBeInTheDocument();
    await user.clear(address);
    await user.type(address, "example.com/docs{Enter}");
    expect(editor.getAttributes("link")).toMatchObject({ href: "https://example.com/docs" });

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toEqual({
        type: "doc",
        content: [
          {
            type: "paragraph",
            attrs: { id: pid(0) },
            content: [
              { type: "text", text: "Hello " },
              {
                type: "text",
                marks: [{ type: "bold" }, { type: "link", attrs: { href: "https://example.com/docs" } }],
                text: "world",
              },
            ],
          },
        ],
      });
    });
  });
});

describe("the bubble menu in an equation", () => {
  it("does not show on selected LaTeX (a math block holds no marks), and does on the text around it", async () => {
    const { user } = await renderPages([
      {
        name: "Personal",
        pages: [
          {
            title: "Notes",
            content: {
              type: "doc",
              content: [
                { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Hello world" }] },
                { type: "mathBlock", attrs: { id: pid(1) }, content: [{ type: "text", text: "x^2 + y" }] },
              ],
            },
          },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    editor.view.dom.tabIndex = 0; // jsdom only focuses a contenteditable that has one
    const mathStart = editor.state.doc.child(0).nodeSize + 1;
    act(() => {
      editor.chain().focus().setTextSelection({ from: 7, to: 12 }).run();
    });
    expect(await screen.findByRole("toolbar", { name: t("format.label") })).toBeInTheDocument();

    act(() => {
      editor.chain().focus().setTextSelection({ from: mathStart, to: mathStart + 3 }).run();
    });
    await waitFor(() => {
      expect(screen.queryByRole("toolbar", { name: t("format.label") })).not.toBeInTheDocument();
    });
  });
});

describe("controls inside blocks", () => {
  const blocks: JsonValue = {
    type: "doc",
    content: [
      {
        type: "details",
        attrs: { id: "d0", open: false },
        content: [
          { type: "detailsSummary", attrs: { id: "s0" }, content: [{ type: "text", text: "More" }] },
          {
            type: "detailsContent",
            attrs: { id: "c0" },
            content: [{ type: "paragraph", attrs: { id: "p0" }, content: [{ type: "text", text: "Inside" }] }],
          },
        ],
      },
      {
        type: "taskList",
        attrs: { id: "l0" },
        content: [
          {
            type: "taskItem",
            attrs: { id: "t0", checked: false },
            content: [{ type: "paragraph", attrs: { id: "p1" }, content: [{ type: "text", text: "Do it" }] }],
          },
        ],
      },
      {
        type: "callout",
        attrs: { id: "co", icon: "💡" },
        content: [{ type: "paragraph", attrs: { id: "p2" }, content: [{ type: "text", text: "Note" }] }],
      },
    ],
  };

  it("opens a toggle, checks a to-do and changes a callout's icon without touching block ids", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Blocks", content: blocks }] }]);
    const editor = await openPage(user, "Blocks");
    const { page } = await idsOf(memory);

    fireEvent.click(editor.view.dom.querySelector("[data-type='details'] > button") as Element);
    expect(editor.state.doc.child(0).attrs).toEqual({ id: "d0", open: true });

    fireEvent.click(editor.view.dom.querySelector("input[type='checkbox']") as Element);
    await waitFor(() => {
      expect(editor.state.doc.child(1).child(0).attrs).toEqual({ id: "t0", checked: true });
    });

    await user.click(await screen.findByRole("button", { name: t("callout.icon") }));
    await user.click(within(await screen.findByRole("dialog", { name: t("icon.title") })).getByRole("button", { name: "📌" }));
    expect(editor.state.doc.child(2).attrs).toEqual({ id: "co", icon: "📌", color: null });

    await waitFor(async () => {
      const saved = JSON.stringify(await contentOf(memory, page("Blocks")));
      expect(saved).toContain('"open":true');
      expect(saved).toContain('"checked":true');
      expect(saved).toContain('"icon":"📌"');
    });
  });
});
