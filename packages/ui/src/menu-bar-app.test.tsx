// The desktop shell's native menu bar (ADR-0040 item 12) in the whole app over the in-memory core with a shell around it
// (`test/shell.ts`): the page gives the shell the menu's words, asks for the bar on a lone Alt press and nowhere else,
// follows what the shell says about it, hides it on Escape and on the first key or click in the page, and runs each
// item the shell reports as the page's existing action (an action that cannot run now says so and does nothing).

import { CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, MenuCommand, PageId } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import messages from "./i18n/en.json";
import type { AppStore } from "./store";
import type * as StoreModule from "./store";
import { selectSidebarHidden } from "./store-layout";
import { idsOf, openPage } from "./test/page";
import { NOW, findMainNavigation, renderApp, renderWith } from "./test/render";
import { PASSPHRASE } from "./test/security";
import { withShell } from "./test/shell";
import type { ShellOptions } from "./test/shell";

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

function doc(text: string): JsonValue {
  return { type: "doc", content: [{ type: "paragraph", content: [{ type: "text", text }] }] };
}

const SPACES = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: doc("Water the plants") },
      { title: "Plan", content: doc("Ship it") },
    ],
  },
];

const EXPORT_METHODS = {
  exportRun: () => Promise.reject(new Error("not run")),
  importRun: () => Promise.reject(new Error("not run")),
};

const LOCK_ON = { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE };

async function setup(options: ShellOptions & { locked?: boolean; appLock?: boolean; files?: boolean } = {}) {
  const { locked, appLock, files = true, ...shellOptions } = options;
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: SPACES,
    ...(locked === true ? { security: { ...LOCK_ON, startLocked: true } } : appLock === true ? { security: LOCK_ON } : {}),
  });
  if (locked === true) {
    await memory.unlock(PASSPHRASE);
  }
  const ids = await idsOf(memory);
  if (locked === true) {
    await memory.lock();
  }
  const shell = withShell(memory, { ...(files ? { methods: EXPORT_METHODS } : {}), ...shellOptions });
  const app = renderWith(shell.transport);
  return { ...app, memory, ids, shell };
}

type Setup = Awaited<ReturnType<typeof setup>>;

function menu(shell: Setup["shell"], id: MenuCommand): void {
  act(() => {
    shell.emit({ type: "menu", data: { id } });
  });
}

function barShown(shell: Setup["shell"], visible: boolean): void {
  act(() => {
    shell.emit({ type: "menuBar", data: { visible } });
  });
}

const toasts = (): HTMLElement => screen.getByRole("log", { name: t("toast.label") });

function activePage(): PageId | null {
  const { tabs, activeTabId } = appStore().getState();
  const tab = tabs.find((candidate) => candidate.id === activeTabId);
  return tab?.entries[tab.index]?.pageId ?? null;
}

describe("the words of the menu", () => {
  it("are sent to the shell as soon as the app starts, with the tray's, as the strings of en.json", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    expect(shell.labels).toHaveLength(1);
    const labels = shell.labels[0] ?? {};
    const sent = Object.entries(labels).filter(([id]) => id.startsWith("menu"));
    expect(sent).toHaveLength(24);
    for (const [id, text] of sent) {
      // `menuNewPage` is `menu.newPage`.
      const key = `menu.${id.slice("menu".length, "menu".length + 1).toLowerCase()}${id.slice("menu".length + 1)}`;
      expect((messages as Record<string, string>)[key], id).toBe(text);
    }
    expect(labels).toMatchObject({ open: t("tray.open"), quit: t("tray.quit"), menuFile: "&File", menuPrint: "&Print…" });
  });

  it("are all valid labels for the shell: 1 to 64 characters, no control character, and a mnemonic that is not repeated in a menu", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    const labels = (shell.labels[0] ?? {}) as Record<string, string>;
    // The tray's and the menu bar's: a label is one short line (the dialogs' texts are longer, `shell-dialog-labels.test.tsx`).
    for (const [id, text] of Object.entries(labels).filter(([id]) => !id.startsWith("dialog") && !id.startsWith("fileFilter"))) {
      expect(text.length, id).toBeGreaterThanOrEqual(1);
      expect(text.length, id).toBeLessThanOrEqual(64);
      // eslint-disable-next-line no-control-regex -- the shell refuses control characters
      expect(/[\u0000-\u001f\u007f]/.test(text), id).toBe(false);
    }
    const mnemonic = (id: string): string | undefined => /&([^&])/.exec(labels[id] ?? "")?.[1]?.toLowerCase();
    const menus: Record<string, string[]> = {
      bar: ["menuFile", "menuEdit", "menuView", "menuWindow", "menuHelp"],
      file: ["menuNewPage", "menuImport", "menuExport", "menuPrint", "menuLock", "menuQuit"],
      edit: ["menuUndo", "menuRedo", "menuCut", "menuCopy", "menuPaste", "menuSelectAll", "menuFind"],
      view: ["menuToggleSidebar", "menuBack", "menuForward"],
      window: ["menuMinimize", "menuCloseWindow"],
      help: ["menuAbout"],
    };
    for (const [name, ids] of Object.entries(menus)) {
      const letters = ids.map(mnemonic);
      expect(letters.every((letter) => letter !== undefined), name).toBe(true);
      expect(new Set(letters).size, name).toBe(letters.length);
    }
  });

  it("are not sent without a shell", async () => {
    renderApp({ workspaceName: "Research", spaces: SPACES });
    await findMainNavigation();
    // Nothing to assert on but that it ran: the in-memory core has no shell to send to.
    expect(appStore().getState().menuBar).toBe(false);
  });
});

