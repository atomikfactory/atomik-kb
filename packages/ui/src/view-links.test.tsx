// View-only links in the page menu (Milestone 13, slice 4; ADR-0051) over the in-memory core: "Share a view link..." says what the
// link shows and who can open it, makes one with an optional expiry and shows its URL once with Copy, lists the links of the page
// with when they were made, by whom and when they end, and revokes one after a question; the entry follows the role (an Owner, an
// Editor when the space allows it, anyone in a personal space; not a Commenter, a Viewer or a read-only copy).

import { LINK_EXPIRY_MAX_DAYS } from "@atomik-kb/core-client";
import type { MemoryTransportOptions } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import { formatMoment } from "./sharing";
import { NOW } from "./test/render";
import type { User } from "./test/render";
import { clickPage, openMembers, openSpace, renderSharing } from "./test/sharing";
import type { SharingRender } from "./test/sharing";

afterEach(() => {
  vi.restoreAllMocks();
});

const DAY = 86_400_000;

/** A link of Bob's, made a day ago, on the page "Plan". */
const BOB_LINK: MemoryTransportOptions = {
  links: { publishMs: 30, links: [{ space: "Work", page: "Plan", createdBy: "Bob", ageMs: DAY }] },
};

/** The app with the Plan page of Work open. */
async function openPlan(options: SharingRender = {}) {
  const app = await renderSharing({ memory: { links: { publishMs: 30 } }, ...options });
  await openSpace(app.user, "Work");
  await clickPage(app.user, "Plan");
  return app;
}

/** Opens the page menu and answers it. */
async function pageMenu(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("page.actions") }));
  return screen.findByRole("menu", { name: t("page.actions") });
}

/** Chooses "Share a view link..." and answers the dialog. */
async function openDialog(user: User): Promise<HTMLElement> {
  const menu = await pageMenu(user);
  await user.click(within(menu).getByRole("menuitem", { name: t("viewLink.menu") }));
  return screen.findByRole("dialog", { name: t("viewLink.title") });
}

/** The rows of the list of the links of the page, once there are `count` of them (the list is read and refreshed after the dialog opens). */
async function rowsOf(dialog: HTMLElement, count: number): Promise<HTMLElement[]> {
  let rows: HTMLElement[] = [];
  await waitFor(() => {
    const list = within(dialog).getByRole("list", { name: t("viewLink.list.title") });
    rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(count);
  });
  return rows;
}

/** The one link of the page. */
async function onlyRow(dialog: HTMLElement): Promise<HTMLElement> {
  const [row] = await rowsOf(dialog, 1);
  if (row === undefined) {
    throw new Error("no row");
  }
  return row;
}

