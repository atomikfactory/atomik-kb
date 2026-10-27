// The path in the page header (ADR-0040): the space, then the ancestors, above the title; nothing for a
// top-level page; each ancestor opens its page; it follows moves and renames; it does not print.

import type { JsonValue } from "@atomik-kb/core-client";
import { CoreClient } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { paragraphs } from "./test/editor";
import { idsOf, openPage, renderPages } from "./test/page";
import { row } from "./test/sidebar";

const BODY: JsonValue = paragraphs("x");

const SPACES = [
  {
    name: "Personal",
    pages: [
      { title: "Projects", icon: "🧭", children: [{ title: "atomik-kb", children: [{ title: "Roadmap", content: BODY }] }] },
      { title: "Notes", content: BODY },
    ],
  },
];

/** The path of the open page, or `null` when the header shows none. */
function pagePath(): HTMLElement | null {
  return screen.queryByRole("navigation", { name: t("pagePath.label") });
}

describe("the page header path", () => {
  it("shows the space and the ancestors above the title, and not the page itself", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Roadmap");
    const path = await screen.findByRole("navigation", { name: t("pagePath.label") });
    expect(path.closest(".page-header")).not.toBeNull();
    const items = within(path)
      .getAllByRole("listitem")
      .map((item) => item.textContent);
    expect(items).toEqual(["Personal", "🧭Projects", "atomik-kb"]);
    expect(within(path).queryByText("Roadmap")).not.toBeInTheDocument();
    // Ancestors are buttons, the space is not.
    expect(within(path).getAllByRole("button").map((button) => button.textContent)).toEqual(["🧭Projects", "atomik-kb"]);
    // Above the title, in reading order.
    const title = screen.getByRole("heading", { level: 1, name: "Roadmap" });
    expect(path.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is not shown for a top-level page, whatever else is open", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Notes");
    expect(await screen.findByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
    expect(pagePath()).toBeNull();
    // The same page link of the editor's own breadcrumb block is another landmark, with its own name.
    expect(screen.queryByRole("navigation", { name: t("breadcrumb.label") })).not.toBeInTheDocument();
  });

  it("opens the page of a crumb, and hides when that page is a top-level one", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Roadmap");
    const path = await screen.findByRole("navigation", { name: t("pagePath.label") });
    await user.click(within(path).getByRole("button", { name: "atomik-kb" }));
    expect(await screen.findByRole("heading", { level: 1, name: "atomik-kb" })).toBeInTheDocument();
    expect(within(await screen.findByRole("navigation", { name: t("pagePath.label") })).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "Personal",
      "🧭Projects",
    ]);
    await user.click(within(pagePath() as HTMLElement).getByRole("button", { name: "Projects" }));
    expect(await screen.findByRole("heading", { level: 1, name: /Projects/ })).toBeInTheDocument();
    expect(pagePath()).toBeNull();
  });

  it("follows a rename and a move of the pages above, and leaves the page when it is moved to the top", async () => {
    const { user, memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    await openPage(user, "Roadmap");
    const client = new CoreClient(memory);
    await act(async () => {
      await client.pageRename(ids.page("atomik-kb"), "Product");
    });
    await waitFor(() => {
      expect(within(pagePath() as HTMLElement).getByRole("button", { name: "Product" })).toBeInTheDocument();
    });
    await act(async () => {
      await client.pageMove(ids.page("Roadmap"), null, 0);
    });
    await waitFor(() => {
      expect(pagePath()).toBeNull();
    });
    expect(screen.getByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    expect(row("Roadmap")).toBeInTheDocument();
  });
});
