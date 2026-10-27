// "Duplicate" (ADR-0040) in the sidebar row menu and the page menu: the copy of a page with its sub-pages lands right
// after it, is opened in the active tab like a page just made, and a refusal shows the core's reason. The copy itself
// (ids, content, values, links) is tested in the core and its mirror; here the whole flow runs over the in-memory core.

import { CoreClient, CoreError } from "@atomik-kb/core-client";
import type { MemoryTransport } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { renderInterop } from "./test/interop";
import { idsOf } from "./test/page";
import type { User } from "./test/render";
import { PERSONAL, PERSONAL_OUTLINE, WORK, outline, row } from "./test/sidebar";

async function setup(spaces = [PERSONAL, WORK], options: Parameters<typeof renderInterop>[1] = {}) {
  const app = await renderInterop({ spaces }, options);
  return app;
}

/** Right-clicks a row and chooses an item of its menu. */
async function chooseFromRow(user: User, title: string, item: string): Promise<void> {
  await user.pointer({ keys: "[MouseRight]", target: row(title) });
  const menu = await screen.findByRole("menu", { name: t("tree.actions") });
  await user.click(within(menu).getByRole("menuitem", { name: item }));
}

/** Opens the page menu of the open page and chooses an item. */
async function chooseFromPage(user: User, item: string): Promise<void> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  const menu = await screen.findByRole("menu", { name: t("page.actions") });
  await user.click(within(menu).getByRole("menuitem", { name: item }));
}

/** The id of the live page of the current space with this title that is not `original`. */
async function copyOf(memory: MemoryTransport, title: string, original: string): Promise<string> {
  const client = new CoreClient(memory);
  const [space] = await client.spaceList();
  const view = await client.treeGet(space?.id ?? "");
  const found: string[] = [];
  const walk = (nodes: typeof view.roots): void => {
    for (const node of nodes) {
      if (node.page.title === title && node.page.id !== original) {
        found.push(node.page.id);
      }
      walk(node.children);
    }
  };
  walk(view.roots);
  return found[0] ?? "";
}

