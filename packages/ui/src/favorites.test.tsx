// The favourites of a space (ADR-0029): the section above the page tree, its rows (open, menu,
// reorder by menu and by drag), the star in the page header and the item in the tree's menu.

import { CoreClient } from "@atomik-kb/core-client";
import type { MemoryTransport, PageId, Request } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { FakeDataTransfer, beginDrag, dragAndDrop } from "./test/dnd";
import { idsOf } from "./test/page";
import { renderSidebar, row } from "./test/sidebar";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const COLLAPSED_KEY = "ui.sidebar.favorites.collapsed";

/** The favourites list of the sidebar; `null` while the section is absent or collapsed. */
function list(): HTMLElement | null {
  return screen.queryByRole("list", { name: t("favorites.title") });
}

const toggle = (): HTMLElement => screen.getByRole("button", { name: t("favorites.title"), expanded: true });

/** The favourites as titles, in order. */
function titles(): string[] {
  const element = list();
  if (element === null) {
    return [];
  }
  return within(element)
    .getAllByRole("listitem")
    .map((item) => item.querySelector(".tree-title")?.textContent ?? "");
}

/** The row of a favourite (a `li`), found by the page title. */
function favorite(title: string): HTMLElement {
  const element = list();
  if (element === null) {
    throw new Error("there is no favourites list");
  }
  const item = within(element)
    .getAllByRole("listitem")
    .find((candidate) => within(candidate).queryByRole("button", { name: title }) !== null);
  if (item === undefined) {
    throw new Error(`no favourite named ${title}`);
  }
  return item;
}

async function favorites(memory: MemoryTransport, spaceName = "Personal"): Promise<string[]> {
  const client = new CoreClient(memory);
  const { space } = await idsOf(memory);
  const tree = await client.treeGet(space(spaceName));
  const names: string[] = [];
  for (const id of tree.favorites ?? []) {
    names.push((await client.pageGet(id)).title);
  }
  return names;
}

/** Marks pages as favourites through the core, in this order. */
async function mark(memory: MemoryTransport, ...pageTitles: string[]): Promise<void> {
  const client = new CoreClient(memory);
  const { page } = await idsOf(memory);
  for (const title of pageTitles) {
    await act(async () => {
      await client.pageFavoriteSet(page(title), true);
    });
  }
}

async function openMenu(user: User, title: string): Promise<HTMLElement> {
  await user.click(within(favorite(title)).getByRole("button", { name: t("favorites.actions") }));
  return screen.findByRole("menu", { name: t("favorites.actions") });
}

describe("the favourites section", () => {
  it("is not there while the space has no favourite", async () => {
    await renderSidebar();
    expect(list()).toBeNull();
    expect(screen.queryByRole("button", { name: t("favorites.title") })).not.toBeInTheDocument();
  });

  it("lists the favourites in order above the tree, with the titles of the pages", async () => {
    const { memory } = await renderSidebar();
    await mark(memory, "Reading list", "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list", "Journal"]);
    });
    const section = list()?.parentElement;
    const tree = screen.getByRole("tree", { name: t("tree.label") });
    // The section comes first in the sidebar's scrolling area.
    expect(section?.compareDocumentPosition(tree)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(within(favorite("Journal")).getByRole("button", { name: "Journal" }).querySelector(".page-icon")).not.toBeNull();
  });

  it("shows an untitled page as Untitled and follows a rename", async () => {
    const { memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    const { page } = await idsOf(memory);
    await act(async () => {
      await new CoreClient(memory).pageRename(page("Journal"), "Diary");
    });
    await waitFor(() => {
      expect(titles()).toEqual(["Diary"]);
    });
    await act(async () => {
      await new CoreClient(memory).pageRename(page("Journal"), "");
    });
    await waitFor(() => {
      expect(titles()).toEqual([t("page.untitled")]);
    });
  });

  it("belongs to the space: another space has its own, and the first comes back with it", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    await user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    const popover = await screen.findByRole("dialog", { name: t("spaces.listLabel") });
    await user.click(within(popover).getByRole("button", { name: "Work" }));
    await screen.findByRole("treeitem", { name: "Meetings" });
    expect(list()).toBeNull();
    await user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    await user.click(within(await screen.findByRole("dialog", { name: t("spaces.listLabel") })).getByRole("button", { name: "Personal" }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
  });

  it("hides the favourite of a page in the trash, and brings it back with the restore", async () => {
    const { memory } = await renderSidebar();
    await mark(memory, "Journal", "Reading list");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list"]);
    });
    const { page } = await idsOf(memory);
    const client = new CoreClient(memory);
    await act(async () => {
      await client.pageTrash(page("Journal"));
    });
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list"]);
    });
    await act(async () => {
      await client.pageRestore(page("Journal"));
    });
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list"]);
    });
  });

  it("goes away with the last favourite", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    const menu = await openMenu(user, "Journal");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.remove") }));
    await waitFor(() => {
      expect(list()).toBeNull();
    });
    expect(await favorites(memory)).toEqual([]);
  });
});

