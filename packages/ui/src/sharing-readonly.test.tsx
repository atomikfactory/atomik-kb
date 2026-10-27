// What a role lets the app show (Milestone 13, ADR-0043 and ADR-0047) over the in-memory core, which refuses the writes of a role
// that cannot make them: a Viewer or a Commenter (until comments arrive) reads a space through the editor's read-only state, with
// no new page, rename, drag, trash or undo, and a page menu without the commands that write; an Editor writes but does not delete
// for good or move pages out; an Owner does everything. A read-only copy (a space this account was removed from, or that was closed)
// says so in a banner. What happens to this account while it is open (a role change, a removal with its unsent changes, a device
// added with a kit, an incomplete snapshot, refused changes) shows as notices, and AI is off in a shared space until it is turned on.

import { AI_SETTINGS_SETTING } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { writingOn } from "./editor/ai/EditorAi";
import { t, tp } from "./i18n";
import { roleLabel } from "./sharing";
import { AI_ON } from "./test/ai";
import { findEditor, idsOf } from "./test/page";
import { openScreen } from "./test/render";
import { row } from "./test/sidebar";
import { clickPage, openSpace, renderSharing, toasts } from "./test/sharing";

const workPage = async (user: Parameters<typeof clickPage>[0], title: string): Promise<void> => {
  await openSpace(user, "Work");
  await clickPage(user, title);
};

describe("a Viewer and a Commenter", () => {
  for (const role of ["viewer", "commenter"] as const) {
    it(`read as a ${role}: a read-only editor, title, icon and properties, and a mark that says so`, async () => {
      const { user } = await renderSharing({ role });
      await workPage(user, "Notes");
      const editor = await findEditor();
      expect(editor.isEditable).toBe(false);
      // The title is plain text: there is no rename button in it.
      const heading = screen.getByRole("heading", { level: 1, name: "Notes" });
      expect(within(heading).queryByRole("button")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: t("page.addIcon") })).toBeDisabled();
      expect(screen.queryByRole("button", { name: t("page.addCover") })).not.toBeInTheDocument();
      const mark = screen.getByText(t("sharing.readOnly.pill"));
      expect(mark.closest("p")).toHaveAttribute("title", t(role === "viewer" ? "sharing.readOnly.viewer" : "sharing.readOnly.commenter"));
      // It is a permission, not the page's lock: nothing offers to unlock.
      expect(screen.queryByRole("button", { name: t("page.unlock") })).not.toBeInTheDocument();
    });

    it(`have a tree to read as a ${role}: no new page, no sub-page, no rename, no trash`, async () => {
      const { user } = await renderSharing({ role });
      await openSpace(user, "Work");
      expect(screen.queryByRole("button", { name: t("tree.newPage") })).not.toBeInTheDocument();
      expect(within(row("Plan")).queryByRole("button", { name: t("tree.addChild") })).not.toBeInTheDocument();
      expect(row("Plan")).toHaveAttribute("draggable", "false");
      // The context menu offers what reading and the favourites need.
      await user.pointer({ keys: "[MouseRight]", target: row("Plan") });
      const menu = await screen.findByRole("menu", { name: t("tree.actions") });
      expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
        t("page.openInNewTab"),
        t("favorites.add"),
      ]);
      await user.keyboard("{Escape}");
      // F2 and Delete do nothing.
      row("Plan").focus();
      await user.keyboard("{F2}");
      expect(screen.queryByRole("textbox", { name: t("tree.renameLabel") })).not.toBeInTheDocument();
      await user.keyboard("{Delete}");
      expect(row("Plan")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: t("sidebar.undo") })).toBeDisabled();
    });
  }

  it("get a page menu without duplicate, import into the page and the look of the page, and a space menu without rename or properties", async () => {
    const { user } = await renderSharing({ role: "viewer" });
    await workPage(user, "Notes");
    await user.click(screen.getByRole("button", { name: t("page.actions") }));
    const menu = await screen.findByRole("menu", { name: t("page.actions") });
    expect(within(menu).queryByRole("menuitem", { name: t("page.duplicate") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("interop.menu.importPage") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("moveToSpace.menu") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("checkbox")).not.toBeInTheDocument();
    // Copying and exporting are for every member.
    expect(within(menu).getByRole("menuitem", { name: t("page.copyMarkdown") })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: t("page.info") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    const spaceMenu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    for (const label of [
      t("spaces.rename"),
      t("spaces.changeIcon"),
      t("properties.menu.space"),
      t("spaces.sortTitle"),
      t("spaces.sortCreated"),
      t("interop.menu.importSpace"),
    ]) {
      expect(within(spaceMenu).queryByRole("menuitem", { name: label })).not.toBeInTheDocument();
    }
    expect(within(spaceMenu).getByRole("menuitem", { name: t("interop.menu.exportSpace") })).toBeInTheDocument();
  });

  it("see the trash without restoring or deleting anything", async () => {
    const { user, client, memory } = await renderSharing({ role: "editor" });
    const ids = await idsOf(memory);
    await client.pageTrash(ids.page("Notes"));
    await openSpace(user, "Work");
    const me = (await client.spaceMembers(ids.space("Work"))).members.find((member) => member.isMe);
    act(() => {
      memory.receiveSharing({ type: "roleSet", spaceId: ids.space("Work"), accountId: me?.accountId ?? "", role: "viewer" });
    });
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: t("tree.newPage") })).not.toBeInTheDocument();
    });
    await user.click(await screen.findByRole("button", { name: /^Trash/u }));
    const list = await screen.findByRole("list", { name: t("trash.listLabel") });
    expect(within(list).getByText("Notes")).toBeInTheDocument();
    expect(within(list).queryByRole("button")).not.toBeInTheDocument();
  });
});

