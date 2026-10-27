// Links between pages made from the text of a page (ADR-0016, amendment of 2026-10-06): the
// card that links to a page (`pageLink`), the "Link to page" and "New sub-page" commands of the
// `/` menu, and links to blocks (`atomik-kb://page/<pageId>#<blockId>`: copied from the block
// menu, pasted as a chip or a card, followed by a click).

import { CoreClient, internalError } from "@atomik-kb/core-client";
import type { JsonValue, PageId } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { LoroList, LoroMap } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "../i18n";
import { paragraphs, pid } from "../test/editor";
import { contentOf, findEditor, idsOf, openPage, pasteEvent, renderPages } from "../test/page";
import { outline as treeOutline } from "../test/sidebar";
import { blockLinkText, parseBlockLink } from "./block-link";
import { createSchemaExtensions } from "./schema";
import { SubPages, subPageUnavailable } from "./sub-page";

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "clipboard");
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const MISSING_PAGE = "00000000-0000-4000-8000-000000000000";

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

/** Top-level blocks as `type:text`, for compact assertions. */
function outline(editor: Editor): string[] {
  const lines: string[] = [];
  editor.state.doc.forEach((node) => {
    lines.push(`${node.type.name}:${node.textContent}`);
  });
  return lines;
}

/** The page link cards of the open page, in order, as `{ pageId, blockId }`. */
function cards(editor: Editor): { pageId: unknown; blockId: unknown }[] {
  const found: { pageId: unknown; blockId: unknown }[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageLink") {
      found.push({ pageId: node.attrs.pageId, blockId: node.attrs.blockId });
    }
    return true;
  });
  return found;
}

/** Appends a page link card to a page document, as another writer would. */
function appendCard(doc: LoroDoc, pageId: PageId, blockId: string, toBlock?: string): void {
  const children = doc.getMap("doc").get("children") as LoroList;
  const card = children.insertContainer(children.length, new LoroMap());
  card.set("nodeName", "pageLink");
  const attributes = card.setContainer("attributes", new LoroMap());
  attributes.set("id", blockId);
  attributes.set("pageId", pageId);
  if (toBlock !== undefined) {
    attributes.set("blockId", toBlock);
  }
  card.setContainer("children", new LoroList());
}

/** The editor of the page `title`, once a click navigated there (the heading changes first). */
async function findOpened(title: string): Promise<Editor> {
  await screen.findByRole("heading", { level: 1, name: new RegExp(title) });
  return findEditor();
}

/** The card element of the open page's editor. */
async function findCard(editor: Editor): Promise<HTMLElement> {
  return waitFor(() => {
    const element = editor.view.dom.querySelector<HTMLElement>("[data-type='pageLink']");
    expect(element).not.toBeNull();
    return element as HTMLElement;
  });
}

describe("the link text of the app", () => {
  const page = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
  const block = "8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7";

  it("is atomik-kb://page/<pageId>#<blockId>, or without the block for the page itself", () => {
    expect(blockLinkText(page, block)).toBe(`atomik-kb://page/${page}#${block}`);
    expect(blockLinkText(page)).toBe(`atomik-kb://page/${page}`);
    expect(blockLinkText(page, null)).toBe(`atomik-kb://page/${page}`);
  });

  it("is read back from exactly one link, trimmed", () => {
    expect(parseBlockLink(`atomik-kb://page/${page}#${block}`)).toEqual({ pageId: page, blockId: block });
    expect(parseBlockLink(`  atomik-kb://page/${page}\n`)).toEqual({ pageId: page, blockId: null });
    expect(parseBlockLink(`ATOMIK-KB://PAGE/${page.toUpperCase()}#${block.toUpperCase()}`)).toEqual({
      pageId: page,
      blockId: block,
    });
    for (const other of [
      "",
      "hello",
      `see atomik-kb://page/${page}`,
      `atomik-kb://page/${page} and more`,
      `atomik-kb://page/${page}#${block}#${block}`,
      `atomik-kb://page/${page}\natomik-kb://page/${page}`,
      `atomik-kb://page/${page}#not-a-block`,
      "atomik-kb://page/not-a-page",
      `https://example.com/page/${page}`,
    ]) {
      expect(parseBlockLink(other), other).toBeNull();
    }
  });
});