describe("opening a favourite", () => {
  it("opens the page and highlights the row of the page that is open", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal", "Reading list");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list"]);
    });
    expect(within(favorite("Journal")).getByRole("button", { name: "Journal" })).not.toHaveAttribute("aria-current");
    await user.click(within(favorite("Reading list")).getByRole("button", { name: "Reading list" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Reading list" })).toBeInTheDocument();
    expect(within(favorite("Reading list")).getByRole("button", { name: "Reading list" })).toHaveAttribute("aria-current", "page");
    expect(favorite("Reading list")).toHaveClass("is-selected");
    expect(favorite("Journal")).not.toHaveClass("is-selected");
    // The same page is selected in the tree.
    expect(row("Reading list")).toHaveAttribute("aria-selected", "true");
    // Another page moves the highlight.
    await user.click(row("Journal"));
    expect(favorite("Journal")).toHaveClass("is-selected");
    expect(favorite("Reading list")).not.toHaveClass("is-selected");
  });

  it("opens with the keyboard", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    within(favorite("Journal")).getByRole("button", { name: "Journal" }).focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
  });

  it("makes the page the most recent one", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    await user.click(within(favorite("Journal")).getByRole("button", { name: "Journal" }));
    await waitFor(async () => {
      const response = await memory.dispatch({ type: "SettingsGet", data: { key: "ui.recent_pages" } });
      const { page } = await idsOf(memory);
      expect(response).toMatchObject({ type: "Setting", data: [{ pageId: page("Journal") }] });
    });
  });
});

describe("collapsing the section", () => {
  it("hides the rows with the title button, remembers it and shows them again", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    expect(toggle()).toHaveAttribute("aria-expanded", "true");
    await user.click(toggle());
    expect(list()).toBeNull();
    const collapsed = screen.getByRole("button", { name: t("favorites.title"), expanded: false });
    await waitFor(async () => {
      expect(await memory.dispatch({ type: "SettingsGet", data: { key: COLLAPSED_KEY } })).toEqual({ type: "Setting", data: true });
    });
    await user.click(collapsed);
    expect(titles()).toEqual(["Journal"]);
    await waitFor(async () => {
      expect(await memory.dispatch({ type: "SettingsGet", data: { key: COLLAPSED_KEY } })).toEqual({ type: "Setting", data: false });
    });
  });

  it("starts collapsed when the setting says so", async () => {
    const { memory } = await renderSidebar({ settings: { [COLLAPSED_KEY]: true } });
    await mark(memory, "Journal");
    const button = await screen.findByRole("button", { name: t("favorites.title"), expanded: false });
    expect(button).toBeInTheDocument();
    expect(list()).toBeNull();
  });
});