describe("a lone Alt press", () => {
  it("asks the shell for the menu bar, once per press, and leaves a press while it is shown to the system", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show"]);
    // Not yet told that it is shown: the page asks again, which the shell takes as nothing new.
    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show", "show"]);

    // Told that it is shown: Alt is the operating system's now (its menu mode), not the page's to answer.
    barShown(shell, true);
    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show", "show"]);

    // And once it is hidden again, the next press shows it.
    barShown(shell, false);
    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show", "show", "show"]);
  });

  it("works from the editor and from a field too", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    const editor = await screen.findByRole("textbox", { name: t("editor.label") });
    await user.click(editor);
    shell.menuBar.length = 0;

    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show"]);
  });

  it("does not ask for Alt with an arrow (back and forward, word movement), with Ctrl (the quick capture) or with a letter", async () => {
    const { user, shell, ids } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    await openPage(user, "Plan");

    // Alt+Left still goes back, Alt+Right forward, and neither shows the bar.
    await user.keyboard("{Alt>}{ArrowLeft}{/Alt}");
    expect(activePage()).toBe(ids.page("Notes"));
    await user.keyboard("{Alt>}{ArrowRight}{/Alt}");
    expect(activePage()).toBe(ids.page("Plan"));
    await user.keyboard("{Alt>}{ArrowDown}{ArrowUp}{/Alt}");
    await user.keyboard("{Control>}{Alt>}n{/Alt}{/Control}");
    await user.keyboard("{Alt>}f{/Alt}");
    await user.keyboard("{Alt>}{Tab}{/Alt}");
    expect(shell.menuBar).toEqual([]);
  });

  it("does not ask for Alt with a mouse click, or Alt that was pressed in another window", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();
    const nav = await findMainNavigation();

    await user.keyboard("{Alt>}");
    await user.click(within(nav).getByRole("button", { name: t("nav.settings") }));
    await user.keyboard("{/Alt}");
    expect(shell.menuBar).toEqual([]);

    // Alt down, the window loses the focus (Alt+Tab), and the release never comes here.
    await user.keyboard("{Alt>}");
    act(() => {
      window.dispatchEvent(new Event("blur"));
    });
    await user.keyboard("{/Alt}");
    expect(shell.menuBar).toEqual([]);

    // The next real press works.
    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show"]);
  });

  it("does nothing without a menu bar: a shell without one, the web build and a locked app", async () => {
    const withoutBar = await setup({ noMenuBar: true });
    await findMainNavigation();
    expect(appStore().getState().menuBar).toBe(false);
    await withoutBar.user.keyboard("{Alt}");
    expect(withoutBar.shell.menuBar).toEqual([]);
    withoutBar.unmount();

    const web = renderApp({ workspaceName: "Research", spaces: SPACES });
    await findMainNavigation();
    await web.user.keyboard("{Alt}"); // nothing to ask, nothing thrown
    web.unmount();

    const locked = await setup({ locked: true });
    await screen.findByRole("heading", { name: t("lock.title") });
    await locked.user.keyboard("{Alt}");
    expect(locked.shell.menuBar).toEqual([]);
  });

  it("logs a refusal by its code, never anything else, and the page goes on", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user, shell } = await setup();
    await findMainNavigation();
    shell.failMenuBar(new CoreError({ code: "Internal", message: "SECRET shell detail" }));

    await user.keyboard("{Alt}");

    await waitFor(() => {
      expect(logged).toHaveBeenCalledWith("the menu bar could not be shown", "Internal");
    });
    expect(JSON.stringify(logged.mock.calls)).not.toContain("SECRET");
    await user.keyboard("{Alt}");
    expect(shell.menuBar).toEqual(["show", "show"]);
  });
});

