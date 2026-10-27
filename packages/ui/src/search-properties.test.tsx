// Search and properties (ADR-0026): a search for a tag name finds the page, the "Tag" filter chip
// limits the results to the pages that have a tag (in the current space), and a tag of the
// properties panel opens the search already limited to it.

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { paragraphs } from "./test/editor";
import { openProperties, renderProperties } from "./test/properties";
import { sidebarButton } from "./test/sidebar";

const SPACES = [
  {
    name: "Personal",
    tagOptions: [
      { name: "idea", color: "green" as const },
      { name: "project", color: "blue" as const },
      { name: "orphan", color: "red" as const },
    ],
    pages: [
      { title: "Notes", tags: ["idea"], content: paragraphs("Plans for the garden") },
      { title: "Garden", tags: ["idea", "project"], content: paragraphs("Roses and plans") },
      { title: "Journal", content: paragraphs("Plans for tomorrow") },
    ],
  },
  { name: "Work", pages: [{ title: "Meetings", tags: ["client"], content: paragraphs("Plans for the quarter") }] },
];

function dialog(): HTMLElement {
  return screen.getByRole("dialog", { name: t("search.label") });
}

async function openSearch(user: Awaited<ReturnType<typeof renderProperties>>["user"]): Promise<HTMLElement> {
  await user.click(sidebarButton(t("search.open")));
  return dialog();
}

function titles(): string[] {
  return within(dialog())
    .queryAllByRole("group")
    .filter((group) => group.classList.contains("search-group"))
    .map((group) => group.getAttribute("aria-label") ?? "");
}

describe("searching for a tag name", () => {
  it("finds the pages that have the tag, as a hit on the page with the tag as its text", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    await user.type(within(search).getByRole("combobox", { name: t("search.label") }), "project");
    const results = within(search).getByRole("listbox", { name: t("search.results") });
    await waitFor(() => {
      expect(within(results).getAllByRole("option")).toHaveLength(1);
    });
    const hit = within(results).getByRole("option");
    expect(within(results).getByRole("group", { name: "Garden" })).toContainElement(hit);
    expect(hit.querySelector("mark")).toHaveTextContent(/^project$/);
    // A page-level hit: it opens the page without a block to scroll to.
    await user.click(hit);
    expect(await screen.findByRole("heading", { level: 1, name: "Garden" })).toBeInTheDocument();
  });
});

