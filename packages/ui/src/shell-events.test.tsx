// What the desktop shell tells the page (ADR-0040), in the whole app over the in-memory core with a shell around it
// (`test/shell.ts`): the quick capture (the global shortcut and the tray's "New page") makes a page at the top level
// of the space in use with its title ready to type, and waits for the workspace like a deep link does; the tray's
// "Lock" runs the UI's lock action; the state of the shortcut is kept for Settings; and the page gives the shell the
// words of the tray menu as soon as it starts.

import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { JsonValue, PageId, ShellEvent, SpaceId } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import type { AppStore } from "./store";
import type * as StoreModule from "./store";
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
  { name: "Work", pages: [{ title: "Meetings", content: doc("Weekly sync") }] },
];

const CAPTURE: ShellEvent = { type: "capture" };
const LOCK_ON = { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE };
const LOCKED = { ...LOCK_ON, startLocked: true };

async function setup(options: ShellOptions & { locked?: boolean; appLock?: boolean; spaces?: typeof SPACES } = {}) {
  const { locked, appLock, spaces = SPACES, ...shellOptions } = options;
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces,
    ...(locked === true ? { security: LOCKED } : appLock === true ? { security: LOCK_ON } : {}),
  });
  // A locked core answers nothing: the ids are read with it open, then it is locked again.
  if (locked === true) {
    await memory.unlock(PASSPHRASE);
  }
  const ids = await idsOf(memory);
  if (locked === true) {
    await memory.lock();
  }
  const shell = withShell(memory, shellOptions);
  const app = renderWith(shell.transport);
  return { ...app, memory, ids, shell };
}

/** The top-level pages of a space as the core holds them, in order. */
async function rootTitles(memory: MemoryTransport, spaceId: SpaceId): Promise<{ id: PageId; title: string }[]> {
  const tree = await new CoreClient(memory).treeGet(spaceId);
  return tree.roots.map((node) => ({ id: node.page.id, title: node.page.title }));
}

function activeEntry() {
  const { tabs, activeTabId } = appStore().getState();
  const tab = tabs.find((candidate) => candidate.id === activeTabId);
  const entry = tab?.entries[tab.index];
  return { tabs: tabs.length, pageId: entry?.pageId ?? null, spaceId: entry?.spaceId ?? null };
}

const toasts = () => screen.getByRole("log", { name: t("toast.label") });

const titleField = () => screen.findByRole("textbox", { name: t("tree.renameLabel") });

