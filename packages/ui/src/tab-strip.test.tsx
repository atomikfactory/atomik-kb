// The tab strip above the main area (ADR-0034 section 4): what a tab shows, the buttons, the menu,
// the keyboard, drag and drop, scrolling. The actions of the tabs are spies here (the strip only
// asks for them); the last block runs the strip against the real tabs slice.

import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { MAX_TABS, currentEntry } from "./store-tabs";
import { t } from "./i18n";
import { beginDrag, layOut } from "./test/dnd";
import { row } from "./test/sidebar";
import { MODIFIER } from "./shortcuts";
import {
  closeButtonOf,
  historyTab,
  makeTab,
  renderLayout,
  seedTabs,
  spyOnTabActions,
  tabBox,
  tabList,
  tabNamed,
  tabNames,
  tabs,
} from "./test/tabs";

/** A strip with the tabs Journal (active), Reading list and Home, over spied actions. */
async function renderStrip(options: Parameters<typeof renderLayout>[0] = {}) {
  const app = await renderLayout(options);
  const space = app.ids.space("Personal");
  const journal = makeTab("journal", space, app.ids.page("Journal"));
  const reading = makeTab("reading", space, app.ids.page("Reading list"));
  const home = makeTab("home", space, null);
  const spies = spyOnTabActions(app.store);
  act(() => {
    seedTabs(app.store, [journal, reading, home], journal.id);
  });
  return { ...app, space, journal, reading, home, spies };
}

describe("the strip", () => {
  it("shows a tab for each entry: the page's title, Home, Untitled", async () => {
    const { store, ids } = await renderLayout({
      spaces: [{ name: "Personal", pages: [{ title: "Projects" }, { title: "" }] }],
    });
    const space = ids.space("Personal");
    act(() => {
      seedTabs(store, [makeTab("a", space, ids.page("Projects")), makeTab("b", space, null), makeTab("c", space, ids.page(""))], "tab-a");
    });
    expect(tabNames()).toEqual(["Projects", t("tabs.home"), t("page.untitled")]);
    expect(within(tabNamed(t("page.untitled"))).getByText(t("page.untitled"))).toHaveClass("tab-title");
  });

  it("is above the page, outside what scrolls, and has a tab list named for the open pages", async () => {
    await renderStrip();
    const list = tabList();
    const body = document.querySelector(".content-body");
    const strip = document.querySelector(".tab-strip");
    expect(strip).not.toBeNull();
    expect(body).not.toBeNull();
    expect(strip?.parentElement).toBe(body?.parentElement);
    expect(strip?.contains(body)).toBe(false);
    expect(strip?.contains(list)).toBe(true);
  });

  it("marks the active tab selected and leaves one tab stop", async () => {
    await renderStrip();
    expect(tabs().map((tab) => tab.getAttribute("aria-selected"))).toEqual(["true", "false", "false"]);
    expect(tabs().map((tab) => tab.tabIndex)).toEqual([0, -1, -1]);
  });

  it("makes the main area the panel of the active tab while a page screen shows", async () => {
    const { store } = await renderStrip();
    const panel = screen.getByRole("tabpanel", { name: "Journal" });
    expect(panel).toHaveClass("content-body");
    expect(tabNamed("Journal")).toHaveAttribute("aria-controls", panel.id);
    expect(tabNamed("Reading list")).not.toHaveAttribute("aria-controls");
    act(() => {
      store.getState().navigate("settings");
    });
    expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
    expect(tabNamed("Journal")).not.toHaveAttribute("aria-controls");
  });

  it("stays on the Settings and Diagnostics screens, where a tab returns to the main screen", async () => {
    const { store, journal, spies } = await renderStrip({ initialRoute: "settings" });
    expect(screen.getByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
    expect(tabList()).toBeInTheDocument();
    fireEvent.click(tabNamed("Journal"));
    expect(spies.activateTab).toHaveBeenCalledWith(journal.id);
    act(() => {
      store.getState().navigate("diagnostics");
    });
    expect(tabList()).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("tabs.back") })).toBeInTheDocument();
  });

  it("shows no tab list while there are no tabs (no space), and a New tab button that cannot be used", async () => {
    await renderLayout({ spaces: [] });
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("tabs.new") })).toBeDisabled();
    expect(screen.getByRole("button", { name: t("tabs.back") })).toBeDisabled();
  });
});

