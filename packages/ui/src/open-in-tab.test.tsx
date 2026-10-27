// Every way of opening a page passes the disposition of its gesture (ADR-0034 section 2): a plain
// click or Enter navigates the active tab, Ctrl/Cmd + click and a middle click open a background tab,
// Ctrl/Cmd + Shift + click, "Open in new tab" and Ctrl/Cmd + Enter in Ctrl+K open a tab in front.
//
// The component tests replace the store's `navigateTo` with a spy once the page they need is open,
// so they hold whatever the tabs slice does. The last block runs the gestures against the real store.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import { t, tp } from "./i18n";
import { dispositionFromKey, openGestures } from "./page-open";
import type { AppStore, AppState } from "./store";
import type * as StoreModule from "./store";
import { appendReference, idsOf, openPage, renderPages } from "./test/page";
import { paragraphs, pid } from "./test/editor";
import { NOW } from "./test/render";
import type { User } from "./test/render";
import { PERSONAL, WORK, row } from "./test/sidebar";
import { openProperties, propertyRow, renderProperties } from "./test/properties";

// The app creates its store inside `<App>`; the tests reach it through this record.
const created = vi.hoisted((): { stores: AppStore[] } => ({ stores: [] }));

vi.mock("./store", async (importOriginal) => {
  const actual = await importOriginal<typeof StoreModule>();
  return {
    ...actual,
    createAppStore: (...args: Parameters<typeof actual.createAppStore>) => {
      const store = actual.createAppStore(...args);
      created.stores.push(store);
      return store;
    },
  };
});

afterEach(() => {
  created.stores.length = 0;
  vi.restoreAllMocks();
});

function appStore(): AppStore {
  const store = created.stores.at(-1);
  if (store === undefined) {
    throw new Error("the app has not created its store");
  }
  return store;
}

type Navigate = Mock<AppState["navigateTo"]>;

/** Replaces `navigateTo` with a spy; call it after the page the test needs is open. */
function spyOnNavigation(): Navigate {
  const navigateTo: Navigate = vi.fn();
  appStore().setState({ navigateTo });
  return navigateTo;
}

type Modifier = "Control" | "Meta" | "Shift";

/** A click with keys held. */
async function click(user: User, element: Element, ...modifiers: Modifier[]): Promise<void> {
  for (const modifier of modifiers) {
    await user.keyboard(`{${modifier}>}`);
  }
  await user.click(element);
  for (const modifier of [...modifiers].reverse()) {
    await user.keyboard(`{/${modifier}}`);
  }
}

/** An `auxclick` (a click of a button other than the primary one), which `fireEvent` has no shorthand for. */
function auxClick(element: Element, init: MouseEventInit): boolean {
  return fireEvent(element, new MouseEvent("auxclick", { bubbles: true, cancelable: true, ...init }));
}

/** A click of the middle button (`mousedown`, `mouseup`, then `auxclick`; no `click`). */
async function middleClick(user: User, element: Element): Promise<void> {
  await user.pointer({ keys: "[MouseMiddle]", target: element });
}

/** The page's open call as the spy should have seen it. */
function target(pageId: string, spaceId: string, disposition: string, blockId?: string) {
  return blockId === undefined ? { pageId, spaceId, disposition } : { pageId, spaceId, blockId, disposition };
}

/** Runs the same four gestures on an element of a page link and checks the calls. */
async function expectGestures(
  user: User,
  navigateTo: Navigate,
  element: () => Element,
  expected: (disposition: string) => unknown,
): Promise<void> {
  await user.click(element());
  expect(navigateTo).toHaveBeenLastCalledWith(expected("current"));
  await click(user, element(), "Control");
  expect(navigateTo).toHaveBeenLastCalledWith(expected("background"));
  await click(user, element(), "Meta");
  expect(navigateTo).toHaveBeenLastCalledWith(expected("background"));
  await click(user, element(), "Control", "Shift");
  expect(navigateTo).toHaveBeenLastCalledWith(expected("foreground"));
  await click(user, element(), "Meta", "Shift");
  expect(navigateTo).toHaveBeenLastCalledWith(expected("foreground"));
  navigateTo.mockClear();
  await middleClick(user, element());
  expect(navigateTo).toHaveBeenCalledTimes(1);
  expect(navigateTo).toHaveBeenLastCalledWith(expected("background"));
  // Shift alone changes nothing.
  navigateTo.mockClear();
  await click(user, element(), "Shift");
  expect(navigateTo).toHaveBeenCalledTimes(1);
  expect(navigateTo).toHaveBeenLastCalledWith(expected("current"));
  navigateTo.mockClear();
}

// ----- the gestures themselves ---------------------------------------------------------------------