describe("quick capture", () => {
  it("makes a page at the top level of the space in use, opens it and puts the title in edit", async () => {
    const { user, shell, memory, ids } = await setup();
    await findMainNavigation();
    await openPage(user, "Plan");
    const before = await rootTitles(memory, ids.space("Personal"));

    act(() => {
      shell.emit(CAPTURE);
    });

    const field = await titleField();
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    const after = await rootTitles(memory, ids.space("Personal"));
    expect(after).toHaveLength(before.length + 1);
    // At the end of the top level, untitled.
    const made = after.at(-1);
    expect(made?.title).toBe("");
    expect(activeEntry()).toMatchObject({ tabs: 1, pageId: made?.id, spaceId: ids.space("Personal") });
    // Nothing was made in the other space.
    expect(await rootTitles(memory, ids.space("Work"))).toHaveLength(1);
  });

  it("types the title it was waiting for and keeps it", async () => {
    const { user, shell, memory, ids } = await setup();
    await findMainNavigation();

    act(() => {
      shell.emit(CAPTURE);
    });
    const field = await titleField();
    await user.type(field, "Idea{Enter}");

    await waitFor(async () => {
      expect((await rootTitles(memory, ids.space("Personal"))).map((page) => page.title)).toContain("Idea");
    });
  });

  it("makes the page in the space of the active tab", async () => {
    const { shell, memory, ids } = await setup();
    await findMainNavigation();
    // Open a page of the other space through the store, as a link to it would.
    act(() => {
      appStore().getState().navigateTo({ pageId: ids.page("Meetings"), spaceId: ids.space("Work") });
    });
    await waitFor(() => {
      expect(appStore().getState().currentSpaceId).toBe(ids.space("Work"));
    });

    act(() => {
      shell.emit(CAPTURE);
    });

    await titleField();
    const work = await rootTitles(memory, ids.space("Work"));
    expect(work).toHaveLength(2);
    expect(activeEntry().spaceId).toBe(ids.space("Work"));
    expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(2);
  });

  it("makes a top-level page even when a sub-page is the one that is open", async () => {
    const { shell, memory, ids } = await setup();
    await findMainNavigation();
    const child = await new CoreClient(memory).pageCreate({ spaceId: ids.space("Personal"), parentId: ids.page("Notes"), title: "Child" });
    act(() => {
      appStore().getState().navigateTo({ pageId: child.id, spaceId: ids.space("Personal") });
    });
    await waitFor(() => {
      expect(activeEntry().pageId).toBe(child.id);
    });

    act(() => {
      shell.emit(CAPTURE);
    });

    await titleField();
    const roots = await rootTitles(memory, ids.space("Personal"));
    expect(roots).toHaveLength(3);
    expect(roots.at(-1)?.title).toBe("");
    expect(activeEntry().pageId).toBe(roots.at(-1)?.id);
  });

  it("opens the page from the settings screen too", async () => {
    const { user, shell } = await setup();
    const nav = await findMainNavigation();
    await user.click(within(nav).getByRole("button", { name: t("nav.settings") }));
    await screen.findByRole("heading", { name: t("settings.title") });

    act(() => {
      shell.emit(CAPTURE);
    });

    await titleField();
    expect(screen.queryByRole("heading", { name: t("settings.title") })).not.toBeInTheDocument();
    expect(appStore().getState().route).toBe("home");
  });

  it("makes one page for each capture", async () => {
    const { shell, memory, ids } = await setup();
    await findMainNavigation();
    for (let made = 1; made <= 2; made += 1) {
      act(() => {
        shell.emit(CAPTURE);
      });
      await waitFor(async () => {
        expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(2 + made);
      });
    }
  });

  it("takes a capture that came before the page loaded, once the app is ready", async () => {
    // The shortcut was pressed while the app was still starting: the shell delivers it at the subscription.
    const { memory, ids } = await setup({ held: [CAPTURE] });

    await titleField();
    await findMainNavigation();
    expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(3);
  });

  it("does nothing without a space to make a page in", async () => {
    const { shell, memory } = await setup({ spaces: [] });
    await findMainNavigation();
    act(() => {
      shell.emit(CAPTURE);
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(await new CoreClient(memory).spaceList()).toEqual([]);
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
  });

  it("says so when the page could not be made, naming nothing of the core's answer", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { shell, memory } = await setup();
    await findMainNavigation();
    const dispatch = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PageCreate" ? Promise.reject(new Error("the secret reason")) : dispatch(request),
    );

    act(() => {
      shell.emit(CAPTURE);
    });

    expect(await within(toasts()).findByText(t("shell.capture.error"))).toBeInTheDocument();
    expect(toasts().textContent).not.toContain("secret");
    expect(logged).toHaveBeenCalled();
    expect(JSON.stringify(logged.mock.calls)).not.toContain("secret");
  });
});

