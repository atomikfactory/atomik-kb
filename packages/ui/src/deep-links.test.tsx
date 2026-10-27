// Deep links in the whole app (ADR-0040): a link of the operating system (`atomik-kb://page/<id>[#<block>]`, which
// the desktop shell accepts and hands to the UI as `{ pageId, blockId? }`) opens the page in the active tab like a
// plain click, scrolls to the block, says so when the page is gone or in the trash, waits for the workspace when the
// app is still opening or locked (the last link wins), and the page menu's "Copy link to page" writes the link.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { DeepLink, JsonValue } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { blockLinkText } from "./editor/block-link";
import type { AppStore } from "./store";
import type * as StoreModule from "./store";
import { withDeepLinks } from "./test/deep-links";
import { editorOf, idsOf, openPage, renderPages } from "./test/page";
import { NOW, findMainNavigation, renderWith } from "./test/render";
import { PASSPHRASE } from "./test/security";

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
  Reflect.deleteProperty(navigator, "clipboard");
});

function appStore(): AppStore {
  const store = created.stores.at(-1);
  if (store === undefined) {
    throw new Error("the app has not created its store");
  }
  return store;
}

const NOTES_1 = "bbbbbbbb-0000-4000-8000-000000000001";
const NOTES_2 = "bbbbbbbb-0000-4000-8000-000000000002";
const PLAN_1 = "bbbbbbbb-0000-4000-8000-000000000003";
const MEETING_1 = "bbbbbbbb-0000-4000-8000-000000000004";
const UNKNOWN = "99999999-0000-4000-8000-000000000009";

function doc(...blocks: [id: string, text: string][]): JsonValue {
  return {
    type: "doc",
    content: blocks.map(([id, text]) => ({ type: "paragraph", attrs: { id }, content: [{ type: "text", text }] })),
  };
}

const SPACES = [
  {
    name: "Personal",
    pages: [
      { title: "Notes", content: doc([NOTES_1, "Water the plants"], [NOTES_2, "Call the dentist"]) },
      { title: "Plan", content: doc([PLAN_1, "Ship it"]) },
    ],
  },
  { name: "Work", pages: [{ title: "Meetings", content: doc([MEETING_1, "Weekly sync"]) }] },
];

/** The app-lock options of the lock tests: a kit and a passphrase, starting locked. */
const LOCKED = { kitCreatedAt: NOW, appLockPassphrase: PASSPHRASE, startLocked: true };

/**
 * The app over the in-memory core with the links of an operating system. `held` is a link the app was started
 * with, given by the ids of the seeded pages (the shell delivers it at the first subscription).
 */
async function setup(options: { held?: (ids: Awaited<ReturnType<typeof idsOf>>) => DeepLink; locked?: boolean } = {}) {
  const memory = new MemoryTransport({
    now: () => NOW,
    workspaceName: "Research",
    spaces: SPACES,
    ...(options.locked === true ? { security: LOCKED } : {}),
  });
  // A locked core answers nothing: the ids are read with it open, then it is locked again.
  if (options.locked === true) {
    await memory.unlock(PASSPHRASE);
  }
  const ids = await idsOf(memory);
  if (options.locked === true) {
    await memory.lock();
  }
  const links = withDeepLinks(memory, options.held?.(ids));
  const app = renderWith(links.transport);
  return { ...app, memory, ids, links };
}

/** The editor once it shows a page with `text`: the page that was open before may still be on its way out. */
function editorShowing(text: string) {
  return waitFor(() => {
    const box = screen.getByRole("textbox", { name: t("editor.label") });
    expect(box).toHaveTextContent(text);
    return editorOf(box);
  });
}

/** The active tab's entry. */
function activeEntry() {
  const { tabs, activeTabId } = appStore().getState();
  const tab = tabs.find((candidate) => candidate.id === activeTabId);
  const entry = tab?.entries[tab.index];
  return { tabs: tabs.length, pageId: entry?.pageId ?? null, spaceId: entry?.spaceId ?? null, history: tab?.entries.length ?? 0 };
}

const toasts = () => screen.getByRole("log", { name: t("toast.label") });