describe("the gestures that open a page", () => {
  it("chooses the disposition of a key from Ctrl and Cmd", () => {
    expect(dispositionFromKey({ ctrlKey: false, metaKey: false })).toBe("current");
    expect(dispositionFromKey({ ctrlKey: true, metaKey: false })).toBe("foreground");
    expect(dispositionFromKey({ ctrlKey: false, metaKey: true })).toBe("foreground");
  });

  it("opens a click by its modifiers and a middle click in the background, never both for one press", () => {
    const open = vi.fn();
    render(
      <div>
        <button type="button" {...openGestures<HTMLButtonElement>(open)}>
          link <span>inner</span>
        </button>
      </div>,
    );
    const link = screen.getByRole("button");
    fireEvent.click(link);
    fireEvent.click(link, { ctrlKey: true });
    fireEvent.click(link, { metaKey: true, shiftKey: true });
    fireEvent.click(screen.getByText("inner"), { ctrlKey: true });
    expect(open.mock.calls).toEqual([["current"], ["background"], ["foreground"], ["background"]]);
    open.mockClear();

    // The middle button is the `auxclick`'s; a `click` that carries it opens nothing, the other buttons neither.
    fireEvent.click(link, { button: 1 });
    auxClick(link, { button: 2 });
    expect(open).not.toHaveBeenCalled();
    auxClick(link, { button: 1 });
    auxClick(link, { button: 1, ctrlKey: true, shiftKey: true });
    expect(open.mock.calls).toEqual([["background"], ["foreground"]]);
  });

  it("leaves the gestures of a control inside the link to it", () => {
    const open = vi.fn();
    render(
      <div role="row" {...openGestures<HTMLDivElement>(open)}>
        <input aria-label="rename" />
        <button type="button">toggle</button>
      </div>,
    );
    const host = screen.getByRole("row");
    for (const control of [screen.getByRole("textbox"), screen.getByRole("button")]) {
      fireEvent.click(control, { ctrlKey: true });
      auxClick(control, { button: 1 });
      // The middle button's autoscroll is left alone there too.
      expect(fireEvent.mouseDown(control, { button: 1 })).toBe(true);
    }
    expect(open).not.toHaveBeenCalled();
    // A plain click on the control still bubbles to the row, as it always did.
    fireEvent.click(screen.getByRole("button"));
    expect(open).toHaveBeenCalledWith("current");
    // On the row itself the middle button's autoscroll is stopped.
    expect(fireEvent.mouseDown(host, { button: 1 })).toBe(false);
    expect(fireEvent.mouseDown(host, { button: 0 })).toBe(true);
  });
});

// ----- the sidebar ---------------------------------------------------------------------------------

