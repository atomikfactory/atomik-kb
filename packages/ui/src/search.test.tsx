// The search dialog: Ctrl+K or the sidebar's button, results grouped by page with the
// matches highlighted, keyboard navigation, and opening a result in its space at its block.

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { highlighted } from "./components/SearchDialog";
import { t } from "./i18n";
import { paragraphs, pid } from "./test/editor";
import { findEditor, renderPages } from "./test/page";
import { sidebarButton } from "./test/sidebar";

afterEach(() => {
  vi.restoreAllMocks();
});

const SPACES = [
  {
    name: "Personal",
    pages: [
      { title: "Journal", content: paragraphs("Planning the roadmap for next year", "Unrelated") },
      { title: "Groceries", content: paragraphs("Milk") },
    ],
  },
  { name: "Work", pages: [{ title: "Roadmap", icon: "🗺️", content: paragraphs("Milestones") }] },
];

function dialog(): HTMLElement {
  return screen.getByRole("dialog", { name: t("search.label") });
}

describe("search", () => {
  it("opens with Ctrl+K, finds titles and text with highlights, and opens a result at its block", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const { user } = await renderPages(SPACES);
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const input = within(dialog()).getByRole("combobox", { name: t("search.label") });
    expect(input).toHaveFocus();
    expect(within(dialog()).getByText(t("search.hint"))).toBeInTheDocument();

    await user.type(input, "roadmap");
    const results = within(dialog()).getByRole("listbox", { name: t("search.results") });
    await waitFor(() => {
      expect(within(results).getAllByRole("option")).toHaveLength(2);
    });
    const [title, block] = within(results).getAllByRole("option");
    // Grouped by page: the title hit in Roadmap's group, the text hit in Journal's.
    expect(within(results).getByRole("group", { name: "Roadmap" })).toContainElement(title ?? null);
    expect(within(results).getByRole("group", { name: "Journal" })).toContainElement(block ?? null);
    expect(title?.querySelector("mark")).toHaveTextContent(/^Roadmap$/);
    expect(block).toHaveTextContent("Planning the roadmap for next year");
    expect(block?.querySelector("mark")).toHaveTextContent(/^roadmap$/);
    expect(title).toHaveAttribute("aria-selected", "true");

    await user.keyboard("{ArrowDown}");
    expect(block).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", block?.id);
    await user.keyboard("{Enter}");

    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    const editor = await findEditor();
    await waitFor(() => {
      expect(editor.view.dom.querySelector(`[data-id='${pid(0)}']`)).toHaveClass("is-target");
    });
    expect(scrolled).toHaveBeenCalled();
  });

  it("opens a result in another space with a click", async () => {
    const { user } = await renderPages(SPACES);
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "road");
    await user.click(await within(dialog()).findByText("Roadmap", { selector: "mark" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    expect(screen.getByRole("treeitem", { name: "Roadmap" })).toHaveAttribute("aria-selected", "true");
  });

  it("says when nothing matches, and closes with Escape", async () => {
    const { user } = await renderPages(SPACES);
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "zebra");
    expect(await within(dialog()).findByText(t("search.none", { query: "zebra" }))).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
  });

  it("asks the core once the typing pauses, not on every keystroke", async () => {
    const { user, memory } = await renderPages(SPACES);
    const dispatch = vi.spyOn(memory, "dispatch");
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "mile");
    await waitFor(() => {
      expect(within(dialog()).getAllByRole("option")).toHaveLength(1);
    });
    const searches = dispatch.mock.calls.filter(([request]) => request.type === "Search");
    expect(searches.length).toBeLessThan(4);
    expect(searches.at(-1)?.[0]).toEqual({ type: "Search", data: { query: "mile", limit: 30 } });
  });
});