describe("the page link card", () => {
  it("shows the page's icon and live title, is stored as a pageLink block, and a click opens the page", async () => {
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [
          { title: "Notes", content: paragraphs("Intro") },
          { title: "Target", icon: "🎯", content: paragraphs("First", "Second") },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);

    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, { type: "pageLink", attrs: { pageId: page("Target") } });
    });
    const card = await findCard(editor);
    await waitFor(() => {
      expect(card).toHaveTextContent("🎯Target");
    });
    expect(card).toHaveAttribute("data-page-id", page("Target"));
    expect(card).not.toHaveAttribute("data-block-id");
    expect(within(card).getByRole("link")).toBeInTheDocument();

    // A block of its own, with an id; `blockId` is not stored while it is null.
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          { type: "paragraph" },
          { type: "pageLink", attrs: { id: expect.stringMatching(UUID) as unknown, pageId: page("Target") } },
        ],
      });
    });
    const stored = (await contentOf(memory, page("Notes"))) as { content: { attrs: Record<string, unknown> }[] };
    expect(stored.content[1]?.attrs).not.toHaveProperty("blockId");

    // The card follows the page's title.
    await act(async () => {
      await memory.dispatch({ type: "PageRename", data: { pageId: page("Target"), title: "Renamed" } });
    });
    await waitFor(() => {
      expect(card).toHaveTextContent("🎯Renamed");
    });

    // A click opens the page.
    await user.click(within(card).getByRole("link"));
    expect(await screen.findByRole("heading", { level: 1, name: /Renamed/ })).toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: "Renamed" })).toHaveAttribute("aria-selected", "true");
  });

  it("opens the page at the linked block when the card names one", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [
          { title: "Notes", content: paragraphs("Intro") },
          { title: "Target", content: paragraphs("First", "Second", "Third") },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, {
        type: "pageLink",
        attrs: { pageId: page("Target"), blockId: pid(1) },
      });
    });
    const card = await findCard(editor);
    expect(card).toHaveAttribute("data-block-id", pid(1));
    await waitFor(() => {
      expect(card).toHaveTextContent("Target");
    });

    await user.click(within(card).getByRole("link"));
    const target = await findOpened("Target");
    await waitFor(() => {
      expect(scrolled).toHaveBeenCalled();
    });
    expect(target.view.dom.querySelector(`[data-id='${pid(1)}']`)).toHaveClass("is-target");
    expect(target.view.dom.querySelector(`[data-id='${pid(0)}']`)).not.toHaveClass("is-target");
  });

  it("shows a link to a page that cannot be found as an unknown page and goes nowhere", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Before" }] },
        { type: "pageLink", attrs: { id: pid(1), pageId: MISSING_PAGE } },
      ],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(user, "Notes");
    const card = await findCard(editor);
    await waitFor(() => {
      expect(card).toHaveTextContent(t("pageLink.unknown"));
    });
    expect(card.querySelector(".page-link")).toHaveClass("is-unknown");
    expect(within(card).getByRole("link")).toHaveAttribute("aria-disabled", "true");
    await user.click(within(card).getByRole("link"));
    expect(screen.getByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
  });

  it("is selected like any atom and Backspace removes it", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro") }, { title: "Target" }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, { type: "pageLink", attrs: { pageId: page("Target") } });
    });
    const card = await findCard(editor);
    expect(outline(editor)).toEqual(["paragraph:Intro", "pageLink:"]);

    act(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, startOf(editor, 1) - 1)));
    });
    await waitFor(() => {
      expect(card).toHaveClass("ProseMirror-selectednode");
    });
    fireEvent.keyDown(editor.view.dom, { key: "Backspace" });

    expect(cards(editor)).toEqual([]);
    expect(outline(editor)).toEqual(["paragraph:Intro"]);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Notes")))).not.toContain("pageLink");
    });
  });

  it("opens the page from a read-only editor too", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro") }, { title: "Target", icon: "🎯" }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size, { type: "pageLink", attrs: { pageId: page("Target") } });
    });
    const card = await findCard(editor);
    await waitFor(() => {
      expect(card).toHaveTextContent("Target");
    });
    act(() => {
      editor.setEditable(false);
    });
    expect(editor.isEditable).toBe(false);
    await user.click(within(card).getByRole("link"));
    expect(await screen.findByRole("heading", { level: 1, name: /Target/ })).toBeInTheDocument();
  });

  it("is a backlink: the page it links to lists the page that holds the card", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Target", content: paragraphs("Linked to") }, { title: "Index" }] },
      { name: "Work", pages: [{ title: "Source", icon: "🧭", content: paragraphs("Intro") }] },
    ]);
    const { page } = await idsOf(memory);
    act(() => {
      memory.changePage(
        page("Source"),
        (doc) => {
          appendCard(doc, page("Target"), "11111111-1111-4111-8111-111111111111");
        },
        "Core",
      );
    });
    await openPage(user, "Target");

    const panel = await screen.findByRole("region", { name: tp("backlinks.title", 1) });
    expect(within(panel).getByRole("button", { name: "Source" })).toBeInTheDocument();
    // The card reads as the title of its page, like a chip does.
    expect(within(panel).getByRole("button", { name: t("backlinks.block", { title: "Source", text: "Target" }) })).toBeInTheDocument();

    // And it is searchable by that title.
    const hits = await new CoreClient(memory).search("Target");
    expect(hits.some((hit) => hit.title === "Source" && hit.blockId === "11111111-1111-4111-8111-111111111111")).toBe(true);
  });
});

