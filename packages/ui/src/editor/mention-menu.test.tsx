// The `@` menu (days, people and pages) and `/mention`: what the menu lists and in which order, how typing
// filters it, what choosing Today, Tomorrow, "Pick a date..." and a page does, Escape, the places
// where `@` is only a character, and that `[[` and `/` are left as they were.

import type { JsonValue } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { paragraphs, pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { dateChoices, mentionItemKey, mentionSection } from "./mention-menu";
import type { MentionChoice } from "./mention-menu";
import { addDays, fullDate, mediumDate } from "./dates";

const NOW = new Date(2026, 9, 7, 12, 0, 0);
const TODAY = "2026-10-07";
const TOMORROW = "2026-10-08";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fakeToday(): void {
  vi.useFakeTimers({ now: NOW, toFake: ["Date"] });
}

function placeCursor(editor: Editor, pos: number): void {
  editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, pos)));
}

/** The titles of the options of the open menu, in order. */
function titlesOf(menu: HTMLElement): (string | null | undefined)[] {
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

function findMenu(): Promise<HTMLElement> {
  return screen.findByRole("listbox", { name: t("mention.label") });
}

/** The titles of the open menu's options, looked up afresh (the menu is drawn again when it reopens). */
function titles(): (string | null | undefined)[] {
  return titlesOf(screen.getByRole("listbox", { name: t("mention.label") }));
}

function menuIsOpen(): boolean {
  return screen.queryByRole("listbox", { name: t("mention.label") }) !== null || screen.queryByText(t("mention.empty")) !== null;
}

/** The mentions of the editor's document, as `date` or `date time`, in order. */
function mentions(editor: Editor): string[] {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === "dateMention") {
      found.push(typeof node.attrs.time === "string" ? `${String(node.attrs.date)} ${node.attrs.time}` : String(node.attrs.date));
    }
    return true;
  });
  return found;
}

function refs(editor: Editor): string[] {
  const found: string[] = [];
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageRef") {
      found.push(String(node.attrs.pageId));
    }
    return true;
  });
  return found;
}

const PAGES = [
  { title: "Notes", content: paragraphs("See ") },
  { title: "Target page", icon: "🎯" },
  { title: "Todo list" },
  { title: "Project plan" },
];

/** Opens "Notes" (one paragraph, `See `) with the cursor at its end and types `typed` there. */
async function typeAt(typed: string, pages: readonly { title: string; icon?: string; content?: JsonValue }[] = PAGES) {
  const app = await renderPages([{ name: "Personal", pages: [...pages] }]);
  const editor = await openPage(app.user, "Notes");
  placeCursor(editor, 5);
  editor.commands.insertContent(typed);
  return { ...app, editor };
}

describe("the date entries", () => {
  it("are Today, Tomorrow and Pick a date, and are found by their first letters or their words", () => {
    const names = (query: string): string[] =>
      dateChoices(query).map((choice) => (choice.kind === "page" ? "page" : choice.kind));
    expect(names("")).toEqual(["today", "tomorrow", "pick"]);
    expect(names("   ")).toEqual(["today", "tomorrow", "pick"]);
    expect(names("t")).toEqual(["today", "tomorrow"]);
    expect(names("to")).toEqual(["today", "tomorrow"]);
    expect(names("tod")).toEqual(["today"]);
    expect(names("tom")).toEqual(["tomorrow"]);
    expect(names("TOMORROW")).toEqual(["tomorrow"]);
    expect(names("pick")).toEqual(["pick"]);
    expect(names("pick a")).toEqual(["pick"]); // spaces do not count
    expect(names("date")).toEqual(["pick"]);
    expect(names("cal")).toEqual(["pick"]);
    expect(names("when")).toEqual(["pick"]);
    expect(names("now")).toEqual(["today"]);
    expect(names("proj")).toEqual([]);
    expect(names("x")).toEqual([]);
  });

  it("have a key each, and a section: dates before pages", () => {
    const page: MentionChoice = { kind: "page", page: { pageId: "p1", spaceId: "s1", title: "A" } };
    expect([...dateChoices(""), page].map(mentionItemKey)).toEqual(["today", "tomorrow", "pick", "page:p1"]);
    expect([...dateChoices(""), page].map(mentionSection)).toEqual(["dates", "dates", "dates", "pages"]);
    const person: MentionChoice = { kind: "person", person: { accountId: "a1", name: "Bob", verified: false, isMe: false } };
    expect(mentionItemKey(person)).toBe("person:a1");
    expect(mentionSection(person)).toBe("people");
  });
});