describe("the page tree", () => {
  async function renderTree() {
    const app = await renderPages([PERSONAL, WORK], { workspaceName: "Research" });
    const ids = await idsOf(app.memory);
    const navigateTo = spyOnNavigation();
    const opened = (title: string, disposition: string) => target(ids.page(title), ids.space("Personal"), disposition);
    return { ...app, ids, navigateTo, opened };
  }

  it("opens a row by its gesture: click and Enter here, Ctrl or Cmd + click and the middle button in the background, with Shift in front", async () => {
    const { user, navigateTo, opened } = await renderTree();
    await expectGestures(user, navigateTo, () => row("Journal"), (disposition) => opened("Journal", disposition));

    row("Reading list").focus();
    await user.keyboard("{Enter}");
    expect(navigateTo).toHaveBeenLastCalledWith(opened("Reading list", "current"));
    await user.keyboard(" ");
    expect(navigateTo).toHaveBeenLastCalledWith(opened("Reading list", "current"));
  });

  it("keeps the middle button from starting the browser's autoscroll", () => {
    return renderTree().then(() => {
      const prevented = !fireEvent.mouseDown(row("Journal"), { button: 1 });
      expect(prevented).toBe(true);
      // The primary button keeps its default (focus, drag).
      expect(fireEvent.mouseDown(row("Journal"), { button: 0 })).toBe(true);
    });
  });

  it("has Open in new tab first in the context menu, which opens the page in front", async () => {
    const { user, navigateTo, opened } = await renderTree();
    await user.pointer({ keys: "[MouseRight]", target: row("Ideas") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    expect(within(menu).getAllByRole("menuitem")[0]).toHaveTextContent(t("page.openInNewTab"));
    expect(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") })).toBeEnabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") }));
    expect(navigateTo).toHaveBeenCalledTimes(1);
    expect(navigateTo).toHaveBeenCalledWith(opened("Ideas", "foreground"));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("opens nothing for the other gestures of a row: the expand toggle, the add and menu buttons, the rename", async () => {
    const { user, navigateTo } = await renderTree();
    const projects = row("Projects");
    const toggle = () => within(projects).getByRole("button", { name: (name) => name === t("tree.collapse") || name === t("tree.expand") });
    await click(user, toggle(), "Control");
    await middleClick(user, toggle());
    await click(user, toggle(), "Control", "Shift");
    const menuButton = within(projects).getByRole("button", { name: t("tree.actions") });
    await click(user, menuButton, "Control");
    await middleClick(user, menuButton);
    await user.keyboard("{Escape}");
    expect(navigateTo).not.toHaveBeenCalled();

    // The add button makes a page and opens it in the tab, whatever the keys; the middle button does nothing.
    const add = within(projects).getByRole("button", { name: t("tree.addChild") });
    await middleClick(user, add);
    expect(navigateTo).not.toHaveBeenCalled();
    await click(user, add, "Control");
    await user.keyboard("{Escape}");
    expect(navigateTo.mock.calls.every(([call]) => call.disposition === undefined || call.disposition === "current")).toBe(true);
    navigateTo.mockClear();

    // A double click renames; with Ctrl it is two opens in the background and no rename.
    await user.keyboard("{Control>}");
    await user.dblClick(row("Journal"));
    await user.keyboard("{/Control}");
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
    expect(navigateTo.mock.calls.map(([call]) => call.disposition)).toEqual(["background", "background"]);
    await user.dblClick(row("Journal"));
    expect(await screen.findByRole("textbox", { name: t("tree.renameLabel") })).toBeInTheDocument();
  });

  it("does not use Ctrl or Shift + click for anything else (no multi-select to override)", async () => {
    const { user, navigateTo, opened } = await renderTree();
    await click(user, row("Journal"), "Shift");
    await click(user, row("Reading list"), "Shift");
    expect(navigateTo.mock.calls.map(([call]) => call.disposition)).toEqual(["current", "current"]);
    expect(navigateTo).toHaveBeenLastCalledWith(opened("Reading list", "current"));
    expect(row("Journal")).toHaveAttribute("aria-selected", "false");
  });
});

// ----- favourites ----------------------------------------------------------------------------------

describe("the favourites", () => {
  async function renderFavorites() {
    const app = await renderPages([PERSONAL, WORK]);
    const ids = await idsOf(app.memory);
    const client = new CoreClient(app.memory);
    await act(async () => {
      await client.pageFavoriteSet(ids.page("Journal"), true);
    });
    const list = await screen.findByRole("list", { name: t("favorites.title") });
    const navigateTo = spyOnNavigation();
    const opened = (disposition: string) => target(ids.page("Journal"), ids.space("Personal"), disposition);
    const open = () => within(list).getByRole("button", { name: "Journal" });
    return { ...app, ids, list, navigateTo, opened, open };
  }

  it("opens a favourite by its gesture", async () => {
    const { user, navigateTo, opened, open } = await renderFavorites();
    await expectGestures(user, navigateTo, open, opened);
  });

  it("has Open in new tab first in its menu", async () => {
    const { user, navigateTo, opened, list } = await renderFavorites();
    await user.click(within(list).getByRole("button", { name: t("favorites.actions") }));
    const menu = await screen.findByRole("menu", { name: t("favorites.actions") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("page.openInNewTab"),
      t("favorites.moveUp"),
      t("favorites.moveDown"),
      t("favorites.remove"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") }));
    expect(navigateTo).toHaveBeenCalledTimes(1);
    expect(navigateTo).toHaveBeenCalledWith(opened("foreground"));
  });

  it("does not open the page for a middle click on the menu button", async () => {
    const { user, navigateTo, list } = await renderFavorites();
    await middleClick(user, within(list).getByRole("button", { name: t("favorites.actions") }));
    expect(navigateTo).not.toHaveBeenCalled();
  });
});

// ----- the Home screen and Ctrl+K ------------------------------------------------------------------

const SPACES: SeedSpace[] = [
  {
    name: "Personal",
    pages: [
      { title: "Journal", content: paragraphs("Planning the roadmap for next year", "Unrelated") },
      { title: "Groceries", content: paragraphs("Milk") },
    ],
  },
  { name: "Work", pages: [{ title: "Roadmap", icon: "🗺️", content: paragraphs("Milestones") }] },
];

async function seedRecents(memory: MemoryTransport, entries: readonly { pageId: string; spaceId: string; openedAt: number }[]) {
  await memory.dispatch({ type: "SettingsSet", data: { key: "ui.recent_pages", value: entries.map((item) => ({ ...item })) } });
}

describe("the recent pages of the Home screen", () => {
  it("opens a page by its gesture, in its own space", async () => {
    const { user, memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    await seedRecents(memory, [{ pageId: ids.page("Roadmap"), spaceId: ids.space("Work"), openedAt: NOW }]);
    const list = await screen.findByRole("list", { name: t("recent.visitedList") });
    const navigateTo = spyOnNavigation();
    await expectGestures(
      user,
      navigateTo,
      () => within(list).getByRole("button", { name: new RegExp("Roadmap") }),
      (disposition) => target(ids.page("Roadmap"), ids.space("Work"), disposition),
    );
  });
});

describe("the search dialog", () => {
  const dialog = (): HTMLElement => screen.getByRole("dialog", { name: t("search.label") });

  async function searchFor(query: string) {
    const app = await renderPages(SPACES);
    const ids = await idsOf(app.memory);
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const input = within(dialog()).getByRole("combobox", { name: t("search.label") });
    await app.user.type(input, query);
    const results = within(dialog()).getByRole("listbox", { name: t("search.results") });
    await waitFor(() => {
      expect(within(results).getAllByRole("option").length).toBeGreaterThan(0);
    });
    return { ...app, ids, input, results, navigateTo: spyOnNavigation() };
  }

  it("opens the hit with Enter in the active tab and with Ctrl or Cmd + Enter in a tab in front, at its block", async () => {
    const { user, ids, navigateTo } = await searchFor("roadmap");
    // The title hit of Roadmap (Work) is first, the text hit of Journal second.
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Roadmap"), ids.space("Work"), "foreground"));
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();

    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await user.type(within(dialog()).getByRole("combobox"), "roadmap");
    await within(dialog()).findAllByRole("option");
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Journal"), ids.space("Personal"), "foreground", pid(0)));

    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await user.type(within(dialog()).getByRole("combobox"), "roadmap");
    await within(dialog()).findAllByRole("option");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Journal"), ids.space("Personal"), "current", pid(0)));
    expect(navigateTo).toHaveBeenCalledTimes(3);
  });

  it("opens a hit by its mouse gesture: the active tab and a tab in front close the dialog", async () => {
    const { user, ids, navigateTo } = await searchFor("roadmap");
    const expected = (disposition: string) => target(ids.page("Journal"), ids.space("Personal"), disposition, pid(0));
    for (const [gesture, disposition] of [
      [(element: Element) => user.click(element), "current"],
      [(element: Element) => click(user, element, "Control", "Shift"), "foreground"],
      [(element: Element) => click(user, element, "Meta", "Shift"), "foreground"],
    ] as const) {
      const options = within(dialog()).getAllByRole("option");
      expect(options[1]).toHaveTextContent("Planning the roadmap");
      await gesture(options[1] as Element);
      expect(navigateTo).toHaveBeenLastCalledWith(expected(disposition));
      expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
      fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
      await user.type(within(dialog()).getByRole("combobox"), "roadmap");
      await within(dialog()).findAllByRole("option");
    }
    expect(navigateTo).toHaveBeenCalledTimes(3);
  });

  it("keeps the dialog as it is for a background open, so several hits can be opened in a row", async () => {
    const { user, ids, input, navigateTo } = await searchFor("roadmap");
    const options = () => within(dialog()).getAllByRole("option");
    const [first, second] = options();
    expect(first).toHaveAttribute("aria-selected", "true");

    // `fireEvent` does not move the pointer, so the hover does not change the active item.
    fireEvent.click(second as Element, { ctrlKey: true });
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Journal"), ids.space("Personal"), "background", pid(0)));
    auxClick(first as Element, { button: 1 });
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Roadmap"), ids.space("Work"), "background"));
    fireEvent.click(first as Element, { metaKey: true });
    expect(navigateTo).toHaveBeenCalledTimes(3);

    // The dialog, its query, its results and its active item are as they were, with the keyboard still in the field.
    expect(dialog()).toBeInTheDocument();
    expect(input).toHaveValue("roadmap");
    expect(options().map((option) => option.textContent)).toEqual([first?.textContent, second?.textContent]);
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveFocus();

    // And it closes as before with the next plain open.
    await user.keyboard("{ArrowDown}{Enter}");
    expect(navigateTo).toHaveBeenLastCalledWith(target(ids.page("Journal"), ids.space("Personal"), "current", pid(0)));
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
  });

  it("opens in the background with the real mouse too, and the field keeps focus", async () => {
    const { user, ids, input, navigateTo } = await searchFor("roadmap");
    await click(user, within(dialog()).getAllByRole("option")[1] as Element, "Control");
    await middleClick(user, within(dialog()).getAllByRole("option")[0] as Element);
    expect(navigateTo.mock.calls.map(([call]) => call.disposition)).toEqual(["background", "background"]);
    expect(navigateTo.mock.calls[1]?.[0]).toEqual(target(ids.page("Roadmap"), ids.space("Work"), "background"));
    expect(dialog()).toBeInTheDocument();
    expect(input).toHaveFocus();
    expect(input).toHaveValue("roadmap");
  });

  it("opens a recent page the same ways while nothing is typed", async () => {
    const { user, memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    await seedRecents(memory, [
      { pageId: ids.page("Roadmap"), spaceId: ids.space("Work"), openedAt: NOW - 60_000 },
      { pageId: ids.page("Journal"), spaceId: ids.space("Personal"), openedAt: NOW - 120_000 },
    ]);
    const navigateTo = spyOnNavigation();
    const expected = (title: string, space: string, disposition: string) => target(ids.page(title), ids.space(space), disposition);
    const openDialog = async (): Promise<HTMLElement> => {
      fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
      const group = await within(dialog()).findByRole("group", { name: t("recent.title") });
      await waitFor(() => {
        expect(within(group).getAllByRole("option")).toHaveLength(2);
      });
      return group;
    };

    await openDialog();
    await user.keyboard("{Enter}");
    expect(navigateTo).toHaveBeenLastCalledWith(expected("Roadmap", "Work", "current"));

    await openDialog();
    await user.keyboard("{ArrowDown}{Control>}{Enter}{/Control}");
    expect(navigateTo).toHaveBeenLastCalledWith(expected("Journal", "Personal", "foreground"));

    for (const [gesture, disposition] of [
      [(element: Element) => user.click(element), "current"],
      [(element: Element) => click(user, element, "Meta", "Shift"), "foreground"],
    ] as const) {
      const group = await openDialog();
      await gesture(within(group).getAllByRole("option")[1] as Element);
      expect(navigateTo).toHaveBeenLastCalledWith(expected("Journal", "Personal", disposition));
      expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    }
    expect(navigateTo).toHaveBeenCalledTimes(4);

    // A background open leaves the dialog and its list as they are.
    const group = await openDialog();
    const [newest, older] = within(group).getAllByRole("option");
    fireEvent.click(older as Element, { ctrlKey: true });
    auxClick(newest as Element, { button: 1 });
    expect(navigateTo.mock.calls.slice(-2)).toEqual([
      [expected("Journal", "Personal", "background")],
      [expected("Roadmap", "Work", "background")],
    ]);
    expect(dialog()).toBeInTheDocument();
    expect(within(group).getAllByRole("option")).toHaveLength(2);
    expect(newest).toHaveAttribute("aria-selected", "true");
    expect(within(dialog()).getByRole("combobox")).toHaveFocus();
    await user.keyboard("{Escape}");
  });
});

// ----- the page around the editor ------------------------------------------------------------------

const MIDDLE = "22222222-2222-4222-8222-222222222222";

describe("the backlinks panel", () => {
  it("opens the page, and a block of it, by each gesture", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Target", content: paragraphs("Linked to") }] },
      { name: "Work", pages: [{ title: "Source", content: paragraphs("Intro") }] },
    ]);
    const ids = await idsOf(memory);
    act(() => {
      memory.changePage(ids.page("Source"), (doc) => {
        appendReference(doc, "Read ", ids.page("Target"), MIDDLE);
      }, "Core");
    });
    await openPage(user, "Target");
    const panel = await screen.findByRole("region", { name: tp("backlinks.title", 1) });
    const navigateTo = spyOnNavigation();

    await expectGestures(
      user,
      navigateTo,
      () => within(panel).getByRole("button", { name: "Source" }),
      (disposition) => target(ids.page("Source"), ids.space("Work"), disposition),
    );
    await expectGestures(
      user,
      navigateTo,
      () => within(panel).getByRole("button", { name: t("backlinks.block", { title: "Source", text: "Read Target" }) }),
      (disposition) => target(ids.page("Source"), ids.space("Work"), disposition, MIDDLE),
    );
  });
});

describe("the breadcrumbs", () => {
  it("open an ancestor by each gesture, in the space of the page", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [{ type: "breadcrumb", attrs: { id: pid(1) } }, { type: "paragraph", attrs: { id: pid(2) }, content: [{ type: "text", text: "Below" }] }],
    };
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [{ title: "Projects", children: [{ title: "atomik-kb", children: [{ title: "Roadmap", content }] }] }],
      },
    ]);
    const ids = await idsOf(memory);
    await openPage(user, "Roadmap");
    const nav = await screen.findByRole("navigation", { name: t("breadcrumb.label") });
    const navigateTo = spyOnNavigation();
    await expectGestures(
      user,
      navigateTo,
      () => within(nav).getByRole("button", { name: "Projects" }),
      (disposition) => target(ids.page("Projects"), ids.space("Personal"), disposition),
    );
    await expectGestures(
      user,
      navigateTo,
      () => within(nav).getByRole("button", { name: "atomik-kb" }),
      (disposition) => target(ids.page("atomik-kb"), ids.space("Personal"), disposition),
    );
  });
});

