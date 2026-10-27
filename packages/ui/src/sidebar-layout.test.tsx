// The sidebar's width and visibility (ADR-0029): the settings and how they are read, the resize
// handle (drag, keys, double click), hiding and showing it with the buttons and Ctrl+\, and what
// is written to the core when.

import type { MemoryTransport, Request } from "@atomik-kb/core-client";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_KEY_STEP,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
  clampSidebarWidth,
  parseSidebarWidth,
} from "./store-layout";
import { renderApp } from "./test/render";
import { PERSONAL, WORK, renderSidebar } from "./test/sidebar";

const WIDTH_KEY = "ui.sidebar.width";
const HIDDEN_KEY = "ui.sidebar.hidden";

describe("the width setting", () => {
  it("reads an integer from 200 to 480 as it is and anything else as 272", () => {
    expect(parseSidebarWidth(200)).toBe(200);
    expect(parseSidebarWidth(321)).toBe(321);
    expect(parseSidebarWidth(480)).toBe(480);
    for (const bad of [199, 481, 300.5, "300", null, undefined, true, [300], { width: 300 }, Number.NaN]) {
      expect(parseSidebarWidth(bad as never), JSON.stringify(bad)).toBe(SIDEBAR_DEFAULT_WIDTH);
    }
  });

  it("clamps a chosen width to 200 to 480 and rounds it", () => {
    expect(clampSidebarWidth(100)).toBe(SIDEBAR_MIN_WIDTH);
    expect(clampSidebarWidth(9000)).toBe(SIDEBAR_MAX_WIDTH);
    expect(clampSidebarWidth(300.4)).toBe(300);
    expect(clampSidebarWidth(300.6)).toBe(301);
    expect(clampSidebarWidth(Number.NaN)).toBe(SIDEBAR_DEFAULT_WIDTH);
  });
});

function layout(): HTMLElement {
  const element = document.querySelector<HTMLElement>(".layout");
  if (element === null) {
    throw new Error("the layout is missing");
  }
  return element;
}

const widthVariable = (): string => layout().style.getPropertyValue("--sidebar-width");
const handle = (): HTMLElement => screen.getByRole("separator", { name: t("sidebar.resize") });
const hideButton = (): HTMLElement => screen.getByRole("button", { name: t("sidebar.hide") });
const showButton = (): HTMLElement => screen.getByRole("button", { name: t("sidebar.show") });
const sidebarShown = (): boolean => screen.queryByRole("navigation", { name: t("nav.label") }) !== null;

function pointer(type: "pointerDown" | "pointerMove" | "pointerUp" | "pointerCancel", clientX: number): void {
  fireEvent[type](handle(), { clientX, pointerId: 3, button: 0, pointerType: "mouse" });
}

/** The writes of one setting that reached the core. */
function writesOf(dispatch: ReturnType<typeof spyOnDispatch>, key: string): unknown[] {
  return dispatch.mock.calls
    .map(([request]) => request)
    .filter((request): request is Extract<Request, { type: "SettingsSet" }> => request.type === "SettingsSet")
    .filter((request) => request.data.key === key)
    .map((request) => request.data.value);
}

function spyOnDispatch(memory: MemoryTransport) {
  return vi.spyOn(memory, "dispatch");
}

async function stored(memory: MemoryTransport, key: string): Promise<unknown> {
  const response = await memory.dispatch({ type: "SettingsGet", data: { key } });
  return response.type === "Setting" ? response.data : undefined;
}

describe("the width of the sidebar", () => {
  it("is 272 pixels by default and the stored width when there is one, from the first paint", async () => {
    await renderSidebar();
    expect(widthVariable()).toBe("272px");
    expect(handle()).toHaveAttribute("aria-valuenow", "272");
  });

  it("starts at the stored width", async () => {
    await renderSidebar({ settings: { [WIDTH_KEY]: 340 } });
    expect(widthVariable()).toBe("340px");
    expect(handle()).toHaveAttribute("aria-valuenow", "340");
  });

  it("follows a change made elsewhere (another window)", async () => {
    const { memory } = await renderSidebar();
    await memory.dispatch({ type: "SettingsSet", data: { key: WIDTH_KEY, value: 410 } });
    await waitFor(() => {
      expect(widthVariable()).toBe("410px");
    });
    await memory.dispatch({ type: "SettingsSet", data: { key: HIDDEN_KEY, value: true } });
    await waitFor(() => {
      expect(sidebarShown()).toBe(false);
    });
  });

  it("reads a stored value that is not 200 to 480 as 272", async () => {
    await renderSidebar({ settings: { [WIDTH_KEY]: 90 } });
    expect(widthVariable()).toBe("272px");
  });

  it("has a handle that describes itself as a vertical separator between 200 and 480", async () => {
    await renderSidebar();
    expect(handle()).toHaveAttribute("aria-orientation", "vertical");
    expect(handle()).toHaveAttribute("aria-valuemin", "200");
    expect(handle()).toHaveAttribute("aria-valuemax", "480");
    expect(handle()).toHaveAttribute("tabindex", "0");
  });
});