describe("the @ menu", () => {
  it("opens on @ with the days first, then the people (this account alone in a personal space), then the pages, under headers", async () => {
    fakeToday();
    const { editor } = await typeAt("@");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual([
        t("date.today"),
        t("date.tomorrow"),
        t("mention.pick"),
        "You",
        "Notes",
        "Target page",
        "Todo list",
        "Project plan",
      ]);
    });
    expect(menu).toHaveAccessibleName(t("mention.label"));
    expect(menu.closest(".suggestion-menu")).toHaveClass("mention-menu");
    const groups = within(menu).getAllByRole("group");
    expect(groups.map((group) => group.querySelector(".suggestion-section")?.textContent)).toEqual([
      t("mention.section.dates"),
      t("mention.section.people"),
      t("mention.section.pages"),
    ]);
    expect(groups.map((group) => within(group).getAllByRole("option").length)).toEqual([3, 1, 4]);
    // The days say which day they are; every entry has a decorative glyph.
    const options = within(menu).getAllByRole("option");
    expect(options[0]).toHaveTextContent(mediumDate(TODAY));
    expect(options[1]).toHaveTextContent(mediumDate(TOMORROW));
    expect(options[2]).toHaveTextContent(t("mention.pick.hint"));
    for (const option of options) {
      expect(option.querySelector(".slash-item-icon")).not.toBeNull();
    }
    expect(options[3]).toHaveTextContent(t("mention.person.you"));
    expect(options[5]?.querySelector(".slash-item-icon")).toHaveTextContent("🎯");
    expect(highlighted(menu)).toBe(t("date.today"));
    expect(menu).toHaveAttribute("aria-activedescendant", options[0]?.id);
    expect(editor.getText()).toBe("See @");
  });

  it("is only a character after a letter, in a code block and in text that cannot hold a mention", async () => {
    const { editor } = await typeAt("x");
    placeCursor(editor, editor.state.doc.content.size - 1);
    editor.commands.insertContent("@");
    expect(menuIsOpen()).toBe(false); // `x@`: part of a word, as in an address
    editor.commands.insertContent("b ");
    expect(menuIsOpen()).toBe(false);
    expect(mentions(editor)).toEqual([]);
  });

  it("is not opened by @ in a code block or in a toggle's summary", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "codeBlock", attrs: { id: pid(0) }, content: [{ type: "text", text: "a" }] },
        {
          type: "details",
          attrs: { id: pid(1), open: true },
          content: [
            { type: "detailsSummary", attrs: { id: pid(2) }, content: [{ type: "text", text: "S" }] },
            { type: "detailsContent", attrs: { id: pid(3) }, content: [{ type: "paragraph", attrs: { id: pid(4) } }] },
          ],
        },
      ],
    };
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content }] }]);
    const editor = await openPage(app.user, "Notes");
    placeCursor(editor, 2); // after the `a` of the code block
    editor.commands.insertContent(" @");
    expect(menuIsOpen()).toBe(false);
    let summaryEnd = -1;
    editor.state.doc.descendants((node, pos) => {
      if (node.type.name === "detailsSummary") {
        summaryEnd = pos + node.nodeSize - 1;
      }
      return true;
    });
    placeCursor(editor, summaryEnd);
    editor.commands.insertContent(" @");
    expect(menuIsOpen()).toBe(false);
  });

  it("does not open for an @ followed by a space, and closes when a query with a space finds nothing", async () => {
    const { editor } = await typeAt("@ ");
    expect(menuIsOpen()).toBe(false);

    editor.commands.insertContent("@zz");
    expect(await screen.findByText(t("mention.empty"))).toBeInTheDocument(); // nothing matches `zz`, and it may still be a word
    editor.commands.insertContent(" qq"); // a space: it is not a mention
    await waitFor(() => {
      expect(menuIsOpen()).toBe(false);
    });
    expect(editor.getText()).toBe("See @ @zz qq");
    expect(mentions(editor)).toEqual([]);
    expect(refs(editor)).toEqual([]);
  });

  it("keeps the menu open over a page title with spaces, and matches it", async () => {
    const { editor } = await typeAt("@target p");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual(["Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(refs(editor)).toHaveLength(1);
  });
});