describe("Duplicate in the sidebar row menu", () => {
  it("copies a page with its sub-pages right after it and opens the copy in the active tab", async () => {
    const { user, memory, store, requests } = await setup();
    const ids = await idsOf(memory);
    await chooseFromRow(user, "Projects", t("page.duplicate"));

    await waitFor(() => {
      expect(outline()).toEqual([
        "Projects",
        "  Roadmap",
        "    Q1",
        "  Ideas",
        "Projects (copy)",
        "  Roadmap",
        "    Q1",
        "  Ideas",
        "Journal",
        "Reading list",
      ]);
    });
    expect(requests).toContainEqual({ type: "PageDuplicate", data: { pageId: ids.page("Projects") } });
    const copy = await copyOf(memory, "Projects (copy)", ids.page("Projects"));
    expect(copy).not.toBe("");
    // Opened like a page just made: the active tab navigates, nothing else opens.
    expect(store.getState().selectedPageId).toBe(copy);
    expect(store.getState().tabs).toHaveLength(1);
    expect(row("Projects (copy)")).toHaveAttribute("aria-selected", "true");
    expect(row("Projects")).toHaveAttribute("aria-selected", "false");
    expect(await screen.findByRole("heading", { level: 1, name: "Projects (copy)" })).toBeInTheDocument();
    // The menu closed.
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("offers it between Outdent and Move to…, also when there is no other space to move to", async () => {
    const { user } = await setup([PERSONAL]);
    await user.pointer({ keys: "[MouseRight]", target: row("Journal") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    const labels = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);
    expect(labels).toEqual([
      t("page.openInNewTab"),
      t("tree.menu.moveUp"),
      t("tree.menu.moveDown"),
      t("tree.menu.indent"),
      t("tree.menu.outdent"),
      t("tree.menu.sortTitle"),
      t("tree.menu.sortCreated"),
      t("page.duplicate"),
      t("tree.menu.rename"),
      t("tree.menu.setIcon"),
      t("favorites.add"),
      t("template.newSubPageFrom"),
      t("template.turnInto"),
      t("tree.menu.trash"),
    ]);
    expect(within(menu).getByRole("menuitem", { name: t("page.duplicate") })).toBeEnabled();
  });

  it("copies a page of a nested level among its own siblings", async () => {
    const { user } = await setup();
    await chooseFromRow(user, "Roadmap", t("page.duplicate"));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Roadmap (copy)", "    Q1", "  Ideas", "Journal", "Reading list"]);
    });
  });

  it("shows the core's reason when it refuses, and changes nothing", async () => {
    const { user, memory } = await setup([PERSONAL, WORK], {
      intercept: (request) => {
        if (request.type === "PageDuplicate") {
          throw new CoreError({ code: "InvalidInput", message: "the page is in the trash", details: { field: "pageId" } });
        }
        return undefined;
      },
    });
    const ids = await idsOf(memory);
    await chooseFromRow(user, "Journal", t("page.duplicate"));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(t("page.duplicate.error", { message: "the page is in the trash" }));
    expect(outline()).toEqual(PERSONAL_OUTLINE);
    expect(await copyOf(memory, "Journal (copy)", ids.page("Journal"))).toBe("");
    await user.click(within(alert).getByRole("button", { name: t("sidebar.dismissError") }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("is one structural undo step", async () => {
    const { user, store } = await setup();
    await chooseFromRow(user, "Projects", t("page.duplicate"));
    await waitFor(() => {
      expect(outline()).toHaveLength(10);
    });
    await store.getState().undo();
    await waitFor(() => {
      expect(outline()).toEqual(PERSONAL_OUTLINE);
    });
    await store.getState().redo();
    await waitFor(() => {
      expect(outline()).toHaveLength(10);
    });
  });
});

describe("Duplicate in the page menu", () => {
  it("copies the open page and opens the copy in the same tab", async () => {
    const { user, memory, store, requests } = await setup();
    const ids = await idsOf(memory);
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });

    await chooseFromPage(user, t("page.duplicate"));

    expect(await screen.findByRole("heading", { level: 1, name: "Journal (copy)" })).toBeInTheDocument();
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal", "Journal (copy)", "Reading list"]);
    expect(requests).toContainEqual({ type: "PageDuplicate", data: { pageId: ids.page("Journal") } });
    const copy = await copyOf(memory, "Journal (copy)", ids.page("Journal"));
    expect(store.getState().selectedPageId).toBe(copy);
    expect(store.getState().tabs).toHaveLength(1);
    expect(row("Journal (copy)")).toHaveAttribute("aria-selected", "true");
  });

  it("sits before Move to… and Page info in the menu", async () => {
    const { user } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const menu = await screen.findByRole("menu", { name: t("page.actions") });
    const labels = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent ?? "");
    const at = (label: string): number => labels.findIndex((text) => text.startsWith(label));
    expect(at(t("page.duplicate"))).toBeGreaterThan(-1);
    expect(at(t("page.duplicate"))).toBeLessThan(at(t("moveToSpace.menu")));
    expect(at(t("moveToSpace.menu"))).toBeLessThan(at(t("page.info")));
  });

  it("expands the collapsed parent of the copy, so the copy shows in the tree", async () => {
    const { user } = await setup();
    await user.click(row("Roadmap"));
    await screen.findByRole("heading", { level: 1, name: "Roadmap" });
    await user.click(within(row("Projects")).getByRole("button", { name: t("tree.collapse") }));
    expect(outline()).toEqual(["Projects", "Journal", "Reading list"]);

    await chooseFromPage(user, t("page.duplicate"));

    expect(await screen.findByRole("heading", { level: 1, name: "Roadmap (copy)" })).toBeInTheDocument();
    await waitFor(() => {
      expect(row("Projects")).toHaveAttribute("aria-expanded", "true");
    });
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Roadmap (copy)", "    Q1", "  Ideas", "Journal", "Reading list"]);
  });

  it("says why the page could not be duplicated, in the page's notice", async () => {
    const { user } = await setup([PERSONAL, WORK], {
      intercept: (request) => {
        if (request.type === "PageDuplicate") {
          throw new CoreError({ code: "Storage", message: "database error: disk I/O error" });
        }
        return undefined;
      },
    });
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await chooseFromPage(user, t("page.duplicate"));
    expect(await screen.findByText(t("page.duplicate.error", { message: "database error: disk I/O error" }))).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    expect(outline()).toEqual(PERSONAL_OUTLINE);
  });
});