describe("Link to page in the / menu", () => {
  it("opens the page picker and inserts a card in place of the empty paragraph", async () => {
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [{ title: "Notes", content: paragraphs("Intro", "") }, { title: "Target page", icon: "🎯" }],
      },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/link");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual([
        "Link to page", // its label starts with `link`: first, and what Enter chooses
        "Image from URL", // its keyword `link`
      ]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // The page picker, the one `[[` opens.
    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    expect(editor.getText({ blockSeparator: "|" })).toBe("Intro|[[");
    expect(editor.storage.pagePicker.asBlock).toBe(true);
    await waitFor(() => {
      expect(within(picker).getAllByRole("option").map((option) => option.textContent)).toEqual(["Notes", "🎯Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // A card, not a chip: the empty paragraph became the card, with a paragraph below it to type in.
    await waitFor(() => {
      expect(outline(editor)).toEqual(["paragraph:Intro", "pageLink:", "paragraph:"]);
    });
    expect(cards(editor)).toEqual([{ pageId: page("Target page"), blockId: null }]);
    expect(editor.view.dom.querySelector("[data-page-ref]")).toBeNull();
    expect(editor.state.selection.$from.parent.type.name).toBe("paragraph");
    expect(editor.state.selection.$from.index(0)).toBe(2);
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-type='pageLink']")).toHaveTextContent("🎯Target page");
    });
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          { type: "paragraph" },
          { type: "pageLink", attrs: { pageId: page("Target page"), id: expect.stringMatching(UUID) as unknown } },
          { type: "paragraph" },
        ],
      });
    });
    expect(screen.queryByRole("listbox", { name: t("picker.label") })).not.toBeInTheDocument();
  });

  it("puts the card after a paragraph that holds text, and leaves the typed text alone", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Read this", "Next") }, { title: "Target" }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    placeCursor(editor, startOf(editor, 0) + "Read this".length);
    editor.commands.insertContent(" /link");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(within(picker).getAllByRole("option").map((option) => option.textContent)).toEqual(["Notes", "Target"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    await waitFor(() => {
      expect(cards(editor)).toEqual([{ pageId: page("Target"), blockId: null }]);
    });
    expect(outline(editor)).toEqual(["paragraph:Read this ", "pageLink:", "paragraph:Next"]);
  });

  it("leaves the next [[ a chip, also after a card picker was closed without a choice", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }, { title: "Target" }] },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/link");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await screen.findByRole("listbox", { name: t("picker.label") });
    // Closed without choosing.
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("picker.label") })).not.toBeInTheDocument();
    });
    expect(editor.storage.pagePicker.asBlock).toBe(false);

    // What is left of the trigger is text: clear it and type `[[` by hand.
    editor.commands.deleteRange({ from: 1, to: 1 + editor.state.doc.child(0).content.size });
    placeCursor(editor, 1);
    editor.commands.insertContent("[[");
    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(within(picker).getAllByRole("option").length).toBeGreaterThan(0);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).not.toBeNull();
    });
    expect(cards(editor)).toEqual([]);
  });
});

