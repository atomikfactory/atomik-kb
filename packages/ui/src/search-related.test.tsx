// Ctrl+K and the "Related" group (ADR-0037, components/SearchDialog.tsx): with AI on and an index that has chunks, a
// query of three characters or more also asks for related pages, shown under the matches, one row per page, without
// the pages the words already found; keyboard and mouse work as for any result; a failure is silent.

import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { AI_ON, AI_SETTING, AI_SPACES, aiOf } from "./test/ai";
import { renderPages } from "./test/page";

afterEach(() => {
  vi.restoreAllMocks();
});

async function render(options: Parameters<typeof renderPages>[1] = {}, { indexed = true }: { indexed?: boolean } = {}) {
  const app = await renderPages(AI_SPACES, { ai: { mode: "fake" }, settings: { [AI_SETTING]: AI_ON }, ...options });
  if (indexed && app.memory.ai !== undefined) {
    await aiOf(app.memory).advanceIndex();
  }
  return app;
}

const dialog = () => screen.getByRole("dialog", { name: t("search.label") });
const searches = (app: Awaited<ReturnType<typeof render>>) => aiOf(app.memory).calls.filter((call) => call.kind === "search");

async function search(app: Awaited<ReturnType<typeof render>>, query: string) {
  fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
  const input = within(dialog()).getByRole("combobox", { name: t("search.label") });
  await app.user.type(input, query);
  return input;
}

describe("the Related group", () => {
  it("shows pages the words did not find, under the matches, one row per page with a passage", async () => {
    const app = await render();
    await search(app, "tomato soup");
    const group = await within(dialog()).findByRole("group", { name: t("search.related") });
    // Cooking has both words, so the full-text search found it; the others are related.
    expect(within(dialog()).getByRole("group", { name: "Cooking" })).toBeInTheDocument();
    const rows = within(group).getAllByRole("option");
    expect(rows.map((row) => row.getAttribute("aria-label")).sort()).toEqual([
      t("search.related.row", { title: "Garden plans" }),
      t("search.related.row", { title: "Roadmap" }),
    ]);
    const garden = within(group).getByRole("option", { name: t("search.related.row", { title: "Garden plans" }) });
    expect(garden).toHaveTextContent("Tomatoes need full sun and regular watering.");
    expect(garden).toHaveTextContent("✦");
    // The group comes after the full-text results.
    const all = within(dialog()).getAllByRole("option");
    expect(all.at(-1)).toBe(rows.at(-1));
    expect(all[0]).not.toBe(rows[0]);
  });

  it("waits 300 ms after the last keystroke and asks for six passages from every space", async () => {
    const app = await render();
    await search(app, "tom");
    expect(searches(app)).toHaveLength(0);
    await waitFor(() => {
      expect(searches(app)).toHaveLength(1);
    });
    expect(searches(app)[0]).toEqual({ kind: "search", input: { query: "tom", scope: { type: "all" }, limit: 6 } });
  });

  it("does not ask for fewer than three characters", async () => {
    const app = await render();
    await search(app, "to");
    await new Promise((resolve) => setTimeout(resolve, 450));
    expect(searches(app)).toHaveLength(0);
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
  });

  it("is not there while AI is off, or without an AI", async () => {
    const off = await render({ settings: { [AI_SETTING]: { enabled: false } } });
    await search(off, "tomato soup");
    await within(dialog()).findByRole("group", { name: "Cooking" });
    await new Promise((resolve) => setTimeout(resolve, 450));
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
    expect(searches(off)).toHaveLength(0);
  });

  it("is not there while the index is empty", async () => {
    const app = await render({}, { indexed: false });
    await search(app, "tomato soup");
    await within(dialog()).findByRole("group", { name: "Cooking" });
    await new Promise((resolve) => setTimeout(resolve, 450));
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
    expect(searches(app)).toHaveLength(0);
  });

  it("is silent when the search fails", async () => {
    const app = await render();
    aiOf(app.memory).failNext({ code: "Internal", message: "boom" });
    await search(app, "tomato soup");
    await within(dialog()).findByRole("group", { name: "Cooking" });
    await waitFor(() => {
      expect(searches(app)).toHaveLength(1);
    });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(within(dialog()).queryByText(t("search.error"))).not.toBeInTheDocument();
  });

  it("leaves out a page the words already found", async () => {
    const app = await render();
    await search(app, "tomatoes");
    await within(dialog()).findByRole("group", { name: "Garden plans" });
    await waitFor(() => {
      expect(searches(app)).toHaveLength(1);
    });
    const related = within(dialog()).queryByRole("group", { name: t("search.related") });
    const titles = related === null ? [] : within(related).getAllByRole("option").map((row) => row.getAttribute("aria-label"));
    expect(titles).not.toContain(t("search.related.row", { title: "Garden plans" }));
    expect(titles).not.toContain(t("search.related.row", { title: "Cooking" }));
  });

  it("is not a reason to say there are no results, and does not hide that there are none", async () => {
    const app = await render();
    await search(app, "zebra");
    expect(await within(dialog()).findByText(t("search.none", { query: "zebra" }))).toBeInTheDocument();
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
  });
});

describe("opening a related page", () => {
  it("with the arrow keys and Enter: the next row after the matches is the first related page", async () => {
    const app = await render();
    const input = await search(app, "tomato soup");
    const group = await within(dialog()).findByRole("group", { name: t("search.related") });
    const options = within(dialog()).getAllByRole("option");
    const first = within(group).getAllByRole("option")[0] as HTMLElement;
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    await app.user.keyboard("{ArrowDown}");
    expect(first).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", first.id);
    const title = first.getAttribute("aria-label")?.replace(/^Related: /, "") ?? "";
    await app.user.keyboard("{Enter}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: title })).toBeInTheDocument();
  });

  it("with a click, in another space, scrolled to the passage", async () => {
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    const app = await render();
    await search(app, "tomato soup");
    const row = await within(dialog()).findByRole("option", { name: t("search.related.row", { title: "Roadmap" }) });
    await app.user.click(row);
    expect(await screen.findByRole("heading", { level: 1, name: "Roadmap" })).toBeInTheDocument();
    await waitFor(() => {
      expect(scrolled).toHaveBeenCalled();
    });
  });

  it("cycles through the matches and the related pages with the arrow keys", async () => {
    const app = await render();
    const input = await search(app, "tomato soup");
    await within(dialog()).findByRole("group", { name: t("search.related") });
    const count = within(dialog()).getAllByRole("option").length;
    expect(count).toBe(3);
    await app.user.keyboard("{ArrowUp}");
    expect(within(dialog()).getAllByRole("option").at(-1)).toHaveAttribute("aria-selected", "true");
    await app.user.keyboard("{ArrowDown}");
    expect(within(dialog()).getAllByRole("option")[0]).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveFocus();
  });
});

describe("closing the dialog", () => {
  it("forgets the related pages", async () => {
    const app = await render();
    await search(app, "tomato soup");
    await within(dialog()).findByRole("group", { name: t("search.related") });
    fireEvent.keyDown(dialog(), { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    });
    // Opened again: the old related pages are not there for the new, empty query.
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    expect(within(dialog()).queryByRole("group", { name: t("search.related") })).not.toBeInTheDocument();
  });
});