describe("dragging the handle", () => {
  it("changes the width live and writes the setting once, on release", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    pointer("pointerDown", 272);
    pointer("pointerMove", 300);
    expect(widthVariable()).toBe("300px");
    pointer("pointerMove", 340);
    expect(widthVariable()).toBe("340px");
    expect(handle()).toHaveAttribute("aria-valuenow", "340");
    // Nothing is written while the pointer is down.
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
    pointer("pointerUp", 340);
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([340]);
    expect(widthVariable()).toBe("340px");
    await waitFor(async () => {
      expect(await stored(memory, WIDTH_KEY)).toBe(340);
    });
    expect(handle()).toHaveAttribute("aria-valuenow", "340");
  });

  it("starts from the width it has and follows the pointer's distance", async () => {
    await renderSidebar({ settings: { [WIDTH_KEY]: 300 } });
    pointer("pointerDown", 300);
    pointer("pointerMove", 250);
    expect(widthVariable()).toBe("250px");
    pointer("pointerUp", 250);
    expect(widthVariable()).toBe("250px");
  });

  it("holds the width between 200 and 480", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    pointer("pointerDown", 272);
    pointer("pointerMove", -500);
    expect(widthVariable()).toBe("200px");
    pointer("pointerMove", 5000);
    expect(widthVariable()).toBe("480px");
    pointer("pointerUp", 5000);
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([480]);
  });

  it("writes nothing when the pointer comes back to where it started", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    pointer("pointerDown", 272);
    pointer("pointerMove", 400);
    pointer("pointerMove", 272);
    pointer("pointerUp", 272);
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
    expect(widthVariable()).toBe("272px");
  });

  it("puts the old width back, writing nothing, when the drag is cancelled", async () => {
    const { memory } = await renderSidebar({ settings: { [WIDTH_KEY]: 300 } });
    const dispatch = spyOnDispatch(memory);
    pointer("pointerDown", 300);
    pointer("pointerMove", 420);
    expect(widthVariable()).toBe("420px");
    pointer("pointerCancel", 420);
    expect(widthVariable()).toBe("300px");
    expect(handle()).toHaveAttribute("aria-valuenow", "300");
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
  });

  it("is cancelled by Escape", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    pointer("pointerDown", 272);
    pointer("pointerMove", 400);
    fireEvent.keyDown(handle(), { key: "Escape" });
    expect(widthVariable()).toBe("272px");
    pointer("pointerUp", 400);
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
  });

  it("ignores a button other than the main one", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    fireEvent.pointerDown(handle(), { clientX: 272, pointerId: 3, button: 2 });
    pointer("pointerMove", 400);
    pointer("pointerUp", 400);
    expect(widthVariable()).toBe("272px");
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
  });
});

describe("the keyboard on the handle", () => {
  it("changes the width by 16 pixels with the arrow keys, one write each", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    handle().focus();
    fireEvent.keyDown(handle(), { key: "ArrowRight" });
    expect(widthVariable()).toBe(`${String(272 + SIDEBAR_KEY_STEP)}px`);
    fireEvent.keyDown(handle(), { key: "ArrowRight" });
    fireEvent.keyDown(handle(), { key: "ArrowLeft" });
    expect(widthVariable()).toBe("288px");
    expect(handle()).toHaveAttribute("aria-valuenow", "288");
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([288, 304, 288]);
    await waitFor(async () => {
      expect(await stored(memory, WIDTH_KEY)).toBe(288);
    });
  });

  it("stops at 200 and at 480 without writing", async () => {
    const { memory } = await renderSidebar({ settings: { [WIDTH_KEY]: 200 } });
    const dispatch = spyOnDispatch(memory);
    fireEvent.keyDown(handle(), { key: "ArrowLeft" });
    expect(widthVariable()).toBe("200px");
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
    fireEvent.keyDown(handle(), { key: "ArrowRight" });
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([216]);
  });

  it("leaves keys with a modifier and other keys alone", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    fireEvent.keyDown(handle(), { key: "ArrowRight", ctrlKey: true });
    fireEvent.keyDown(handle(), { key: "ArrowUp" });
    fireEvent.keyDown(handle(), { key: "Enter" });
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
  });
});

describe("a double click on the handle", () => {
  it("resets the width to 272 with one write", async () => {
    const { memory } = await renderSidebar({ settings: { [WIDTH_KEY]: 400 } });
    const dispatch = spyOnDispatch(memory);
    fireEvent.doubleClick(handle());
    expect(widthVariable()).toBe("272px");
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([272]);
    await waitFor(async () => {
      expect(await stored(memory, WIDTH_KEY)).toBe(272);
    });
  });

  it("writes nothing when the width is already 272", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    fireEvent.doubleClick(handle());
    expect(writesOf(dispatch, WIDTH_KEY)).toEqual([]);
  });
});