describe("New sub-page in the / menu", () => {
  it("creates a child page, puts a card to it in place of the empty paragraph and opens it with its title ready", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/page");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      // The sub-page comes first (its keyword `page`), the mention next (the same keyword, in the
      // section after it), the list view after them (its longer keyword `pages`), then the card
      // (`page` in its label); the entries whose hints mention a page follow.
      expect(
        within(menu)
          .getAllByRole("option")
          .map((option) => option.querySelector(".suggestion-title")?.textContent)
          .slice(0, 4),
      ).toEqual(["New sub-page", "Mention a page", "List view", "Link to page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // The app opened the new page, with its title ready to be typed.
    const title = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(title).toHaveFocus();
    const { page } = await idsOf(memory);
    const child = page("");
    expect(child).not.toBe("");

    // It is a child of the page in the tree.
    const tree = await new CoreClient(memory).treeGet((await new CoreClient(memory).spaceList())[0]?.id ?? "");
    expect(tree.roots.map((node) => node.page.title)).toEqual(["Notes"]);
    expect(tree.roots[0]?.children.map((node) => node.page.id)).toEqual([child]);
    expect(tree.roots[0]?.children[0]?.page.parentId).toBe(page("Notes"));
    await waitFor(() => {
      expect(treeOutline()).toEqual(["Notes", `  ${t("page.untitled")}`]);
    });
    expect(screen.getByRole("treeitem", { name: t("page.untitled") })).toHaveAttribute("aria-selected", "true");

    // The card is in the parent's document: it replaced the empty paragraph.
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          { type: "paragraph", attrs: { id: pid(0) } },
          { type: "pageLink", attrs: { pageId: child, id: expect.stringMatching(UUID) as unknown } },
          { type: "paragraph" },
        ],
      });
    });

    // Typing the title renames the page; the card in the parent follows it.
    await user.keyboard("Plans{Enter}");
    expect(await screen.findByRole("heading", { level: 1, name: "Plans" })).toBeInTheDocument();
    await waitFor(() => {
      expect(treeOutline()).toEqual(["Notes", "  Plans"]);
    });
    await user.click(screen.getByRole("treeitem", { name: "Notes" }));
    const parent = await screen.findByRole("textbox", { name: t("editor.label") });
    await waitFor(() => {
      expect(within(parent).getByRole("link", { name: /Plans/ })).toBeInTheDocument();
    });
  });

  it("puts the card after the block when the command was typed in text", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Ideas", "Next") }] },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 0) + "Ideas".length);
    editor.commands.insertContent(" /page");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          { type: "paragraph", content: [{ type: "text", text: "Ideas " }] },
          { type: "pageLink", attrs: { pageId: page("") } },
          { type: "paragraph", content: [{ type: "text", text: "Next" }] },
        ],
      });
    });
  });

  it("creates nothing and stays on the page when the page cannot be created", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro", "") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const dispatch = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PageCreate" ? Promise.reject(internalError("the disk is full")) : dispatch(request),
    );
    placeCursor(editor, startOf(editor, 1));
    editor.commands.insertContent("/page");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("subpage.error", { message: "the disk is full" }));
    expect(screen.getByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
    expect(cards(editor)).toEqual([]);
    expect(outline(editor)).toEqual(["paragraph:Intro", "paragraph:"]); // only the typed `/page` is gone
    expect(treeOutline()).toEqual(["Notes"]);
  });

  it("is unavailable in a read-only editor and in one with no page behind it", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro") }] }]);
    const editor = await openPage(user, "Notes");
    expect(subPageUnavailable(editor)).toBeNull();
    act(() => {
      editor.setEditable(false);
    });
    expect(subPageUnavailable(editor)).toBe("slash.subpage.readOnly");

    // A schema-only editor has no service to create pages with.
    const bare = new Editor({ extensions: [...createSchemaExtensions(), SubPages] });
    expect(subPageUnavailable(bare)).toBe("slash.subpage.noPage");
    bare.destroy();
  });
});

