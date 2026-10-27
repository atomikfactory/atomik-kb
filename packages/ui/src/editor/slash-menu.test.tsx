// The `/` menu as a product: entries in sections with glyphs, ranked matching, the "No results"
// row, the keyboard (wrap, page jumps, Tab, Escape), the pointer, entries that are shown but
// cannot run, and the `[[` picker that shares the popup and must not change.

import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import type { SuggestionProps } from "@tiptap/suggestion";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { paragraphs, pid } from "../test/editor";
import { openPage, renderPages } from "../test/page";
import { outline } from "../test/sidebar";
import { redo, undo } from "./loro-binding";
import { filterSlashCommands, SLASH_COMMANDS, SLASH_SECTION_LABELS, SLASH_SECTIONS } from "./slash-commands";
import { SuggestionMenu, SuggestionMenuStore } from "./suggestion-menu";

afterEach(() => {
  vi.restoreAllMocks();
});

/** The labels of the entries `/<query>` finds, best first. */
function found(query: string): string[] {
  return filterSlashCommands(query).map((command) => t(command.label));
}

/** The labels of the entries of the open menu, in order. */
function labelsOf(menu: HTMLElement): (string | null | undefined)[] {
  return within(menu)
    .getAllByRole("option")
    .map((option) => option.querySelector(".suggestion-title")?.textContent);
}

/** The text of the highlighted option. */
function highlighted(menu: HTMLElement): string | null | undefined {
  return within(menu)
    .getAllByRole("option")
    .find((option) => option.getAttribute("aria-selected") === "true")
    ?.querySelector(".suggestion-title")?.textContent;
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** Opens a page of one empty paragraph and types `typed` in it. */
async function typeInEmptyPage(typed: string): Promise<{ editor: Editor; menu: HTMLElement | null }> {
  const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }] }]);
  const editor = await openPage(user, "Notes");
  placeCursor(editor, 1);
  editor.commands.insertContent(typed);
  const menu = await screen.findByRole("listbox", { name: t("slash.label") }).catch(() => null);
  return { editor, menu };
}

const ENTRY_ORDER = [
  // Basic
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
  // Media
  "Image",
  "Image from URL",
  "Photo gallery",
  "PDF",
  "Video",
  "Audio",
  "File",
  // Advanced
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
  // Pages
  "Link to page",
  "New sub-page",
  "New page from template",
  "Sort subpages A–Z",
  "Sort subpages by date created",
  // Inline
  "Date",
  "Mention a page",
  "Emoji",
  "Inline equation",
];

/**
 * Every entry of the list: what local AI adds (ADR-0037: "Record audio" after "Audio", and the writing
 * assistant's entries before "Table") and local dictation (ADR-0053: "Microphone" before "Record audio") is in the
 * list, and left out of the menu while AI is off or the transport cannot dictate.
 */
const ALL_ENTRIES = ENTRY_ORDER.flatMap((label) =>
  label === "Audio"
    ? [label, "Microphone", "Record audio"]
    : label === "Table"
      ? ["Ask AI", "Summarize page", "Action items", label]
      : [label],
);

describe("the entries", () => {
  it("are listed by section, in the order of the sections", () => {
    expect(SLASH_COMMANDS.map((command) => t(command.label))).toEqual(ALL_ENTRIES);
    expect([...SLASH_SECTIONS]).toEqual(["basic", "media", "advanced", "pages", "inline"]);
    const sections = SLASH_COMMANDS.map((command) => command.section);
    expect(sections.filter((section, index) => section !== sections[index - 1])).toEqual([
      "basic",
      "media",
      "advanced",
      "pages",
      "inline",
    ]); // each section is one run: nothing is split
  });

  it("each have an id, a glyph, a label, a hint and a section with a header", () => {
    const ids = SLASH_COMMANDS.map((command) => command.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const command of SLASH_COMMANDS) {
      expect(typeof command.icon, command.id).toBe("function");
      expect(t(command.label), command.id).not.toBe("");
      expect(t(command.hint), command.id).not.toBe("");
      expect(SLASH_SECTIONS, command.id).toContain(command.section);
    }
    for (const section of SLASH_SECTIONS) {
      expect(t(SLASH_SECTION_LABELS[section]), section).not.toBe("");
    }
  });

  it("keep their ids", () => {
    expect(SLASH_COMMANDS.map((command) => command.id)).toEqual([
      "text",
      "heading1",
      "heading2",
      "heading3",
      "bulletList",
      "orderedList",
      "taskList",
      "toggle",
      "toggleHeading1",
      "toggleHeading2",
      "toggleHeading3",
      "quote",
      "callout",
      "divider",
      "image",
      "imageUrl",
      "gallery",
      "pdf",
      "video",
      "audio",
      "microphone",
      "record",
      "file",
      "codeBlock",
      "diagram",
      "whiteboard",
      "math",
      "ai",
      "aiSummarize",
      "aiActionItems",
      "table",
      "columns2",
      "columns3",
      "templateButton",
      "toc",
      "breadcrumb",
      "pageList",
      "tableView",
      "boardView",
      "calendarView",
      "galleryView",
      "dashboardView",
      "sortList",
      "pageLink",
      "subpage",
      "template",
      "sortSubpages",
      "sortSubpagesByDate",
      "date",
      "mention",
      "emoji",
      "mathInline",
    ]);
  });
});

