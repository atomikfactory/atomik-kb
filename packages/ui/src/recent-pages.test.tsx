// The recent pages (ADR-0029): what is recorded when a page is opened and by which routes, the
// list's shape (first, once, at most 30, no purged page), and where it is shown (Ctrl+K while
// nothing is typed, the Home screen) and cleared (Settings).

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, Request } from "@atomik-kb/core-client";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { formatAgo } from "./format";
import { t } from "./i18n";
import {
  RECENT_PAGES_MAX,
  parseRecentPages,
  withRecentPage,
} from "./store-layout";
import type { RecentEntry } from "./store-layout";
import { NOW, openScreen } from "./test/render";
import { idsOf, renderPages } from "./test/page";
import { PERSONAL, WORK, row } from "./test/sidebar";

const KEY = "ui.recent_pages";
const MINUTE = 60_000;

afterEach(() => {
  vi.restoreAllMocks();
});

const entry = (pageId: string, openedAt = 1000, spaceId = "space"): RecentEntry => ({ pageId, spaceId, openedAt });

describe("the stored list", () => {
  it("reads entries with a page, a space and a time, and skips what is malformed", () => {
    const value: JsonValue = [
      { pageId: "a", spaceId: "s", openedAt: 5 },
      { pageId: "b", spaceId: "s" },
      { pageId: 7, spaceId: "s", openedAt: 1 },
      "c",
      null,
      [1],
      { pageId: "d", spaceId: "s", openedAt: 9, extra: true },
    ];
    expect(parseRecentPages(value)).toEqual([entry("a", 5, "s"), entry("d", 9, "s")]);
    expect(parseRecentPages(null)).toEqual([]);
    expect(parseRecentPages({ pageId: "a" })).toEqual([]);
    expect(parseRecentPages("nope")).toEqual([]);
  });

  it("keeps a page that is listed twice once, at its first place, and at most 30 pages", () => {
    const twice = parseRecentPages([
      { pageId: "a", spaceId: "s", openedAt: 9 },
      { pageId: "b", spaceId: "s", openedAt: 8 },
      { pageId: "a", spaceId: "s", openedAt: 1 },
    ]);
    expect(twice.map((item) => item.pageId)).toEqual(["a", "b"]);
    const many = Array.from({ length: 45 }, (_, index) => ({ pageId: `p${String(index)}`, spaceId: "s", openedAt: index }));
    expect(parseRecentPages(many)).toHaveLength(RECENT_PAGES_MAX);
  });

  it("puts an opened page first, once, and drops the pages that are gone", () => {
    const list = [entry("a"), entry("b"), entry("c")];
    expect(withRecentPage(list, entry("c", 2000), () => false).map((item) => item.pageId)).toEqual(["c", "a", "b"]);
    expect(withRecentPage(list, entry("d", 2000), (id) => id === "b").map((item) => item.pageId)).toEqual(["d", "a", "c"]);
    expect(withRecentPage(list, entry("c", 2000), () => false)[0]?.openedAt).toBe(2000);
  });

  it("keeps the 30 most recent", () => {
    const list = Array.from({ length: 30 }, (_, index) => entry(`p${String(index)}`));
    const next = withRecentPage(list, entry("new"), () => false);
    expect(next).toHaveLength(30);
    expect(next[0]?.pageId).toBe("new");
    expect(next.at(-1)?.pageId).toBe("p28");
  });
});

const SPACES = [PERSONAL, WORK];

function recentWrites(memory: MemoryTransport): { spy: ReturnType<typeof vi.spyOn>; values: () => unknown[] } {
  const spy = vi.spyOn(memory, "dispatch");
  return {
    spy,
    values: () =>
      spy.mock.calls
        .map(([request]) => request)
        .filter((request): request is Extract<Request, { type: "SettingsSet" }> => request.type === "SettingsSet")
        .filter((request) => request.data.key === KEY)
        .map((request) => request.data.value),
  };
}

async function storedRecents(memory: MemoryTransport): Promise<readonly RecentEntry[]> {
  const response = await memory.dispatch({ type: "SettingsGet", data: { key: KEY } });
  return response.type === "Setting" ? parseRecentPages(response.data) : [];
}