describe("the Space, Edited and Titles-only chips", () => {
  const DAY = 24 * 60 * 60 * 1000;

  function chips(): HTMLElement {
    return within(dialog()).getByRole("group", { name: t("search.filters") });
  }

  function titlesOfResults(): string[] {
    return within(dialog())
      .queryAllByRole("group")
      .filter((group) => group.classList.contains("search-group"))
      .map((group) => group.getAttribute("aria-label") ?? "");
  }

  function searches(dispatch: { mock: { calls: unknown[][] } }): { data: Record<string, unknown> }[] {
    return dispatch.mock.calls
      .map(([request]) => request as { type: string; data: Record<string, unknown> })
      .filter((request) => request.type === "Search");
  }

  it("show their defaults, in the order Space, Edited, Titles only", async () => {
    const { user } = await renderPages(SPACES);
    await user.click(sidebarButton(t("search.open")));
    const buttons = within(chips()).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      t("search.filter.tag"),
      `${t("search.filter.space")}: ${t("search.filter.space.all")}`,
      `${t("search.filter.edited")}: ${t("search.filter.edited.any")}`,
      t("search.filter.titlesOnly"),
    ]);
    expect(within(chips()).getByRole("button", { name: t("search.filter.titlesOnly") })).toHaveAttribute("aria-pressed", "false");
  });

  it("limit the results to the open space with the Space chip, and ask the core for that space", async () => {
    const { user, memory } = await renderPages(SPACES);
    const dispatch = vi.spyOn(memory, "dispatch");
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "road");
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });

    await user.click(within(chips()).getByRole("button", { name: /^Space/ }));
    const list = within(dialog()).getByRole("group", { name: t("search.filter.spaceList") });
    expect(within(list).getAllByRole("button").map((button) => button.textContent)).toEqual([
      t("search.filter.space.all"),
      t("search.filter.space.current"),
    ]);
    expect(within(list).getByRole("button", { name: t("search.filter.space.all") })).toHaveAttribute("aria-pressed", "true");
    await user.click(within(list).getByRole("button", { name: t("search.filter.space.current") }));

    // The list closes, focus is back in the field, the chip says what it does, and Roadmap (in Work) is gone.
    expect(within(dialog()).queryByRole("group", { name: t("search.filter.spaceList") })).not.toBeInTheDocument();
    expect(within(dialog()).getByRole("combobox")).toHaveFocus();
    expect(within(chips()).getByRole("button", { name: `${t("search.filter.space")}: ${t("search.filter.space.current")}` })).toHaveClass("is-active");
    await waitFor(() => {
      expect(titlesOfResults()).toEqual(["Journal"]);
    });
    const sent = searches(dispatch).at(-1)?.data;
    expect(sent).toMatchObject({ query: "road", limit: 30 });
    expect(typeof sent?.spaceId).toBe("string");
    expect(sent).not.toHaveProperty("editedAfter");

    // Back to every space.
    await user.click(within(chips()).getByRole("button", { name: /^Space/ }));
    await user.click(within(dialog()).getByRole("button", { name: t("search.filter.space.all") }));
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });
    expect(searches(dispatch).at(-1)?.data).toEqual({ query: "road", limit: 30 });
  });

  it("limit the results to the pages edited lately with the Edited chip", async () => {
    // The pages were edited just now (the test clock is a fixed day by default).
    const { user, memory } = await renderPages(SPACES, { now: () => Date.now() });
    const dispatch = vi.spyOn(memory, "dispatch");
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "road");
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });
    await user.click(within(chips()).getByRole("button", { name: /^Edited/ }));
    const list = within(dialog()).getByRole("group", { name: t("search.filter.editedList") });
    expect(within(list).getAllByRole("button").map((button) => button.textContent)).toEqual([
      t("search.filter.edited.any"),
      t("search.filter.edited.today"),
      t("search.filter.edited.week"),
      t("search.filter.edited.month"),
    ]);

    // The pages were made just now: they pass "Today" and "Past 7 days".
    await user.click(within(list).getByRole("button", { name: t("search.filter.edited.today") }));
    await waitFor(() => {
      expect(searches(dispatch).at(-1)?.data.editedAfter).toBeTypeOf("number");
    });
    const midnight = new Date();
    midnight.setHours(0, 0, 0, 0);
    expect(searches(dispatch).at(-1)?.data.editedAfter).toBe(midnight.getTime());
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });

    // Forty days later they are not: "Past 30 days" finds nothing, and the chip is the way back.
    vi.useFakeTimers({ toFake: ["Date"], now: Date.now() + 40 * DAY });
    try {
      await user.click(within(chips()).getByRole("button", { name: /^Edited/ }));
      await user.click(within(dialog()).getByRole("button", { name: t("search.filter.edited.month") }));
      expect(await within(dialog()).findByText(t("search.none", { query: "road" }))).toBeInTheDocument();
      expect(titlesOfResults()).toEqual([]);
      expect(within(chips()).getByRole("button", { name: `${t("search.filter.edited")}: ${t("search.filter.edited.month")}` })).toHaveClass("is-active");

      await user.click(within(chips()).getByRole("button", { name: /^Edited/ }));
      await user.click(within(dialog()).getByRole("button", { name: t("search.filter.edited.any") }));
      await waitFor(() => {
        expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("search titles only with the toggle, which is pressed while it is on", async () => {
    const { user, memory } = await renderPages(SPACES);
    const dispatch = vi.spyOn(memory, "dispatch");
    await user.click(sidebarButton(t("search.open")));
    await user.type(within(dialog()).getByRole("combobox"), "road");
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });
    const toggle = within(chips()).getByRole("button", { name: t("search.filter.titlesOnly") });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(toggle).toHaveClass("is-active");
    // Journal only has the word in its text.
    await waitFor(() => {
      expect(titlesOfResults()).toEqual(["Roadmap"]);
    });
    expect(searches(dispatch).at(-1)?.data).toEqual({ query: "road", limit: 30, scope: "Titles" });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    await waitFor(() => {
      expect(titlesOfResults().sort()).toEqual(["Journal", "Roadmap"]);
    });
  });

  it("work with the keyboard: Tab to a chip, Enter opens its list, Escape closes the list and not the dialog", async () => {
    const { user } = await renderPages(SPACES);
    await user.click(sidebarButton(t("search.open")));
    const input = within(dialog()).getByRole("combobox");
    await user.type(input, "road");
    const space = within(chips()).getByRole("button", { name: /^Space/ });
    expect(space).toHaveAttribute("aria-haspopup", "true");
    expect(space).toHaveAttribute("aria-expanded", "false");
    space.focus();
    await user.keyboard("{Enter}");
    expect(space).toHaveAttribute("aria-expanded", "true");
    const list = within(dialog()).getByRole("group", { name: t("search.filter.spaceList") });
    // Escape closes the list, not the dialog, and the field has the focus again.
    await user.keyboard("{Escape}");
    expect(within(dialog()).queryByRole("group", { name: t("search.filter.spaceList") })).not.toBeInTheDocument();
    expect(dialog()).toBeInTheDocument();
    expect(input).toHaveFocus();
    expect(list).not.toBeInTheDocument();

    // The choices are buttons: Enter on one picks it.
    space.focus();
    await user.keyboard("{Enter}");
    within(dialog()).getByRole("button", { name: t("search.filter.space.current") }).focus();
    await user.keyboard("{Enter}");
    expect(within(dialog()).queryByRole("group", { name: t("search.filter.spaceList") })).not.toBeInTheDocument();
    expect(input).toHaveFocus();
    expect(space).toHaveClass("is-active");

    // Another chip's list replaces the open one.
    space.focus();
    await user.keyboard("{Enter}");
    const edited = within(chips()).getByRole("button", { name: /^Edited/ });
    await user.click(edited);
    expect(edited).toHaveAttribute("aria-expanded", "true");
    expect(space).toHaveAttribute("aria-expanded", "false");
    expect(within(dialog()).queryByRole("group", { name: t("search.filter.spaceList") })).not.toBeInTheDocument();
    expect(within(dialog()).getByRole("group", { name: t("search.filter.editedList") })).toBeInTheDocument();
    await user.keyboard("{Escape}");

    // The toggle answers Space and Enter.
    const toggle = within(chips()).getByRole("button", { name: t("search.filter.titlesOnly") });
    toggle.focus();
    await user.keyboard("{Enter}");
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    await user.keyboard(" ");
    expect(toggle).toHaveAttribute("aria-pressed", "false");
  });

  it("start again from the defaults the next time the search opens", async () => {
    const { user } = await renderPages(SPACES);
    await user.click(sidebarButton(t("search.open")));
    await user.click(within(chips()).getByRole("button", { name: t("search.filter.titlesOnly") }));
    await user.click(within(chips()).getByRole("button", { name: /^Space/ }));
    await user.click(within(dialog()).getByRole("button", { name: t("search.filter.space.current") }));
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    await user.click(sidebarButton(t("search.open")));
    expect(within(chips()).getByRole("button", { name: t("search.filter.titlesOnly") })).toHaveAttribute("aria-pressed", "false");
    expect(within(chips()).getByRole("button", { name: `${t("search.filter.space")}: ${t("search.filter.space.all")}` })).toBeInTheDocument();
  });
});

describe("highlighted snippets", () => {
  it("wraps UTF-16 ranges in <mark>, astral characters included", () => {
    const parts = highlighted("a 𝔘x b", [{ start: 2, end: 5 }]);
    expect(parts[0]).toBe("a ");
    expect(parts[2]).toBe(" b");
    const mark = parts[1] as { props: { children: string } };
    expect(mark.props.children).toBe("𝔘x");
  });

  it("ignores ranges outside the text and overlapping ones", () => {
    expect(highlighted("abc", [{ start: 2, end: 9 }])).toEqual(["abc"]);
    const parts = highlighted("abcdef", [
      { start: 0, end: 3 },
      { start: 1, end: 2 },
    ]);
    expect(parts).toHaveLength(2);
  });
});