describe("the dialog", () => {
  it("says what the link shows and who can open it, and lists no links yet", async () => {
    const { user } = await openPlan();
    const dialog = await openDialog(user);
    expect(dialog).toHaveAccessibleDescription(t("viewLink.intro"));
    for (const key of [
      "viewLink.shows.page",
      "viewLink.shows.text",
      "viewLink.shows.comments",
      "viewLink.who.anyone",
      "viewLink.who.server",
      "viewLink.fresh",
    ] as const) {
      expect(within(dialog).getByText(t(key))).toBeInTheDocument();
    }
    // The expiry is optional: it starts on "Never" and offers days up to the longest the core allows.
    const expiry = within(dialog).getByRole("combobox", { name: t("viewLink.expiry") });
    expect(expiry).toHaveValue("");
    const options = within(expiry).getAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual([
      t("viewLink.expiry.never"),
      tp("viewLink.expiry.days", 1),
      tp("viewLink.expiry.days", 7),
      tp("viewLink.expiry.days", 30),
      tp("viewLink.expiry.days", 90),
      tp("viewLink.expiry.days", LINK_EXPIRY_MAX_DAYS),
    ]);
    expect(await within(dialog).findByText(t("viewLink.list.empty"))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog", { name: t("viewLink.title") })).not.toBeInTheDocument();
  });

  it("makes a link without an expiry and shows its URL once, with Copy", async () => {
    const { user, client } = await openPlan();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));

    expect(await within(dialog).findByText(t("viewLink.once"))).toBeInTheDocument();
    const box = within(dialog).getByRole<HTMLInputElement>("textbox", { name: t("viewLink.linkLabel") });
    expect(box.value).toMatch(/^https:\/\/sync\.example\.com\/view#[A-Za-z0-9_-]{43}$/u);
    expect(box).toHaveAttribute("readonly");
    const key = box.value.split("#")[1] ?? "";
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteLink.copy") }));
    expect(writeText).toHaveBeenCalledWith(box.value);
    expect(await within(dialog).findByRole("button", { name: t("sharing.inviteLink.copied") })).toBeInTheDocument();
    // The form is gone: one link per question, and the key is nowhere else on the page.
    expect(within(dialog).queryByRole("combobox", { name: t("viewLink.expiry") })).not.toBeInTheDocument();
    expect(await client.viewLinkList((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).toHaveLength(1);

    // The link is listed: when it was made, by whom, that it never expires, and that it is publishing, then live.
    const row = await onlyRow(dialog);
    expect(row).toHaveTextContent(t("viewLink.row.created", { when: formatMoment(NOW), name: "Maya" }));
    expect(row).toHaveTextContent(t("viewLink.row.never"));
    expect(row).not.toHaveTextContent(key);
    expect(within(dialog).queryByDisplayValue(box.value)).toBe(box);
    expect(await within(row).findByText(t("viewLink.status.live"))).toBeInTheDocument();
    expect(within(dialog).queryByText(t("viewLink.publishing.note"))).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(key);
  });

  it("cannot show the URL again: a dialog opened later lists the link without it", async () => {
    const { user } = await openPlan();
    let dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    const box = await within(dialog).findByRole<HTMLInputElement>("textbox", { name: t("viewLink.linkLabel") });
    const url = box.value;
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(document.body.textContent).not.toContain(url.split("#")[1]);

    dialog = await openDialog(user);
    expect(within(dialog).queryByText(t("viewLink.once"))).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox", { name: t("viewLink.linkLabel") })).not.toBeInTheDocument();
    expect(await onlyRow(dialog)).toHaveTextContent(t("viewLink.row.created", { when: formatMoment(NOW), name: "Maya" }));
    expect(document.body.textContent).not.toContain(url.split("#")[1]);
    // The form is back, for another link.
    expect(within(dialog).getByRole("button", { name: t("viewLink.create") })).toBeInTheDocument();
  });

  it("makes a link that expires after the days chosen, and says when", async () => {
    const { user } = await openPlan();
    const dialog = await openDialog(user);
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("viewLink.expiry") }), "7");
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    await within(dialog).findByText(t("viewLink.once"));
    const row = await onlyRow(dialog);
    expect(row).toHaveTextContent(t("viewLink.row.expires", { when: formatMoment(NOW + 7 * DAY) }));
  });

  it("lists the links of this page only, with the creator and the expiry, and marks an expired one", async () => {
    const { user } = await openPlan({
      memory: {
        links: {
          publishMs: 30,
          links: [
            { space: "Work", page: "Plan", createdBy: "Bob", ageMs: 2 * DAY, expiresInMs: 5 * DAY },
            { space: "Work", page: "Plan", ageMs: 3 * DAY, expiresInMs: -DAY },
            { space: "Work", page: "Notes", ageMs: DAY },
          ],
        },
      },
    });
    const dialog = await openDialog(user);
    const rows = await rowsOf(dialog, 2);
    // The links of Plan, newest first; the one of Notes is not here.
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent(t("viewLink.row.created", { when: formatMoment(NOW - 2 * DAY), name: "Bob" }));
    expect(rows[0]).toHaveTextContent(t("viewLink.row.expires", { when: formatMoment(NOW + 5 * DAY) }));
    expect(within(rows[0] as HTMLElement).getByText(t("viewLink.status.live"))).toBeInTheDocument();
    expect(rows[1]).toHaveTextContent(t("viewLink.row.created", { when: formatMoment(NOW - 3 * DAY), name: "Maya" }));
    expect(rows[1]).toHaveTextContent(t("viewLink.row.expired", { when: formatMoment(NOW - DAY) }));
    expect(within(rows[1] as HTMLElement).getByText(t("viewLink.status.expired"))).toBeInTheDocument();
  });

  it("says why a link could not be made, with a sentence of its own when sync is off", async () => {
    const { user, memory } = await openPlan();
    const dialog = await openDialog(user);
    memory.failNext({ code: "Internal", message: "boom" });
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    expect(await within(dialog).findByText(t("viewLink.error.create", { message: "boom" }))).toBeInTheDocument();
    // The form is back for another try.
    memory.failNext({ code: "InvalidInput", message: "turn sync on", details: { field: "serverUrl" } });
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    expect(await within(dialog).findByText(t("viewLink.error.noSync"))).toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox", { name: t("viewLink.linkLabel") })).not.toBeInTheDocument();
  });
});

