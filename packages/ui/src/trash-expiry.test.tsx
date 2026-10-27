// Trash expiry (ADR-0040 item 3) in the UI: the Trash says per page when it will be deleted, Settings ->
// Data -> Trash holds the number of days (`app.trash.retention_days`), and an open Trash follows the
// pages the core expires.

import { TRASH_RETENTION_SETTING } from "@atomik-kb/core-client";
import type { MemoryTransport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { formatTrashExpiry } from "./format";
import { t, tp } from "./i18n";
import { parseTrashDays } from "./screens/TrashSettings";
import { NOW, openScreen } from "./test/render";
import type { User } from "./test/render";
import { outline, renderSidebar, row } from "./test/sidebar";

const DAY = 24 * 60 * 60 * 1000;

const toggle = (count: number): HTMLElement => screen.getByRole("button", { name: t("trash.title", { count }) });

async function trash(user: User, title: string): Promise<void> {
  act(() => {
    row(title).focus();
  });
  await user.keyboard("{Delete}");
  await waitFor(() => {
    expect(outline()).not.toContain(title);
  });
}

/** The entries of the open Trash, newest first, as `[title, text of the whole entry]`. */
function entries(): [string, string][] {
  return within(screen.getByRole("list", { name: t("trash.listLabel") }))
    .getAllByRole("listitem")
    .map((item) => [item.querySelector(".trash-title")?.textContent ?? "", item.textContent]);
}

/** A moment in 2026, on the local clock. */
const local = (month: number, day: number, hour = 12): number => new Date(2026, month - 1, day, hour).getTime();

describe("the Trash says when a page is deleted", () => {
  beforeEach(() => {
    // Only `Date`: the UI reads the time from it, and everything else keeps running as it does.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(local(10, 3));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  /** The Trash is opened again so that it reads the time again. */
  async function reopen(user: User, count: number): Promise<void> {
    await user.click(toggle(count));
    await user.click(toggle(count));
  }

  it("says 30 days for a page that has just been trashed and counts down in calendar days", async () => {
    const { user } = await renderSidebar({ now: () => Date.now() });
    await trash(user, "Journal");
    await user.click(toggle(1));
    expect(entries()).toEqual([["Journal", expect.stringContaining(t("trash.expires.other", { count: 30 }))]]);

    // The 30 days end on 2 November.
    await user.click(toggle(1));
    for (const [date, text] of [
      [local(10, 14), t("trash.expires.other", { count: 19 })],
      [local(10, 31), t("trash.expires.other", { count: 2 })],
      [local(11, 1), t("trash.expires.tomorrow")],
      [local(11, 2), t("trash.expires.today")],
      // Overdue (the next check has not run yet): still today.
      [local(11, 9), t("trash.expires.today")],
    ] as const) {
      vi.setSystemTime(date);
      await user.click(toggle(1));
      expect(entries()[0]?.[1], text).toContain(text);
      await user.click(toggle(1));
    }
  });

  it("gives every page its own day", async () => {
    const { user } = await renderSidebar({ now: () => Date.now() });
    await trash(user, "Journal");
    vi.setSystemTime(local(10, 13));
    await trash(user, "Reading list");
    await user.click(toggle(2));
    expect(entries()).toEqual([
      ["Reading list", expect.stringContaining(t("trash.expires.other", { count: 30 }))],
      ["Journal", expect.stringContaining(t("trash.expires.other", { count: 20 }))],
    ]);
  });

  it("follows the device setting", async () => {
    const { user } = await renderSidebar({ now: () => Date.now(), settings: { [TRASH_RETENTION_SETTING]: 7 } });
    await trash(user, "Journal");
    await user.click(toggle(1));
    expect(entries()[0]?.[1]).toContain(t("trash.expires.other", { count: 7 }));
    vi.setSystemTime(local(10, 9));
    await reopen(user, 1);
    expect(entries()[0]?.[1]).toContain(t("trash.expires.tomorrow"));
  });

  it("says nothing when the setting keeps trashed pages", async () => {
    const { user } = await renderSidebar({ now: () => Date.now(), settings: { [TRASH_RETENTION_SETTING]: 0 } });
    await trash(user, "Journal");
    await user.click(toggle(1));
    expect(entries()).toEqual([["Journal", "JournalRestoreDelete permanently"]]);
    expect(screen.queryByText(/^Deleted (today|tomorrow|in )/)).not.toBeInTheDocument();
  });

  it("words the days and says nothing without a limit", () => {
    const trashedAt = local(10, 3);
    expect(formatTrashExpiry(trashedAt, 30 * DAY, trashedAt)).toBe("Deleted in 30 days");
    expect(formatTrashExpiry(trashedAt, 30 * DAY, local(11, 1))).toBe("Deleted tomorrow");
    expect(formatTrashExpiry(trashedAt, 30 * DAY, local(11, 2))).toBe("Deleted today");
    expect(formatTrashExpiry(BigInt(trashedAt), 2 * DAY, trashedAt)).toBe("Deleted in 2 days");
    expect(formatTrashExpiry(trashedAt, null, trashedAt)).toBeNull();
  });
});

describe("the Trash follows the expiry of the core", () => {
  it("drops the pages the core expires, and announces nothing else", async () => {
    let clock = NOW;
    const { user, memory } = await renderSidebar({ now: () => clock });
    await trash(user, "Journal");
    clock = NOW + 20 * DAY;
    await trash(user, "Reading list");
    await user.click(toggle(2));
    expect(entries().map(([title]) => title)).toEqual(["Reading list", "Journal"]);

    clock = NOW + 31 * DAY;
    await act(async () => {
      expect(await memory.trashExpire()).toBe(1);
    });
    await waitFor(() => {
      expect(entries().map(([title]) => title)).toEqual(["Reading list"]);
    });
    expect(toggle(1)).toHaveAttribute("aria-expanded", "true");
    expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas"]);
  });
});

describe("Settings -> Data -> Trash", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(local(10, 3));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  async function openSettings(options: Parameters<typeof renderSidebar>[0] = {}) {
    const app = await renderSidebar({ now: () => Date.now(), ...options });
    await openScreen(app.user, "nav.settings");
    await screen.findByRole("heading", { name: t("settings.title") });
    return app;
  }

  const panel = (): HTMLElement => screen.getByRole("region", { name: t("settings.trash.title") });
  const field = (): HTMLInputElement =>
    within(panel()).getByRole("spinbutton", { name: t("settings.trash.retention") });

  async function stored(memory: MemoryTransport): Promise<unknown> {
    const response = await memory.dispatch({ type: "SettingsGet", data: { key: TRASH_RETENTION_SETTING } });
    return response.type === "Setting" ? response.data : response;
  }

  async function ready(): Promise<void> {
    await waitFor(() => {
      expect(field()).toBeEnabled();
    });
  }

  async function type(user: User, text: string): Promise<void> {
    await user.clear(field());
    if (text !== "") {
      await user.type(field(), text);
    }
  }

  it("shows 30 days by default, and a stored value", async () => {
    await openSettings();
    await ready();
    expect(field()).toHaveValue(30);
    expect(field()).toHaveAccessibleDescription(
      `${tp("settings.trash.hint", 30)} ${t("settings.trash.hint.sync")}`,
    );
  });

  it("shows the stored number of days", async () => {
    await openSettings({ settings: { [TRASH_RETENTION_SETTING]: 14 } });
    await waitFor(() => {
      expect(field()).toHaveValue(14);
    });
  });

  it("saves the number when the field loses focus, not on every key, and the Trash follows", async () => {
    const { user, memory } = await openSettings();
    await ready();
    const dispatch = vi.spyOn(memory, "dispatch");
    await type(user, "14");
    expect(dispatch.mock.calls.filter(([request]) => request.type === "SettingsSet")).toEqual([]);
    await user.tab();
    await waitFor(async () => {
      expect(await stored(memory)).toBe(14);
    });
    expect(field()).toHaveValue(14);
    expect(field()).toHaveAccessibleDescription(
      `${tp("settings.trash.hint", 14)} ${t("settings.trash.hint.sync")}`,
    );

    // A page trashed now says 14 days (the sidebar is beside Settings).
    await trash(user, "Journal");
    await user.click(toggle(1));
    expect(entries()[0]?.[1]).toContain(t("trash.expires.other", { count: 14 }));
  });

  it("saves on Enter, and Escape gives the stored number back", async () => {
    const { user, memory } = await openSettings();
    await ready();
    await type(user, "7");
    await user.keyboard("{Enter}");
    await waitFor(async () => {
      expect(await stored(memory)).toBe(7);
    });

    await type(user, "99");
    await user.keyboard("{Escape}");
    expect(field()).toHaveValue(7);
    await user.tab();
    expect(await stored(memory)).toBe(7);
  });

  it("says 1 day in the singular and that 0 keeps trashed pages", async () => {
    const { user, memory } = await openSettings();
    await ready();
    await type(user, "1");
    await user.tab();
    await waitFor(() => {
      expect(field()).toHaveAccessibleDescription(`${tp("settings.trash.hint", 1)} ${t("settings.trash.hint.sync")}`);
    });

    await type(user, "0");
    await user.tab();
    await waitFor(() => {
      expect(field()).toHaveAccessibleDescription(`${t("settings.trash.hint.never")} ${t("settings.trash.hint.sync")}`);
    });
    expect(await stored(memory)).toBe(0);
  });

  it("refuses what is not a whole number of days and keeps the stored one", async () => {
    const { user, memory } = await openSettings({ settings: { [TRASH_RETENTION_SETTING]: 21 } });
    await waitFor(() => {
      expect(field()).toHaveValue(21);
    });
    for (const bad of ["", "1.5", "-2"]) {
      await type(user, bad);
      await user.tab();
      expect(await within(panel()).findByRole("alert")).toHaveTextContent(t("settings.trash.invalid"));
      expect(field()).toHaveValue(21);
      expect(await stored(memory)).toBe(21);
      // A good number clears the message.
      await type(user, "21");
      await user.tab();
      expect(within(panel()).queryByRole("alert")).not.toBeInTheDocument();
    }
  });

  it("says why the setting could not be saved", async () => {
    const { user, memory } = await openSettings();
    await ready();
    await type(user, "5");
    memory.failNext({ code: "Storage", message: "database error: disk I/O error" });
    await user.tab();
    expect(await within(panel()).findByRole("alert")).toHaveTextContent(
      t("settings.trash.error", { message: "database error: disk I/O error" }),
    );
    expect(await stored(memory)).toBeNull();
  });

  it("parses what is typed", () => {
    expect([parseTrashDays("0"), parseTrashDays(" 30 "), parseTrashDays("007")]).toEqual([0, 30, 7]);
    expect([parseTrashDays(""), parseTrashDays("1.5"), parseTrashDays("-1"), parseTrashDays("1e3"), parseTrashDays("abc")]).toEqual([
      null,
      null,
      null,
      null,
      null,
    ]);
    expect(parseTrashDays("9".repeat(30))).toBeNull();
  });
});
