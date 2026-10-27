import { CoreClient, CoreError, MemoryTransport } from "@atomik-kb/core-client";
import type { Transport } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { reportActivity } from "./activity";
import { t } from "./i18n";
import { AUTO_LOCK_KEY } from "./store-security";
import { paragraphs } from "./test/editor";
import { contentOf, idsOf, openPage, renderPages } from "./test/page";
import { NOW, findMainNavigation, recordingTransport, renderApp, renderWith } from "./test/render";
import type { User } from "./test/render";
import { PASSPHRASE } from "./test/security";
import { PERSONAL, outline, renderSidebar, sidebarButton } from "./test/sidebar";

/** A kit exists and the app lock is on. */
const LOCK_ON = { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE };
/** The same, and the app starts locked. */
const LOCKED = { ...LOCK_ON, startLocked: true };

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function findPassphrase(): Promise<HTMLInputElement> {
  return screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));
}

async function unlockWith(user: User, passphrase: string): Promise<void> {
  await user.type(await findPassphrase(), passphrase);
  await user.click(screen.getByRole("button", { name: t("lock.submit") }));
}

describe("lock screen", () => {
  it("shows nothing but the passphrase field when the app starts locked", async () => {
    renderApp({ workspaceName: "Research", spaces: [PERSONAL], settings: { "ui.theme": "dark" }, security: LOCKED });
    const field = await findPassphrase();
    expect(screen.getByRole("heading", { name: t("lock.title"), level: 1 })).toBeInTheDocument();
    expect(field).toHaveAttribute("type", "password");
    expect(field).toHaveAttribute("autocomplete", "current-password");
    expect(field).toHaveFocus();
    expect(screen.getByRole("button", { name: t("lock.submit") })).toBeEnabled();
    // Nothing from the workspace: no navigation, no name, no pages, not even its theme.
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(screen.queryByText("Research")).not.toBeInTheDocument();
    expect(screen.queryByText("Projects")).not.toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute("data-theme", "system");
  });

  it("asks for the passphrase before trying", async () => {
    const { user, memory } = renderApp({ workspaceName: "Research", security: LOCKED });
    const unlock = vi.spyOn(memory, "unlock");
    await findPassphrase();
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(t("lock.error.empty"));
    expect(unlock).not.toHaveBeenCalled();
  });

  it("says so when the passphrase is wrong, empties the field and stays locked", async () => {
    const { user, memory } = renderApp({ workspaceName: "Research", security: LOCKED });
    await unlockWith(user, "not the passphrase");
    expect(await screen.findByRole("alert")).toHaveTextContent(t("lock.error.wrong"));
    const field = screen.getByLabelText(t("lock.passphraseLabel"));
    expect(field).toHaveValue("");
    expect(field).toHaveAttribute("aria-invalid", "true");
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    await expect(memory.coreState()).resolves.toBe("locked");
    // Typing again clears the error.
    await user.type(field, "x");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("opens the workspace with the right passphrase", async () => {
    const { user } = renderApp({
      workspaceName: "Research",
      spaces: [PERSONAL],
      settings: { "ui.theme": "dark" },
      security: LOCKED,
    });
    await unlockWith(user, PASSPHRASE);
    const nav = await findMainNavigation();
    expect(within(nav).getByText("Research")).toBeInTheDocument();
    expect(outline()).toContain("Projects");
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("says 'Unlocking...' while the core derives the key", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: LOCKED });
    const recorder = recordingTransport(memory);
    const { user } = renderWith(recorder.transport);
    await findPassphrase();
    const release = recorder.hold();
    await unlockWith(user, PASSPHRASE);
    expect(await screen.findByRole("button", { name: t("lock.submitting") })).toBeDisabled();
    expect(screen.getByLabelText(t("lock.passphraseLabel"))).toHaveAttribute("readonly");
    release();
    await findMainNavigation();
  });

  it("shows other failures as they are", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: LOCKED });
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      lock: () => memory.lock(),
      unlock: () => Promise.reject(new CoreError({ code: "KeystoreUnavailable", message: "no usable keystore" })),
    };
    const { user } = renderWith(transport);
    await unlockWith(user, PASSPHRASE);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("lock.error.generic", { message: "no usable keystore" }),
    );
  });
});