describe("an empty space", () => {
  it("does not tell a role that cannot make pages to use New page", async () => {
    const { user } = await renderSharing({
      role: "viewer",
      memory: { spaces: [{ name: "Personal", pages: [{ title: "Diary" }] }, { name: "Work" }] },
    });
    await openSpace(user, "Work");
    expect(await screen.findByText(t("sharing.home.emptyReadOnly"))).toBeInTheDocument();
    expect(screen.queryByText(t("home.hint.emptySpace"))).not.toBeInTheDocument();
  });

  it("keeps the advice for a role that can", async () => {
    const { user } = await renderSharing({
      role: "editor",
      memory: { spaces: [{ name: "Personal", pages: [{ title: "Diary" }] }, { name: "Work" }] },
    });
    await openSpace(user, "Work");
    expect(await screen.findByText(t("home.hint.emptySpace"))).toBeInTheDocument();
  });
});

describe("an Editor", () => {
  it("edits pages and the tree, restores from the trash, but cannot delete for good or move a page out of the space", async () => {
    const { user, client, memory } = await renderSharing({ role: "editor" });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    await clickPage(user, "Notes");
    expect((await findEditor()).isEditable).toBe(true);
    expect(screen.getByRole("button", { name: t("tree.newPage") })).toBeInTheDocument();
    expect(within(row("Plan")).getByRole("button", { name: t("tree.addChild") })).toBeInTheDocument();
    await user.pointer({ keys: "[MouseRight]", target: row("Plan") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    const items = within(menu).getAllByRole("menuitem").map((item) => item.textContent);
    expect(items).toContain(t("tree.menu.rename"));
    expect(items).toContain(t("page.duplicate"));
    expect(items).not.toContain(t("moveToSpace.menu"));
    await user.keyboard("{Escape}");
    // The trash: restore yes, delete for good no.
    await client.pageTrash(ids.page("Sub plan"));
    await user.click(await screen.findByRole("button", { name: /^Trash/u }));
    const list = await screen.findByRole("list", { name: t("trash.listLabel") });
    expect(within(list).getByRole("button", { name: t("trash.restore") })).toBeInTheDocument();
    expect(within(list).queryByRole("button", { name: t("trash.purge") })).not.toBeInTheDocument();
  });
});

describe("an Owner", () => {
  it("does everything: moves pages out, restores and deletes for good", async () => {
    const { user, client, memory } = await renderSharing({ role: "owner" });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    await user.pointer({ keys: "[MouseRight]", target: row("Plan") });
    const menu = await screen.findByRole("menu", { name: t("tree.actions") });
    expect(within(menu).getByRole("menuitem", { name: t("moveToSpace.menu") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await client.pageTrash(ids.page("Sub plan"));
    await user.click(await screen.findByRole("button", { name: /^Trash/u }));
    const list = await screen.findByRole("list", { name: t("trash.listLabel") });
    expect(within(list).getByRole("button", { name: t("trash.restore") })).toBeInTheDocument();
    expect(within(list).getByRole("button", { name: t("trash.purge") })).toBeInTheDocument();
  });
});

describe("when the role changes while the page is open", () => {
  it("turns the editor read-only, announces it, and hides what the new role cannot do", async () => {
    const { user, memory, client } = await renderSharing({ role: "editor" });
    await workPage(user, "Notes");
    const editor = await findEditor();
    expect(editor.isEditable).toBe(true);
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const me = (await client.spaceMembers(work?.id ?? "")).members.find((member) => member.isMe);
    act(() => {
      memory.receiveSharing({ type: "roleSet", spaceId: work?.id ?? "", accountId: me?.accountId ?? "", role: "viewer" });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(false);
    });
    expect(
      await within(toasts()).findByText(t("sharing.notice.roleChanged", { space: "Work", previous: roleLabel("editor"), role: roleLabel("viewer") })),
    ).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: t("tree.newPage") })).not.toBeInTheDocument();
    });
    expect(screen.getByText(t("sharing.readOnly.pill"))).toBeInTheDocument();
    // Promoted again, it is editable again.
    act(() => {
      memory.receiveSharing({ type: "roleSet", spaceId: work?.id ?? "", accountId: me?.accountId ?? "", role: "editor" });
    });
    await waitFor(() => {
      expect(editor.isEditable).toBe(true);
    });
  });
});

