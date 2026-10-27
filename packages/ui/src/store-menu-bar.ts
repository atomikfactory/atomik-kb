// The native menu bar's part of the app store (ADR-0040 item 12): the desktop shell has a menu bar (File, Edit, View,
// Window, Help) that a lone Alt press shows (`menu-bar.ts`). What it needs of the page is here:
//
// - **Its words.** The menu is native: the page sends its strings (`menu.*` of `en.json`) with `shell_labels_set` as
//   soon as it starts and when its language changes (`labels`, sent by `store-shell.ts` with the tray's).
// - **Its visibility.** The shell says when the bar is shown or hidden (`menuBar` messages): `menuBarVisible`.
//   `menuBarShow` (the lone Alt press) and `menuBarHide` (Escape, the first key or click in the page) ask for the change.
// - **Its choices.** The shell's own items (quit, minimize, close the window) and the new page and lock, which are
//   the tray's, never get here. A choice of one of the rest arrives as a `menu` message and runs the page's existing
//   action, and **does nothing harmful when it cannot run now**: an action that needs an open page says so in a toast.
//
//   | Item                | Runs                                                                                      |
//   |---------------------|-------------------------------------------------------------------------------------------|
//   | Import...           | the space's Import dialog (the sidebar's space menu)                                      |
//   | Export...           | the open page's Export dialog, or the current space's when no page is open                |
//   | Print...            | print mode of the open page (Ctrl+P)                                                      |
//   | Find in page        | the open page's find bar (Ctrl+F)                                                         |
//   | Toggle sidebar      | Ctrl+\                                                                                    |
//   | Back, Forward       | the active tab's history (Alt+Left, Alt+Right)                                            |
//   | About               | a small dialog with the app's name and version (`AboutDialog`)                            |
//   | Undo, Redo          | only where the platform's menu cannot undo itself: the same Ctrl+Z / Ctrl+Shift+Z the keyboard sends |
//
//   Export and Import open the dialogs through `fileRequest`, which the page (`PageView`) or the space switcher
//   (`SpaceSwitcher`) shows and clears. Nothing runs while the app is not ready (loading, locked, first run).

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreClient, MenuCommand, PageInfo, ShellLabels } from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { requestFindInPage } from "./editor/find-request";
import { t } from "./i18n";
import { IS_MAC } from "./shortcuts";
import type { AppState } from "./store";
import { currentEntry, selectActiveTab } from "./store-tabs";
import { findPage } from "./tree";

/** An Export or Import dialog the menu bar asked for, shown by the page (`target: "page"`) or the space switcher. */
export interface FileRequest {
  kind: "export" | "import";
  target: "page" | "space";
}

export interface MenuBarState {
  /** The desktop shell has a menu bar that a lone Alt shows (not the web build, not a shell without one). */
  menuBar: boolean;
  /** The bar is on the window now, as the shell last said. */
  menuBarVisible: boolean;
  /** The About dialog is open. */
  aboutOpen: boolean;
  /** The Export or Import dialog the menu asked for, until the dialog's owner clears it. */
  fileRequest: FileRequest | null;
  /** Asks the shell to show the bar (a lone Alt press while it is hidden). */
  menuBarShow: () => void;
  /** Asks the shell to hide the bar. */
  menuBarHide: () => void;
  closeAbout: () => void;
  clearFileRequest: () => void;
}

export interface MenuBarSlice {
  initial: MenuBarState;
  /** A `menu` message of the shell: runs the item, or says why it cannot. */
  run: (command: MenuCommand) => void;
  /** A `menuBar` message of the shell: the bar was shown or hidden. */
  visibility: (visible: boolean) => void;
  /** The workspace is open (the app became ready) or there is none to act in: closes what the menu opened. */
  reset: () => void;
  /** The menu's words, from the UI's strings. */
  labels: () => ShellLabels;
}

/** The page the main screen shows now (the active tab's, on the Home route), or `null`: Home, Settings, a page not loaded. */
export function selectOpenPage(state: AppState): PageInfo | null {
  if (state.route !== "home" || state.tree === null) {
    return null;
  }
  const tab = selectActiveTab(state);
  const entry = tab === null ? null : currentEntry(tab);
  const pageId = entry === null ? state.selectedPageId : entry.pageId;
  return pageId === null ? null : findPage(state.tree.roots, pageId);
}