describe("a capture that comes before the workspace is open", () => {
  it("waits for the unlock when the app is locked, and shows nothing before", async () => {
    const { user, shell, memory, ids } = await setup({ locked: true });
    const field = await screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));

    act(() => {
      shell.emit(CAPTURE);
    });
    await act(async () => {
      await Promise.resolve();
    });
    // Still the lock screen: no navigation, no page, no title being edited.
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();

    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    await titleField();
    await findMainNavigation();
    expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(3);
  });

  it("makes one page however many captures came while locked", async () => {
    const { user, shell, memory, ids } = await setup({ locked: true });
    const field = await screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));
    act(() => {
      shell.emit(CAPTURE);
      shell.emit(CAPTURE);
      shell.emit(CAPTURE);
    });

    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    await titleField();
    await findMainNavigation();
    expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(3);
  });

  it("holds a capture that comes after the app locks itself, and makes the page after the unlock", async () => {
    const { user, shell, memory, ids } = await setup({ locked: true });
    await user.type(await screen.findByLabelText(t("lock.passphraseLabel")), PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    await findMainNavigation();

    await act(async () => {
      await appStore().getState().lock();
    });
    await screen.findByLabelText(t("lock.passphraseLabel"));
    act(() => {
      shell.emit(CAPTURE);
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();

    await user.type(screen.getByLabelText(t("lock.passphraseLabel")), PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    await titleField();
    expect(await rootTitles(memory, ids.space("Personal"))).toHaveLength(3);
  });

  it("is dropped when there is no workspace to make a page in", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const shell = withShell(memory, { held: [CAPTURE] });
    renderWith(shell.transport);
    // The first-run screen: nothing is made, and nothing is made later either.
    await screen.findByRole("heading", { level: 1 });
    expect(appStore().getState().phase).toBe("onboarding");
    expect(await new CoreClient(memory).spaceList()).toEqual([]);
  });
});

describe("the tray's Lock", () => {
  it("runs the lock action of the app: the lock screen shows", async () => {
    const { shell } = await setup({ appLock: true });
    await findMainNavigation();

    act(() => {
      shell.emit({ type: "lock" });
    });

    expect(await screen.findByLabelText(t("lock.passphraseLabel"))).toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
  });

  it("sends the editor's unsaved text before the core closes", async () => {
    const { user, shell, memory } = await setup({ appLock: true });
    await findMainNavigation();
    const flush = vi.spyOn(appStore().getState().pageDocs, "flushAll");
    await openPage(user, "Plan");
    const lock = vi.spyOn(memory, "lock");

    act(() => {
      shell.emit({ type: "lock" });
    });

    await screen.findByLabelText(t("lock.passphraseLabel"));
    expect(flush).toHaveBeenCalled();
    expect(lock).toHaveBeenCalled();
    expect(flush.mock.invocationCallOrder[0]).toBeLessThan(lock.mock.invocationCallOrder[0] ?? 0);
  });

  it("says that there is nothing to lock when the app lock is off", async () => {
    const { shell } = await setup();
    await findMainNavigation();

    act(() => {
      shell.emit({ type: "lock" });
    });

    expect(await within(toasts()).findByText(t("shell.lock.unavailable"))).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
  });

  it("does nothing to an app that is locked already", async () => {
    const { shell } = await setup({ locked: true });
    await screen.findByLabelText(t("lock.passphraseLabel"));
    act(() => {
      shell.emit({ type: "lock" });
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByLabelText(t("lock.passphraseLabel"))).toBeInTheDocument();
    expect(screen.queryByText(t("shell.lock.unavailable"))).not.toBeInTheDocument();
  });
});

describe("the state of the capture shortcut", () => {
  it("is kept as the shell reports it, from the subscription on", async () => {
    const { shell } = await setup({ shortcut: "taken" });
    await findMainNavigation();
    expect(appStore().getState().shortcutState).toBe("taken");

    act(() => {
      shell.emit({ type: "shortcut", data: { state: "registered" } });
    });
    expect(appStore().getState().shortcutState).toBe("registered");
    act(() => {
      shell.emit({ type: "shortcut", data: { state: "invalid" } });
    });
    expect(appStore().getState().shortcutState).toBe("invalid");
  });

  it("is not known before the shell says anything", async () => {
    await setup();
    await findMainNavigation();
    expect(appStore().getState().shortcutState).toBeNull();
  });

  it("survives a lock: the shell does not say it again", async () => {
    const { user, shell } = await setup({ locked: true, shortcut: "registered" });
    await user.type(await screen.findByLabelText(t("lock.passphraseLabel")), PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    await findMainNavigation();
    expect(appStore().getState().shortcutState).toBe("registered");
    act(() => {
      shell.emit({ type: "shortcut", data: { state: "taken" } });
    });
    await act(async () => {
      await appStore().getState().lock();
    });
    await screen.findByLabelText(t("lock.passphraseLabel"));
    expect(appStore().getState().shortcutState).toBe("taken");
  });
});

describe("listening", () => {
  it("listens from the start and stops with the app", async () => {
    const { shell, unmount } = await setup();
    await findMainNavigation();
    expect(shell.listeners()).toBe(1);
    unmount();
    expect(shell.listeners()).toBe(0);
  });

  it("knows it has a desktop shell only when the transport has one", async () => {
    await setup();
    await findMainNavigation();
    expect(appStore().getState().desktopShell).toBe(true);
  });

  it("knows it has none over the in-memory core alone", async () => {
    renderApp({ workspaceName: "Research", spaces: SPACES });
    await findMainNavigation();
    expect(appStore().getState().desktopShell).toBe(false);
  });
});

describe("the words of the tray", () => {
  it("are sent to the shell as soon as the app starts, from the strings of the UI", async () => {
    const { shell } = await setup();
    await findMainNavigation();
    // One message: the tray's four words and the menu bar's (`menu-bar-app.test.tsx` has those).
    expect(shell.labels).toHaveLength(1);
    expect(shell.labels[0]).toMatchObject({ open: t("tray.open"), newPage: t("tray.newPage"), lock: t("tray.lock"), quit: t("tray.quit") });
    expect(shell.labels[0]).toMatchObject({ open: "Open atomik-kb", newPage: "New page", lock: "Lock", quit: "Quit" });
  });

  it("are sent before the workspace is open: while the app is locked", async () => {
    const { shell } = await setup({ locked: true });
    await screen.findByLabelText(t("lock.passphraseLabel"));
    expect(shell.labels).toHaveLength(1);
  });

  it("do not keep the app from starting when the shell refuses them", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: SPACES });
    const shell = withShell(memory);
    shell.failLabels(new CoreError({ code: "InvalidInput", message: "a label is 1 to 64 characters: SECRETLABEL" }));
    renderWith(shell.transport);

    await findMainNavigation();
    await waitFor(() => {
      expect(logged).toHaveBeenCalled();
    });
    // The code is logged, not the message (a label is the person's language).
    expect(JSON.stringify(logged.mock.calls)).not.toContain("SECRETLABEL");
    expect(logged.mock.calls.some((call) => call.includes("InvalidInput"))).toBe(true);
  });
});