describe("a read-only copy", () => {
  for (const [archive, text] of [
    ["Removed", "sharing.archive.removed"],
    ["Closed", "sharing.archive.closed"],
  ] as const) {
    it(`says so in a banner when the space was ${archive === "Removed" ? "left through a removal" : "closed"}, and is read-only for everybody`, async () => {
      const { user } = await renderSharing({ role: "editor", archive });
      await openSpace(user, "Work");
      const banner = await screen.findByText(t(text));
      expect(banner.closest(".archive-banner")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: t("sharing.archive.remove") })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: t("tree.newPage") })).not.toBeInTheDocument();
      await clickPage(user, "Notes");
      expect((await findEditor()).isEditable).toBe(false);
      expect(screen.getByText(t("sharing.archive.badge"), { selector: "span:not(.visually-hidden)" })).toBeInTheDocument();
    });
  }

  it("removes the copy from this device, after asking", async () => {
    const { user, client } = await renderSharing({ role: "owner", archive: "Closed" });
    await openSpace(user, "Work");
    await user.click(await screen.findByRole("button", { name: t("sharing.archive.remove") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.archive.removeTitle", { name: "Work" }) });
    await user.click(within(question).getByRole("button", { name: t("sharing.archive.removeConfirm") }));
    await waitFor(async () => {
      expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal"]);
    });
  });

  it("offers Remove from this device in the space menu instead of Delete", async () => {
    const { user } = await renderSharing({ role: "owner", archive: "Closed" });
    await openSpace(user, "Work");
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    const menu = await screen.findByRole("menu", { name: t("spaces.optionsLabel") });
    expect(within(menu).getByRole("menuitem", { name: t("sharing.archive.remove") })).toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: t("spaces.delete") })).not.toBeInTheDocument();
  });
});