describe("hiding and showing the sidebar", () => {
  it("shows the sidebar and no show button by default", async () => {
    await renderSidebar();
    expect(sidebarShown()).toBe(true);
    expect(screen.queryByRole("button", { name: t("sidebar.show") })).not.toBeInTheDocument();
    expect(layout()).not.toHaveClass("is-sidebar-hidden");
  });

  it("hides with the button in the sidebar's header and shows with the one in the content", async () => {
    const { user, memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    await user.click(hideButton());
    expect(sidebarShown()).toBe(false);
    expect(layout()).toHaveClass("is-sidebar-hidden");
    expect(screen.queryByRole("separator", { name: t("sidebar.resize") })).not.toBeInTheDocument();
    // Focus follows to the button that brings the sidebar back.
    expect(showButton()).toHaveFocus();
    expect(writesOf(dispatch, HIDDEN_KEY)).toEqual([true]);
    expect(screen.queryByRole("tree")).not.toBeInTheDocument();

    await user.click(showButton());
    expect(sidebarShown()).toBe(true);
    expect(layout()).not.toHaveClass("is-sidebar-hidden");
    expect(hideButton()).toHaveFocus();
    expect(handle()).toBeInTheDocument();
    expect(writesOf(dispatch, HIDDEN_KEY)).toEqual([true, false]);
    await waitFor(async () => {
      expect(await stored(memory, HIDDEN_KEY)).toBe(false);
    });
  });

  it("starts hidden when the setting says so, and keeps the width for when it is shown again", async () => {
    // The sidebar is what `renderSidebar` waits for, so wait for the button that brings it back.
    const { user } = renderApp({
      workspaceName: "Research",
      spaces: [PERSONAL, WORK],
      settings: { [HIDDEN_KEY]: true, [WIDTH_KEY]: 360 },
    });
    expect(await screen.findByRole("button", { name: t("sidebar.show") })).toBeInTheDocument();
    expect(sidebarShown()).toBe(false);
    // No first paint with the sidebar in it.
    expect(document.querySelector(".sidebar")).toHaveAttribute("hidden");
    expect(widthVariable()).toBe("360px");
    await user.click(showButton());
    expect(sidebarShown()).toBe(true);
    expect(widthVariable()).toBe("360px");
  });

  it("keeps the tree where it was while the sidebar is hidden", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("treeitem", { name: "Journal" }));
    await user.click(hideButton());
    await user.click(showButton());
    expect(screen.getByRole("treeitem", { name: "Journal" })).toHaveAttribute("aria-selected", "true");
  });

  it("toggles with Ctrl+\\ and Cmd+\\", async () => {
    const { memory } = await renderSidebar();
    const dispatch = spyOnDispatch(memory);
    fireEvent.keyDown(document.body, { key: "\\", ctrlKey: true });
    expect(sidebarShown()).toBe(false);
    fireEvent.keyDown(document.body, { key: "\\", metaKey: true });
    expect(sidebarShown()).toBe(true);
    expect(writesOf(dispatch, HIDDEN_KEY)).toEqual([true, false]);
  });

  it("toggles on other keyboard layouts: AltGr typing a backslash, or the physical backslash key", async () => {
    await renderSidebar();
    // AZERTY: AltGr+8 types "\" and reports Ctrl and Alt.
    fireEvent.keyDown(document.body, { key: "\\", code: "Digit8", ctrlKey: true, altKey: true });
    expect(sidebarShown()).toBe(false);
    // AZERTY prints "*" on the physical backslash key.
    fireEvent.keyDown(document.body, { key: "*", code: "Backslash", ctrlKey: true });
    expect(sidebarShown()).toBe(true);
  });

  it("ignores the shortcut without Ctrl or Cmd, with Shift, and with Alt on the physical key", async () => {
    await renderSidebar();
    fireEvent.keyDown(document.body, { key: "\\" });
    fireEvent.keyDown(document.body, { key: "*", code: "Backslash", ctrlKey: true, altKey: true });
    fireEvent.keyDown(document.body, { key: "\\", ctrlKey: true, shiftKey: true });
    expect(sidebarShown()).toBe(true);
  });

  it("ignores the shortcut while typing in a field", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    // The new page's name field has focus.
    const field = await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    fireEvent.keyDown(field, { key: "\\", ctrlKey: true });
    expect(sidebarShown()).toBe(true);
  });

  it("ignores the shortcut inside the search dialog", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("button", { name: t("search.open") }));
    const input = await screen.findByRole("combobox", { name: t("search.label") });
    fireEvent.keyDown(input, { key: "\\", ctrlKey: true });
    expect(sidebarShown()).toBe(true);
  });

  it("works from the page editor, where nothing is typed by the shortcut", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("treeitem", { name: "Journal" }));
    const editor = await screen.findByRole("textbox", { name: t("editor.label") });
    fireEvent.keyDown(editor, { key: "\\", ctrlKey: true });
    expect(sidebarShown()).toBe(false);
  });

  it("does not move focus when it is somewhere else and the shortcut hides the sidebar", async () => {
    const { user } = await renderSidebar();
    await user.click(screen.getByRole("treeitem", { name: "Journal" }));
    const editor = await screen.findByRole("textbox", { name: t("editor.label") });
    editor.focus();
    fireEvent.keyDown(editor, { key: "\\", ctrlKey: true });
    expect(editor).toHaveFocus();
  });
});