describe("the path in the page header", () => {
  it("opens an ancestor by each gesture, in the space of the page", async () => {
    const { user, memory } = await renderPages([
      {
        name: "Personal",
        pages: [{ title: "Projects", children: [{ title: "atomik-kb", children: [{ title: "Roadmap", content: paragraphs("x") }] }] }],
      },
    ]);
    const ids = await idsOf(memory);
    await openPage(user, "Roadmap");
    const nav = await screen.findByRole("navigation", { name: t("pagePath.label") });
    const navigateTo = spyOnNavigation();
    await expectGestures(
      user,
      navigateTo,
      () => within(nav).getByRole("button", { name: "Projects" }),
      (disposition) => target(ids.page("Projects"), ids.space("Personal"), disposition),
    );
    await expectGestures(
      user,
      navigateTo,
      () => within(nav).getByRole("button", { name: "atomik-kb" }),
      (disposition) => target(ids.page("atomik-kb"), ids.space("Personal"), disposition),
    );
  });
});

describe("a relation property", () => {
  it("opens the linked page by each gesture", async () => {
    const { user, memory } = await renderProperties([
      {
        name: "Personal",
        properties: [{ name: "Links", kind: "relation" }],
        pages: [{ title: "Launch", properties: { Links: ["Write"] } }, { title: "Write" }],
      },
    ]);
    const ids = await idsOf(memory);
    const panel = await openProperties(user, "Launch");
    const chip = () => within(propertyRow(panel, "Links")).getByRole("button", { name: t("properties.relation.open", { title: "Write" }) });
    await waitFor(() => {
      expect(chip()).toBeEnabled();
    });
    const navigateTo = spyOnNavigation();
    await expectGestures(user, navigateTo, chip, (disposition) => target(ids.page("Write"), ids.space("Personal"), disposition));
    // The remove button next to it is not an "open".
    await middleClick(user, within(propertyRow(panel, "Links")).getByRole("button", { name: t("properties.relation.remove", { title: "Write", name: "Links" }) }));
    expect(navigateTo).not.toHaveBeenCalled();
  });
});