describe("matching and ranking", () => {
  it("shows everything, grouped, when nothing is typed", () => {
    expect(found("")).toEqual(ALL_ENTRIES);
    expect(found("   ")).toEqual(ALL_ENTRIES);
  });

  it("lists the entries whose label starts with the text first, in the order of the list", () => {
    expect(found("h").slice(0, 3)).toEqual(["Heading 1", "Heading 2", "Heading 3"]);
    // Then the entries with the keyword `heading` (the toggle headings), then the one with `headings`.
    expect(found("heading")).toEqual([
      "Heading 1",
      "Heading 2",
      "Heading 3",
      "Toggle heading 1",
      "Toggle heading 2",
      "Toggle heading 3",
      "Table of contents",
    ]);
  });

  it("puts Table before Table of contents", () => {
    // The views come last: `tab` is inside their keyword `database`.
    expect(found("tab")).toEqual(["Table", "Table view", "Table of contents", "List view", "Board view", "Calendar view", "Gallery view", "Dashboard view"]); // the shorter label first
  });

  it("finds the list view by its name and its words", () => {
    expect(found("list view")).toEqual(["List view"]);
    expect(found("view")).toEqual(["List view", "Table view", "Board view", "Calendar view", "Gallery view", "Dashboard view", "Diagram"]); // the diagram's hint says `preview`
    expect(found("database")).toEqual(["List view", "Table view", "Board view", "Calendar view", "Gallery view", "Dashboard view"]);
    expect(found("TABLE VIEW")).toEqual(["Table view"]);
    expect(found("children")).toEqual(["List view"]);
    // `list` starts the label of the list view, is a keyword of both lists, is in the label of the to-do
    // list and of the sort command, and in the hint of the table of contents.
    expect(found("list")).toEqual([
      "List view",
      "Bulleted list",
      "Numbered list",
      "To-do list",
      "Sort list A–Z",
      "Table of contents",
    ]);
  });

  it("finds the three sort entries with /sort, the list first (the shortest label), and by their words", () => {
    expect(found("sort")).toEqual(["Sort list A–Z", "Sort subpages A–Z", "Sort subpages by date created"]);
    // The sort entries have the keyword `order`; the numbered and bulleted lists only contain it in `ordered`.
    expect(found("order")).toEqual([
      "Sort list A–Z",
      "Sort subpages A–Z",
      "Sort subpages by date created",
      "Numbered list",
      "Bulleted list",
    ]);
    expect(found("alphabetical")).toEqual(["Sort list A–Z", "Sort subpages A–Z"]);
    expect(found("a-z")).toEqual(["Sort list A–Z", "Sort subpages A–Z"]);
    expect(found("abc")).toEqual(["Sort list A–Z", "Sort subpages A–Z"]);
    expect(found("chronological")).toEqual(["Sort subpages by date created"]);
    expect(found("oldest")).toEqual(["Sort subpages by date created"]);
    // `date` is the date mention first, then the sort by date.
    expect(found("date").slice(0, 2)).toEqual(["Date", "Sort subpages by date created"]);
    expect(filterSlashCommands("sort").map((command) => command.id)).toEqual(["sortList", "sortSubpages", "sortSubpagesByDate"]);
  });

  it("ranks Microphone first for /mic, /microphone and /dictate, above Record audio, which stays (ADR-0053)", () => {
    expect(found("mic").slice(0, 2)).toEqual(["Microphone", "Record audio"]); // a label that starts with it, then a keyword
    expect(found("microphone").slice(0, 2)).toEqual(["Microphone", "Record audio"]);
    // `dictate` is a keyword of both: the order of the list decides, and Microphone comes first.
    expect(found("dictate").slice(0, 2)).toEqual(["Microphone", "Record audio"]);
    expect(filterSlashCommands("dictate").map((command) => command.id).slice(0, 2)).toEqual(["microphone", "record"]);
    expect(found("dictation")).toEqual(["Microphone"]);
    for (const word of ["voice", "speech", "speak", "talk", "stt"]) {
      expect(found(word), word).toContain("Microphone");
    }
    expect(found("transcribe")).toEqual(["Record audio"]); // the recorder keeps its own words
    expect(found("meeting")).toEqual(["Record audio"]);
  });

  it("finds Image from URL by its name and its words, right after Image", () => {
    expect(found("image").slice(0, 2)).toEqual(["Image", "Image from URL"]);
    expect(found("image from")).toEqual(["Image from URL"]);
    for (const word of ["url", "web", "photo", "picture"]) {
      expect(found(word), word).toContain("Image from URL");
    }
    expect(found("url")).toEqual(["Image from URL"]);
    // `link` and `embed` are keywords of the card to a page too, which comes later in the list.
    expect(found("embed")).toEqual(["Image from URL", "Link to page"]);
    expect(found("link").slice(0, 2)).toEqual(["Link to page", "Image from URL"]); // label prefix first
  });

  it("lists the column entries for /col, before the entry whose keyword is collapse", () => {
    expect(found("col").slice(0, 3)).toEqual(["2 columns", "3 columns", "Toggle"]);
    expect(found("colum")).toEqual(["2 columns", "3 columns"]);
  });

  it("ranks label prefix, then keyword prefix, then label substring, then keyword or hint substring", () => {
    // `/to`: Toggle, To-do list and the toggle headings (labels, the shorter first), Table of contents
    // (keyword `toc`) and Date (keyword `today`), Link to page (inside its label), then keywords and hints
    // that hold it, in the order of the list.
    expect(found("to")).toEqual([
      "Toggle",
      "To-do list",
      "Toggle heading 1",
      "Toggle heading 2",
      "Toggle heading 3",
      "Table of contents",
      "Date",
      "Link to page",
      "Photo gallery", // `to` inside its label too, a longer one
      "Template button", // likewise (bu-tt-on), longer still
      "Quote", // the hint `Set off a quotation`
      "Divider", // the keyword `separator`
      "Image", // the keyword `photo`, the hint `Pick a picture to show`
      "Image from URL", // the keyword `photo`
      "File", // the hint `Pick a file to attach`
      "Breadcrumb", // the keyword `ancestors`
    ]);
    // A keyword prefix beats a label substring: `page` is a keyword of the sub-page and of the mention
    // (listed in the order of the sections), then of the list view (`pages`, a longer one), inside the
    // card's label.
    expect(found("page").slice(0, 4)).toEqual(["New sub-page", "Mention a page", "List view", "Link to page"]);
  });

  it("matches the hint", () => {
    expect(found("inside")).toEqual(["New sub-page"]); // `Create a page inside this one`
    expect(found("quotation")).toEqual(["Quote"]);
  });

  it("matches inside keywords, not only at their start", () => {
    expect(found("cestors")).toEqual(["Breadcrumb"]);
    // The block equation (its label), the inline one (its keyword `equation`), then Diagram (`sequence`).
    expect(found("equ")).toEqual(["Equation", "Inline equation", "Diagram"]);
  });

  it("finds the inline equation by its words, with the block equation first for the words they share", () => {
    expect(found("inline")).toEqual(["Inline equation"]);
    expect(found("inline equation")).toEqual(["Inline equation"]);
    expect(found("math")).toEqual(["Equation", "Inline equation"]);
    expect(found("latex")).toEqual(["Equation", "Inline equation"]);
    expect(found("formula")).toEqual(["Equation", "Inline equation"]);
    expect(found("equation")).toEqual(["Equation", "Inline equation"]);
  });

  it("ignores case and spaces", () => {
    expect(found("TABLE OF")).toEqual(["Table of contents"]);
    expect(found("Side By")).toEqual(["2 columns", "3 columns"]); // the keyword `side by side`
    expect(found(" h1 ")).toEqual(["Heading 1", "Toggle heading 1"]); // the keyword `h1` of both
  });

  it("finds Emoji by its name and keywords", () => {
    expect(found("emoji")).toEqual(["Emoji"]);
    expect(found("emo")).toEqual(["Emoji"]);
    expect(found("smiley")).toEqual(["Emoji"]);
    expect(found("emoticon")).toEqual(["Emoji"]);
    expect(found("icon")).toEqual(["Emoji"]);
  });

  it("finds nothing for text no entry holds", () => {
    expect(found("zzz")).toEqual([]);
  });
});