describe("what a tab shows", () => {
  it("shows the icon of the page and a title that follows a rename", async () => {
    const { store, ids, space } = await renderStrip();
    await act(async () => {
      await store.getState().setPageIcon(ids.page("Journal"), "🌱");
    });
    await waitFor(() => {
      expect(tabBox(tabNamed("Journal")).querySelector('[data-icon-kind="emoji"]')).toHaveTextContent("🌱");
    });
    expect(within(tabNamed("Journal")).getByText("Journal")).toHaveClass("tab-title");
    await act(async () => {
      await store.getState().renamePage(ids.page("Journal"), "Diary");
    });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Diary", "Reading list", t("tabs.home")]);
    });
    expect(space).not.toBe("");
  });

  it("gives a Home its own glyph and a title of Home", async () => {
    await renderStrip();
    const home = tabNamed(t("tabs.home"));
    expect(home.querySelector("svg")).not.toBeNull();
    expect(home.querySelector(".page-icon")).toBeNull();
    expect(within(home).getByText(t("tabs.home"))).toHaveClass("tab-title");
  });

  it("asks for the title and icon of a page of another space, and shows an empty title until they arrive", async () => {
    const { store, ids } = await renderStrip();
    const work = ids.space("Work");
    act(() => {
      seedTabs(store, [makeTab("m", work, ids.page("Meetings")), makeTab("j", ids.space("Personal"), ids.page("Journal"))], "tab-j");
    });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Meetings", "Journal"]);
    });
    expect(store.getState().pageLookup[ids.page("Meetings")]?.status).toBe("found");
  });

  it("shows no title, and a name of Untitled, for a page that is not looked up yet", async () => {
    const { store, ids } = await renderStrip();
    act(() => {
      store.setState({ lookupPage: () => undefined });
      seedTabs(store, [makeTab("m", ids.space("Work"), ids.page("Meetings"))], "tab-m");
    });
    const tab = tabs()[0];
    expect(tab).toHaveAccessibleName(t("page.untitled"));
    expect(tab?.querySelector(".tab-title")).toBeEmptyDOMElement();
  });

  it("shows a pinned tab as its icon alone, named by the title, with no close button", async () => {
    const { store, ids, space } = await renderStrip();
    act(() => {
      seedTabs(
        store,
        [makeTab("p", space, ids.page("Journal"), { pinned: true }), makeTab("r", space, ids.page("Reading list"))],
        "tab-r",
      );
    });
    const label = t("tabs.pinnedLabel", { title: "Journal" });
    const pinned = tabNamed(label);
    expect(pinned).toHaveAttribute("title", label);
    expect(pinned.querySelector(".tab-title")).toBeNull();
    expect(pinned.querySelector(".page-icon")).not.toBeNull();
    expect(closeButtonOf(pinned)).toBeNull();
    expect(closeButtonOf(tabNamed("Reading list"))).not.toBeNull();
  });

  it("gives a tab its full title as a tooltip, and a close button its shortcut", async () => {
    await renderStrip();
    expect(tabNamed("Journal")).toHaveAttribute("title", "Journal");
    expect(closeButtonOf(tabNamed("Journal"))).toHaveAttribute("title", t("tabs.closeTitle", { shortcut: `${MODIFIER}+W` }));
    expect(screen.getByRole("button", { name: t("tabs.new") })).toHaveAttribute(
      "title",
      t("tabs.newTitle", { shortcut: `${MODIFIER}+T` }),
    );
  });
});