describe("the row menu", () => {
  async function renderWithThree() {
    const app = await renderSidebar();
    await mark(app.memory, "Journal", "Reading list", "Projects");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list", "Projects"]);
    });
    return app;
  }

  it("has Open in new tab, Move up, Move down and Remove from favourites", async () => {
    const { user } = await renderWithThree();
    const menu = await openMenu(user, "Reading list");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("page.openInNewTab"),
      t("favorites.moveUp"),
      t("favorites.moveDown"),
      t("favorites.remove"),
    ]);
  });

  it("disables Move up on the first favourite and Move down on the last", async () => {
    const { user } = await renderWithThree();
    let menu = await openMenu(user, "Journal");
    expect(within(menu).getByRole("menuitem", { name: t("favorites.moveUp") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("favorites.moveDown") })).toBeEnabled();
    await user.keyboard("{Escape}");
    menu = await openMenu(user, "Projects");
    expect(within(menu).getByRole("menuitem", { name: t("favorites.moveUp") })).toBeEnabled();
    expect(within(menu).getByRole("menuitem", { name: t("favorites.moveDown") })).toBeDisabled();
  });

  it("moves a favourite up and down, and keeps the order in the core", async () => {
    const { user, memory } = await renderWithThree();
    let menu = await openMenu(user, "Projects");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveUp") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Projects", "Reading list"]);
    });
    menu = await openMenu(user, "Projects");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveUp") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Projects", "Journal", "Reading list"]);
    });
    menu = await openMenu(user, "Projects");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveDown") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Projects", "Reading list"]);
    });
    menu = await openMenu(user, "Projects");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveDown") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list", "Projects"]);
    });
    expect(await favorites(memory)).toEqual(["Journal", "Reading list", "Projects"]);
  });

  it("sends the page it goes in front of, or nothing for the end of the list", async () => {
    const { user, memory } = await renderWithThree();
    const { page } = await idsOf(memory);
    const dispatch = vi.spyOn(memory, "dispatch");
    const moves = (): Request[] =>
      dispatch.mock.calls.map(([request]) => request).filter((request) => request.type === "PageFavoriteMove");
    let menu = await openMenu(user, "Reading list");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveUp") }));
    expect(moves().at(-1)).toEqual({ type: "PageFavoriteMove", data: { pageId: page("Reading list"), before: page("Journal") } });
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list", "Journal", "Projects"]);
    });
    menu = await openMenu(user, "Journal");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.moveDown") }));
    // Down from the second of three: in front of nothing, last.
    expect(moves().at(-1)).toMatchObject({ type: "PageFavoriteMove", data: { pageId: page("Journal") } });
    expect((moves().at(-1) as { data: { before?: PageId } }).data.before).toBeUndefined();
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list", "Projects", "Journal"]);
    });
  });

  it("removes the favourite, and the page stays in the tree", async () => {
    const { user, memory } = await renderWithThree();
    const menu = await openMenu(user, "Reading list");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.remove") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Projects"]);
    });
    expect(row("Reading list")).toBeInTheDocument();
    expect(await favorites(memory)).toEqual(["Journal", "Projects"]);
  });

  it("opens with a right click on the row, at the pointer", async () => {
    const { user } = await renderWithThree();
    await user.pointer({ keys: "[MouseRight]", target: favorite("Journal"), coords: { clientX: 30, clientY: 80 } });
    const menu = await screen.findByRole("menu", { name: t("favorites.actions") });
    expect(menu).toHaveStyle({ left: "30px", top: "80px" });
  });

  it("opens from the options button, which also closes it again", async () => {
    const { user } = await renderWithThree();
    const button = within(favorite("Journal")).getByRole("button", { name: t("favorites.actions") });
    await user.click(button);
    expect(await screen.findByRole("menu")).toBeInTheDocument();
    await user.click(button);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    // The click on the options button did not open the page.
    expect(favorite("Journal")).not.toHaveClass("is-selected");
  });
});

describe("dragging a favourite", () => {
  async function renderWithThree() {
    const app = await renderSidebar();
    await mark(app.memory, "Journal", "Reading list", "Projects");
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list", "Projects"]);
    });
    return app;
  }

  it("puts a favourite before another when it is dropped on its upper half", async () => {
    const { memory } = await renderWithThree();
    expect(dragAndDrop(favorite("Projects"), favorite("Journal"), "before")).toBe(true);
    await waitFor(() => {
      expect(titles()).toEqual(["Projects", "Journal", "Reading list"]);
    });
    expect(await favorites(memory)).toEqual(["Projects", "Journal", "Reading list"]);
  });

  it("puts a favourite after another when it is dropped on its lower half", async () => {
    await renderWithThree();
    expect(dragAndDrop(favorite("Journal"), favorite("Reading list"), "after")).toBe(true);
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list", "Journal", "Projects"]);
    });
  });

  it("puts a favourite last when it is dropped on the lower half of the last one", async () => {
    await renderWithThree();
    expect(dragAndDrop(favorite("Journal"), favorite("Projects"), "after")).toBe(true);
    await waitFor(() => {
      expect(titles()).toEqual(["Reading list", "Projects", "Journal"]);
    });
  });

  it("sends the favourite it goes in front of", async () => {
    const { memory } = await renderWithThree();
    const { page } = await idsOf(memory);
    const dispatch = vi.spyOn(memory, "dispatch");
    dragAndDrop(favorite("Projects"), favorite("Reading list"), "before");
    await waitFor(() => {
      expect(dispatch).toHaveBeenCalledWith({
        type: "PageFavoriteMove",
        data: { pageId: page("Projects"), before: page("Reading list") },
      });
    });
  });

  it("refuses a drop that would change nothing", async () => {
    const { memory } = await renderWithThree();
    const dispatch = vi.spyOn(memory, "dispatch");
    // Onto itself, before the next one, after the previous one, after the last one.
    expect(dragAndDrop(favorite("Reading list"), favorite("Reading list"), "before")).toBe(false);
    expect(dragAndDrop(favorite("Reading list"), favorite("Projects"), "before")).toBe(false);
    expect(dragAndDrop(favorite("Reading list"), favorite("Journal"), "after")).toBe(false);
    expect(dragAndDrop(favorite("Projects"), favorite("Projects"), "after")).toBe(false);
    expect(dispatch.mock.calls.some(([request]) => request.type === "PageFavoriteMove")).toBe(false);
    expect(titles()).toEqual(["Journal", "Reading list", "Projects"]);
  });

  it("shows where the favourite would land while it is dragged, and takes the marks away at the end", async () => {
    await renderWithThree();
    const drag = beginDrag(favorite("Projects"));
    expect(favorite("Projects")).toHaveClass("is-dragging");
    expect(drag.over(favorite("Journal"), "before")).toBe(true);
    expect(favorite("Journal")).toHaveClass("drop-before");
    expect(drag.over(favorite("Journal"), "after")).toBe(true);
    expect(favorite("Journal")).toHaveClass("drop-after");
    drag.leave(favorite("Journal"));
    expect(favorite("Journal")).not.toHaveClass("drop-after");
    drag.end();
    expect(favorite("Projects")).not.toHaveClass("is-dragging");
    expect(titles()).toEqual(["Journal", "Reading list", "Projects"]);
  });

  it("ignores a drag that does not carry a favourite", async () => {
    await renderWithThree();
    const dataTransfer = new FakeDataTransfer();
    const allowed = !fireEvent.dragOver(favorite("Journal"), { dataTransfer, clientY: 102 });
    expect(allowed).toBe(false);
  });
});