describe("the menu", () => {
  it("groups the entries under section headers when nothing is typed", async () => {
    const { menu } = await typeInEmptyPage("/");
    expect(menu).not.toBeNull();
    const list = menu as HTMLElement;
    expect(labelsOf(list)).toEqual(ENTRY_ORDER);

    const groups = within(list).getAllByRole("group");
    expect(groups.map((group) => group.querySelector(".suggestion-section")?.textContent)).toEqual([
      t("slash.section.basic"),
      t("slash.section.media"),
      t("slash.section.advanced"),
      t("slash.section.pages"),
      t("slash.section.inline"),
    ]);
    expect(groups.map((group) => within(group).getAllByRole("option").length)).toEqual([14, 7, 17, 5, 4]);
    // The header names its group.
    expect(within(list).getByRole("group", { name: t("slash.section.advanced") })).toBe(groups[2]);
    expect(within(groups[2] as HTMLElement).getByRole("option", { name: /Table of contents/ })).toBeInTheDocument();
  });

  it("lists matches best first, without headers", async () => {
    const { menu } = await typeInEmptyPage("/to");
    expect(menu).not.toBeNull();
    await waitFor(() => {
      expect(labelsOf(menu as HTMLElement).slice(0, 3)).toEqual(["Toggle", "To-do list", "Toggle heading 1"]);
    });
    expect(within(menu as HTMLElement).queryAllByRole("group")).toEqual([]);
    expect(document.querySelector(".suggestion-section")).toBeNull();
  });

  it("gives every entry a decorative 16 px glyph", async () => {
    const { menu } = await typeInEmptyPage("/");
    for (const option of within(menu as HTMLElement).getAllByRole("option")) {
      const icon = option.querySelector(".slash-item-icon svg");
      expect(icon, option.textContent).not.toBeNull();
      expect(icon).toHaveAttribute("aria-hidden", "true");
      expect(icon).toHaveAttribute("width", "16");
      expect(icon).toHaveAttribute("height", "16");
    }
  });

  it("is a listbox with options, and names the highlighted one", async () => {
    const { editor, menu } = await typeInEmptyPage("/");
    const list = menu as HTMLElement;
    const options = within(list).getAllByRole("option");
    expect(list).toHaveAttribute("aria-activedescendant", options[0]?.id);
    expect(options[0]?.id).not.toBe("");
    expect(new Set(options.map((option) => option.id)).size).toBe(options.length);

    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(list).toHaveAttribute("aria-activedescendant", options[1]?.id);
    expect(options[1]).toHaveAttribute("aria-selected", "true");
    expect(options[0]).toHaveAttribute("aria-selected", "false");
  });

  it("scrolls the highlighted option into view", async () => {
    const scroll = vi.spyOn(Element.prototype, "scrollIntoView");
    const { editor, menu } = await typeInEmptyPage("/");
    const options = within(menu as HTMLElement).getAllByRole("option");
    scroll.mockClear();
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" }); // wraps to the last entry
    await waitFor(() => {
      expect(scroll.mock.contexts.at(-1)).toBe(options.at(-1));
    });
    expect(scroll).toHaveBeenLastCalledWith({ block: "nearest" });
  });

  it("limits its height, so that a long list scrolls", async () => {
    // jsdom has no layout; what the menu asks of the browser is its inline `max-height` (the
    // stylesheet gives the popup `overflow-y: auto`).
    const { menu } = await typeInEmptyPage("/");
    const popup = (menu as HTMLElement).closest(".suggestion-menu");
    expect(popup).not.toBeNull();
    expect((popup as HTMLElement).style.maxHeight).not.toBe("");
    expect(popup).toHaveClass("slash-menu");
  });

  it("moves with Up and Down and wraps around the ends, across sections", async () => {
    const { editor, menu } = await typeInEmptyPage("/");
    const list = menu as HTMLElement;
    expect(highlighted(list)).toBe("Text");
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" });
    expect(highlighted(list)).toBe("Inline equation"); // from the first entry of Basic to the last of Inline
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(highlighted(list)).toBe("Text");

    for (let i = 0; i < 13; i += 1) {
      fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    }
    expect(highlighted(list)).toBe("Divider"); // the last of Basic
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(highlighted(list)).toBe("Image"); // the first of Media
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" });
    expect(highlighted(list)).toBe("Divider");
  });

  it("jumps with PageDown and PageUp and stops at the ends", async () => {
    const { editor, menu } = await typeInEmptyPage("/");
    const list = menu as HTMLElement;
    fireEvent.keyDown(editor.view.dom, { key: "PageDown" });
    expect(highlighted(list)).toBe("To-do list"); // 6 entries further
    fireEvent.keyDown(editor.view.dom, { key: "PageUp" });
    expect(highlighted(list)).toBe("Text");
    fireEvent.keyDown(editor.view.dom, { key: "PageUp" });
    expect(highlighted(list)).toBe("Text"); // no wrap
    for (let i = 0; i < 8; i += 1) {
      fireEvent.keyDown(editor.view.dom, { key: "PageDown" });
    }
    expect(highlighted(list)).toBe("Inline equation");
  });

  it("highlights the entry the pointer moves over", async () => {
    const { menu } = await typeInEmptyPage("/");
    const list = menu as HTMLElement;
    const options = within(list).getAllByRole("option");
    fireEvent.mouseMove(options[21] as HTMLElement);
    expect(highlighted(list)).toBe("Code");
    expect(list).toHaveAttribute("aria-activedescendant", options[21]?.id);
    fireEvent.mouseMove(options[1] as HTMLElement);
    expect(highlighted(list)).toBe("Heading 1");
  });

  it("chooses with Enter and with Tab, and closes with Escape", async () => {
    const { editor } = await typeInEmptyPage("/head");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(labelsOf(menu).slice(0, 3)).toEqual(["Heading 1", "Heading 2", "Heading 3"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Tab" });
    expect(editor.state.doc.child(0).type.name).toBe("heading");
    expect(editor.state.doc.child(0).attrs.level).toBe(2);
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();

    editor.commands.setContent({ type: "doc", content: [{ type: "paragraph" }] });
    placeCursor(editor, 1);
    editor.commands.insertContent("/");
    await screen.findByRole("listbox", { name: t("slash.label") });
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    });
    expect(editor.state.doc.child(0).textContent).toBe("/"); // nothing was inserted
  });

  it("shows a No results row when nothing matches, and closes at the space that ends the command", async () => {
    const { editor } = await typeInEmptyPage("/zzz");
    expect(await screen.findByText(t("slash.empty"))).toBeInTheDocument();
    expect(t("slash.empty")).toBe("No results");
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    expect(screen.queryByRole("option")).not.toBeInTheDocument();

    // More characters with no match: the row stays (as long as the text could still be a command).
    editor.commands.insertContent("qq");
    expect(await screen.findByText(t("slash.empty"))).toBeInTheDocument();
    // Back to the bare `/`: the entries come back.
    editor.commands.deleteRange({ from: 2, to: 7 });
    expect(await screen.findByRole("listbox", { name: t("slash.label") })).toBeInTheDocument();
    expect(screen.queryByText(t("slash.empty"))).not.toBeInTheDocument();

    // A space ends the command, so a text with none that matches closes the menu.
    editor.commands.insertContent("zq ");
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    });
    expect(screen.queryByText(t("slash.empty"))).not.toBeInTheDocument();
  });
});