describe("notices", () => {
  it("says a space access was lost when this account is removed, and that unsent changes were copied, with a way to open them", async () => {
    const { user, memory, client } = await renderSharing({ role: "editor" });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    act(() => {
      memory.receiveSharing({ type: "removed", spaceId: ids.space("Work"), unsentPageIds: [ids.page("Notes")] });
    });
    expect(await within(toasts()).findByText(t("sharing.notice.removed", { space: "Work" }))).toBeInTheDocument();
    expect(await screen.findByText(t("sharing.archive.removed"))).toBeInTheDocument();
    const unsent = await within(toasts()).findByText(tp("sharing.notice.unsent", 1, { space: "Work", target: "Personal" }));
    expect(unsent).toBeInTheDocument();
    await user.click(within(toasts()).getByRole("button", { name: t("sharing.notice.unsent.open") }));
    expect(await screen.findByRole("heading", { level: 1, name: "Unsent changes - Notes" })).toBeInTheDocument();
    const copied = (await client.spaceList()).find((space) => space.name === "Personal");
    expect(copied?.shareState).toBeUndefined();
  });

  it("says when the space was closed by an Owner", async () => {
    const { user, memory } = await renderSharing({ role: "editor" });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    act(() => {
      memory.receiveSharing({ type: "closed", spaceId: ids.space("Work") });
    });
    expect(await within(toasts()).findByText(t("sharing.notice.closed", { space: "Work" }))).toBeInTheDocument();
    expect(await screen.findByText(t("sharing.archive.closed"))).toBeInTheDocument();
  });

  it("says once that a member's account added a device with its recovery kit", async () => {
    const { user, memory, client } = await renderSharing({ role: "owner" });
    await openSpace(user, "Work");
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const bob = (await client.spaceMembers(work?.id ?? "")).members.find((member) => member.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "viaKit", spaceId: work?.id ?? "", accountId: bob?.accountId ?? "" });
    });
    expect(await within(toasts()).findByText(t("sharing.notice.viaKit", { name: "Bob" }))).toBeInTheDocument();
  });

  it("tells an Owner that a member's device published an incomplete copy of a page", async () => {
    const { user, memory, client } = await renderSharing({ role: "owner" });
    const ids = await idsOf(memory);
    await openSpace(user, "Work");
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const bob = (await client.spaceMembers(work?.id ?? "")).members.find((member) => member.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "disputed", spaceId: work?.id ?? "", accountId: bob?.accountId ?? "", pageId: ids.page("Plan") });
    });
    expect(await within(toasts()).findByText(t("sharing.notice.disputed", { name: "Bob", page: "Plan" }))).toBeInTheDocument();
  });

  it("shows the changes this device refused as a notice of the space that can be dismissed, and again when there are more", async () => {
    const { user, memory, client } = await renderSharing({
      role: "owner",
      sharing: { spaces: [{ space: "Work", members: [{ name: "Carol", role: "commenter", refusedChanges: 3 }] }] },
    });
    await openSpace(user, "Work");
    const text = tp("sharing.notice.refused", 3, { name: "Carol" });
    const notice = await screen.findByText(text);
    await user.click(within(notice.closest(".space-notice") as HTMLElement).getByRole("button", { name: t("toast.dismiss") }));
    expect(screen.queryByText(text)).not.toBeInTheDocument();
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    const carol = (await client.spaceMembers(work?.id ?? "")).members.find((member) => member.name === "Carol");
    act(() => {
      memory.receiveSharing({ type: "refused", spaceId: work?.id ?? "", accountId: carol?.accountId ?? "", count: 2 });
    });
    expect(await screen.findByText(tp("sharing.notice.refused", 5, { name: "Carol" }))).toBeInTheDocument();
  });

  it("warns when the server delivered a change from someone who is not a member", async () => {
    const { user } = await renderSharing({ role: "editor", sharing: { spaces: [{ space: "Work", role: "editor", serverFlagged: true }] } });
    await openSpace(user, "Work");
    const warning = await screen.findByText(t("sharing.notice.serverFlagged"));
    expect(warning.closest("[role='alert']")).toBeInTheDocument();
    await user.click(within(warning.closest(".space-notice") as HTMLElement).getByRole("button", { name: t("toast.dismiss") }));
    expect(screen.queryByText(t("sharing.notice.serverFlagged"))).not.toBeInTheDocument();
  });

  it("shows nothing of this in a personal space", async () => {
    const { user } = await renderSharing({ role: "owner" });
    await openSpace(user, "Personal");
    expect(document.querySelector(".archive-banner, .space-notice")).toBeNull();
    expect(screen.queryByText(t("sharing.readOnly.pill"))).not.toBeInTheDocument();
  });
});

