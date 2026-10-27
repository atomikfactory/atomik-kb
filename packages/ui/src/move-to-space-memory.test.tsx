// "Move to…" over the in-memory core (ADR-0038), which mirrors the move: ids kept, the subtree re-parented into the
// target's tree, events as the core emits them. What the dialog asks and shows is in `move-to-space.test.tsx` (canned
// answers); here the whole flow runs: the plan the dialog shows, the move, the sidebar of both spaces, the tabs, the page open.

import { CoreClient } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { renderInterop } from "./test/interop";
import { idsOf } from "./test/page";
import type { User } from "./test/render";
import { PERSONAL, WORK, outline, row } from "./test/sidebar";

async function setup() {
  const app = await renderInterop({ spaces: [PERSONAL, WORK] });
  const ids = await idsOf(app.memory);
  return { ...app, ids };
}

async function moveFromRow(user: User, title: string, space: string, parent?: string): Promise<HTMLElement> {
  await user.pointer({ keys: "[MouseRight]", target: row(title) });
  const menu = await screen.findByRole("menu", { name: t("tree.actions") });
  await user.click(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") }));
  const dialog = await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: title }) });
  await user.click(within(dialog).getByRole("button", { name: new RegExp(`^${space}`) }));
  await within(dialog).findByRole("radio", { name: t("moveToSpace.topLevel") });
  if (parent !== undefined) {
    await user.click(within(dialog).getByRole("radio", { name: parent }));
  }
  await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.next") }));
  await within(dialog).findByText(/^Move \d+ pages? to/);
  return dialog;
}