// ----- the chips and cards of the editor -----------------------------------------------------------

describe("the page chips and cards of the editor", () => {
  async function renderNotes() {
    const app = await renderPages([
      {
        name: "Personal",
        pages: [
          { title: "Notes", content: paragraphs("Intro", "Second") },
          { title: "Target", content: paragraphs("First", "Second") },
        ],
      },
    ]);
    const ids = await idsOf(app.memory);
    const editor = await openPage(app.user, "Notes");
    return { ...app, ids, editor };
  }

  const insert = (editor: Editor, content: JsonValue): void => {
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, content as never);
    });
  };

  const chipOf = async (editor: Editor): Promise<HTMLElement> =>
    waitFor(() => {
      const element = editor.view.dom.querySelector<HTMLElement>(".page-ref");
      expect(element).not.toBeNull();
      expect(element).toHaveTextContent("Target");
      return element as HTMLElement;
    });

  const cardOf = async (editor: Editor): Promise<HTMLElement> =>
    waitFor(() => {
      const element = editor.view.dom.querySelector<HTMLElement>(".page-link");
      expect(element).not.toBeNull();
      expect(element).toHaveTextContent("Target");
      return element as HTMLElement;
    });

  it("opens the chip's page by each gesture, keeping the block it names", async () => {
    const { user, ids, editor } = await renderNotes();
    insert(editor, { type: "paragraph", content: [{ type: "pageRef", attrs: { pageId: ids.page("Target"), blockId: pid(1) } }] });
    const chip = await chipOf(editor);
    const navigateTo = spyOnNavigation();
    await expectGestures(
      user,
      navigateTo,
      () => chip,
      (disposition) => target(ids.page("Target"), ids.space("Personal"), disposition, pid(1)),
    );
  });

  it("opens the card's page by each gesture, keeping the block it names", async () => {
    const { user, ids, editor } = await renderNotes();
    insert(editor, { type: "pageLink", attrs: { pageId: ids.page("Target"), blockId: pid(1) } });
    const card = await cardOf(editor);
    const navigateTo = spyOnNavigation();
    await expectGestures(
      user,
      navigateTo,
      () => card,
      (disposition) => target(ids.page("Target"), ids.space("Personal"), disposition, pid(1)),
    );
  });

  it("opens a chip without a block as the page only", async () => {
    const { user, ids, editor } = await renderNotes();
    insert(editor, { type: "paragraph", content: [{ type: "pageRef", attrs: { pageId: ids.page("Target") } }] });
    const chip = await chipOf(editor);
    const navigateTo = spyOnNavigation();
    await click(user, chip, "Control");
    expect(navigateTo).toHaveBeenCalledTimes(1);
    expect(navigateTo.mock.calls[0]?.[0]).toEqual(target(ids.page("Target"), ids.space("Personal"), "background"));
  });

  it("keeps a Ctrl, Cmd or middle press from the editor, so the caret and the selection stay, and a plain press reaches it", async () => {
    const { ids, editor } = await renderNotes();
    insert(editor, { type: "paragraph", content: [{ type: "pageRef", attrs: { pageId: ids.page("Target") } }] });
    insert(editor, { type: "pageLink", attrs: { pageId: ids.page("Target") } });
    const chip = await chipOf(editor);
    const card = await cardOf(editor);
    const seen = vi.fn();
    editor.view.dom.addEventListener("mousedown", seen);
    spyOnNavigation();
    const selection = editor.state.selection.toJSON() as unknown;

    for (const element of [chip, card]) {
      for (const init of [{ ctrlKey: true }, { metaKey: true }, { button: 1 }]) {
        // `fireEvent` answers false when the press was cancelled.
        expect(fireEvent.mouseDown(element, init), JSON.stringify(init)).toBe(false);
      }
    }
    expect(seen).not.toHaveBeenCalled();
    expect(editor.state.selection.toJSON() as unknown).toEqual(selection);

    // A plain press (and Shift alone) goes to the editor as before.
    expect(fireEvent.mouseDown(chip, { button: 0 })).toBe(true);
    fireEvent.mouseDown(card, { shiftKey: true });
    expect(seen).toHaveBeenCalledTimes(2);
    editor.view.dom.removeEventListener("mousedown", seen);
  });

  it("does not open a chip whose page cannot be read, whatever the gesture", async () => {
    const { user, editor } = await renderNotes();
    insert(editor, {
      type: "paragraph",
      content: [{ type: "pageRef", attrs: { pageId: "00000000-0000-4000-8000-000000000000" } }],
    });
    const chip = await waitFor(() => {
      const element = editor.view.dom.querySelector<HTMLElement>(".page-ref.is-deleted");
      expect(element).not.toBeNull();
      return element as HTMLElement;
    });
    const navigateTo = spyOnNavigation();
    await user.click(chip);
    await click(user, chip, "Control");
    await middleClick(user, chip);
    expect(navigateTo).not.toHaveBeenCalled();
  });
});