describe("the Tag filter", () => {
  it("lists the tags of the current space, and limits the results to the pages that have the one picked", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    const chips = within(search).getByRole("group", { name: t("search.filters") });
    await user.click(within(chips).getByRole("button", { name: t("search.filter.tag") }));
    const tags = within(search).getByRole("group", { name: t("search.filter.tagList") });
    // The current space's tags only: "client" is in the other space.
    expect(within(tags).getAllByRole("button").map((button) => button.textContent)).toEqual(["idea", "project", "orphan"]);

    await user.click(within(tags).getByRole("button", { name: "project" }));
    expect(within(search).queryByRole("group", { name: t("search.filter.tagList") })).not.toBeInTheDocument();
    expect(within(chips).getByText("project")).toBeInTheDocument();
    expect(within(chips).queryByRole("button", { name: t("search.filter.tag") })).not.toBeInTheDocument();
    // Nothing typed: the pages that have the tag.
    await waitFor(() => {
      expect(titles()).toEqual(["Garden"]);
    });
    expect(within(search).getByRole("combobox", { name: t("search.label") })).toHaveFocus();
  });

  it("with text typed, searches among those pages only", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    await user.click(within(search).getByRole("button", { name: "idea" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Garden", "Notes"]);
    });

    const input = within(search).getByRole("combobox", { name: t("search.label") });
    await user.type(input, "tomorrow");
    // "Journal" matches the text but does not have the tag.
    expect(await within(search).findByText(t("search.none", { query: "tomorrow" }))).toBeInTheDocument();
    expect(titles()).toEqual([]);

    await user.clear(input);
    await user.type(input, "roses");
    await waitFor(() => {
      expect(titles()).toEqual(["Garden"]);
    });
  });

  it("is lifted with the chip's remove button, which brings back the plain search", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    await user.click(within(search).getByRole("button", { name: "idea" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Garden", "Notes"]);
    });
    await user.click(within(search).getByRole("button", { name: t("search.filter.remove", { name: "idea" }) }));
    expect(await within(search).findByText(t("search.hint"))).toBeInTheDocument();
    expect(titles()).toEqual([]);
    expect(within(search).getByRole("button", { name: t("search.filter.tag") })).toBeInTheDocument();

    const input = within(search).getByRole("combobox", { name: t("search.label") });
    await user.type(input, "plans");
    await waitFor(() => {
      expect(titles().sort()).toEqual(["Garden", "Journal", "Meetings", "Notes"]);
    });
  });

  it("says when no page has the tag, and Enter opens the first result", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    await user.click(within(search).getByRole("button", { name: "orphan" }));
    expect(await within(search).findByText(t("search.noTagged", { tag: "orphan" }))).toBeInTheDocument();
    await user.click(within(search).getByRole("button", { name: t("search.filter.remove", { name: "orphan" }) }));
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    await user.click(within(search).getByRole("button", { name: "project" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Garden"]);
    });
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: "Garden" })).toBeInTheDocument();
  });

  it("lists the tags of the space that is open, and says when there are none", async () => {
    const { user } = await renderProperties(SPACES);
    await user.click(screen.getByRole("button", { name: /Personal/ }));
    await user.click(await screen.findByRole("button", { name: /Work/ }));
    await screen.findByRole("treeitem", { name: "Meetings" });
    let search = await openSearch(user);
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    // "client" came from a page's `tags`, so Work lists it.
    expect(within(within(search).getByRole("group", { name: t("search.filter.tagList") })).getAllByRole("button").map((button) => button.textContent)).toEqual(["client"]);
    await user.keyboard("{Escape}");
    // Escape closed the list, not the dialog.
    expect(screen.getByRole("dialog", { name: t("search.label") })).toBeInTheDocument();
    expect(within(search).queryByRole("group", { name: t("search.filter.tagList") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    search = await openSearch(user);
    expect(within(search).getByRole("button", { name: t("search.filter.tag") })).toBeInTheDocument();
  });

  it("reaches the chips with Tab and keeps Up, Down and Enter for the results while the field has focus", async () => {
    const { user } = await renderProperties(SPACES);
    const search = await openSearch(user);
    const input = within(search).getByRole("combobox", { name: t("search.label") });
    expect(input).toHaveFocus();
    await user.tab();
    expect(within(search).getByRole("button", { name: t("search.filter.tag") })).toHaveFocus();
    // The Space, Edited and Titles-only chips (ADR-0040) follow the Tag chip.
    await user.tab();
    expect(within(search).getByRole("button", { name: /^Space/ })).toHaveFocus();
    await user.tab();
    expect(within(search).getByRole("button", { name: /^Edited/ })).toHaveFocus();
    await user.tab();
    expect(within(search).getByRole("button", { name: t("search.filter.titlesOnly") })).toHaveFocus();
    await user.tab();
    // Focus wraps around inside the dialog.
    expect(input).toHaveFocus();
    await user.tab({ shift: true });
    expect(within(search).getByRole("button", { name: t("search.filter.titlesOnly") })).toHaveFocus();
    await user.tab({ shift: true });
    await user.tab({ shift: true });
    await user.tab({ shift: true });
    expect(within(search).getByRole("button", { name: t("search.filter.tag") })).toHaveFocus();
    // Enter on the chip opens the list; it does not open a result.
    await user.keyboard("{Enter}");
    expect(within(search).getByRole("group", { name: t("search.filter.tagList") })).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: t("search.label") })).toBeInTheDocument();
    fireEvent.keyDown(search, { key: "Escape" });
  });
});

describe("a tag of the properties panel", () => {
  it("opens the search limited to that tag, and the pages that have it", async () => {
    const { user } = await renderProperties(SPACES);
    const panel = await openProperties(user, "Garden");
    await user.click(within(panel).getByRole("button", { name: t("properties.tag.search", { name: "project" }) }));
    const search = dialog();
    expect(within(search).getByRole("group", { name: t("search.filters") })).toHaveTextContent("project");
    await waitFor(() => {
      expect(titles()).toEqual(["Garden"]);
    });

    await user.click(within(search).getByRole("button", { name: t("search.filter.remove", { name: "project" }) }));
    await user.click(within(search).getByRole("button", { name: t("search.filter.tag") }));
    await user.click(within(search).getByRole("button", { name: "idea" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Garden", "Notes"]);
    });
    await user.click(within(dialog()).getByRole("option", { name: /Notes/ }));
    expect(await screen.findByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
  });
});