describe("choosing a day", () => {
  it("inserts Today with Enter: the typed @ is replaced by a mention and a space, the caret after it", async () => {
    fakeToday();
    const { editor, memory } = await typeAt("@");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    expect(mentions(editor)).toEqual([TODAY]);
    expect(editor.getText()).toBe("See 2026-10-07 ");
    expect(editor.state.selection.from).toBe(editor.state.doc.firstChild!.nodeSize - 1);
    expect(menuIsOpen()).toBe(false);
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".date-mention")).toHaveTextContent(t("date.today"));
    });
    // No picker: it is a plain choice.
    expect(screen.queryByRole("dialog", { name: t("date.pickerTitle") })).not.toBeInTheDocument();

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "See " },
              { type: "dateMention", attrs: { date: TODAY, id: expect.stringMatching(/^[0-9a-f-]{36}$/u) as unknown } },
              { type: "text", text: " " },
            ],
          },
        ],
      });
    });
  });

  it("inserts Tomorrow with the arrow keys and Enter, and by a click", async () => {
    fakeToday();
    const { editor } = await typeAt("@");
    const menu = await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(highlighted(menu)).toBe(t("date.tomorrow"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual([TOMORROW]);
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".date-mention")).toHaveTextContent(t("date.tomorrow"));
    });

    // By a click (and Tab), from `@to`.
    editor.commands.insertContent("@tom");
    const second = await findMenu();
    await waitFor(() => {
      expect(titlesOf(second)).toEqual([t("date.tomorrow")]);
    });
    fireEvent.click(within(second).getByRole("option"));
    expect(mentions(editor)).toEqual([TOMORROW, TOMORROW]);
    expect(editor.getText()).toBe("See 2026-10-08 2026-10-08 ");
  });

  it("takes the day from the clock when it is chosen: Tomorrow is the day after the one it is", async () => {
    vi.useFakeTimers({ now: new Date(2026, 11, 31, 23, 30), toFake: ["Date"] });
    const { editor } = await typeAt("@tomorrow");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual(["2027-01-01"]);
  });

  it("filters as you type: to, tom, pick, cal, and leaves the pages that match too", async () => {
    fakeToday();
    const { editor } = await typeAt("@to");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual([t("date.today"), t("date.tomorrow"), "Todo list"]); // Todo list: a page, after the days
    });
    // No headers while something is typed: the matches are listed best first.
    expect(within(menu).queryAllByRole("group")).toEqual([]);
    expect(document.querySelector(".suggestion-section")).toBeNull();

    editor.commands.insertContent("m");
    await waitFor(() => {
      expect(titles()).toEqual([t("date.tomorrow")]);
    });

    editor.commands.deleteRange({ from: 5, to: editor.state.doc.firstChild!.nodeSize - 1 });
    editor.commands.insertContent("@pick");
    await waitFor(() => {
      expect(titles()).toEqual([t("mention.pick")]);
    });
  });

  it("finds the calendar by its words and shows only pages for text that is no day", async () => {
    fakeToday();
    const { editor } = await typeAt("@cal");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual([t("mention.pick")]);
    });
    editor.commands.deleteRange({ from: 5, to: editor.state.doc.firstChild!.nodeSize - 1 });
    editor.commands.insertContent("@proj");
    await waitFor(() => {
      expect(titles()).toEqual(["Project plan"]);
    });
  });
});