describe("the buttons", () => {
  it("brings a tab to the front when it is pressed", async () => {
    const { user, spies, reading } = await renderStrip();
    await user.click(tabNamed("Reading list"));
    expect(spies.activateTab).toHaveBeenCalledWith(reading.id);
    expect(spies.activateTab).toHaveBeenCalledTimes(1);
  });

  it("closes a tab with its close button, and does not bring it to the front first", async () => {
    const { user, spies, reading } = await renderStrip();
    await user.click(closeButtonOf(tabNamed("Reading list")) as HTMLElement);
    expect(spies.closeTab).toHaveBeenCalledWith(reading.id);
    expect(spies.activateTab).not.toHaveBeenCalled();
  });

  it("closes a tab with the middle button, which does not start the autoscroll", async () => {
    const { user, spies, reading } = await renderStrip();
    const tab = tabNamed("Reading list");
    expect(fireEvent.mouseDown(tab, { button: 1 })).toBe(false);
    expect(fireEvent.mouseDown(tab, { button: 0 })).toBe(true);
    await user.pointer({ keys: "[MouseMiddle]", target: tab });
    expect(spies.closeTab).toHaveBeenCalledTimes(1);
    expect(spies.closeTab).toHaveBeenCalledWith(reading.id);
    expect(spies.activateTab).not.toHaveBeenCalled();
    fireEvent(tab, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 2 }));
    expect(spies.closeTab).toHaveBeenCalledTimes(1);
  });

  it("opens a tab with the New tab button", async () => {
    const { user, spies } = await renderStrip();
    await user.click(screen.getByRole("button", { name: t("tabs.new") }));
    expect(spies.newTab).toHaveBeenCalledTimes(1);
  });

  it("shows the main screen when it opens a tab from Settings", async () => {
    const { user, store } = await renderStrip({ initialRoute: "settings" });
    await user.click(screen.getByRole("button", { name: t("tabs.new") }));
    expect(store.getState().route).toBe("home");
  });

  it("cannot open a tab at 50 tabs", async () => {
    const { store, space } = await renderStrip();
    const many = Array.from({ length: MAX_TABS }, (_, n) => makeTab(String(n), space, null));
    act(() => {
      seedTabs(store, many, "tab-0");
    });
    expect(screen.getByRole("button", { name: t("tabs.new") })).toBeDisabled();
    act(() => {
      seedTabs(store, many.slice(1), "tab-1");
    });
    expect(screen.getByRole("button", { name: t("tabs.new") })).toBeEnabled();
  });

  it("opens a tab on a double click on the empty space of the strip, not on a tab", async () => {
    const { spies } = await renderStrip();
    fireEvent.doubleClick(tabNamed("Journal"));
    expect(spies.newTab).not.toHaveBeenCalled();
    const space = document.querySelector(".tab-strip-space");
    expect(space).not.toBeNull();
    fireEvent.doubleClick(space as Element);
    expect(spies.newTab).toHaveBeenCalledTimes(1);
  });

  it("goes back and forward with its buttons, which are disabled at the ends of the history", async () => {
    const { user, store, ids, space, spies } = await renderStrip();
    const back = screen.getByRole("button", { name: t("tabs.back") });
    const forward = screen.getByRole("button", { name: t("tabs.forward") });
    expect(back).toBeDisabled();
    expect(forward).toBeDisabled();
    const pages = [ids.page("Journal"), ids.page("Projects"), ids.page("Ideas")];
    act(() => {
      seedTabs(store, [historyTab("h", space, pages, 1)], "tab-h");
    });
    expect(back).toBeEnabled();
    expect(forward).toBeEnabled();
    await user.click(back);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    await user.click(forward);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
    act(() => {
      seedTabs(store, [historyTab("h", space, pages, 0)], "tab-h");
    });
    expect(back).toBeDisabled();
    expect(forward).toBeEnabled();
    act(() => {
      seedTabs(store, [historyTab("h", space, pages, 2)], "tab-h");
    });
    expect(back).toBeEnabled();
    expect(forward).toBeDisabled();
  });

  it("describes back and forward with their keys", async () => {
    await renderStrip();
    expect(screen.getByRole("button", { name: t("tabs.back") })).toHaveAttribute("title", t("tabs.backTitle", { shortcut: "Alt+Left" }));
    expect(screen.getByRole("button", { name: t("tabs.forward") })).toHaveAttribute(
      "title",
      t("tabs.forwardTitle", { shortcut: "Alt+Right" }),
    );
  });

  it("shows the main screen when back or forward is pressed from Settings", async () => {
    const { user, store, ids, space } = await renderStrip({ initialRoute: "settings" });
    act(() => {
      seedTabs(store, [historyTab("h", space, [ids.page("Journal"), ids.page("Projects")], 1)], "tab-h");
    });
    await user.click(screen.getByRole("button", { name: t("tabs.back") }));
    expect(store.getState().route).toBe("home");
  });
});

