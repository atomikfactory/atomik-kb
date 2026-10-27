// The share dialog (Milestone 13, ADR-0041 and ADR-0047) over the in-memory core, which mirrors the share: the menu offers "Share
// this space..." on a personal space and "Members and sharing..." on a shared one; the dialog explains the four roles, warns that
// everyone invited receives the whole history (pages in the trash and everything ever deleted from them) and that members keep
// copies, counts what would be sent, says that members may turn on local AI on their own devices, asks for what is missing (sync, the
// recovery kit, the name other members see), and follows the space from Pending to Shared.

import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t, tp } from "./i18n";
import { ROLES, roleHint, roleLabel } from "./sharing";
import { idsOf } from "./test/page";
import { openSpace, renderSharing, spaceMenu, spaceMenuItems } from "./test/sharing";

async function openShare(user: Parameters<typeof spaceMenu>[0]): Promise<HTMLElement> {
  await openSpace(user, "Work");
  await spaceMenu(user, t("sharing.share.menu"));
  return screen.findByRole("dialog", { name: t("sharing.share.title", { name: "Work" }) });
}

describe("the space menu", () => {
  it("offers Share on a personal space, and Members and sharing (not Share) on a shared one", async () => {
    const { user } = await renderSharing({ role: "personal" });
    await openSpace(user, "Work");
    const items = await spaceMenuItems(user);
    expect(items).toContain(t("sharing.share.menu"));
    expect(items).not.toContain(t("sharing.menu.members"));
    // A personal space is deleted, not left.
    expect(items).toContain(t("spaces.delete"));
  });

  it("offers Members and sharing and Leave or close on a shared space, instead of Share and Delete", async () => {
    const { user } = await renderSharing({ role: "owner" });
    await openSpace(user, "Work");
    const items = await spaceMenuItems(user);
    expect(items).toContain(t("sharing.menu.members"));
    expect(items).toContain(t("sharing.menu.leaveOrClose"));
    expect(items).not.toContain(t("sharing.share.menu"));
    expect(items).not.toContain(t("spaces.delete"));
  });
});

describe("the share dialog", () => {
  it("explains the roles, warns about the history and the copies, and says members may use local AI", async () => {
    const { user } = await renderSharing({ role: "personal" });
    const dialog = await openShare(user);
    expect(within(dialog).getByText(t("sharing.share.intro"))).toBeInTheDocument();
    for (const role of ROLES) {
      expect(within(dialog).getByText(roleLabel(role))).toBeInTheDocument();
      expect(within(dialog).getByText(roleHint(role))).toBeInTheDocument();
    }
    expect(within(dialog).getByRole("note")).toHaveTextContent(
      "Everyone you invite receives this space’s whole history, including pages in the trash and everything ever deleted from them. Members keep copies; removing a member cannot take them back.",
    );
    expect(within(dialog).getByText(t("sharing.share.ai"))).toBeInTheDocument();
  });

  it("counts the pages, the pages in the trash, the files and the history before anything is sent", async () => {
    const { user, memory, client } = await renderSharing({ role: "personal" });
    const ids = await idsOf(memory);
    await client.pageTrash(ids.page("Notes"));
    const dialog = await openShare(user);
    const fact = async (label: string): Promise<string> => {
      const term = await within(dialog).findByText(label);
      return term.nextElementSibling?.textContent ?? "";
    };
    expect(await fact(t("sharing.share.fact.pages"))).toBe("2");
    expect(await fact(t("sharing.share.fact.trashed"))).toBe("1");
    expect(await fact(t("sharing.share.fact.files"))).toBe("0");
    expect(await fact(t("sharing.share.fact.history"))).toMatch(/kB|byte/u);
    // Nothing was sent: the space is still personal.
    expect((await client.spaceList()).find((space) => space.name === "Work")?.shareState ?? "Personal").toBe("Personal");
  });

  it("shares: Pending, then Shared, with a badge of the member count in the space list and a way on to the invitations", async () => {
    const { user, client } = await renderSharing({ role: "personal", sharing: { stepMs: 400 } });
    const dialog = await openShare(user);
    await user.click(await within(dialog).findByRole("button", { name: t("sharing.share.submit") }));
    expect(await within(dialog).findByText(t("sharing.share.pending"))).toBeInTheDocument();
    expect(await within(dialog).findByText(t("sharing.share.done", { name: "Work" }), undefined, { timeout: 10_000 })).toBeInTheDocument();
    expect(screen.getByText(tp("sharing.badge", 1))).toBeInTheDocument();
    expect(await client.spaceMembers((await client.spaceList()).find((space) => space.name === "Work")?.id ?? "")).toMatchObject({
      myRole: "owner",
    });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.share.invite") }));
    expect(await screen.findByRole("dialog", { name: t("sharing.inviteCode.title") })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: t("sharing.share.title", { name: "Work" }) })).not.toBeInTheDocument();
  });

  it("asks for the name other members see when there is none, and shares with it", async () => {
    const { user, client } = await renderSharing({ role: "personal", sharing: { profileName: undefined } });
    const dialog = await openShare(user);
    const field = await within(dialog).findByRole("textbox", { name: t("sharing.share.nameLabel") });
    expect(within(dialog).getByText(t("sharing.share.nameHint"))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.share.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.share.error.name"));
    await user.type(field, "  Zed ");
    await user.click(within(dialog).getByRole("button", { name: t("sharing.share.submit") }));
    await within(dialog).findByText(t("sharing.share.done", { name: "Work" }), undefined, { timeout: 10_000 });
    expect(await client.profileGet()).toEqual({ displayName: "Zed" });
    const work = (await client.spaceList()).find((space) => space.name === "Work");
    expect((await client.spaceMembers(work?.id ?? "")).members[0]?.name).toBe("Zed");
  });

  it("does not offer Share without a sync server, and says to turn it on first", async () => {
    const { user } = await renderSharing({ role: "personal", memory: { sync: undefined } });
    const dialog = await openShare(user);
    expect(await within(dialog).findByText(t("sharing.share.needSync"))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("sharing.share.submit") })).toBeDisabled();
  });

  it("does not offer Share without the recovery kit, and says to create it first", async () => {
    const { user } = await renderSharing({ role: "personal", memory: { security: undefined } });
    const dialog = await openShare(user);
    expect(await within(dialog).findByText(t("sharing.share.needKit"))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("sharing.share.submit") })).toBeDisabled();
  });

  it("shows a refusal of the core and stays open", async () => {
    const { user, memory } = await renderSharing({ role: "personal" });
    const dialog = await openShare(user);
    await within(dialog).findByText(t("sharing.share.fact.pages"));
    memory.failNext({ code: "Internal", message: "boom" });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.share.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.share.error", { message: "boom" }));
    expect(within(dialog).getByRole("button", { name: t("sharing.share.submit") })).toBeEnabled();
  });

  it("cancels without sharing anything", async () => {
    const { user, client } = await renderSharing({ role: "personal" });
    const dialog = await openShare(user);
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect((await client.spaceList()).every((space) => space.shareState === undefined)).toBe(true);
  });
});