describe("moving a page with the in-memory core", () => {
  it("moves a page to the top level of another space, and the sidebar follows it there", async () => {
    const { user, store, ids } = await setup();
    const dialog = await moveFromRow(user, "Journal", "Work");
    expect(within(dialog).getByText(tp("moveToSpace.confirm", 1, { space: "Work" }))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    await waitFor(() => {
      expect(outline()).toEqual(["Meetings", "Journal"]);
    });
    expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    expect(row("Journal")).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    // The page left the first space.
    await user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    await user.click(await screen.findByRole("button", { name: "Personal" }));
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Reading list"]);
    });
  });

  it("moves a page with its sub-pages under a page of the other space, and says how many move", async () => {
    const { user, store, ids } = await setup();
    const dialog = await moveFromRow(user, "Projects", "Work", "Meetings");
    // Projects, Roadmap, Q1 and Ideas.
    expect(within(dialog).getByText(tp("moveToSpace.confirm", 4, { space: "Work" }))).toBeInTheDocument();
    expect(within(dialog).getByText(t("moveToSpace.where.under", { parent: "Meetings" }))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));

    await waitFor(() => {
      expect(outline()).toEqual(["Meetings", "  Projects", "    Roadmap", "      Q1", "    Ideas"]);
    });
    expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    expect(store.getState().selectedPageId).toBe(ids.page("Projects"));
    // The parent was expanded, so the page shows where it went.
    expect(row("Meetings")).toHaveAttribute("aria-expanded", "true");
  });

  it("moves the open page from its page menu, and the page stays open", async () => {
    const { user, store, ids } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    await user.click(await screen.findByRole("menuitem", { name: t("moveToSpace.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: "Journal" }) });
    await user.click(within(dialog).getByRole("button", { name: /^Work/ }));
    await user.click(await within(dialog).findByRole("radio", { name: "Meetings" }));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.next") }));
    await within(dialog).findByText(tp("moveToSpace.confirm", 1, { space: "Work" }));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));

    await waitFor(() => {
      expect(outline()).toEqual(["Meetings", "  Journal"]);
    });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(store.getState().tabs).toHaveLength(1);
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Work");
  });

  it("keeps the other tabs open on pages that moved, and the recent pages", async () => {
    const { user, store, ids, memory } = await setup();
    await user.click(row("Reading list"));
    await screen.findByRole("heading", { level: 1, name: "Reading list" });
    // Another tab, in front, on Journal.
    store.getState().navigateTo({ pageId: ids.page("Journal"), spaceId: ids.space("Personal"), disposition: "foreground" });
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    expect(store.getState().tabs).toHaveLength(2);

    // Reading list moves while another tab has the front.
    await moveFromRow(user, "Reading list", "Work").then((dialog) =>
      user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") })),
    );
    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    });
    // The page opens in its new place, in the tab that already had it.
    expect(store.getState().tabs).toHaveLength(2);
    expect(store.getState().selectedPageId).toBe(ids.page("Reading list"));
    const spaces = store.getState().tabs.map((tab) => [tab.entries.at(-1)?.pageId, tab.entries.at(-1)?.spaceId]);
    expect(spaces).toEqual([
      [ids.page("Reading list"), ids.space("Work")],
      [ids.page("Journal"), ids.space("Personal")],
    ]);
    // The recent pages name the space the page is in now, and so does what is stored.
    await waitFor(() => {
      const recent = store.getState().settings["ui.recent_pages"];
      expect(recent).toEqual(
        expect.arrayContaining([expect.objectContaining({ pageId: ids.page("Reading list"), spaceId: ids.space("Work") })]),
      );
    });
    await waitFor(async () => {
      const stored = await new CoreClient(memory).settingsGet("ui.tabs");
      expect(JSON.stringify(stored)).toContain(`"spaceId":"${ids.space("Work")}","pageId":"${ids.page("Reading list")}"`);
    });
  });

  it("follows a move made elsewhere: the open page's tab goes with it", async () => {
    const { user, store, ids, memory } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });

    // Another window (or device) moves the page.
    await new CoreClient(memory).pageMoveToSpace(ids.page("Journal"), ids.space("Work"), null, null);

    await waitFor(() => {
      expect(store.getState().currentSpaceId).toBe(ids.space("Work"));
    });
    await waitFor(() => {
      expect(outline()).toEqual(["Meetings", "Journal"]);
    });
    expect(store.getState().selectedPageId).toBe(ids.page("Journal"));
    expect(store.getState().tabs).toHaveLength(1);
    expect(await screen.findByRole("heading", { level: 1, name: "Journal" })).toBeInTheDocument();
  });

  it("follows a move made elsewhere of a page that is not open: the list of spaces and the open tree are read again", async () => {
    const { user, ids, memory } = await setup();
    await user.click(row("Journal"));
    await screen.findByRole("heading", { level: 1, name: "Journal" });
    await new CoreClient(memory).pageMoveToSpace(ids.page("Reading list"), ids.space("Work"), null, null);
    await waitFor(() => {
      expect(outline()).toEqual(["Projects", "  Roadmap", "    Q1", "  Ideas", "Journal"]);
    });
    await user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    const list = await screen.findByRole("list", { name: t("spaces.listLabel") });
    expect(within(list).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["🏠Personal", "Work"]);
    // The Work page count is not shown in the list, but the move dialog shows it.
    await user.keyboard("{Escape}");
    await user.pointer({ keys: "[MouseRight]", target: row("Journal") });
    await user.click(await screen.findByRole("menuitem", { name: t("moveToSpace.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("moveToSpace.title", { name: "Journal" }) });
    expect(within(dialog).getByRole("button", { name: /^Work/ })).toHaveTextContent(tp("spaces.pageCount", 2));
  });

  it("refuses a page that is in the trash by the time the move is asked, and says why in the dialog", async () => {
    const { user, ids, memory } = await setup();
    const dialog = await moveFromRow(user, "Journal", "Work");
    await new CoreClient(memory).pageTrash(ids.page("Journal"));
    await user.click(within(dialog).getByRole("button", { name: t("moveToSpace.move") }));
    expect(await within(dialog).findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBe(dialog);
  });
});