describe("the menu of a tab", () => {
  const ITEMS = [t("tabs.pin"), t("tabs.close"), t("tabs.closeOthers"), t("tabs.closeRight"), t("tabs.reopen")];

  it("opens on a right click with the commands, and closes on Escape", async () => {
    const { user } = await renderStrip();
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    const menu = await screen.findByRole("menu", { name: t("tabs.menu") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(ITEMS);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("opens from the context menu key and from Shift+F10, below the focused tab", async () => {
    const { user } = await renderStrip();
    tabNamed("Reading list").focus();
    await user.keyboard("{ContextMenu}");
    expect(await screen.findByRole("menu", { name: t("tabs.menu") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(tabNamed("Reading list")).toHaveFocus();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    expect(await screen.findByRole("menu", { name: t("tabs.menu") })).toBeInTheDocument();
  });

  it("opens on the tab it was asked for, whichever is in front", async () => {
    const { user, spies, reading } = await renderStrip();
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.pin") }));
    expect(spies.setTabPinned).toHaveBeenCalledWith(reading.id, true);
    expect(spies.activateTab).not.toHaveBeenCalled();
  });

  it("offers Unpin on a pinned tab, and its Close", async () => {
    const { user, store, ids, space, spies } = await renderStrip();
    act(() => {
      seedTabs(store, [makeTab("p", space, ids.page("Journal"), { pinned: true }), makeTab("r", space, ids.page("Reading list"))], "tab-r");
    });
    await user.pointer({ keys: "[MouseRight]", target: tabNamed(t("tabs.pinnedLabel", { title: "Journal" })) });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.unpin") }));
    expect(spies.setTabPinned).toHaveBeenCalledWith("tab-p", false);
    await user.pointer({ keys: "[MouseRight]", target: tabNamed(t("tabs.pinnedLabel", { title: "Journal" })) });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.close") }));
    expect(spies.closeTab).toHaveBeenCalledWith("tab-p");
  });

  it("closes the tab, the other tabs and the tabs to the right", async () => {
    const { user, spies, reading } = await renderStrip();
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.close") }));
    expect(spies.closeTab).toHaveBeenCalledWith(reading.id);
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.closeOthers") }));
    expect(spies.closeOtherTabs).toHaveBeenCalledWith(reading.id);
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.closeRight") }));
    expect(spies.closeTabsToRight).toHaveBeenCalledWith(reading.id);
  });

  it("reopens the closed tab only when there is one", async () => {
    const { user, store, spies, journal, reading, home } = await renderStrip();
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Journal") });
    expect(await screen.findByRole("menuitem", { name: t("tabs.reopen") })).toBeDisabled();
    await user.keyboard("{Escape}");
    act(() => {
      seedTabs(store, [journal, reading, home], journal.id, [makeTab("old", journal.entries[0]?.spaceId ?? "", null)]);
    });
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Journal") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.reopen") }));
    expect(spies.reopenClosedTab).toHaveBeenCalledTimes(1);
  });

  it("disables Close other tabs and Close tabs to the right when there is nothing to close, pinned tabs staying", async () => {
    const { user, store, ids, space } = await renderStrip();
    act(() => {
      seedTabs(
        store,
        [
          makeTab("p", space, ids.page("Journal"), { pinned: true }),
          makeTab("a", space, ids.page("Projects")),
          makeTab("b", space, ids.page("Ideas")),
        ],
        "tab-a",
      );
    });
    // The last tab has none to its right; the other unpinned tab is there to close.
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Ideas") });
    expect(await screen.findByRole("menuitem", { name: t("tabs.closeRight") })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: t("tabs.closeOthers") })).toBeEnabled();
    await user.keyboard("{Escape}");
    // The first unpinned tab has one to its right.
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Projects") });
    expect(await screen.findByRole("menuitem", { name: t("tabs.closeRight") })).toBeEnabled();
    await user.keyboard("{Escape}");
    // The pinned tab sees only unpinned tabs to close.
    await user.pointer({ keys: "[MouseRight]", target: tabNamed(t("tabs.pinnedLabel", { title: "Journal" })) });
    expect(await screen.findByRole("menuitem", { name: t("tabs.closeOthers") })).toBeEnabled();
    await user.keyboard("{Escape}");
    act(() => {
      seedTabs(store, [makeTab("p", space, ids.page("Journal"), { pinned: true }), makeTab("a", space, ids.page("Projects"))], "tab-a");
    });
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Projects") });
    expect(await screen.findByRole("menuitem", { name: t("tabs.closeOthers") })).toBeDisabled();
  });
});

describe("the keyboard in the tab list", () => {
  it("moves focus with the arrow keys, Home and End, wrapping, without bringing a tab to the front", async () => {
    const { user, spies } = await renderStrip();
    tabNamed("Journal").focus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed("Reading list")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed(t("tabs.home"))).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed("Journal")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(tabNamed(t("tabs.home"))).toHaveFocus();
    await user.keyboard("{Home}");
    expect(tabNamed("Journal")).toHaveFocus();
    await user.keyboard("{End}");
    expect(tabNamed(t("tabs.home"))).toHaveFocus();
    expect(spies.activateTab).not.toHaveBeenCalled();
  });

  it("brings the focused tab to the front with Enter and with Space", async () => {
    const { user, spies, reading, home } = await renderStrip();
    tabNamed("Reading list").focus();
    await user.keyboard("{Enter}");
    expect(spies.activateTab).toHaveBeenLastCalledWith(reading.id);
    tabNamed(t("tabs.home")).focus();
    await user.keyboard(" ");
    expect(spies.activateTab).toHaveBeenLastCalledWith(home.id);
  });

  it("keeps one tab stop, where focus was last while the same tab is in front, else the active tab", async () => {
    const { user, store, journal, reading, home } = await renderStrip();
    tabNamed("Journal").focus();
    await user.keyboard("{ArrowRight}");
    expect(tabs().map((tab) => tab.tabIndex)).toEqual([-1, 0, -1]);
    act(() => {
      seedTabs(store, [journal, reading, home], home.id);
    });
    expect(tabs().map((tab) => tab.tabIndex)).toEqual([-1, -1, 0]);
  });

  it("leaves the close buttons out of the tab order", async () => {
    await renderStrip();
    for (const tab of tabs()) {
      expect(closeButtonOf(tab)?.tabIndex).toBe(-1);
    }
  });

  it("closes the focused tab with Delete, and focus goes to the tab that takes its place", async () => {
    const { user, store, journal, reading, home, spies } = await renderStrip();
    spies.closeTab.mockImplementation((id: string) => {
      store.setState((state) => ({ tabs: state.tabs.filter((tab) => tab.id !== id) }));
    });
    tabNamed("Reading list").focus();
    await user.keyboard("{Delete}");
    expect(spies.closeTab).toHaveBeenCalledWith(reading.id);
    expect(tabNamed(t("tabs.home"))).toHaveFocus();
    await user.keyboard("{Delete}");
    expect(spies.closeTab).toHaveBeenLastCalledWith(home.id);
    expect(tabNamed("Journal")).toHaveFocus();
    expect(tabNames()).toEqual(["Journal"]);
    expect(journal.id).toBe("tab-journal");
  });

  it("leaves Delete with a modifier, and keys of other things, alone", async () => {
    const { user, spies } = await renderStrip();
    tabNamed("Reading list").focus();
    await user.keyboard("{Control>}{Delete}{/Control}");
    await user.keyboard("{Shift>}{Delete}{/Shift}");
    await user.keyboard("a");
    expect(spies.closeTab).not.toHaveBeenCalled();
  });

  it("puts focus on the active tab when the tab that had it closes with the mouse and none follows", async () => {
    const { user, store, spies } = await renderStrip();
    spies.closeTab.mockImplementation((id: string) => {
      store.setState((state) => ({ tabs: state.tabs.filter((tab) => tab.id !== id) }));
    });
    const close = closeButtonOf(tabNamed("Reading list")) as HTMLElement;
    close.focus();
    await user.click(close);
    expect(tabNames()).toEqual(["Journal", t("tabs.home")]);
    expect(tabNamed(t("tabs.home"))).toHaveFocus();
  });
});