describe("opening a link of the operating system", () => {
  it("opens the page in the active tab, as a plain click does", async () => {
    const { user, links, ids } = await setup();
    await findMainNavigation();
    await openPage(user, "Plan");
    const before = activeEntry();
    expect(before.pageId).toBe(ids.page("Plan"));

    act(() => {
      links.emit({ pageId: ids.page("Notes") });
    });

    await editorShowing("Water the plants");
    const after = activeEntry();
    // The same tab, one entry further along its history: back goes to the page that was open.
    expect(after).toMatchObject({ tabs: 1, pageId: ids.page("Notes"), history: before.history + 1 });
    act(() => {
      appStore().getState().goBack();
    });
    expect(activeEntry().pageId).toBe(ids.page("Plan"));
  });

  it("scrolls to the block the link names", async () => {
    const { links, ids } = await setup();
    await findMainNavigation();

    act(() => {
      links.emit({ pageId: ids.page("Notes"), blockId: NOTES_2 });
    });

    const editor = await editorShowing("Call the dentist");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".is-target")?.textContent).toContain("Call the dentist");
    });
  });

  it("switches to the space of the page", async () => {
    const { links, ids } = await setup();
    await findMainNavigation();
    expect(appStore().getState().currentSpaceId).toBe(ids.space("Personal"));

    act(() => {
      links.emit({ pageId: ids.page("Meetings") });
    });

    await editorShowing("Weekly sync");
    expect(appStore().getState().currentSpaceId).toBe(ids.space("Work"));
    expect(activeEntry()).toMatchObject({ pageId: ids.page("Meetings"), spaceId: ids.space("Work") });
  });

  it("opens a second link over the first in the same tab", async () => {
    const { links, ids } = await setup();
    await findMainNavigation();
    act(() => {
      links.emit({ pageId: ids.page("Notes") });
    });
    await waitFor(() => {
      expect(activeEntry().pageId).toBe(ids.page("Notes"));
    });
    act(() => {
      links.emit({ pageId: ids.page("Plan"), blockId: PLAN_1 });
    });
    await waitFor(() => {
      expect(activeEntry().pageId).toBe(ids.page("Plan"));
    });
    expect(activeEntry().tabs).toBe(1);
  });

  it("listens from the start and stops with the app", async () => {
    const { links, unmount } = await setup();
    await findMainNavigation();
    expect(links.listeners()).toBe(1);
    unmount();
    expect(links.listeners()).toBe(0);
  });
});

describe("a link that names no page", () => {
  it("says so in a toast and leaves the open page alone", async () => {
    const { user, links, ids } = await setup();
    await findMainNavigation();
    await openPage(user, "Plan");

    act(() => {
      links.emit({ pageId: UNKNOWN, blockId: NOTES_1 });
    });

    expect(await within(toasts()).findByText(t("deepLink.unknown"))).toBeInTheDocument();
    expect(activeEntry()).toMatchObject({ pageId: ids.page("Plan"), tabs: 1 });
  });

  it("treats a page in the trash as one that is not there", async () => {
    const { user, links, ids, memory } = await setup();
    await findMainNavigation();
    await openPage(user, "Plan");
    await new CoreClient(memory).pageTrash(ids.page("Notes"));

    act(() => {
      links.emit({ pageId: ids.page("Notes") });
    });

    expect(await within(toasts()).findByText(t("deepLink.unknown"))).toBeInTheDocument();
    expect(activeEntry().pageId).toBe(ids.page("Plan"));
  });

  it("never puts the link or its ids in the toast", async () => {
    const { links } = await setup();
    await findMainNavigation();
    act(() => {
      links.emit({ pageId: UNKNOWN, blockId: NOTES_1 });
    });
    await within(toasts()).findByText(t("deepLink.unknown"));
    expect(toasts().textContent).not.toContain(UNKNOWN);
    expect(toasts().textContent).not.toContain(NOTES_1);
  });
});