describe("AI in a shared space", () => {
  async function renderAi() {
    const app = await renderSharing({
      role: "editor",
      memory: { ai: { mode: "fake" }, settings: { [AI_SETTINGS_SETTING]: AI_ON } },
    });
    return app;
  }

  it("is off until this account turns it on: the chat says why and offers to turn it on for the space", async () => {
    const { user, client } = await renderAi();
    await openSpace(user, "Work");
    await user.click(screen.getByRole("button", { name: t("aiChat.toggle") }));
    const panel = await screen.findByRole("complementary", { name: t("aiChat.title") });
    expect(within(panel).getByText(t("sharing.ai.off", { space: "Work" }))).toBeInTheDocument();
    expect(within(panel).getByRole("textbox", { name: t("aiChat.placeholder") })).toBeDisabled();
    await user.click(within(panel).getByRole("button", { name: t("sharing.ai.turnOn") }));
    await waitFor(() => {
      expect(within(panel).queryByText(t("sharing.ai.off", { space: "Work" }))).not.toBeInTheDocument();
    });
    expect(within(panel).getByRole("textbox", { name: t("aiChat.placeholder") })).toBeEnabled();
    expect((await client.spaceList()).find((space) => space.name === "Work")?.aiEnabled).toBeUndefined();
  });

  it("does not hinder a personal space or the All spaces scope", async () => {
    const { user } = await renderAi();
    await user.click(screen.getByRole("button", { name: t("aiChat.toggle") }));
    const panel = await screen.findByRole("complementary", { name: t("aiChat.title") });
    expect(within(panel).queryByText(t("sharing.ai.off", { space: "Personal" }))).not.toBeInTheDocument();
    expect(within(panel).getByRole("textbox", { name: t("aiChat.placeholder") })).toBeEnabled();
    await openSpace(user, "Work");
    expect(await within(panel).findByText(t("sharing.ai.off", { space: "Work" }))).toBeInTheDocument();
    await user.selectOptions(within(panel).getByRole("combobox", { name: t("aiChat.scope") }), "all");
    expect(within(panel).queryByText(t("sharing.ai.off", { space: "Work" }))).not.toBeInTheDocument();
  });

  it("hides the writing assistant of the editor in a space whose AI is off, and offers it again once it is on", async () => {
    const { user, client } = await renderAi();
    await openSpace(user, "Work");
    await clickPage(user, "Notes");
    const editor = await findEditor();
    expect(writingOn(editor)).toBe(false);
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    await client.spaceAiSet(work?.id ?? "", true);
    await waitFor(() => {
      expect(writingOn(editor)).toBe(true);
    });
    await client.spaceAiSet(work?.id ?? "", false);
    await waitFor(() => {
      expect(writingOn(editor)).toBe(false);
    });
  });

  it("keeps the MCP grants away from a space whose AI is off: its boxes are off, with the reason", async () => {
    const { user } = await renderAi();
    await openScreen(user, "nav.settings");
    await screen.findByRole("heading", { name: t("mcp.title") });
    const section = screen.getByRole("region", { name: t("mcp.title") });
    await user.type(within(section).getByLabelText(t("mcp.add.label")), "Agent");
    await user.click(within(section).getByRole("button", { name: t("mcp.add.button") }));
    const block = await within(section).findByRole("region", { name: t("mcp.new.title", { name: "Agent" }) });
    await user.click(within(block).getByRole("button", { name: t("mcp.new.done") }));
    await user.click(within(section).getByRole("button", { name: t("mcp.permissions", { name: "Agent" }) }));
    const grid = within(section).getByRole("form", { name: t("mcp.grants.title", { name: "Agent" }) });
    expect(within(grid).getByRole("checkbox", { name: t("mcp.grants.readIn", { space: "Work" }) })).toBeDisabled();
    expect(within(grid).getByRole("checkbox", { name: t("mcp.grants.writeIn", { space: "Work" }) })).toBeDisabled();
    expect(within(grid).getByText(t("sharing.mcp.aiOff"))).toBeInTheDocument();
    expect(within(grid).getByRole("checkbox", { name: t("mcp.grants.readIn", { space: "Personal" }) })).toBeEnabled();
  });

  it("the answer of the chat cannot be inserted into a page this role cannot edit", async () => {
    const { user, client } = await renderSharing({
      role: "viewer",
      memory: { ai: { mode: "fake" }, settings: { [AI_SETTINGS_SETTING]: AI_ON } },
    });
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    await client.spaceAiSet(work?.id ?? "", true);
    await workPage(user, "Notes");
    await findEditor();
    await user.click(screen.getByRole("button", { name: t("aiChat.toggle") }));
    const panel = await screen.findByRole("complementary", { name: t("aiChat.title") });
    await user.type(within(panel).getByRole("textbox", { name: t("aiChat.placeholder") }), "dentist{Enter}");
    const insert = await within(panel).findByRole("button", { name: t("aiChat.insert") });
    expect(insert).toBeDisabled();
    expect(insert).toHaveAttribute("title", t("sharing.ai.insertReadOnly"));
  });
});