describe("dragging tabs", () => {
  /** A tab's box with the geometry of a 100 px wide tab, since a test DOM has no layout. */
  function placed(name: string): HTMLElement {
    const box = tabBox(tabNamed(name));
    layOut(box);
    box.getBoundingClientRect = () => ({
      x: 0, y: 0, left: 0, right: 100, top: 0, bottom: 30, width: 100, height: 30, toJSON: () => ({}),
    });
    return box;
  }

  function dragOver(source: HTMLElement, target: HTMLElement, clientX: number) {
    const drag = beginDrag(source);
    const allowed = !fireEvent.dragOver(target, { dataTransfer: drag.dataTransfer, clientX });
    return { ...drag, allowed, dropAt: () => fireEvent.drop(target, { dataTransfer: drag.dataTransfer, clientX }) };
  }

  it("makes each tab draggable", async () => {
    await renderStrip();
    for (const tab of tabs()) {
      expect(tabBox(tab)).toHaveAttribute("draggable", "true");
    }
  });

  it("moves a tab to where it is dropped: after the target (to its right) and before it", async () => {
    const { spies, journal, home } = await renderStrip();
    // Journal, Reading list, Home. Journal dropped on the right half of Reading list lands at 1.
    let drag = dragOver(tabBox(tabNamed("Journal")), placed("Reading list"), 80);
    expect(drag.allowed).toBe(true);
    expect(tabBox(tabNamed("Reading list"))).toHaveClass("drop-after");
    drag.dropAt();
    drag.end();
    expect(spies.moveTab).toHaveBeenLastCalledWith(journal.id, 1);
    // Journal dropped on the right half of Home lands at the end.
    drag = dragOver(tabBox(tabNamed("Journal")), placed(t("tabs.home")), 80);
    drag.dropAt();
    drag.end();
    expect(spies.moveTab).toHaveBeenLastCalledWith(journal.id, 2);
    // Home dropped on the left half of Journal lands first.
    drag = dragOver(tabBox(tabNamed(t("tabs.home"))), placed("Journal"), 10);
    expect(tabBox(tabNamed("Journal"))).toHaveClass("drop-before");
    drag.dropAt();
    drag.end();
    expect(spies.moveTab).toHaveBeenLastCalledWith(home.id, 0);
  });

  it("allows no drop that would leave the order as it is", async () => {
    const { spies } = await renderStrip();
    // Journal on itself, on the left of Reading list (right where it is), and a tab that is not from here.
    expect(dragOver(tabBox(tabNamed("Journal")), placed("Journal"), 10).allowed).toBe(false);
    expect(dragOver(tabBox(tabNamed("Journal")), placed("Reading list"), 10).allowed).toBe(false);
    expect(dragOver(tabBox(tabNamed("Reading list")), placed("Journal"), 80).allowed).toBe(false);
    expect(spies.moveTab).not.toHaveBeenCalled();
    expect(document.querySelector(".drop-before, .drop-after")).toBeNull();
  });

  it("keeps a tab in its group: pinned and unpinned tabs do not trade places", async () => {
    const { store, ids, space, spies } = await renderStrip();
    act(() => {
      seedTabs(
        store,
        [makeTab("p", space, ids.page("Journal"), { pinned: true }), makeTab("a", space, ids.page("Projects")), makeTab("b", space, ids.page("Ideas"))],
        "tab-a",
      );
    });
    const pinned = tabBox(tabNamed(t("tabs.pinnedLabel", { title: "Journal" })));
    layOut(pinned);
    expect(dragOver(tabBox(tabNamed("Ideas")), pinned, 10).allowed).toBe(false);
    expect(dragOver(pinned, placed("Ideas"), 80).allowed).toBe(false);
    expect(spies.moveTab).not.toHaveBeenCalled();
    const drag = dragOver(tabBox(tabNamed("Ideas")), placed("Projects"), 10);
    expect(drag.allowed).toBe(true);
    drag.dropAt();
    drag.end();
    expect(spies.moveTab).toHaveBeenCalledWith("tab-b", 1);
  });

  it("carries a marker of its own, and ignores a drag that is not a tab", async () => {
    const { spies } = await renderStrip();
    const target = placed("Reading list");
    const drag = beginDrag(tabBox(tabNamed("Journal")));
    expect(drag.dataTransfer.types).toEqual(["application/x-atomik-tab"]);
    drag.end();
    // A drag of something else (a page of the sidebar, a file): nothing here takes it.
    expect(fireEvent.dragOver(target, { clientX: 80 })).toBe(true);
    fireEvent.drop(target, { clientX: 80 });
    expect(spies.moveTab).not.toHaveBeenCalled();
  });

  it("clears its marks when the drag ends or leaves", async () => {
    await renderStrip();
    const reading = placed("Reading list");
    const drag = dragOver(tabBox(tabNamed("Journal")), reading, 80);
    expect(tabBox(tabNamed("Journal"))).toHaveClass("is-dragging");
    expect(reading).toHaveClass("drop-after");
    fireEvent.dragLeave(reading, { dataTransfer: drag.dataTransfer });
    expect(reading).not.toHaveClass("drop-after");
    drag.end();
    expect(tabBox(tabNamed("Journal"))).not.toHaveClass("is-dragging");
  });
});