/** `Before`, a layout of two columns (`A1`, `B1`). */
function twoColumns(): JsonValue {
  const text = (value: string): JsonValue => ({ type: "text", text: value });
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(0) }, content: [text("Before")] },
      {
        type: "columnList",
        attrs: { id: pid(1) },
        content: [
          {
            type: "column",
            attrs: { id: pid(2) },
            content: [{ type: "paragraph", attrs: { id: pid(3) }, content: [text("A1")] }],
          },
          {
            type: "column",
            attrs: { id: pid(4) },
            content: [{ type: "paragraph", attrs: { id: pid(5) }, content: [text("B1")] }],
          },
        ],
      },
    ],
  };
}

describe("unavailable entries", () => {
  it("are shown with their reason and do nothing when chosen", async () => {
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Board", content: twoColumns() }] }]);
    const editor = await openPage(user, "Board");
    let caret = -1;
    editor.state.doc.descendants((node, pos) => {
      if (caret === -1 && node.isTextblock && node.textContent === "A1") {
        caret = pos + 1 + 2;
      }
      return caret === -1;
    });
    placeCursor(editor, caret);
    editor.commands.insertContent(" /");

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const options = within(menu).getAllByRole("option");
    const disabled = options.filter((option) => option.getAttribute("aria-disabled") === "true");
    // The columns cannot nest; the sort of a list needs a list, and the sort of subpages needs two (the page has none).
    const reasons: [string, string][] = [
      ["2 columns", t("slash.columns.inColumn")],
      ["3 columns", t("slash.columns.inColumn")],
      ["Sort list A–Z", t("slash.sortList.notInList")],
      ["Sort subpages A–Z", t("slash.sortSubpages.few")],
      ["Sort subpages by date created", t("slash.sortSubpages.few")],
    ];
    expect(disabled.map((option) => option.querySelector(".suggestion-title")?.textContent)).toEqual(
      reasons.map(([title]) => title),
    );
    disabled.forEach((option, index) => {
      expect(within(option).getByText(reasons[index]?.[1] ?? "")).toBeInTheDocument();
      expect(option).toHaveClass("is-disabled");
    });
    expect(options.length - disabled.length).toBe(ENTRY_ORDER.length - reasons.length); // all the others can run

    // Highlight one and press Enter, then click the other: nothing happens, the menu stays.
    const outlineBefore = JSON.stringify(editor.getJSON());
    fireEvent.mouseMove(disabled[0] as HTMLElement);
    expect(highlighted(menu)).toBe("2 columns");
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    fireEvent.click(disabled[1] as HTMLElement);
    expect(JSON.stringify(editor.getJSON())).toBe(outlineBefore);
    expect(screen.getByRole("listbox", { name: t("slash.label") })).toBeInTheDocument();

    // An entry that can run still does.
    await userChoose(menu, "Quote");
    expect(editor.view.dom.querySelector("blockquote")).not.toBeNull();
  });
});