describe("the bar while it is shown", () => {
  it("is remembered as the shell says, and hidden as the shell says", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    expect(appStore().getState().menuBarVisible).toBe(false);
    barShown(shell, true);
    expect(appStore().getState().menuBarVisible).toBe(true);
    barShown(shell, false);
    expect(appStore().getState().menuBarVisible).toBe(false);
  });

  it("goes away on Escape, once, and Escape does not ask while it is hidden", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    await user.keyboard("{Escape}");
    expect(shell.menuBar).toEqual([]);

    barShown(shell, true);
    await user.keyboard("{Escape}");
    expect(shell.menuBar).toEqual(["hide"]);
    // The page does not ask again while it waits for the shell to say so.
    expect(appStore().getState().menuBarVisible).toBe(false);
    await user.keyboard("{Escape}");
    expect(shell.menuBar).toEqual(["hide"]);
  });

  it("goes away on the first key typed in the page, which is also how a native Copy or Paste ends", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    for (const keys of ["a", "{Enter}", "{Control>}c{/Control}", "{Control>}v{/Control}", "{ArrowDown}"]) {
      barShown(shell, true);
      shell.menuBar.length = 0;
      await user.keyboard(keys);
      expect(shell.menuBar, keys).toEqual(["hide"]);
    }
  });

  it("stays for what reaches for it: Alt, a mnemonic with Alt, and modifiers on their own", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();
    barShown(shell, true);

    await user.keyboard("{Control}{Shift}{Meta}");
    await user.keyboard("{Alt>}f{/Alt}");
    await user.keyboard("{Alt>}{ArrowLeft}{/Alt}");
    // The Alt that follows is the press that hides it, which the shell does.
    expect(shell.menuBar.filter((call) => call === "hide")).toEqual([]);
    expect(appStore().getState().menuBarVisible).toBe(true);
  });

  it("goes away on a click in the page", async () => {
    const { user, shell } = await setup();
    const nav = await findMainNavigation();
    barShown(shell, true);

    await user.click(within(nav).getByRole("button", { name: t("nav.settings") }));

    expect(shell.menuBar).toEqual(["hide"]);
  });

  it("is hidden when the app goes (locked): the bar must not stay behind the lock screen", async () => {
    const { shell, unmount } = await setup();
    await findMainNavigation();
    barShown(shell, true);

    unmount();

    expect(shell.menuBar).toEqual(["hide"]);
  });

  it("is not asked to hide when it was not shown as the app goes", async () => {
    const { shell, unmount } = await setup();
    await findMainNavigation();
    unmount();
    expect(shell.menuBar).toEqual([]);
  });
});