describe("Copy link to block", () => {
  async function blockMenu(editor: Editor, id: string): Promise<HTMLElement> {
    const block = editor.view.dom.querySelector(`[data-id='${id}']`);
    if (block === null) {
      throw new Error(`no block ${id}`);
    }
    fireEvent.mouseMove(block);
    fireEvent.click(await screen.findByRole("button", { name: t("block.actions") }));
    return screen.findByRole("menu", { name: t("block.actions") });
  }

  it("writes atomik-kb://page/<pageId>#<blockId> to the clipboard and says so", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");

    await user.click(within(await blockMenu(editor, pid(1))).getByRole("menuitem", { name: t("block.copyLink") }));

    const link = `atomik-kb://page/${page("Notes")}#${pid(1)}`;
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith(link);
    });
    expect(await navigator.clipboard.readText()).toBe(link);
    expect(await screen.findByText(t("block.linkCopied"))).toHaveAttribute("role", "status");
    // Nothing else changed.
    expect(outline(editor)).toEqual(["paragraph:One", "paragraph:Two"]);
  });

  it("shows the link to copy by hand where the clipboard cannot be written", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "Two") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: undefined });

    await user.click(within(await blockMenu(editor, pid(0))).getByRole("menuitem", { name: t("block.copyLink") }));

    const link = `atomik-kb://page/${page("Notes")}#${pid(0)}`;
    expect(await screen.findByRole("status")).toHaveTextContent(t("block.linkNoClipboard", { link }));
  });

  it("round-trips: the copied link pasted elsewhere is a chip that opens the block", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [
          { title: "Target", content: paragraphs("First", "Second") },
          { title: "Notes", content: paragraphs("See ") },
        ],
      },
    ]);
    const { page } = await idsOf(memory);
    const target = await openPage(user, "Target");
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(within(await blockMenu(target, pid(1))).getByRole("menuitem", { name: t("block.copyLink") }));
    await waitFor(() => {
      expect(writeText).toHaveBeenCalled();
    });
    const link = String(writeText.mock.calls[0]?.[0]);
    expect(link).toBe(`atomik-kb://page/${page("Target")}#${pid(1)}`);

    const notes = await openPage(user, "Notes");
    placeCursor(notes, startOf(notes, 0) + "See ".length);
    act(() => {
      notes.view.dom.dispatchEvent(pasteEvent("", link));
    });
    const chip = await waitFor(() => {
      const element = notes.view.dom.querySelector<HTMLElement>("[data-page-ref]");
      expect(element).not.toBeNull();
      return element as HTMLElement;
    });
    expect(chip).toHaveAttribute("data-page-id", page("Target"));
    expect(chip).toHaveAttribute("data-block-id", pid(1));
    await waitFor(() => {
      expect(chip).toHaveTextContent("Target");
    });

    // A click on the chip opens the page and brings the block into view.
    await user.click(within(chip).getByRole("link"));
    const opened = await findOpened("Target");
    await waitFor(() => {
      expect(scrolled).toHaveBeenCalled();
    });
    expect(opened.view.dom.querySelector(`[data-id='${pid(1)}']`)).toHaveClass("is-target");
  });
});