/** A list of bullets with the given texts (the item ids `pid(base + n)`), as portable JSON. */
function bulletList(base: number, ...texts: string[]): JsonValue {
  return {
    type: "bulletList",
    attrs: { id: pid(base) },
    content: texts.map((value, index) => ({
      type: "listItem",
      attrs: { id: pid(base + 1 + index * 2) },
      content: [
        {
          type: "paragraph",
          attrs: { id: pid(base + 2 + index * 2) },
          ...(value === "" ? {} : { content: [{ type: "text", text: value }] }),
        },
      ],
    })),
  };
}

/** The texts of the items of the first list of the page. */
function listTexts(editor: Editor): string[] {
  const texts: string[] = [];
  let found = false;
  editor.state.doc.descendants((node) => {
    if (!found && node.type.name === "bulletList") {
      found = true;
      node.forEach((item) => {
        texts.push(item.textContent);
      });
    }
    return !found;
  });
  return texts;
}

describe("sorting from the menu (ADR-0052)", () => {
  it("sorts the list the caret is in, deleting the typed command in the same step: one undo brings all back", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(0) }, content: [{ type: "text", text: "Shopping" }] },
        bulletList(10, "Cherry", "apple", "Banana", ""),
        { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "after" }] },
      ],
    };
    const { user } = await renderPages([{ name: "Personal", pages: [{ title: "Groceries", content }] }]);
    const editor = await openPage(user, "Groceries");
    // Each edit is its own undo step, so that "one undo" below is the command and nothing else.
    const storage = editor.storage as unknown as { loroBinding: { undoManager: { setMergeInterval: (ms: number) => void } | null } };
    storage.loroBinding.undoManager?.setMergeInterval(0);
    const original = JSON.stringify(editor.getJSON());
    expect(listTexts(editor)).toEqual(["Cherry", "apple", "Banana", ""]);

    // Typing "/" in the empty last item opens the menu, inside the list.
    let lastItem = -1;
    editor.state.doc.descendants((node, pos) => {
      if (node.type.name === "listItem" && node.textContent === "") {
        lastItem = pos + 2;
      }
      return true;
    });
    placeCursor(editor, lastItem);
    editor.commands.insertContent("/sort");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(labelsOf(menu).slice(0, 3)).toEqual(["Sort list A–Z", "Sort subpages A–Z", "Sort subpages by date created"]);
    });
    const typed = JSON.stringify(editor.getJSON());
    const docTransactions: number[] = [];
    editor.on("transaction", ({ transaction }) => {
      docTransactions.push(transaction.docChanged ? 1 : 0);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await waitFor(() => {
      expect(listTexts(editor)).toEqual(["apple", "Banana", "Cherry", ""]); // the empty one stays last
    });
    // One document change did both: the typed text is gone and the list is in order.
    expect(docTransactions.filter(Boolean)).toHaveLength(1);
    expect(editor.state.doc.textContent).not.toContain("/");
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    // The caret is in the item it was in (the empty one, now last).
    expect(editor.state.selection.$from.parent.textContent).toBe("");
    expect(editor.state.selection.$from.node(-1).type.name).toBe("listItem");
    expect(editor.state.selection.$from.index(-2)).toBe(3);

    // One undo puts the order back and the typed command with it (the state before the command);
    // the second takes the typed text away: no "/" is left behind. A redo sorts again.
    expect(undo(editor.state, editor.view.dispatch)).toBe(true);
    expect(JSON.stringify(editor.getJSON())).toBe(typed);
    expect(listTexts(editor)).toEqual(["Cherry", "apple", "Banana", "/sort"]);
    expect(undo(editor.state, editor.view.dispatch)).toBe(true);
    expect(JSON.stringify(editor.getJSON())).toBe(original);
    expect(editor.state.doc.textContent).not.toContain("/");
    expect(redo(editor.state, editor.view.dispatch)).toBe(true);
    expect(redo(editor.state, editor.view.dispatch)).toBe(true);
    expect(listTexts(editor)).toEqual(["apple", "Banana", "Cherry", ""]);
  });

  it("is unavailable outside a list, with its reason, and does nothing when chosen", async () => {
    const { editor } = await typeInEmptyPage("/sort");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    const option = within(menu)
      .getAllByRole("option")
      .find((candidate) => candidate.querySelector(".suggestion-title")?.textContent === "Sort list A–Z");
    expect(option).toHaveAttribute("aria-disabled", "true");
    expect(within(option as HTMLElement).getByText(t("slash.sortList.notInList"))).toBeInTheDocument();
    const before = JSON.stringify(editor.getJSON());
    fireEvent.click(option as HTMLElement);
    expect(JSON.stringify(editor.getJSON())).toBe(before);
  });

  it("sorts the subpages of the open page A to Z and by date created, and removes the typed command", async () => {
    let clock = 1_800_000_000_000;
    const { user } = await renderPages(
      [
        {
          name: "Personal",
          pages: [
            {
              title: "Recipes",
              content: paragraphs(""),
              children: [{ title: "Pear" }, { title: "apple" }, { title: "Fig" }],
            },
          ],
        },
      ],
      { now: () => (clock += 1) },
    );
    expect(outline()).toEqual(["Recipes", "  Pear", "  apple", "  Fig"]);
    const editor = await openPage(user, "Recipes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/sort");
    let menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(
        within(menu)
          .getAllByRole("option")
          .filter((option) => option.getAttribute("aria-disabled") !== "true")
          .map((option) => option.querySelector(".suggestion-title")?.textContent),
      ).toEqual(["Sort subpages A–Z", "Sort subpages by date created"]); // the list command needs a list
    });
    await userChoose(menu, "Sort subpages A–Z");
    await waitFor(() => {
      expect(outline()).toEqual(["Recipes", "  apple", "  Fig", "  Pear"]);
    });
    expect(editor.state.doc.textContent).toBe("");

    editor.commands.insertContent("/sort");
    menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await userChoose(menu, "Sort subpages by date created");
    await waitFor(() => {
      expect(outline()).toEqual(["Recipes", "  Pear", "  apple", "  Fig"]); // the order they were made in
    });
    expect(editor.state.doc.textContent).toBe("");
  });

  it("is unavailable for a page with fewer than two subpages", async () => {
    const { user } = await renderPages([
      { name: "Personal", pages: [{ title: "Solo", content: paragraphs(""), children: [{ title: "Only child" }] }] },
    ]);
    const editor = await openPage(user, "Solo");
    placeCursor(editor, 1);
    editor.commands.insertContent("/sort");
    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    for (const label of ["Sort subpages A–Z", "Sort subpages by date created"]) {
      const option = within(menu)
        .getAllByRole("option")
        .find((candidate) => candidate.querySelector(".suggestion-title")?.textContent === label);
      expect(option, label).toHaveAttribute("aria-disabled", "true");
      expect(within(option as HTMLElement).getByText(t("slash.sortSubpages.few")), label).toBeInTheDocument();
    }
  });
});