describe("a choice of the menu", () => {
  it("toggle sidebar hides the sidebar and shows it again", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    expect(selectSidebarHidden(appStore().getState())).toBe(false);

    menu(shell, "toggleSidebar");
    await waitFor(() => {
      expect(selectSidebarHidden(appStore().getState())).toBe(true);
    });
    menu(shell, "toggleSidebar");
    await waitFor(() => {
      expect(selectSidebarHidden(appStore().getState())).toBe(false);
    });
  });

  it("back and forward step the active tab's history, and bring the page in front of Settings", async () => {
    const { user, shell, ids } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    await openPage(user, "Plan");
    expect(activePage()).toBe(ids.page("Plan"));

    menu(shell, "back");
    expect(activePage()).toBe(ids.page("Notes"));
    menu(shell, "forward");
    expect(activePage()).toBe(ids.page("Plan"));
    // With nothing further, forward does nothing harmful.
    menu(shell, "forward");
    expect(activePage()).toBe(ids.page("Plan"));

    act(() => {
      appStore().getState().navigate("settings");
    });
    menu(shell, "back");
    expect(appStore().getState().route).toBe("home");
    expect(activePage()).toBe(ids.page("Notes"));
  });

  it("find opens the find bar of the open page", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    await screen.findByRole("textbox", { name: t("editor.label") });
    expect(screen.queryByRole("search", { name: t("find.label") })).toBeNull();

    menu(shell, "find");

    expect(await screen.findByRole("search", { name: t("find.label") })).toBeVisible();
  });

  it("print starts print mode for the open page", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { user, shell } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    await screen.findByRole("textbox", { name: t("editor.label") });

    menu(shell, "print");

    await waitFor(() => {
      expect(print).toHaveBeenCalledOnce();
    });
  });

  it("find and print with no page open say so and do nothing", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { shell } = await setup();
    await findMainNavigation();
    expect(screen.queryByRole("heading", { name: "Notes" })).toBeNull();

    for (const id of ["find", "print"] as const) {
      menu(shell, id);
      await waitFor(() => {
        expect(within(toasts()).getAllByText(t("menu.noPage")).length).toBeGreaterThan(0);
      });
    }
    expect(appStore().getState().printing).toBe(false);
    expect(print).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("export opens the Export dialog of the open page, and of the space when no page is open", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    menu(shell, "export");
    const space = await screen.findByRole("dialog", { name: t("export.title.space", { name: "Personal" }) });
    await user.click(within(space).getByRole("button", { name: t("dialog.cancel") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(appStore().getState().fileRequest).toBeNull();

    await openPage(user, "Notes");
    await screen.findByRole("textbox", { name: t("editor.label") });
    menu(shell, "export");
    const page = await screen.findByRole("dialog", { name: t("export.title.page", { name: "Notes" }) });
    // One dialog, not the space's as well.
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    await user.click(within(page).getByRole("button", { name: t("dialog.cancel") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(appStore().getState().fileRequest).toBeNull();
  });

  it("import opens the Import dialog of the current space, with a page open or not", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    menu(shell, "import");
    let dialog = await screen.findByRole("dialog", { name: t("import.title") });
    expect(within(dialog).getByText(t("import.intro.space", { name: "Personal" }))).toBeVisible();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    await openPage(user, "Notes");
    await screen.findByRole("textbox", { name: t("editor.label") });
    menu(shell, "import");
    dialog = await screen.findByRole("dialog", { name: t("import.title") });
    expect(within(dialog).getByText(t("import.intro.space", { name: "Personal" }))).toBeVisible();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
  });

  it("export and import say so when the shell has no files", async () => {
    const { shell } = await setup({ files: false });
    await findMainNavigation();

    menu(shell, "export");
    menu(shell, "import");

    await waitFor(() => {
      expect(within(toasts()).getAllByText(t("menu.unavailable")).length).toBeGreaterThan(0);
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(appStore().getState().fileRequest).toBeNull();
  });

  it("about shows the app's name and version, and closes", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();

    menu(shell, "about");

    const dialog = await screen.findByRole("dialog", { name: t("about.title") });
    expect(within(dialog).getByText(t("app.name"))).toBeVisible();
    expect(within(dialog).getByText(/^Version \d+\.\d+\.\d+/)).toBeVisible();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(appStore().getState().aboutOpen).toBe(false);

    // And Escape closes it as well.
    menu(shell, "about");
    await screen.findByRole("dialog", { name: t("about.title") });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("undo and redo send the keys the keyboard sends: Ctrl+Z and Ctrl+Shift+Z where the focus is", async () => {
    const { user, shell } = await setup();
    await findMainNavigation();
    await openPage(user, "Notes");
    const editor = await screen.findByRole("textbox", { name: t("editor.label") });
    await user.click(editor);

    const seen: { key: string; ctrl: boolean; shift: boolean; target: EventTarget | null }[] = [];
    const listener = (event: KeyboardEvent): void => {
      seen.push({ key: event.key, ctrl: event.ctrlKey, shift: event.shiftKey, target: event.target });
    };
    document.addEventListener("keydown", listener);

    menu(shell, "undo");
    menu(shell, "redo");
    document.removeEventListener("keydown", listener);

    expect(seen.map(({ key, ctrl, shift }) => ({ key, ctrl, shift }))).toEqual([
      { key: "z", ctrl: true, shift: false },
      { key: "z", ctrl: true, shift: true },
    ]);
    // Sent where the focus is, so the editor's own binding runs (the sidebar's structural undo is a document listener).
    expect(editor.contains(seen[0]?.target as Node)).toBe(true);
  });

  it("every command of the menu is handled by something", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    const all: MenuCommand[] = ["import", "export", "print", "undo", "redo", "find", "toggleSidebar", "back", "forward", "about"];
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    for (const id of all) {
      expect(() => {
        menu(shell, id);
      }, id).not.toThrow();
    }
  });

  it("does nothing while the app is locked", async () => {
    const { shell } = await setup({ locked: true });
    await screen.findByRole("heading", { name: t("lock.title") });

    for (const id of ["about", "export", "import", "print", "find", "toggleSidebar", "back", "forward", "undo"] as const) {
      menu(shell, id);
    }

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(appStore().getState().aboutOpen).toBe(false);
    expect(appStore().getState().fileRequest).toBeNull();
    expect(appStore().getState().printing).toBe(false);
  });

  it("a dialog the menu opened does not wait behind the lock screen for the unlock", async () => {
    const { user, shell } = await setup({ appLock: true });
    await findMainNavigation();
    menu(shell, "about");
    await screen.findByRole("dialog", { name: t("about.title") });

    await act(async () => {
      await appStore().getState().lock();
    });
    await screen.findByRole("heading", { name: t("lock.title") });
    await act(async () => {
      await appStore().getState().unlock(PASSPHRASE);
    });
    await findMainNavigation();

    expect(appStore().getState().aboutOpen).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
    // The menu still works after it.
    menu(shell, "about");
    await user.click(await screen.findByRole("button", { name: t("dialog.close") }));
  });
});