/** The page ids of the stored list, as titles. */
async function storedTitles(memory: MemoryTransport): Promise<string[]> {
  const client = new CoreClient(memory);
  const titles: string[] = [];
  for (const item of await storedRecents(memory)) {
    titles.push((await client.pageGet(item.pageId)).title);
  }
  return titles;
}

async function seedRecents(memory: MemoryTransport, entries: readonly { pageId: string; spaceId: string; openedAt: number }[]) {
  await memory.dispatch({ type: "SettingsSet", data: { key: KEY, value: entries.map((item) => ({ ...item })) } });
}

describe("recording the pages that are opened", () => {
  it("puts a page first when it is opened from the tree, once per page", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(row("Journal"));
    await user.click(row("Projects"));
    await user.click(row("Reading list"));
    await user.click(row("Journal"));
    await waitFor(async () => {
      expect(await storedTitles(memory)).toEqual(["Journal", "Reading list", "Projects"]);
    });
    const [first] = await storedRecents(memory);
    const { page, space } = await idsOf(memory);
    expect(first?.pageId).toBe(page("Journal"));
    expect(first?.spaceId).toBe(space("Personal"));
    expect(typeof first?.openedAt).toBe("number");
  });

  it("writes the setting once for one opening", async () => {
    const { user, memory } = await renderPages(SPACES);
    const writes = recentWrites(memory);
    await user.click(row("Journal"));
    expect(writes.values()).toHaveLength(1);
    await user.click(row("Projects"));
    expect(writes.values()).toHaveLength(2);
  });

  it("does not write again for the page that is already first and was opened a moment ago", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(row("Journal"));
    const writes = recentWrites(memory);
    await user.click(row("Journal"));
    await user.click(row("Journal"));
    expect(writes.values()).toHaveLength(0);
  });

  it("records the time of the opening", async () => {
    vi.spyOn(Date, "now").mockReturnValue(NOW);
    const { user, memory } = await renderPages(SPACES);
    await user.click(row("Journal"));
    await waitFor(async () => {
      expect((await storedRecents(memory)).map((item) => item.openedAt)).toEqual([NOW]);
    });
  });

  it("records a page that is just created", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(screen.getByRole("button", { name: t("tree.newPage") }));
    await screen.findByRole("textbox", { name: t("tree.renameLabel") });
    await waitFor(async () => {
      expect(await storedRecents(memory)).toHaveLength(1);
    });
    await user.keyboard("Fresh{Enter}");
    await waitFor(async () => {
      expect(await storedTitles(memory)).toEqual(["Fresh"]);
    });
  });

  it("records a page opened from a search result, with its space", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(screen.getByRole("button", { name: t("search.open") }));
    await user.type(await screen.findByRole("combobox", { name: t("search.label") }), "Meetings");
    await user.click(await screen.findByText("Meetings", { selector: "mark" }));
    const { page, space } = await idsOf(memory);
    await waitFor(async () => {
      expect(await storedRecents(memory)).toEqual([{ pageId: page("Meetings"), spaceId: space("Work"), openedAt: expect.any(Number) as number }]);
    });
  });

  it("keeps at most 30 pages, the oldest dropping out", async () => {
    const { user, memory } = await renderPages(SPACES);
    const { page, space } = await idsOf(memory);
    const client = new CoreClient(memory);
    const older = await Promise.all(
      Array.from({ length: 30 }, (_, index) => client.pageCreate({ spaceId: space("Personal"), title: `Old ${String(index)}` })),
    );
    // Old 0 is the most recent, Old 29 the oldest.
    await seedRecents(
      memory,
      older.map((item, index) => ({ pageId: item.id, spaceId: item.spaceId, openedAt: NOW - index * MINUTE })),
    );
    await user.click(row("Journal"));
    await waitFor(async () => {
      expect((await storedRecents(memory))[0]?.pageId).toBe(page("Journal"));
    });
    const stored = await storedRecents(memory);
    expect(stored).toHaveLength(30);
    expect(stored[1]?.pageId).toBe(older[0]?.id);
    expect(stored.at(-1)?.pageId).toBe(older[28]?.id);
    expect(stored.map((item) => item.pageId)).not.toContain(older[29]?.id);
  });

  it("drops a page the core says is gone with the next write", async () => {
    const { user, memory } = await renderPages(SPACES);
    const { page, space } = await idsOf(memory);
    await seedRecents(memory, [
      { pageId: "00000000-0000-4000-8000-00000000dead", spaceId: space("Personal"), openedAt: 5 },
      { pageId: page("Projects"), spaceId: space("Personal"), openedAt: 4 },
    ]);
    // The list is read when it is shown: the search dialog asks the core about each page.
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const dialog = await screen.findByRole("dialog", { name: t("search.label") });
    await within(dialog).findByRole("option", { name: "Projects" });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20));
    });
    await user.keyboard("{Escape}");
    await user.click(row("Journal"));
    await waitFor(async () => {
      expect((await storedRecents(memory)).map((item) => item.pageId)).toEqual([page("Journal"), page("Projects")]);
    });
  });
});