async function userChoose(menu: HTMLElement, label: string): Promise<void> {
  const option = within(menu)
    .getAllByRole("option")
    .find((candidate) => candidate.querySelector(".suggestion-title")?.textContent === label);
  expect(option, label).toBeDefined();
  fireEvent.click(option as HTMLElement);
  await waitFor(() => {
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
  });
}

describe("the [[ page picker", () => {
  it("is unchanged by the sections: no headers, no glyph tiles, the same keys", async () => {
    const { user } = await renderPages([
      {
        name: "Personal",
        pages: [
          { title: "Notes", content: paragraphs("") },
          { title: "Alpha", icon: "🅰" },
          { title: "Beta", icon: "🅱" },
        ],
      },
    ]);
    const editor = await openPage(user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("[[");

    const picker = await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(
        within(picker)
          .getAllByRole("option")
          .map((option) => option.textContent)
          .sort(),
      ).toEqual(["Notes", "🅰Alpha", "🅱Beta"].sort()); // the icon, then the title: as before (the page glyph is an SVG: no text)
    });
    expect(within(picker).queryAllByRole("group")).toEqual([]);
    expect(picker.closest(".suggestion-menu")).not.toHaveClass("slash-menu");
    expect(picker.querySelector(".suggestion-section")).toBeNull();
    expect(picker.querySelector(".slash-item-icon")).toBeNull(); // no tiles; the page glyph of a page without an icon is a bare SVG
    expect(picker.querySelectorAll(".page-icon[data-icon-kind=\"default\"]")).toHaveLength(1);
    const options = within(picker).getAllByRole("option");
    expect(picker).toHaveAttribute("aria-activedescendant", options[0]?.id);

    // Up wraps to the last page, Down to the first; Enter inserts the reference.
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" });
    expect(options[2]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    let refs = 0;
    editor.state.doc.descendants((node) => {
      if (node.type.name === "pageRef") {
        refs += 1;
      }
    });
    expect(refs).toBe(1);
  });
});