describe("the star in the page header", () => {
  const star = (name: string): HTMLElement => screen.getByRole("button", { name });

  it("is a toggle that adds the page to the favourites and takes it out again", async () => {
    const { user, memory } = await renderSidebar();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    expect(star(t("favorites.add"))).toHaveAttribute("aria-pressed", "false");
    await user.click(star(t("favorites.add")));
    await waitFor(() => {
      expect(star(t("favorites.remove"))).toHaveAttribute("aria-pressed", "true");
    });
    expect(titles()).toEqual(["Journal"]);
    expect(await favorites(memory)).toEqual(["Journal"]);
    await user.click(star(t("favorites.remove")));
    await waitFor(() => {
      expect(star(t("favorites.add"))).toHaveAttribute("aria-pressed", "false");
    });
    expect(list()).toBeNull();
    expect(await favorites(memory)).toEqual([]);
  });

  it("adds a page last", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await user.click(row("Reading list"));
    await user.click(await screen.findByRole("button", { name: t("favorites.add") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal", "Reading list"]);
    });
  });

  it("shows the state of the page that is open, and follows a change from elsewhere", async () => {
    const { user, memory } = await renderSidebar();
    await mark(memory, "Journal");
    await user.click(row("Journal"));
    expect(await screen.findByRole("button", { name: t("favorites.remove") })).toHaveAttribute("aria-pressed", "true");
    await user.click(row("Reading list"));
    expect(await screen.findByRole("button", { name: t("favorites.add") })).toHaveAttribute("aria-pressed", "false");
    const { page } = await idsOf(memory);
    await act(async () => {
      await new CoreClient(memory).pageFavoriteSet(page("Reading list"), true);
    });
    expect(await screen.findByRole("button", { name: t("favorites.remove") })).toHaveAttribute("aria-pressed", "true");
  });

  it("says when the core refuses", async () => {
    const { user, memory } = await renderSidebar();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    const original = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PageFavoriteSet" ? Promise.reject(new Error("the structure is locked")) : original(request),
    );
    await user.click(star(t("favorites.add")));
    expect(await screen.findByText(t("favorites.error.set", { message: "the structure is locked" }))).toBeInTheDocument();
    expect(star(t("favorites.add"))).toHaveAttribute("aria-pressed", "false");
  });
});

describe("the item in the tree's menu", () => {
  async function openTreeMenu(user: User, title: string): Promise<HTMLElement> {
    await user.pointer({ keys: "[MouseRight]", target: row(title) });
    return screen.findByRole("menu", { name: t("tree.actions") });
  }

  it("adds a page to the favourites, and then offers to remove it", async () => {
    const { user, memory } = await renderSidebar();
    let menu = await openTreeMenu(user, "Journal");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.add") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Journal"]);
    });
    menu = await openTreeMenu(user, "Journal");
    expect(within(menu).queryByRole("menuitem", { name: t("favorites.add") })).not.toBeInTheDocument();
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.remove") }));
    await waitFor(() => {
      expect(list()).toBeNull();
    });
    expect(await favorites(memory)).toEqual([]);
  });

  it("shows the sidebar's alert when the core refuses", async () => {
    const { user, memory } = await renderSidebar();
    const original = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PageFavoriteSet" ? Promise.reject(new Error("no way")) : original(request),
    );
    const menu = await openTreeMenu(user, "Journal");
    await user.click(within(menu).getByRole("menuitem", { name: t("favorites.add") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("favorites.error.set", { message: "no way" }));
  });
});