describe("a strip that is wider than the room", () => {
  /** Gives the tab list the widths of a layout: 300 px of room for tabs 200 px wide, one after the other, after the list's 2 px padding. */
  function measure(list: HTMLElement, room: number): void {
    Object.defineProperty(list, "clientWidth", { configurable: true, value: room });
    Object.defineProperty(list, "scrollWidth", { configurable: true, value: 600 });
    const boxes = Array.from(list.querySelectorAll<HTMLElement>(".tab"));
    boxes.forEach((box, index) => {
      Object.defineProperty(box, "offsetLeft", { configurable: true, value: 2 + index * 202 });
      Object.defineProperty(box, "offsetWidth", { configurable: true, value: 200 });
    });
  }

  it("keeps the active tab in view when it changes", async () => {
    const { store, journal, reading, home } = await renderStrip();
    const list = tabList();
    measure(list, 300);
    act(() => {
      seedTabs(store, [journal, reading, home], home.id);
    });
    // The third tab spans 406 to 606: the list scrolls so that its right edge and the padding are in view.
    expect(list.scrollLeft).toBe(308);
    act(() => {
      seedTabs(store, [journal, reading, home], journal.id);
    });
    expect(list.scrollLeft).toBe(0);
    // The second tab spans 204 to 404: only its right edge is out of the 0 to 300 window.
    act(() => {
      seedTabs(store, [journal, reading, home], reading.id);
    });
    expect(list.scrollLeft).toBe(106);
    // Scrolled on by hand to 250 to 550, it sticks out on the left: its left edge and the padding come into view.
    list.scrollLeft = 250;
    act(() => {
      seedTabs(store, [journal, reading, home], reading.id);
    });
    expect(list.scrollLeft).toBe(202);
    // One that is in view is left where it is.
    list.scrollLeft = 150;
    act(() => {
      seedTabs(store, [journal, reading, home], reading.id);
    });
    expect(list.scrollLeft).toBe(150);
  });

  it("scrolls sideways with the wheel, and cancels the page's scroll when it does", async () => {
    await renderStrip();
    const list = tabList();
    measure(list, 300);
    const wheel = new WheelEvent("wheel", { deltaY: 120, bubbles: true, cancelable: true });
    list.dispatchEvent(wheel);
    expect(list.scrollLeft).toBe(120);
    expect(wheel.defaultPrevented).toBe(true);
    const back = new WheelEvent("wheel", { deltaY: -50, bubbles: true, cancelable: true });
    list.dispatchEvent(back);
    expect(list.scrollLeft).toBe(70);
  });

  it("leaves a sideways wheel, a zoom and a strip that fits alone", async () => {
    await renderStrip();
    const list = tabList();
    const fits = new WheelEvent("wheel", { deltaY: 120, bubbles: true, cancelable: true });
    list.dispatchEvent(fits); // jsdom: no overflow at all
    expect(fits.defaultPrevented).toBe(false);
    measure(list, 300);
    const sideways = new WheelEvent("wheel", { deltaX: 120, deltaY: 10, bubbles: true, cancelable: true });
    list.dispatchEvent(sideways);
    expect(sideways.defaultPrevented).toBe(false);
    const zoom = new WheelEvent("wheel", { deltaY: 120, ctrlKey: true, bubbles: true, cancelable: true });
    list.dispatchEvent(zoom);
    expect(zoom.defaultPrevented).toBe(false);
    expect(list.scrollLeft).toBe(0);
  });
});

