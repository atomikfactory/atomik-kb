// The tab keys and the mouse's back and forward buttons (ADR-0034 section 4): what each key event
// asks for, where the keys work (anywhere but inside a dialog or menu, in fields and the editor
// too, Back and Forward not in form fields), and the whole thing against the real tabs. Ctrl+T,
// Ctrl+W and Ctrl+Tab cannot be pressed in a browser, so the tests dispatch the events.

import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { tabCommand } from "./shortcuts";
import type { TabCommand, TabKeyEvent } from "./shortcuts";
import { currentEntry } from "./store-tabs";
import { findEditor } from "./test/page";
import { row } from "./test/sidebar";
import { makeTab, renderLayout, seedTabs, spyOnTabActions, tabNames, tabNamed, tabs } from "./test/tabs";

/** A key event, with the keys it holds. */
function key(init: Partial<TabKeyEvent> & { key: string }): TabKeyEvent {
  return { code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, isComposing: false, ...init };
}

const CTRL = { ctrlKey: true };

describe("the command a key event asks for", () => {
  const cases: [string, Partial<TabKeyEvent> & { key: string }, TabCommand | null][] = [
    ["Ctrl+T", { key: "t", ...CTRL }, { kind: "new" }],
    ["Cmd+T", { key: "t", metaKey: true }, { kind: "new" }],
    ["Ctrl+Shift+T (the key reports a capital)", { key: "T", ...CTRL, shiftKey: true }, { kind: "reopen" }],
    ["Cmd+Shift+T", { key: "T", metaKey: true, shiftKey: true }, { kind: "reopen" }],
    ["T alone", { key: "t" }, null],
    ["Ctrl+Alt+T", { key: "t", ...CTRL, altKey: true }, null],
    ["Ctrl+W", { key: "w", ...CTRL }, { kind: "close" }],
    ["Cmd+W", { key: "w", metaKey: true }, { kind: "close" }],
    ["Ctrl+Shift+W (closes a window elsewhere)", { key: "W", ...CTRL, shiftKey: true }, null],
    ["Ctrl+F4", { key: "F4", ...CTRL }, { kind: "close" }],
    ["F4 alone", { key: "F4" }, null],
    ["Ctrl+Tab", { key: "Tab", ...CTRL }, { kind: "adjacent", delta: 1 }],
    ["Ctrl+Shift+Tab", { key: "Tab", ...CTRL, shiftKey: true }, { kind: "adjacent", delta: -1 }],
    ["Ctrl+PageDown", { key: "PageDown", ...CTRL }, { kind: "adjacent", delta: 1 }],
    ["Ctrl+PageUp", { key: "PageUp", ...CTRL }, { kind: "adjacent", delta: -1 }],
    ["Ctrl+Shift+PageDown", { key: "PageDown", ...CTRL, shiftKey: true }, { kind: "move", delta: 1 }],
    ["Ctrl+Shift+PageUp", { key: "PageUp", ...CTRL, shiftKey: true }, { kind: "move", delta: -1 }],
    ["Cmd+PageDown (not Ctrl)", { key: "PageDown", metaKey: true }, null],
    ["PageDown alone", { key: "PageDown" }, null],
    ["Ctrl+1", { key: "1", code: "Digit1", ...CTRL }, { kind: "at", position: 0 }],
    ["Cmd+8", { key: "8", code: "Digit8", metaKey: true }, { kind: "at", position: 7 }],
    ["Ctrl+9 (the last tab)", { key: "9", code: "Digit9", ...CTRL }, { kind: "at", position: -1 }],
    ["Ctrl+0", { key: "0", code: "Digit0", ...CTRL }, null],
    ["a digit alone", { key: "2", code: "Digit2" }, null],
    ["Ctrl+Shift+1", { key: "!", code: "Digit1", ...CTRL, shiftKey: true }, null],
    ["Ctrl+Numpad3", { key: "3", code: "Numpad3", ...CTRL }, { kind: "at", position: 2 }],
    // AZERTY: the top row types `&é"'(-è_ç` without Shift, and its digits with it.
    ["Ctrl+& on AZERTY (the first digit key)", { key: "&", code: "Digit1", ...CTRL }, { kind: "at", position: 0 }],
    ["Ctrl+é on AZERTY", { key: "é", code: "Digit2", ...CTRL }, { kind: "at", position: 1 }],
    ["Ctrl+ç on AZERTY (the ninth)", { key: "ç", code: "Digit9", ...CTRL }, { kind: "at", position: -1 }],
    ["Ctrl+Shift+& on AZERTY", { key: "1", code: "Digit1", ...CTRL, shiftKey: true }, null],
    ["AltGr+é on AZERTY, which types ~ (Ctrl and Alt)", { key: "~", code: "Digit2", ...CTRL, altKey: true }, null],
    // The letters follow what the key types; a layout with no Latin letters falls back to the physical key.
    ["Ctrl+T on a Cyrillic layout", { key: "е", code: "KeyT", ...CTRL }, { kind: "new" }],
    ["Ctrl+Y on Dvorak, where the physical T types y", { key: "y", code: "KeyT", ...CTRL }, null],
    ["Ctrl+T on Dvorak, where the physical Y types t", { key: "t", code: "KeyY", ...CTRL }, { kind: "new" }],
    ["Alt+Left", { key: "ArrowLeft", altKey: true }, { kind: "history", delta: -1 }],
    ["Alt+Right", { key: "ArrowRight", altKey: true }, { kind: "history", delta: 1 }],
    ["Alt+Shift+Left (selects)", { key: "ArrowLeft", altKey: true, shiftKey: true }, null],
    ["AltGr+Left (Ctrl and Alt)", { key: "ArrowLeft", altKey: true, ...CTRL }, null],
    ["Left alone", { key: "ArrowLeft" }, null],
    ["Alt+Up", { key: "ArrowUp", altKey: true }, null],
    ["Cmd+[ off macOS", { key: "[", metaKey: true }, null],
    // ADR-0040: Ctrl+[ and Ctrl+] join Alt+Left and Alt+Right off macOS.
    ["Ctrl+[", { key: "[", code: "BracketLeft", ...CTRL }, { kind: "history", delta: -1 }],
    ["Ctrl+]", { key: "]", code: "BracketRight", ...CTRL }, { kind: "history", delta: 1 }],
    ["Ctrl+Shift+[ (types a brace)", { key: "{", code: "BracketLeft", ...CTRL, shiftKey: true }, null],
    ["Ctrl+Shift+] with the bracket key reported", { key: "]", ...CTRL, shiftKey: true }, null],
    ["AltGr+[ (Ctrl and Alt, types a bracket on some layouts)", { key: "[", altKey: true, ...CTRL }, null],
    ["Ctrl+Cmd+[", { key: "[", ...CTRL, metaKey: true }, null],
    ["[ alone", { key: "[" }, null],
  ];

  for (const [name, event, expected] of cases) {
    it(`reads ${name}`, () => {
      expect(tabCommand(key(event), false)).toEqual(expected);
    });
  }

  it("reads Cmd+[ and Cmd+] on macOS, where Option+arrows move by word", () => {
    expect(tabCommand(key({ key: "[", metaKey: true }), true)).toEqual({ kind: "history", delta: -1 });
    expect(tabCommand(key({ key: "]", metaKey: true }), true)).toEqual({ kind: "history", delta: 1 });
    expect(tabCommand(key({ key: "ArrowLeft", altKey: true }), true)).toBeNull();
    expect(tabCommand(key({ key: "ArrowRight", altKey: true }), true)).toBeNull();
    expect(tabCommand(key({ key: "[", metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(tabCommand(key({ key: "[" }), true)).toBeNull();
    // Ctrl+[ is Escape in a terminal and stays out of it there: only Cmd on macOS.
    expect(tabCommand(key({ key: "[", ...CTRL }), true)).toBeNull();
    expect(tabCommand(key({ key: "]", ...CTRL }), true)).toBeNull();
    // The rest is the same there.
    expect(tabCommand(key({ key: "t", metaKey: true }), true)).toEqual({ kind: "new" });
    expect(tabCommand(key({ key: "Tab", ...CTRL }), true)).toEqual({ kind: "adjacent", delta: 1 });
  });

  it("reads nothing while a composition is on (an input method owns the keys)", () => {
    expect(tabCommand(key({ key: "t", ...CTRL, isComposing: true }), false)).toBeNull();
    expect(tabCommand(key({ key: "ArrowLeft", altKey: true, isComposing: true }), false)).toBeNull();
  });
});

/** The layout over a strip of Journal (in front), Reading list and Home, with spied actions. */
async function renderKeys(options: Parameters<typeof renderLayout>[0] = {}) {
  const app = await renderLayout(options);
  const space = app.ids.space("Personal");
  const all = [
    makeTab("journal", space, app.ids.page("Journal")),
    makeTab("reading", space, app.ids.page("Reading list")),
    makeTab("home", space, null),
  ];
  const spies = spyOnTabActions(app.store);
  act(() => {
    seedTabs(app.store, all, "tab-journal");
  });
  return { ...app, space, spies };
}

/** Dispatches a keydown and says whether the page's own handling stayed (nothing cancelled it). */
function press(target: Element, init: KeyboardEventInit): boolean {
  return fireEvent.keyDown(target, init);
}

describe("the tab keys in the app", () => {
  it("opens, closes and reopens tabs", async () => {
    const { spies } = await renderKeys();
    expect(press(document.body, { key: "t", ctrlKey: true })).toBe(false);
    expect(spies.newTab).toHaveBeenCalledTimes(1);
    expect(press(document.body, { key: "t", metaKey: true })).toBe(false);
    expect(spies.newTab).toHaveBeenCalledTimes(2);

    expect(press(document.body, { key: "w", ctrlKey: true })).toBe(false);
    expect(spies.closeTab).toHaveBeenLastCalledWith("tab-journal");
    expect(press(document.body, { key: "F4", ctrlKey: true })).toBe(false);
    expect(spies.closeTab).toHaveBeenCalledTimes(2);
    expect(press(document.body, { key: "w", metaKey: true })).toBe(false);
    expect(spies.closeTab).toHaveBeenCalledTimes(3);

    expect(press(document.body, { key: "T", ctrlKey: true, shiftKey: true })).toBe(false);
    expect(spies.reopenClosedTab).toHaveBeenCalledTimes(1);
    expect(spies.newTab).toHaveBeenCalledTimes(2);
  });

  it("goes to the next and previous tab, to a tab by number and to the last", async () => {
    const { spies } = await renderKeys();
    press(document.body, { key: "Tab", ctrlKey: true });
    press(document.body, { key: "PageDown", ctrlKey: true });
    expect(spies.activateAdjacentTab.mock.calls).toEqual([[1], [1]]);
    press(document.body, { key: "Tab", ctrlKey: true, shiftKey: true });
    press(document.body, { key: "PageUp", ctrlKey: true });
    expect(spies.activateAdjacentTab.mock.calls).toEqual([[1], [1], [-1], [-1]]);
    press(document.body, { key: "1", code: "Digit1", ctrlKey: true });
    press(document.body, { key: "8", code: "Digit8", metaKey: true });
    press(document.body, { key: "9", code: "Digit9", ctrlKey: true });
    expect(spies.activateTabAt.mock.calls).toEqual([[0], [7], [-1]]);
  });

  it("picks tabs with the top row of an AZERTY keyboard", async () => {
    const { spies } = await renderKeys();
    expect(press(document.body, { key: "&", code: "Digit1", ctrlKey: true })).toBe(false);
    expect(press(document.body, { key: "é", code: "Digit2", ctrlKey: true })).toBe(false);
    expect(spies.activateTabAt.mock.calls).toEqual([[0], [1]]);
    // AltGr+é types "~": left to the field.
    expect(press(document.body, { key: "~", code: "Digit2", ctrlKey: true, altKey: true })).toBe(true);
    expect(spies.activateTabAt).toHaveBeenCalledTimes(2);
  });

  it("moves the active tab, never past the ends", async () => {
    const { store, spies } = await renderKeys();
    press(document.body, { key: "PageDown", ctrlKey: true, shiftKey: true });
    expect(spies.moveTab).toHaveBeenLastCalledWith("tab-journal", 1);
    press(document.body, { key: "PageUp", ctrlKey: true, shiftKey: true });
    expect(spies.moveTab).toHaveBeenLastCalledWith("tab-journal", 0);
    act(() => {
      store.setState({ activeTabId: "tab-home" });
    });
    press(document.body, { key: "PageDown", ctrlKey: true, shiftKey: true });
    expect(spies.moveTab).toHaveBeenLastCalledWith("tab-home", 2);
    press(document.body, { key: "PageUp", ctrlKey: true, shiftKey: true });
    expect(spies.moveTab).toHaveBeenLastCalledWith("tab-home", 1);
    expect(spies.activateAdjacentTab).not.toHaveBeenCalled();
  });

  it("goes back and forward with Alt+Left and Alt+Right", async () => {
    const { spies } = await renderKeys();
    expect(press(document.body, { key: "ArrowLeft", altKey: true })).toBe(false);
    expect(press(document.body, { key: "ArrowRight", altKey: true })).toBe(false);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
    expect(press(document.body, { key: "ArrowLeft" })).toBe(true);
    expect(press(document.body, { key: "ArrowLeft", altKey: true, shiftKey: true })).toBe(true);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
  });

  it("goes back and forward with Ctrl+[ and Ctrl+] too, and leaves Ctrl+Shift+[ and a bare bracket alone", async () => {
    const { spies } = await renderKeys();
    expect(press(document.body, { key: "[", code: "BracketLeft", ctrlKey: true })).toBe(false);
    expect(press(document.body, { key: "]", code: "BracketRight", ctrlKey: true })).toBe(false);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
    expect(press(document.body, { key: "{", code: "BracketLeft", ctrlKey: true, shiftKey: true })).toBe(true);
    expect(press(document.body, { key: "[", code: "BracketLeft" })).toBe(true);
    expect(press(document.body, { key: "[", ctrlKey: true, altKey: true })).toBe(true);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
  });

  it("works from a form field and from the page editor, where Back and Forward do not work in a field", async () => {
    const { user, spies } = await renderKeys();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    const field = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    expect(press(field, { key: "t", ctrlKey: true })).toBe(false);
    expect(press(field, { key: "w", ctrlKey: true })).toBe(false);
    expect(press(field, { key: "Tab", ctrlKey: true })).toBe(false);
    expect(spies.newTab).toHaveBeenCalledTimes(1);
    expect(spies.closeTab).toHaveBeenCalledTimes(1);
    expect(spies.activateAdjacentTab).toHaveBeenCalledTimes(1);
    expect(press(field, { key: "ArrowLeft", altKey: true })).toBe(true);
    expect(press(field, { key: "ArrowRight", altKey: true })).toBe(true);
    expect(press(field, { key: "[", code: "BracketLeft", ctrlKey: true })).toBe(true);
    expect(press(field, { key: "]", code: "BracketRight", ctrlKey: true })).toBe(true);
    expect(spies.goBack).not.toHaveBeenCalled();
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it("works from the page editor, Back and Forward included", async () => {
    const { user, store } = await renderLayout();
    await user.click(row("Journal"));
    const editor = await findEditor();
    const dom = editor.view.dom;
    const actions = spyOnTabActions(store);
    expect(press(dom, { key: "t", ctrlKey: true })).toBe(false);
    expect(press(dom, { key: "ArrowLeft", altKey: true })).toBe(false);
    expect(press(dom, { key: "2", code: "Digit2", ctrlKey: true })).toBe(false);
    // The editor binds neither Mod-[ nor Mod-] (lists indent with Tab), so they reach the history.
    expect(press(dom, { key: "[", code: "BracketLeft", ctrlKey: true })).toBe(false);
    expect(press(dom, { key: "]", code: "BracketRight", ctrlKey: true })).toBe(false);
    expect(actions.newTab).toHaveBeenCalledTimes(1);
    expect(actions.goBack).toHaveBeenCalledTimes(2);
    expect(actions.goForward).toHaveBeenCalledTimes(1);
    expect(actions.activateTabAt).toHaveBeenCalledWith(1);
  });

  it("does nothing inside a dialog or a menu, which own the keys", async () => {
    const { user, spies } = await renderKeys();
    await user.keyboard("{Control>}k{/Control}");
    const input = await screen.findByRole("combobox", { name: t("search.label") });
    for (const init of [
      { key: "t", ctrlKey: true },
      { key: "w", ctrlKey: true },
      { key: "Tab", ctrlKey: true },
      { key: "2", code: "Digit2", ctrlKey: true },
      { key: "PageDown", ctrlKey: true, shiftKey: true },
      { key: "ArrowLeft", altKey: true },
    ]) {
      expect(press(input, init), JSON.stringify(init)).toBe(true);
    }
    await user.keyboard("{Escape}");
    await user.pointer({ keys: "[MouseRight]", target: tabNamed("Reading list") });
    const menu = await screen.findByRole("menu", { name: t("tabs.menu") });
    expect(press(menu, { key: "t", ctrlKey: true })).toBe(true);
    expect(press(menu, { key: "Tab", ctrlKey: true })).toBe(true);
    for (const spy of Object.values(spies)) {
      expect(spy).not.toHaveBeenCalled();
    }
  });

  it("leaves a key that something else already took", async () => {
    const { spies } = await renderKeys();
    const taken = (event: Event): void => {
      event.preventDefault();
    };
    document.body.addEventListener("keydown", taken);
    press(document.body, { key: "t", ctrlKey: true });
    document.body.removeEventListener("keydown", taken);
    expect(spies.newTab).not.toHaveBeenCalled();
  });

  it("leaves a composition to the input method", async () => {
    const { spies } = await renderKeys();
    expect(press(document.body, { key: "t", ctrlKey: true, isComposing: true })).toBe(true);
    expect(spies.newTab).not.toHaveBeenCalled();
  });

  it("shows the main screen when a key brings another tab to the front from Settings, but not when it only moves a tab", async () => {
    const { store, spies } = await renderKeys({ initialRoute: "settings" });
    press(document.body, { key: "PageDown", ctrlKey: true, shiftKey: true });
    expect(store.getState().route).toBe("settings");
    press(document.body, { key: "Tab", ctrlKey: true });
    expect(spies.activateAdjacentTab).toHaveBeenCalledTimes(1);
    expect(store.getState().route).toBe("home");
    act(() => {
      store.getState().navigate("diagnostics");
    });
    press(document.body, { key: "t", ctrlKey: true });
    expect(store.getState().route).toBe("home");
  });

  it("keeps Ctrl+K for the search and Ctrl+\\ for the sidebar", async () => {
    const { store } = await renderKeys();
    expect(press(document.body, { key: "\\", ctrlKey: true })).toBe(false);
    expect(store.getState().settings["ui.sidebar.hidden"]).toBe(true);
    expect(press(document.body, { key: "k", ctrlKey: true })).toBe(false);
    expect(await screen.findByRole("combobox", { name: t("search.label") })).toBeInTheDocument();
  });

  it("does nothing with no tab to act on (no space)", async () => {
    const { store } = await renderLayout({ spaces: [] });
    const spies = spyOnTabActions(store);
    press(document.body, { key: "w", ctrlKey: true });
    press(document.body, { key: "PageDown", ctrlKey: true, shiftKey: true });
    expect(spies.closeTab).not.toHaveBeenCalled();
    expect(spies.moveTab).not.toHaveBeenCalled();
  });
});

describe("the mouse's back and forward buttons", () => {
  /** A press and release of a mouse button, with the `auxclick` that follows. Returns whether each was left alone. */
  function mouse(target: Element, button: number, events = ["mousedown", "mouseup", "auxclick"]): boolean[] {
    return events.map((type) => fireEvent(target, new MouseEvent(type, { bubbles: true, cancelable: true, button })));
  }

  it("goes back and forward once per press, and keeps the webview from going back in its own history", async () => {
    const { spies } = await renderKeys();
    expect(mouse(document.body, 3)).toEqual([false, false, false]);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).not.toHaveBeenCalled();
    expect(mouse(document.body, 4)).toEqual([false, false, false]);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    mouse(document.body, 3);
    expect(spies.goBack).toHaveBeenCalledTimes(2);
  });

  it("goes back for an auxclick that no release came before, and for a release alone", async () => {
    const { spies } = await renderKeys();
    mouse(document.body, 3, ["auxclick"]);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    mouse(document.body, 4, ["mouseup"]);
    expect(spies.goForward).toHaveBeenCalledTimes(1);
  });

  it("works anywhere, over a field and the editor included, and leaves the other buttons alone", async () => {
    const { user, spies } = await renderKeys();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    const field = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    mouse(field, 3);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(mouse(document.body, 1)).toEqual([true, true, true]);
    expect(mouse(document.body, 2)).toEqual([true, true, true]);
    expect(mouse(document.body, 0)).toEqual([true, true, true]);
    expect(spies.goBack).toHaveBeenCalledTimes(1);
    expect(spies.goForward).not.toHaveBeenCalled();
  });

  it("is cancelled over a dialog, which keeps its page, but does not move the history", async () => {
    const { user, spies } = await renderKeys();
    await user.keyboard("{Control>}k{/Control}");
    const input = await screen.findByRole("combobox", { name: t("search.label") });
    expect(mouse(input, 3)).toEqual([false, false, false]);
    expect(spies.goBack).not.toHaveBeenCalled();
  });

  it("shows the main screen when it goes back from Settings", async () => {
    const { store } = await renderKeys({ initialRoute: "settings" });
    mouse(document.body, 3);
    expect(store.getState().route).toBe("home");
  });
});

describe("the keys against the real tabs", () => {
  it("opens a Home tab with Ctrl+T, closes it with Ctrl+W and brings it back with Ctrl+Shift+T", async () => {
    const { user } = await renderLayout();
    await user.click(row("Journal"));
    press(document.body, { key: "t", ctrlKey: true });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", t("tabs.home")]);
    });
    expect(tabNamed(t("tabs.home"))).toHaveAttribute("aria-selected", "true");
    press(document.body, { key: "w", ctrlKey: true });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal"]);
    });
    expect(tabNamed("Journal")).toHaveAttribute("aria-selected", "true");
    press(document.body, { key: "T", ctrlKey: true, shiftKey: true });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", t("tabs.home")]);
    });
    expect(tabNamed(t("tabs.home"))).toHaveAttribute("aria-selected", "true");
  });

  it("goes round the tabs with Ctrl+Tab, to one by number, and moves the active tab", async () => {
    const { user, store, ids } = await renderLayout();
    await user.click(row("Journal"));
    const space = ids.space("Personal");
    act(() => {
      store.getState().navigateTo({ pageId: ids.page("Projects"), spaceId: space, disposition: "foreground" });
      store.getState().navigateTo({ pageId: ids.page("Ideas"), spaceId: space, disposition: "foreground" });
    });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal", "Projects", "Ideas"]);
    });
    const selected = (): string[] => tabs().filter((tab) => tab.getAttribute("aria-selected") === "true").map((tab) => tab.getAttribute("aria-label") ?? "");
    expect(selected()).toEqual(["Ideas"]);
    press(document.body, { key: "Tab", ctrlKey: true });
    expect(selected()).toEqual(["Journal"]);
    press(document.body, { key: "Tab", ctrlKey: true, shiftKey: true });
    expect(selected()).toEqual(["Ideas"]);
    press(document.body, { key: "PageUp", ctrlKey: true });
    expect(selected()).toEqual(["Projects"]);
    press(document.body, { key: "1", code: "Digit1", ctrlKey: true });
    expect(selected()).toEqual(["Journal"]);
    press(document.body, { key: "9", code: "Digit9", ctrlKey: true });
    expect(selected()).toEqual(["Ideas"]);
    press(document.body, { key: "PageUp", ctrlKey: true, shiftKey: true });
    expect(tabNames()).toEqual(["Journal", "Ideas", "Projects"]);
    press(document.body, { key: "PageUp", ctrlKey: true, shiftKey: true });
    press(document.body, { key: "PageUp", ctrlKey: true, shiftKey: true });
    expect(tabNames()).toEqual(["Ideas", "Journal", "Projects"]);
    expect(selected()).toEqual(["Ideas"]);
  });

  it("goes back and forward in the tab with Alt+Left, Alt+Right and the mouse buttons", async () => {
    const { user, store, ids } = await renderLayout();
    await user.click(row("Journal"));
    await user.click(row("Reading list"));
    await waitFor(() => {
      expect(tabNames()).toEqual(["Reading list"]);
    });
    press(document.body, { key: "ArrowLeft", altKey: true });
    await waitFor(() => {
      expect(tabNames()).toEqual(["Journal"]);
    });
    expect(currentEntry(store.getState().tabs[0] ?? makeTab("x", "", null)).pageId).toBe(ids.page("Journal"));
    fireEvent(document.body, new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 3 }));
    await waitFor(() => {
      expect(tabNames()).toEqual([t("tabs.home")]);
    });
    press(document.body, { key: "ArrowRight", altKey: true });
    fireEvent(document.body, new MouseEvent("mouseup", { bubbles: true, cancelable: true, button: 4 }));
    await waitFor(() => {
      expect(tabNames()).toEqual(["Reading list"]);
    });
  });
});