describe("the search dialog while nothing is typed", () => {
  async function renderWithRecents() {
    const app = await renderPages(SPACES);
    const ids = await idsOf(app.memory);
    await seedRecents(app.memory, [
      { pageId: ids.page("Meetings"), spaceId: ids.space("Work"), openedAt: NOW - MINUTE },
      { pageId: ids.page("Journal"), spaceId: ids.space("Personal"), openedAt: NOW - 2 * MINUTE },
      { pageId: ids.page("Roadmap"), spaceId: ids.space("Personal"), openedAt: NOW - 3 * MINUTE },
    ]);
    return { ...app, ids };
  }

  const dialog = (): HTMLElement => screen.getByRole("dialog", { name: t("search.label") });

  it("lists the recent pages, naming the space of the ones that are not in the current space", async () => {
    await renderWithRecents();
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const group = await within(dialog()).findByRole("group", { name: t("recent.title") });
    await waitFor(() => {
      expect(within(group).getAllByRole("option")).toHaveLength(3);
    });
    const names = within(group)
      .getAllByRole("option")
      .map((option) => option.getAttribute("aria-label"));
    expect(names).toEqual([t("recent.inSpace", { title: "Meetings", space: "Work" }), "Journal", "Roadmap"]);
    expect(within(group).getByText("Work")).toBeInTheDocument();
    // Personal is the current space: its name is not repeated.
    expect(within(group).queryByText("Personal")).not.toBeInTheDocument();
    expect(within(dialog()).getByRole("combobox")).toHaveAttribute("aria-expanded", "true");
  });

  it("shows at most 10", async () => {
    const { memory, ids } = await renderWithRecents();
    const pages = ["Meetings", "Journal", "Roadmap", "Projects", "Ideas", "Q1", "Reading list"].map((title) => ({
      pageId: ids.page(title),
      spaceId: ids.space(title === "Meetings" ? "Work" : "Personal"),
    }));
    const extra = await Promise.all(
      Array.from({ length: 6 }, (_, index) =>
        new CoreClient(memory).pageCreate({ spaceId: ids.space("Personal"), title: `Extra ${String(index)}` }),
      ),
    );
    await seedRecents(
      memory,
      [...pages, ...extra.map((page) => ({ pageId: page.id, spaceId: page.spaceId }))].map((item, index) => ({
        ...item,
        openedAt: NOW - index * MINUTE,
      })),
    );
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await waitFor(() => {
      expect(within(dialog()).getAllByRole("option")).toHaveLength(10);
    });
  });

  it("moves with the arrow keys and opens the chosen page with Enter, in its space", async () => {
    const { user } = await renderWithRecents();
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const input = within(dialog()).getByRole("combobox");
    await within(dialog()).findAllByRole("option");
    await waitFor(() => {
      expect(within(dialog()).getAllByRole("option")).toHaveLength(3);
    });
    const options = within(dialog()).getAllByRole("option");
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", options[0]?.id);
    await user.keyboard("{ArrowDown}");
    expect(options[1]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(options[2]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowUp}{Enter}");
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    expect(row("Journal")).toHaveAttribute("aria-selected", "true");
  });

  it("opens a page of another space with a click", async () => {
    const { user } = await renderWithRecents();
    await user.click(screen.getByRole("button", { name: t("search.open") }));
    await user.click(await within(dialog()).findByRole("option", { name: new RegExp("Meetings") }));
    expect(screen.queryByRole("dialog", { name: t("search.label") })).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { level: 1, name: "Meetings" })).toBeInTheDocument();
    expect(row("Meetings")).toHaveAttribute("aria-selected", "true");
  });

  it("gives way to the results once something is typed, and comes back when the field is empty again", async () => {
    const { user } = await renderWithRecents();
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await within(dialog()).findByRole("group", { name: t("recent.title") });
    const input = within(dialog()).getByRole("combobox");
    await user.type(input, "journ");
    expect(within(dialog()).queryByRole("group", { name: t("recent.title") })).not.toBeInTheDocument();
    await user.clear(input);
    expect(await within(dialog()).findByRole("group", { name: t("recent.title") })).toBeInTheDocument();
  });

  it("is not shown when there are no recent pages, and the hint stays", async () => {
    await renderPages(SPACES);
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    expect(within(dialog()).queryByRole("group", { name: t("recent.title") })).not.toBeInTheDocument();
    expect(within(dialog()).getByText(t("search.hint"))).toBeInTheDocument();
  });

  it("leaves out a page in the trash, and lists it again once it is restored", async () => {
    const { memory, ids } = await renderWithRecents();
    const client = new CoreClient(memory);
    await act(async () => {
      await client.pageTrash(ids.page("Journal"));
    });
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    const names = (): (string | null)[] =>
      within(dialog())
        .queryAllByRole("option")
        .map((option) => option.getAttribute("aria-label"));
    await waitFor(() => {
      expect(names()).toEqual([t("recent.inSpace", { title: "Meetings", space: "Work" }), "Roadmap"]);
    });
    await act(async () => {
      await client.pageRestore(ids.page("Journal"));
    });
    await waitFor(() => {
      expect(names()).toHaveLength(3);
    });
  });

  it("follows the title of a page that is renamed", async () => {
    const { memory, ids } = await renderWithRecents();
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    await waitFor(() => {
      expect(within(dialog()).getAllByRole("option")).toHaveLength(3);
    });
    await act(async () => {
      await new CoreClient(memory).pageRename(ids.page("Journal"), "Diary");
    });
    expect(await within(dialog()).findByRole("option", { name: "Diary" })).toBeInTheDocument();
    expect(within(dialog()).queryByRole("option", { name: "Journal" })).not.toBeInTheDocument();
  });
});