describe("revoking", () => {
  it("asks first, keeps the link when the question is cancelled and removes it when confirmed", async () => {
    const { user, client } = await openPlan({ memory: { links: { publishMs: 30 } } });
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    await within(dialog).findByText(t("viewLink.once"));
    const spaceId = (await client.spaceList()).find((space) => space.name === "Work")?.id ?? "";
    const revoke = within(await onlyRow(dialog)).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW) }) });

    await user.click(revoke);
    const question = await screen.findByRole("alertdialog", { name: t("viewLink.revoke.confirmTitle") });
    expect(question).toHaveAccessibleDescription(t("viewLink.revoke.confirmBody"));
    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    await waitFor(() => {
      expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    });
    expect(await client.viewLinkList(spaceId)).toHaveLength(1);

    await user.click(within(await onlyRow(dialog)).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW) }) }));
    await user.click(await screen.findByRole("button", { name: t("viewLink.revoke.confirm") }));
    expect(await within(dialog).findByText(t("viewLink.list.empty"))).toBeInTheDocument();
    expect(await client.viewLinkList(spaceId)).toEqual([]);
  });

  it("lets an Owner revoke the link of another member", async () => {
    const { user, client } = await openPlan({ memory: BOB_LINK });
    const dialog = await openDialog(user);
    const row = await onlyRow(dialog);
    expect(row).toHaveTextContent(t("viewLink.row.created", { when: formatMoment(NOW - DAY), name: "Bob" }));
    await user.click(within(row).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW - DAY) }) }));
    await user.click(await screen.findByRole("button", { name: t("viewLink.revoke.confirm") }));
    expect(await within(dialog).findByText(t("viewLink.list.empty"))).toBeInTheDocument();
    expect(await client.viewLinkList((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).toEqual([]);
  });

  it("says why the core refused", async () => {
    const { user, memory } = await openPlan({ memory: BOB_LINK });
    const dialog = await openDialog(user);
    const row = await onlyRow(dialog);
    await user.click(within(row).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW - DAY) }) }));
    const confirm = await screen.findByRole("button", { name: t("viewLink.revoke.confirm") });
    memory.failNext({ code: "Forbidden", message: "this needs the owner role in the space" });
    await user.click(confirm);
    expect(await within(dialog).findByText(t("viewLink.revoke.error", { message: "this needs the owner role in the space" }))).toBeInTheDocument();
    await rowsOf(dialog, 1);
  });
});

describe("the role", () => {
  it("offers an Owner the entry, whatever the setting says", async () => {
    const { user } = await openPlan({ role: "owner" });
    const menu = await pageMenu(user);
    expect(within(menu).getByRole("menuitem", { name: t("viewLink.menu") })).toBeEnabled();
  });

  it("offers a personal space's page the entry, and makes the link there", async () => {
    const { user, client } = await openPlan({ role: "personal" });
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    expect(await within(dialog).findByText(t("viewLink.once"))).toBeInTheDocument();
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    expect(await client.viewLinkList(work?.id ?? "")).toHaveLength(1);
  });

  it("disables the entry for an Editor while the space does not allow it, and says why", async () => {
    const { user } = await openPlan({ role: "editor" });
    const menu = await pageMenu(user);
    const item = within(menu).getByRole("menuitem", { name: t("viewLink.menu") });
    expect(item).toBeDisabled();
    expect(item).toHaveAttribute("title", t("viewLink.menu.ownerOnly"));
    // The rest of the menu is the Editor's as before.
    expect(within(menu).getByRole("menuitem", { name: t("page.duplicate") })).toBeEnabled();
  });

  it("lets an Editor make a link when the space allows it, and revoke their own but not another member's", async () => {
    const { user, client } = await openPlan({
      role: "editor",
      sharing: { spaces: [{ space: "Work", role: "editor", settings: { editorsMayLink: true } }] },
      memory: BOB_LINK,
    });
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("viewLink.create") }));
    await within(dialog).findByText(t("viewLink.once"));
    const rows = await rowsOf(dialog, 2);
    // Newest first: the Editor's own link, then Bob's.
    const own = within(rows[0] as HTMLElement).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW) }) });
    expect(own).toBeEnabled();
    const bobs = within(rows[1] as HTMLElement).getByRole("button", { name: t("viewLink.revoke.for", { when: formatMoment(NOW - DAY) }) });
    expect(bobs).toBeDisabled();
    expect(bobs).toHaveAttribute("title", t("viewLink.revoke.only"));
    await user.click(own);
    await user.click(await screen.findByRole("button", { name: t("viewLink.revoke.confirm") }));
    await rowsOf(dialog, 1);
    expect(await client.viewLinkList((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).toHaveLength(1);
  });

  it("leaves the entry out for a Commenter, a Viewer and a read-only copy", async () => {
    for (const options of [
      { role: "commenter" },
      { role: "viewer" },
      { role: "editor", archive: "Removed" },
      { role: "owner", archive: "Closed" },
    ] as const satisfies readonly SharingRender[]) {
      const { user, unmount } = await openPlan(options);
      const menu = await pageMenu(user);
      expect(within(menu).queryByRole("menuitem", { name: t("viewLink.menu") })).not.toBeInTheDocument();
      expect(within(menu).getByRole("menuitem", { name: t("page.info") })).toBeInTheDocument();
      unmount();
    }
  });
});

describe("the space setting", () => {
  it("is the Owner's checkbox in the member panel, says it is about view links, and reaches the core", async () => {
    const { user, client } = await renderSharing({ role: "owner" });
    await openSpace(user, "Work");
    const panel = await openMembers(user);
    const box = within(panel).getByRole("checkbox", { name: t("sharing.settings.editorsLink") });
    expect(box).not.toBeChecked();
    expect(box).toHaveAccessibleDescription(t("sharing.settings.editorsLinkHint"));
    expect(t("sharing.settings.editorsLinkHint")).toMatch(/view links/u);
    await user.click(box);
    await waitFor(() => {
      expect(box).toBeChecked();
    });
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    expect((await client.spaceMembers(work?.id ?? "")).settings.editorsMayLink).toBe(true);
  });
});