describe("a link that comes before the workspace is open", () => {
  it("waits for the app to be ready when it started the app", async () => {
    // The shell delivers the link it was started with at the first subscription: the core is still opening.
    const { ids } = await setup({ held: (known) => ({ pageId: known.page("Notes"), blockId: NOTES_1 }) });

    const editor = await editorShowing("Water the plants");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".is-target")?.textContent).toContain("Water the plants");
    });
    expect(activeEntry()).toMatchObject({ pageId: ids.page("Notes"), tabs: 1 });
  });

  it("holds a link while the app is locked and opens it after the unlock, showing nothing before", async () => {
    const { user, links, ids } = await setup({ locked: true });
    const field = await screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));

    act(() => {
      links.emit({ pageId: ids.page("Notes"), blockId: NOTES_2 });
    });
    // Still the lock screen: no page, no tree, no message about the link.
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: t("editor.label") })).not.toBeInTheDocument();
    expect(screen.queryByText(t("deepLink.unknown"))).not.toBeInTheDocument();

    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    const editor = await editorShowing("Call the dentist");
    await waitFor(() => {
      expect(editor.view.dom.querySelector(".is-target")?.textContent).toContain("Call the dentist");
    });
    expect(activeEntry()).toMatchObject({ pageId: ids.page("Notes") });
  });

  it("opens only the last of several links that came while locked", async () => {
    const { user, links, ids } = await setup({ locked: true });
    const field = await screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));
    act(() => {
      links.emit({ pageId: ids.page("Notes") });
      links.emit({ pageId: ids.page("Plan") });
    });

    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    await findMainNavigation();
    await waitFor(() => {
      expect(activeEntry().pageId).toBe(ids.page("Plan"));
    });
    expect(activeEntry()).toMatchObject({ tabs: 1, history: 2 }); // Home, then Plan: Notes was never opened
  });

  it("holds a link that comes after the app locks itself, and opens it after the unlock", async () => {
    const { user, links, ids } = await setup({ locked: true });
    const field = await screen.findByLabelText<HTMLInputElement>(t("lock.passphraseLabel"));
    await user.type(field, PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));
    await findMainNavigation();

    await act(async () => {
      await appStore().getState().lock();
    });
    await screen.findByLabelText(t("lock.passphraseLabel"));
    act(() => {
      links.emit({ pageId: ids.page("Plan") });
    });
    await user.type(screen.getByLabelText(t("lock.passphraseLabel")), PASSPHRASE);
    await user.click(screen.getByRole("button", { name: t("lock.submit") }));

    await findMainNavigation();
    await waitFor(() => {
      expect(activeEntry().pageId).toBe(ids.page("Plan"));
    });
  });

  it("drops a link that comes before the workspace exists", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const links = withDeepLinks(memory, { pageId: UNKNOWN });
    renderWith(links.transport);
    // The first-run screen: there is no workspace to open a page in, and nothing is said about the link.
    await screen.findByRole("heading", { level: 1 });
    expect(screen.queryByText(t("deepLink.unknown"))).not.toBeInTheDocument();
    expect(appStore().getState().phase).toBe("onboarding");
  });
});

describe("Copy link to page", () => {
  it("writes atomik-kb://page/<id> for the open page and says so", async () => {
    const { user, memory } = await renderPages(SPACES);
    await openPage(user, "Plan");
    const { page } = await idsOf(memory);
    const writeText = vi.spyOn(navigator.clipboard, "writeText");

    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyLink") }));

    const expected = `atomik-kb://page/${page("Plan")}`;
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith(expected);
    });
    expect(await navigator.clipboard.readText()).toBe(expected);
    expect(expected).toBe(blockLinkText(page("Plan")));
    expect(await screen.findByText(t("page.linkCopied"))).toBeInTheDocument();
  });

  it("shows the link to copy by hand where the clipboard cannot be written", async () => {
    const { user, memory } = await renderPages(SPACES);
    await openPage(user, "Plan");
    const { page } = await idsOf(memory);
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));

    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("page.copyLink") }));

    expect(
      await screen.findByText(t("page.linkNoClipboard", { link: `atomik-kb://page/${page("Plan")}` })),
    ).toBeInTheDocument();
  });

  it("is in the page menu next to Copy as Markdown", async () => {
    const { user } = await renderPages(SPACES);
    await openPage(user, "Plan");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const names = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(names).toContain(t("page.copyLink"));
    expect(names.indexOf(t("page.copyLink"))).toBe(names.indexOf(t("page.copyMarkdown")) + 1);
  });
});