describe("with the sidebar hidden", () => {
  it("keeps the strip beside the button that shows the sidebar again", async () => {
    const { user } = await renderStrip();
    await user.click(screen.getByRole("button", { name: t("sidebar.hide") }));
    expect(document.querySelector(".layout")).toHaveClass("is-sidebar-hidden");
    expect(screen.getByRole("button", { name: t("sidebar.show") })).toBeInTheDocument();
    expect(tabList()).toBeInTheDocument();
  });
});

describe("the strip with the real tabs", () => {
  it("starts with the Home of the first space, and follows the page that opens in the active tab", async () => {
    const { user } = await renderLayout();
    expect(tabNames()).toEqual([t("tabs.home")]);
    expect(screen.getByRole("tabpanel", { name: t("tabs.home") })).toBeInTheDocument();
    await user.click(row("Journal"));
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal"]);
    });
    expect(screen.getByRole("tabpanel", { name: "Journal" })).toBeInTheDocument();
    expect(await screen.findByRole("textbox", { name: t("editor.label") })).toBeInTheDocument();
  });

  it("goes back and forward in the history of the tab with its buttons", async () => {
    const { user } = await renderLayout();
    const back = screen.getByRole("button", { name: t("tabs.back") });
    const forward = screen.getByRole("button", { name: t("tabs.forward") });
    expect(back).toBeDisabled();
    await user.click(row("Journal"));
    await user.click(row("Reading list"));
    await waitFor(() => {
      expect(tabNames()).toEqual(["Reading list"]);
    });
    expect(back).toBeEnabled();
    await user.click(back);
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal"]);
    });
    expect(forward).toBeEnabled();
    await user.click(back);
    await waitFor(() => {
      expect(tabNames()).toEqual([t("tabs.home")]);
    });
    expect(back).toBeDisabled();
    await user.click(forward);
    await user.click(forward);
    await waitFor(() => {
      expect(tabNames()).toEqual(["Reading list"]);
    });
    expect(forward).toBeDisabled();
  });

  it("opens a Home tab with New tab, brings tabs to the front, and closes one with the middle button", async () => {
    const { user } = await renderLayout();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("tabs.new") }));
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", t("tabs.home")]);
    });
    expect(tabNamed(t("tabs.home"))).toHaveAttribute("aria-selected", "true");
    await user.click(tabNamed("Journal"));
    expect(tabNamed("Journal")).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("tabpanel", { name: "Journal" })).toBeInTheDocument();
    await user.pointer({ keys: "[MouseMiddle]", target: tabNamed(t("tabs.home")) });
    expect(tabNames()).toEqual(["Journal"]);
  });

  it("closes the last tab into a Home tab", async () => {
    const { user } = await renderLayout();
    await user.click(row("Journal"));
    await user.click(closeButtonOf(tabNamed("Journal")) as HTMLElement);
    await waitFor(() => {
      expect(tabNames()).toEqual([t("tabs.home")]);
    });
  });

  it("pins a tab from its menu: its icon alone, no close button", async () => {
    const { user } = await renderLayout();
    await user.click(row("Journal"));
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Journal") });
    await user.click(await screen.findByRole("menuitem", { name: t("tabs.pin") }));
    const pinned = tabNamed(t("tabs.pinnedLabel", { title: "Journal" }));
    expect(closeButtonOf(pinned)).toBeNull();
    await user.pointer({ keys: "[MouseRight]", target: pinned });
    expect(await screen.findByRole("menuitem", { name: t("tabs.unpin") })).toBeInTheDocument();
  });

  it("reorders tabs by dragging, with the real order kept in the store", async () => {
    const { user, store, ids } = await renderLayout();
    await user.click(row("Journal"));
    const space = ids.space("Personal");
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Projects"), spaceId: space, disposition: "foreground" });
    });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", "Projects"]);
    });
    const target = tabBox(tabNamed("Journal"));
    target.getBoundingClientRect = () => ({ x: 0, y: 0, left: 0, right: 100, top: 0, bottom: 30, width: 100, height: 30, toJSON: () => ({}) });
    const drag = beginDrag(tabBox(tabNamed("Projects")));
    fireEvent.dragOver(target, { dataTransfer: drag.dataTransfer, clientX: 10 });
    fireEvent.drop(target, { dataTransfer: drag.dataTransfer, clientX: 10 });
    drag.end();
    expect(tabNames()).toEqual(["Projects", "Journal"]);
    expect(store.getState().tabs.map((tab) => currentEntry(tab).pageId)).toEqual([ids.page("Projects"), ids.page("Journal")]);
  });

  it("returns to the main screen when a tab is pressed on the Settings screen", async () => {
    const { user } = await renderLayout();
    await user.click(row("Journal"));
    await user.click(screen.getByRole("button", { name: t("nav.settings") }));
    expect(await screen.findByRole("heading", { name: t("settings.title") })).toBeInTheDocument();
    expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument();
    await user.click(tabNamed("Journal"));
    expect(screen.queryByRole("heading", { name: t("settings.title") })).not.toBeInTheDocument();
    expect(screen.getByRole("tabpanel", { name: "Journal" })).toBeInTheDocument();
  });

  it("shows the title of a page of another space, and switches the sidebar to the space of a tab", async () => {
    const { user, store, ids } = await renderLayout();
    await user.click(row("Journal"));
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Meetings"), spaceId: ids.space("Work"), disposition: "foreground" });
    });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", "Meetings"]);
    });
    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    });
    await user.click(tabNamed("Journal"));
    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Personal"));
    });
    expect(tabNames()).toEqual(["Journal", "Meetings"]);
  });
});