describe("Pick a date", () => {
  it("inserts today's mention and opens its picker; a day picked there replaces it", async () => {
    fakeToday();
    const { editor, memory } = await typeAt("@");
    const menu = await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(highlighted(menu)).toBe(t("mention.pick"));
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    expect(mentions(editor)).toEqual([TODAY]);
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    expect(menuIsOpen()).toBe(false);
    fireEvent.click(within(picker).getByRole("button", { name: fullDate("2026-11-03") }));
    expect(mentions(editor)).toEqual(["2026-11-03"]);
    expect(screen.queryByRole("dialog", { name: t("date.pickerTitle") })).not.toBeInTheDocument();

    const { page } = await idsOf(memory);
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, page("Notes")))).toContain("2026-11-03");
    });
  });

  it("is chosen by a click too", async () => {
    fakeToday();
    const { editor } = await typeAt("@");
    const menu = await findMenu();
    fireEvent.click(within(menu).getAllByRole("option")[2] as HTMLElement);
    expect(await screen.findByRole("dialog", { name: t("date.pickerTitle") })).toBeInTheDocument();
    expect(mentions(editor)).toEqual([TODAY]);
    expect(editor.getText()).toBe("See 2026-10-07 ");
  });
});

describe("choosing a page", () => {
  it("inserts a chip like [[ does, which shows the live title", async () => {
    const { editor, memory } = await typeAt("@targ");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual(["Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    await waitFor(() => {
      expect(editor.view.dom.querySelector("[data-page-ref]")).toHaveTextContent("Target page");
    });
    const { page } = await idsOf(memory);
    expect(refs(editor)).toEqual([page("Target page")]);
    expect(editor.getText()).toBe("See [[Target page]] ");
    expect(mentions(editor)).toEqual([]);
    expect(menuIsOpen()).toBe(false);
    await waitFor(async () => {
      expect(await contentOf(memory, page("Notes"))).toMatchObject({
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "See " },
              { type: "pageRef", attrs: { pageId: page("Target page"), id: expect.stringMatching(/^[0-9a-f-]{36}$/u) as unknown } },
              { type: "text", text: " " },
            ],
          },
        ],
      });
    });
  });

  it("gives the same nodes as the [[ picker for the same page: a chip and a space after it", async () => {
    const { editor, memory } = await typeAt("@target");
    await findMenu();
    await waitFor(() => {
      expect(titles()).toEqual(["Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    const { page } = await idsOf(memory);

    editor.commands.insertContent("[[target");
    await screen.findByRole("listbox", { name: t("picker.label") });
    await waitFor(() => {
      expect(within(screen.getByRole("listbox", { name: t("picker.label") })).getAllByRole("option")).toHaveLength(1);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // Inline content: `See `, the chip, ` `, the second chip, ` `: both inserted the same two nodes.
    const inline: string[] = [];
    editor.state.doc.firstChild?.forEach((node) => {
      inline.push(node.type.name === "pageRef" ? `ref:${String(node.attrs.pageId)}:${String(node.attrs.blockId)}` : `text:${node.text ?? ""}`);
    });
    expect(inline).toEqual([
      "text:See ",
      `ref:${page("Target page")}:null`,
      "text: ",
      `ref:${page("Target page")}:null`,
      "text: ",
    ]);
  });

  it("lists the pages of the space when nothing is typed, and chooses one by a click", async () => {
    const { editor, memory } = await typeAt("@");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toHaveLength(8);
    });
    const { page } = await idsOf(memory);
    fireEvent.click(within(menu).getByRole("option", { name: /Todo list/u }));
    expect(refs(editor)).toEqual([page("Todo list")]);
  });

  it("offers no pages when the search fails, and still the days", async () => {
    fakeToday();
    const app = await renderPages([{ name: "Personal", pages: [...PAGES] }]);
    const editor = await openPage(app.user, "Notes");
    const dispatch = app.memory.dispatch.bind(app.memory);
    vi.spyOn(app.memory, "dispatch").mockImplementation((request) =>
      request.type === "Search" ? Promise.reject(new Error("search failed")) : dispatch(request),
    );
    placeCursor(editor, 5);
    editor.commands.insertContent("@to");
    await findMenu();
    await waitFor(() => {
      expect(titles()).toEqual([t("date.today"), t("date.tomorrow")]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual([TODAY]);
  });
});

describe("Escape and the other keys", () => {
  it("closes the menu on Escape and leaves what was typed", async () => {
    fakeToday();
    const { editor } = await typeAt("@to");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(menuIsOpen()).toBe(false);
    });
    expect(editor.getText()).toBe("See @to");
    expect(mentions(editor)).toEqual([]);
    expect(refs(editor)).toEqual([]);
    // More typing does not bring it back.
    editor.commands.insertContent("m");
    expect(menuIsOpen()).toBe(false);
    expect(editor.getText()).toBe("See @tom");
  });

  it("wraps with the arrow keys, jumps with PageDown and PageUp, and takes Tab", async () => {
    fakeToday();
    const { editor } = await typeAt("@");
    const menu = await findMenu();
    await waitFor(() => {
      expect(titlesOf(menu)).toHaveLength(8);
    });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowUp" });
    expect(highlighted(menu)).toBe("Project plan");
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    expect(highlighted(menu)).toBe(t("date.today"));
    fireEvent.keyDown(editor.view.dom, { key: "PageDown" });
    expect(highlighted(menu)).toBe("Todo list"); // six further
    fireEvent.keyDown(editor.view.dom, { key: "PageUp" });
    expect(highlighted(menu)).toBe(t("date.today"));
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Tab" });
    expect(mentions(editor)).toEqual([TOMORROW]);
  });

  it("highlights the entry the pointer moves over", async () => {
    fakeToday();
    await typeAt("@");
    const menu = await findMenu();
    const options = within(menu).getAllByRole("option");
    fireEvent.mouseMove(options[1] as HTMLElement);
    expect(highlighted(menu)).toBe(t("date.tomorrow"));
  });
});

describe("/mention", () => {
  it("is an entry of the inline section that types the @ and opens the menu", async () => {
    fakeToday();
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }, { title: "Target page" }] }]);
    const editor = await openPage(app.user, "Notes");
    placeCursor(editor, 1);
    editor.commands.insertContent("/mention");

    const slash = await screen.findByRole("listbox", { name: t("slash.label") });
    const first = within(slash).getAllByRole("option")[0] as HTMLElement;
    expect(first).toHaveTextContent(t("slash.mention"));
    expect(first).toHaveTextContent(t("slash.mention.hint"));
    expect(first.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });

    // The command text became an `@`, and the mention menu is open on it.
    const menu = await findMenu();
    expect(editor.getText()).toBe("@");
    expect(screen.queryByRole("listbox", { name: t("slash.label") })).not.toBeInTheDocument();
    await waitFor(() => {
      expect(titlesOf(menu)).toEqual([t("date.today"), t("date.tomorrow"), t("mention.pick"), "You", "Notes", "Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "ArrowDown" });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual([TOMORROW]);
    expect(editor.getText()).toBe("2026-10-08 ");
  });

  it("is found by at, person and page, and a page chosen from it is a chip", async () => {
    const app = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("") }, { title: "Target page" }] }]);
    const editor = await openPage(app.user, "Notes");
    const { page } = await idsOf(app.memory);
    placeCursor(editor, 1);
    editor.commands.insertContent("/person");
    const slash = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(titlesOf(slash)[0]).toBe(t("slash.mention"));
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    await findMenu();
    editor.commands.insertContent("targ");
    await waitFor(() => {
      expect(titlesOf(screen.getByRole("listbox", { name: t("mention.label") }))).toEqual(["Target page"]);
    });
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(refs(editor)).toEqual([page("Target page")]);
  });
});

describe("the other triggers", () => {
  it("leaves [[ to the page picker and / to the block menu", async () => {
    const { editor } = await typeAt("[[");
    await screen.findByRole("listbox", { name: t("picker.label") });
    expect(menuIsOpen()).toBe(false);
    fireEvent.keyDown(editor.view.dom, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("listbox", { name: t("picker.label") })).not.toBeInTheDocument();
    });

    editor.commands.deleteRange({ from: 5, to: editor.state.doc.firstChild!.nodeSize - 1 });
    editor.commands.insertContent("/");
    await screen.findByRole("listbox", { name: t("slash.label") });
    expect(menuIsOpen()).toBe(false);
  });

  it("inserts a day after a mention just made: the menu works next to a chip", async () => {
    fakeToday();
    const { editor } = await typeAt("@");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual([TODAY]);
    editor.commands.insertContent("@tomorrow");
    await findMenu();
    fireEvent.keyDown(editor.view.dom, { key: "Enter" });
    expect(mentions(editor)).toEqual([TODAY, TOMORROW]);
    expect(addDays(TODAY, 1)).toBe(TOMORROW);
  });
});