describe("the shared popup", () => {
  interface Item {
    id: string;
    section?: string;
  }

  function show(store: SuggestionMenuStore<Item>, items: Item[], query = ""): void {
    const props = { items, query, command: vi.fn(), clientRect: null } as unknown as SuggestionProps<Item, Item>;
    act(() => {
      store.renderer().onStart?.(props);
    });
  }

  function mountMenu(store: SuggestionMenuStore<Item>): void {
    render(
      <SuggestionMenu
        store={store}
        label="Things"
        emptyText="Nothing"
        itemKey={(item) => item.id}
        renderItem={(item) => <span className="suggestion-title">{item.id}</span>}
        sections={{ of: (item) => item.section, label: (section) => section.toUpperCase() }}
      />,
    );
  }

  it("draws a header for each run of items of a section, an inline section included", () => {
    const store = new SuggestionMenuStore<Item>();
    mountMenu(store);
    show(store, [
      { id: "a", section: "basic" },
      { id: "b", section: "basic" },
      { id: "c", section: "inline" },
      { id: "d" },
      { id: "e", section: "basic" },
    ]);
    const list = screen.getByRole("listbox", { name: "Things" });
    expect(Array.from(list.querySelectorAll(".suggestion-section")).map((header) => header.textContent)).toEqual([
      "BASIC",
      "INLINE",
      "BASIC",
    ]);
    expect(within(list).getAllByRole("group").map((group) => within(group).getAllByRole("option").length)).toEqual([2, 1, 1]);
    expect(within(list).getAllByRole("option").map((option) => option.textContent)).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("draws no header while something is typed, or for items without a section", () => {
    const store = new SuggestionMenuStore<Item>();
    mountMenu(store);
    show(store, [{ id: "a", section: "basic" }, { id: "b", section: "inline" }], "x");
    expect(document.querySelector(".suggestion-section")).toBeNull();
    show(store, [{ id: "a" }, { id: "b" }]);
    expect(document.querySelector(".suggestion-section")).toBeNull();
    expect(screen.queryAllByRole("group")).toEqual([]);
    expect(screen.getAllByRole("option")).toHaveLength(2);
  });

  it("leaves Enter and Tab to the editor when there is no item, and takes them otherwise", () => {
    const store = new SuggestionMenuStore<Item>();
    mountMenu(store);
    const key = (name: string): boolean =>
      store.renderer().onKeyDown?.({ view: null, event: new KeyboardEvent("keydown", { key: name }), range: { from: 0, to: 0 } } as never) ?? false;
    show(store, []);
    expect(key("Enter")).toBe(false);
    expect(key("Tab")).toBe(false);
    expect(key("ArrowDown")).toBe(true); // nothing to move to, but still the menu's key
    show(store, [{ id: "a" }, { id: "b" }]);
    expect(key("Enter")).toBe(true);
    expect(key("Tab")).toBe(true);
    expect(key("x")).toBe(false);
  });

  it("shows the empty text in a status row when there is no item", () => {
    const store = new SuggestionMenuStore<Item>();
    mountMenu(store);
    show(store, []);
    expect(screen.getByRole("status")).toHaveTextContent("Nothing");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
  });
});