// ----- the list, table and board views -------------------------------------------------------------

describe("the list, table and board views", () => {
  const STAGE = "de000000-0000-4000-8000-00000000f001";

  function hub(layout: string): JsonValue {
    return {
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
        {
          type: "pageList",
          // `openIn: "page"`: a plain click opens the page in the tab, the other gestures keep their tabs (the default is the side peek, ADR-0054).
          attrs: {
            id: pid(2),
            source: { kind: "children" },
            layout,
            properties: [],
            sort: { by: "title", dir: "asc" },
            ...(layout === "board" ? { groupBy: STAGE } : {}),
            views: [
              {
                id: "v1",
                name: "",
                layout,
                properties: [],
                sort: { by: "title", dir: "asc" },
                ...(layout === "board" ? { groupBy: STAGE } : {}),
                openIn: "page",
              },
            ],
          },
        },
        { type: "paragraph", attrs: { id: pid(3) } },
      ],
    };
  }

  async function renderHub(layout: string) {
    const app = await renderPages([
      {
        name: "Personal",
        properties: [
          {
            id: STAGE,
            name: "Stage",
            kind: "select",
            options: [
              { name: "Draft", color: "gray" },
              { name: "Review", color: "orange" },
            ],
          },
        ],
        pages: [
          {
            title: "Hub",
            content: hub(layout),
            children: [
              { title: "Alpha", properties: { Stage: "Draft" } },
              { title: "Beta", properties: { Stage: "Review" } },
            ],
          },
        ],
      },
    ]);
    const editor = await openPage(app.user, "Hub");
    await screen.findAllByRole("group", { name: t("pageList.label") });
    const ids = await idsOf(app.memory);
    return { ...app, editor, ids, opened: (title: string, disposition: string) => target(ids.page(title), ids.space("Personal"), disposition) };
  }

  it("opens a page of the list by each gesture", async () => {
    const { user, opened } = await renderHub("list");
    const list = await screen.findByRole("list", { name: t("pageList.rows") });
    const link = () => within(list).getByRole("button", { name: "Alpha" });
    const navigateTo = spyOnNavigation();
    await expectGestures(user, navigateTo, link, (disposition) => opened("Alpha", disposition));
  });

  it("opens a page of the table from its open button by each gesture, and the cell does not take the click", async () => {
    const { user, opened } = await renderHub("table");
    const grid = await screen.findByRole("grid", { name: t("pageTable.grid") });
    const button = () => within(grid).getByRole("button", { name: t("pageTable.open", { title: "Alpha" }) });
    const navigateTo = spyOnNavigation();
    await expectGestures(user, navigateTo, button, (disposition) => opened("Alpha", disposition));
    expect(within(grid).getAllByRole("gridcell").every((cell) => !cell.hasAttribute("data-active"))).toBe(true);
  });

  it("opens a card of the board by each gesture", async () => {
    const { user, opened } = await renderHub("board");
    const board = await screen.findByRole("group", { name: t("pageBoard.label") });
    const link = () => within(board).getByRole("button", { name: "Alpha" });
    const navigateTo = spyOnNavigation();
    await expectGestures(user, navigateTo, link, (disposition) => opened("Alpha", disposition));
    // The card's own menu moves cards between columns: a middle click on its button opens nothing.
    await middleClick(user, within(board).getAllByRole("button", { name: t("pageBoard.cardMenu", { title: "Alpha" }) })[0] as Element);
    expect(navigateTo).not.toHaveBeenCalled();
  });
});