describe("the Home screen", () => {
  it("lists the pages visited with their icon, title, space and how long ago", async () => {
    vi.spyOn(Date, "now").mockReturnValue(NOW);
    const { memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    await seedRecents(memory, [
      { pageId: ids.page("Projects"), spaceId: ids.space("Personal"), openedAt: NOW - 5 * MINUTE },
      { pageId: ids.page("Meetings"), spaceId: ids.space("Work"), openedAt: NOW - 3 * 60 * MINUTE },
    ]);
    const list = await screen.findByRole("list", { name: t("recent.visitedList") });
    expect(screen.getByRole("heading", { level: 2, name: t("recent.visited") })).toBeInTheDocument();
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("Projects");
    expect(items[0]).toHaveTextContent("Personal");
    expect(items[0]).toHaveTextContent(formatAgo(NOW - 5 * MINUTE, NOW) ?? "");
    expect(items[1]).toHaveTextContent("Meetings");
    expect(items[1]).toHaveTextContent("Work");
    expect(items[1]?.querySelector(".page-icon")).not.toBeNull();
    expect(items[1]?.querySelector("time")).toHaveAttribute("datetime", new Date(NOW - 3 * 60 * MINUTE).toISOString());
  });

  it("shows nothing while no page was visited", async () => {
    await renderPages(SPACES);
    expect(screen.queryByRole("heading", { name: t("recent.visited") })).not.toBeInTheDocument();
  });

  it("shows at most 12", async () => {
    const { memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    const client = new CoreClient(memory);
    const pages = await Promise.all(
      Array.from({ length: 14 }, (_, index) => client.pageCreate({ spaceId: ids.space("Personal"), title: `Many ${String(index)}` })),
    );
    await seedRecents(
      memory,
      pages.map((page, index) => ({ pageId: page.id, spaceId: page.spaceId, openedAt: NOW - index * MINUTE })),
    );
    const list = await screen.findByRole("list", { name: t("recent.visitedList") });
    await waitFor(() => {
      expect(within(list).getAllByRole("listitem")).toHaveLength(12);
    });
  });

  it("opens a page, in its space, with a click, and then shows the page instead of the list", async () => {
    const { user, memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    await seedRecents(memory, [{ pageId: ids.page("Meetings"), spaceId: ids.space("Work"), openedAt: NOW }]);
    const list = await screen.findByRole("list", { name: t("recent.visitedList") });
    await user.click(within(list).getByRole("button", { name: new RegExp("Meetings") }));
    expect(await screen.findByRole("heading", { level: 1, name: "Meetings" })).toBeInTheDocument();
    expect(row("Meetings")).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("heading", { name: t("recent.visited") })).not.toBeInTheDocument();
  });

  it("follows a rename and leaves out a page in the trash", async () => {
    const { user, memory } = await renderPages(SPACES);
    const ids = await idsOf(memory);
    const client = new CoreClient(memory);
    await seedRecents(memory, [
      { pageId: ids.page("Journal"), spaceId: ids.space("Personal"), openedAt: NOW },
      { pageId: ids.page("Ideas"), spaceId: ids.space("Personal"), openedAt: NOW - MINUTE },
    ]);
    const list = await screen.findByRole("list", { name: t("recent.visitedList") });
    await waitFor(() => {
      expect(within(list).getAllByRole("listitem")).toHaveLength(2);
    });
    await act(async () => {
      await client.pageRename(ids.page("Journal"), "Diary");
    });
    expect(await within(list).findByRole("button", { name: new RegExp("Diary") })).toBeInTheDocument();
    await act(async () => {
      await client.pageTrash(ids.page("Ideas"));
    });
    await waitFor(() => {
      expect(within(list).getAllByRole("listitem")).toHaveLength(1);
    });
    expect(within(list).queryByText("Ideas")).not.toBeInTheDocument();
    void user;
  });
});

describe("Settings: clear recent pages", () => {
  it("empties the list, and the Home screen and the search dialog with it", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(row("Journal"));
    await waitFor(async () => {
      expect(await storedRecents(memory)).toHaveLength(1);
    });
    await openScreen(user, "nav.settings");
    const clear = await screen.findByRole("button", { name: t("recent.settings.clear") });
    expect(clear).toBeEnabled();
    await user.click(clear);
    expect(await screen.findByText(t("recent.settings.cleared"))).toBeInTheDocument();
    expect(await storedRecents(memory)).toEqual([]);
    expect(clear).toBeDisabled();
    await openScreen(user, "nav.home");
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    expect(screen.queryByRole("group", { name: t("recent.title") })).not.toBeInTheDocument();
  });

  it("is disabled while the list is empty", async () => {
    const { user } = await renderPages(SPACES);
    await openScreen(user, "nav.settings");
    expect(await screen.findByRole("button", { name: t("recent.settings.clear") })).toBeDisabled();
  });

  it("says when the list could not be cleared", async () => {
    const { user, memory } = await renderPages(SPACES);
    await user.click(row("Journal"));
    await openScreen(user, "nav.settings");
    const original = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "SettingsSet" && request.data.key === KEY
        ? Promise.reject(new Error("the database is locked"))
        : original(request),
    );
    await user.click(await screen.findByRole("button", { name: t("recent.settings.clear") }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the database is locked");
  });
});