export function createMenuBarSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
): MenuBarSlice {
  function logFailure(what: string, error: unknown): void {
    // The code only: a message can name content.
    console.error(what, toCoreError(error).code);
  }

  /** What Ctrl+Z (Ctrl+Shift+Z) does where the focus is, sent as the keyboard would: the editor's undo, else the sidebar's. */
  function history(redo: boolean): void {
    const target = document.activeElement ?? document.body;
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "z",
        code: "KeyZ",
        ctrlKey: !IS_MAC,
        metaKey: IS_MAC,
        shiftKey: redo,
        bubbles: true,
        cancelable: true,
      }),
    );
  }

  /** Back or forward in the active tab, and the main screen in front, as the keys do. */
  function step(delta: -1 | 1): void {
    const state = get();
    if (delta < 0) {
      state.goBack();
    } else {
      state.goForward();
    }
    if (get().route !== "home") {
      get().navigate("home");
    }
  }

  function noPage(): void {
    get().showToast(t("menu.noPage"), "info");
  }

  function requestFile(kind: "export" | "import"): void {
    const state = get();
    const supported = kind === "export" ? state.interopCapabilities.export : state.interopCapabilities.import;
    if (!supported || state.currentSpaceId === null) {
      state.showToast(t("menu.unavailable"), "info");
      return;
    }
    // Importing goes into the space, exporting takes the page in front when there is one.
    const target = kind === "export" && selectOpenPage(state) !== null ? "page" : "space";
    set({ fileRequest: { kind, target } });
  }

  return {
    initial: {
      menuBar: client.hasMenuBar(),
      menuBarVisible: false,
      aboutOpen: false,
      fileRequest: null,

      menuBarShow: () => {
        client.menuBarShow().catch((error: unknown) => {
          logFailure("the menu bar could not be shown", error);
        });
      },

      menuBarHide: () => {
        // The shell confirms with a `menuBar` message; the page does not ask twice while it waits.
        set({ menuBarVisible: false });
        client.menuBarHide().catch((error: unknown) => {
          logFailure("the menu bar could not be hidden", error);
        });
      },

      closeAbout: () => {
        set({ aboutOpen: false });
      },

      clearFileRequest: () => {
        set({ fileRequest: null });
      },
    },

    run: (command) => {
      const state = get();
      if (state.phase !== "ready") {
        return; // opening, locked or no workspace: nothing to act on
      }
      switch (command) {
        case "toggleSidebar":
          state.toggleSidebar();
          break;
        case "back":
          step(-1);
          break;
        case "forward":
          step(1);
          break;
        case "find":
          if (selectOpenPage(state) === null) {
            noPage();
          } else {
            requestFindInPage();
          }
          break;
        case "print":
          if (selectOpenPage(state) === null) {
            noPage();
          } else {
            state.startPrint(); // does nothing while it is already printing
          }
          break;
        case "export":
          requestFile("export");
          break;
        case "import":
          requestFile("import");
          break;
        case "about":
          set({ aboutOpen: true });
          break;
        case "undo":
          history(false);
          break;
        case "redo":
          history(true);
          break;
      }
    },

    visibility: (visible) => {
      set({ menuBarVisible: visible });
    },

    reset: () => {
      if (get().aboutOpen || get().fileRequest !== null) {
        set({ aboutOpen: false, fileRequest: null });
      }
    },

    labels: () => ({
      menuFile: t("menu.file"),
      menuEdit: t("menu.edit"),
      menuView: t("menu.view"),
      menuWindow: t("menu.window"),
      menuHelp: t("menu.help"),
      menuNewPage: t("menu.newPage"),
      menuImport: t("menu.import"),
      menuExport: t("menu.export"),
      menuPrint: t("menu.print"),
      menuLock: t("menu.lock"),
      menuQuit: t("menu.quit"),
      menuUndo: t("menu.undo"),
      menuRedo: t("menu.redo"),
      menuCut: t("menu.cut"),
      menuCopy: t("menu.copy"),
      menuPaste: t("menu.paste"),
      menuSelectAll: t("menu.selectAll"),
      menuFind: t("menu.find"),
      menuToggleSidebar: t("menu.toggleSidebar"),
      menuBack: t("menu.back"),
      menuForward: t("menu.forward"),
      menuMinimize: t("menu.minimize"),
      menuCloseWindow: t("menu.closeWindow"),
      menuAbout: t("menu.about"),
    }),
  };
}