describe("pasting a link to a block", () => {
  const targetPage = "7d5e2f10-3b4c-4d5e-8f60-718293a4b5c6";
  const targetBlock = "8e6f3021-4c5d-4e6f-9071-8293a4b5c6d7";
  const link = `atomik-kb://page/${targetPage}#${targetBlock}`;

  it("makes a chip with both ids inside a paragraph", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("See and more") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    placeCursor(editor, startOf(editor, 0) + "See".length);
    act(() => {
      editor.view.dom.dispatchEvent(pasteEvent("", `  ${link}\n`));
    });

    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).not.toBeNull();
    });
    expect(editor.getText()).toBe(`See[[${targetPage}]] and more`);
    expect(editor.getJSON().content?.[0]?.content?.[1]).toMatchObject({
      type: "pageRef",
      attrs: { pageId: targetPage, blockId: targetBlock },
    });
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "See" },
              {
                type: "pageRef",
                attrs: { id: expect.stringMatching(UUID) as unknown, pageId: targetPage, blockId: targetBlock },
              },
              { type: "text", text: " and more" },
            ],
          },
        ],
      });
    });
  });

  it("makes a chip to the page alone when the link has no block", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("See ") }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, startOf(editor, 0) + "See ".length);
    act(() => {
      editor.view.dom.dispatchEvent(pasteEvent("", `atomik-kb://page/${targetPage}`));
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).not.toBeNull();
    });
    expect(editor.getJSON().content?.[0]?.content?.[1]).toMatchObject({
      type: "pageRef",
      attrs: { pageId: targetPage, blockId: null },
    });
  });

  it("makes a card on an empty paragraph", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("One", "") }] },
    ]);
    const editor = await openPage(user, "Notes");
    const { page } = await idsOf(memory);
    placeCursor(editor, startOf(editor, 1));
    act(() => {
      editor.view.dom.dispatchEvent(pasteEvent("", link));
    });

    await waitFor(() => {
      expect(cards(editor)).toEqual([{ pageId: targetPage, blockId: targetBlock }]);
    });
    expect(outline(editor)).toEqual(["paragraph:One", "pageLink:", "paragraph:"]);
    expect(editor.view.dom.querySelector("[data-page-ref]")).toBeNull();
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          { type: "paragraph" },
          {
            type: "pageLink",
            attrs: { id: expect.stringMatching(UUID) as unknown, pageId: targetPage, blockId: targetBlock },
          },
          { type: "paragraph" },
        ],
      });
    });
  });

  it("makes a chip, not a card, in an empty list item (a block cannot take its place)", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        {
          type: "bulletList",
          attrs: { id: pid(0) },
          content: [{ type: "listItem", attrs: { id: pid(1) }, content: [{ type: "paragraph", attrs: { id: pid(2) } }] }],
        },
      ],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 3);
    expect(editor.state.selection.$from.parent.type.name).toBe("paragraph");
    act(() => {
      editor.view.dom.dispatchEvent(pasteEvent("", link));
    });
    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).not.toBeNull();
    });
    expect(cards(editor)).toEqual([]);
  });

  it("leaves everything else alone: ordinary text, a link among other text, two links", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
    const editor = await openPage(user, "Notes");

    for (const text of ["just some words", `see ${link} for details`, `${link}\n${link}`, `${link} and ${link}`]) {
      placeCursor(editor, startOf(editor, 0));
      editor.commands.deleteRange({ from: 1, to: 1 + editor.state.doc.child(0).content.size });
      placeCursor(editor, startOf(editor, 0));
      act(() => {
        editor.view.dom.dispatchEvent(pasteEvent("", text));
      });
      expect(editor.view.dom.querySelector("[data-page-ref]"), text).toBeNull();
      expect(cards(editor), text).toEqual([]);
      expect(editor.state.doc.child(0).textContent, text).toContain(text.split("\n")[0] ?? "");
    }
  });

  it("leaves the link as text in a code block, where a chip cannot go", async () => {
    const content: JsonValue = { type: "doc", content: [{ type: "codeBlock", attrs: { id: pid(0) } }] };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    act(() => {
      editor.view.dom.dispatchEvent(pasteEvent("", link));
    });
    expect(editor.state.doc.child(0).textContent).toBe(link);
    expect(editor.view.dom.querySelector("[data-page-ref]")).toBeNull();
    expect(cards(editor)).toEqual([]);
  });
});