describe("Lock now", () => {
  it("is in the sidebar only while the app lock is on", async () => {
    await renderSidebar();
    expect(screen.queryByRole("button", { name: t("sidebar.lockNow") })).not.toBeInTheDocument();
  });

  it("locks from the sidebar, forgets the workspace, and comes back with the page that was open (the tabs come back from ui.tabs)", async () => {
    const { user, memory } = await renderSidebar({ security: LOCK_ON });
    await openPage(user, "Journal");
    await user.click(sidebarButton(t("sidebar.lockNow")));

    await findPassphrase();
    await expect(memory.coreState()).resolves.toBe("locked");
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(screen.queryByText("Journal")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: t("editor.label") })).not.toBeInTheDocument();

    await unlockWith(user, PASSPHRASE);
    await findMainNavigation();
    expect(outline()).toContain("Journal");
    // ADR-0034 section 5: the tab set is restored on unlock, so the page is open again (its editor included).
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    expect(await screen.findByRole("textbox", { name: t("editor.label") })).toBeInTheDocument();
    expect(screen.queryByText(t("home.hint.select"))).not.toBeInTheDocument();
  });

  it("saves what was typed just before locking", async () => {
    const { user, memory } = await renderPages([{ name: "Personal", pages: [{ title: "Notes", content: paragraphs("Draft") }] }], {
      security: LOCK_ON,
    });
    const editor = await openPage(user, "Notes");
    act(() => {
      editor.commands.focus("end");
      editor.commands.insertContent(" saved before the lock");
    });
    // Right away, well within the editor's flush interval.
    await user.click(sidebarButton(t("sidebar.lockNow")));
    await findPassphrase();
    await unlockWith(user, PASSPHRASE);
    await findMainNavigation();
    const { page } = await idsOf(memory);
    expect(JSON.stringify(await contentOf(memory, page("Notes")))).toContain("Draft saved before the lock");
  });

  it("reports a lock the shell refuses, in the sidebar", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: LOCK_ON });
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
      lock: () => Promise.reject(new CoreError({ code: "Internal", message: "the shell could not lock" })),
      unlock: (passphrase) => memory.unlock(passphrase),
    };
    const { user } = renderWith(transport);
    await findMainNavigation();
    await user.click(await screen.findByRole("button", { name: t("sidebar.lockNow") }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      t("security.lock.error.lock", { message: "the shell could not lock" }),
    );
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
  });

  it("is not offered when the shell cannot lock, even with the lock on", async () => {
    const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", security: LOCK_ON });
    const transport: Transport = {
      dispatch: (request) => memory.dispatch(request),
      subscribe: (onEvent) => memory.subscribe(onEvent),
      coreState: () => memory.coreState(),
      docs: memory.docs,
    };
    renderWith(transport);
    await findMainNavigation();
    expect(screen.queryByRole("button", { name: t("sidebar.lockNow") })).not.toBeInTheDocument();
  });
});

describe("auto-lock", () => {
  /** Fake timeouts that still move with real time, so the rest of the app and Testing Library run. */
  function fakeTimeouts(): void {
    vi.useFakeTimers({ shouldAdvanceTime: true, toFake: ["setTimeout", "clearTimeout"] });
  }

  async function advance(ms: number): Promise<void> {
    await act(() => vi.advanceTimersByTimeAsync(ms));
  }

  it("locks after the chosen minutes without activity; activity starts the wait again", async () => {
    fakeTimeouts();
    const { memory } = renderApp({ workspaceName: "Research", settings: { [AUTO_LOCK_KEY]: 1 }, security: LOCK_ON });
    await findMainNavigation();

    await advance(45_000);
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
    fireEvent.mouseMove(document.body);
    await advance(45_000);
    fireEvent.keyDown(document.body, { key: "a" });
    await advance(45_000);
    fireEvent.pointerDown(document.body);
    await advance(45_000);
    act(() => {
      sidebarButton(t("nav.settings")).focus();
    });
    await advance(45_000);
    // 3 minutes 45 seconds in, but never a minute without activity.
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();

    await advance(16_000);
    await findPassphrase();
    await expect(memory.coreState()).resolves.toBe("locked");
  });

  it("counts the activity the app reports itself (a dictation that hears speech) like typing", async () => {
    fakeTimeouts();
    const { memory } = renderApp({ workspaceName: "Research", settings: { [AUTO_LOCK_KEY]: 1 }, security: LOCK_ON });
    await findMainNavigation();

    // Four minutes with no key, no mouse and no focus, but something said every 45 seconds: never a minute of quiet.
    for (let spoken = 0; spoken < 5; spoken += 1) {
      await advance(45_000);
      act(() => {
        reportActivity();
      });
    }
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();

    // The speech stops: a minute later the app locks, as it does for a person who stops typing.
    await advance(61_000);
    await findPassphrase();
    await expect(memory.coreState()).resolves.toBe("locked");
  });

  it("does not lock with the delay set to never, or with the app lock off", async () => {
    fakeTimeouts();
    const never = renderApp({ workspaceName: "Research", settings: { [AUTO_LOCK_KEY]: 0 }, security: LOCK_ON });
    await findMainNavigation();
    await advance(2 * 60 * 60_000);
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
    never.unmount();

    renderApp({ workspaceName: "Research", settings: { [AUTO_LOCK_KEY]: 1 }, security: { kitCreatedAt: NOW } });
    await findMainNavigation();
    await advance(2 * 60 * 60_000);
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
  });

  it("follows a delay changed in Settings", async () => {
    fakeTimeouts();
    const { user, memory } = renderApp({
      workspaceName: "Research",
      settings: { [AUTO_LOCK_KEY]: 1 },
      security: LOCK_ON,
    });
    await user.click(await screen.findByRole("button", { name: t("nav.settings") }));
    await user.selectOptions(
      await screen.findByRole("combobox", { name: t("security.lock.auto") }),
      t("security.lock.auto.15"),
    );
    await waitFor(async () => {
      await expect(new CoreClient(memory).settingsGet(AUTO_LOCK_KEY)).resolves.toBe(15);
    });
    await advance(14 * 60_000);
    expect(screen.getByRole("navigation", { name: t("nav.label") })).toBeInTheDocument();
    await advance(61_000);
    await findPassphrase();
  });
});