// ----- against the real store (needs the tabs slice of ADR-0034) ----------------------------------

describe("against the real store (needs the tabs slice)", () => {
  const pageOf = (tab: { entries: readonly { pageId: string | null }[]; index: number } | undefined): string | null =>
    tab?.entries[tab.index]?.pageId ?? null;

  it("opens a background tab for a Ctrl + click and a middle click, and leaves the active tab and the page as they are", async () => {
    const { user, memory } = await renderPages([PERSONAL, WORK]);
    const ids = await idsOf(memory);
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    const before = appStore().getState().activeTabId;
    await click(user, row("Reading list"), "Control");
    await middleClick(user, row("Ideas"));
    const { tabs, activeTabId } = appStore().getState();
    // Successive background tabs line up in the order they were asked for, right after the active one.
    expect(tabs.map(pageOf)).toEqual([ids.page("Journal"), ids.page("Reading list"), ids.page("Ideas")]);
    expect(activeTabId).toBe(before);
    expect(screen.getByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
  });

  it("opens a foreground tab from Ctrl + Shift + click and from the menu, and a plain click keeps the tabs", async () => {
    const { user } = await renderPages([PERSONAL, WORK]);
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    expect(appStore().getState().tabs).toHaveLength(1);
    await click(user, row("Reading list"), "Control", "Shift");
    expect(await screen.findByRole("heading", { level: 1, name: "Reading list" })).toBeInTheDocument();
    expect(appStore().getState().tabs).toHaveLength(2);
    await user.pointer({ keys: "[MouseRight]", target: row("Ideas") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    await user.click(within(menu).getByRole("menuitem", { name: t("page.openInNewTab") }));
    expect(await screen.findByRole("heading", { level: 1, name: "Ideas" })).toBeInTheDocument();
    expect(appStore().getState().tabs).toHaveLength(3);
    await user.click(row("Q1"));
    expect(await screen.findByRole("heading", { level: 1, name: "Q1" })).toBeInTheDocument();
    expect(appStore().getState().tabs).toHaveLength(3);
  });

  it("opens the page of a chip of the editor in a background tab", async () => {
    const { user, memory } = await renderPages([
      { name: "Personal", pages: [{ title: "Notes", content: paragraphs("Intro") }, { title: "Target", content: paragraphs("First") }] },
    ]);
    const ids = await idsOf(memory);
    const editor = await openPage(user, "Notes");
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, {
        type: "paragraph",
        content: [{ type: "pageRef", attrs: { pageId: ids.page("Target") } }],
      });
    });
    const chip = await waitFor(() => {
      const element = editor.view.dom.querySelector<HTMLElement>(".page-ref");
      expect(element).toHaveTextContent("Target");
      return element as HTMLElement;
    });
    await click(user, chip, "Control");
    expect(appStore().getState().tabs.map(pageOf)).toEqual([ids.page("Notes"), ids.page("Target")]);
    expect(screen.getByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
  });
});
