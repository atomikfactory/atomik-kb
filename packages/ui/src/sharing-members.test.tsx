// The member panel (Milestone 13, ADR-0041 to ADR-0047) over the in-memory core, which plays the other people: the member list
// with names, roles, the verified mark and safety numbers, what an Owner may change (roles, removals, invitations, settings) and
// what the others see instead, the AI switch, leaving and closing with what each says it does, the archive's way out, and the
// panel following what other people do while it is open.

import { CoreError } from "@atomik-kb/core-client";
import type { Event as CoreEvent, MemorySharedSpaceSeed, Request, Response, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import { roleLabel } from "./sharing";
import { memberNames, memberRow, openMembers, openSpace, renderSharing } from "./test/sharing";

/** Opens the panel of the Work space of an app in which this account has `role`. */
async function setup(role: Parameters<typeof renderSharing>[0] = {}) {
  const app = await renderSharing(role);
  await openSpace(app.user, "Work");
  const dialog = await openMembers(app.user);
  const work = (await app.client.spaceList()).find((space) => space.name === "Work");
  return { ...app, dialog, spaceId: work?.id ?? "" };
}

/** The select of a member's role. */
const roleSelect = (dialog: HTMLElement, name: string): HTMLSelectElement =>
  within(memberRow(dialog, name)).getByRole("combobox", { name: t("sharing.roleFor", { name }) });

describe("the member list", () => {
  it("lists the members in join order, with you, the verified mark and the role of each", async () => {
    const { dialog } = await setup();
    expect(memberNames(dialog)).toEqual(["Maya", "Bob", "Carol", "Dan"]);
    expect(within(memberRow(dialog, "Maya")).getByText(`(${t("sharing.members.you")})`)).toBeInTheDocument();
    // This account is not marked: it is not in doubt to itself.
    expect(within(memberRow(dialog, "Maya")).queryByText(t("sharing.members.unverified"))).not.toBeInTheDocument();
    expect(within(memberRow(dialog, "Bob")).getByText(t("sharing.members.verified"))).toBeInTheDocument();
    expect(within(memberRow(dialog, "Carol")).getByText(t("sharing.members.unverified"))).toBeInTheDocument();
    expect(roleSelect(dialog, "Bob")).toHaveValue("editor");
    expect(roleSelect(dialog, "Carol")).toHaveValue("commenter");
    expect(roleSelect(dialog, "Dan")).toHaveValue("viewer");
    expect(within(dialog).getByText(new RegExp(`^Work · ${t("sharing.members.yourRole", { role: roleLabel("owner") })}`, "u"))).toBeInTheDocument();
    expect(within(memberRow(dialog, "Bob")).getByText(tp("sharing.members.devices", 1))).toBeInTheDocument();
  });

  it("marks a name two members share, a device added with the kit, and the changes that were refused", async () => {
    const members: MemorySharedSpaceSeed["members"] = [
      { name: "Bob", role: "editor", viaKitAt: Date.UTC(2026, 8, 30) },
      { name: "bob", role: "viewer", refusedChanges: 2 },
    ];
    const { dialog } = await setup({ sharing: { spaces: [{ space: "Work", members }] } });
    expect(within(memberRow(dialog, "Bob")).getByText(t("sharing.members.nameShared"))).toBeInTheDocument();
    expect(within(memberRow(dialog, "bob")).getByText(t("sharing.members.nameShared"))).toBeInTheDocument();
    expect(within(memberRow(dialog, "Bob")).getByText(/^Added a device with its recovery kit on /u)).toBeInTheDocument();
    expect(within(memberRow(dialog, "bob")).getByText(tp("sharing.notice.refused", 2, { name: "bob" }))).toBeInTheDocument();
  });

  it("gives an Editor the list to read: tags for roles, no removal, no invitations, no settings", async () => {
    const { dialog } = await setup({ role: "editor" });
    expect(memberNames(dialog)).toEqual(["Alice", "Maya", "Bob", "Carol", "Dan"]);
    expect(within(memberRow(dialog, "Bob")).queryByRole("combobox")).not.toBeInTheDocument();
    expect(within(memberRow(dialog, "Bob")).getByText(roleLabel("editor"))).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.members.remove") })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.invites.code") })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("heading", { name: t("sharing.settings.title") })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.close.button") })).not.toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("sharing.leave.button") })).toBeEnabled();
    // Every member has the AI switch.
    expect(within(dialog).getByRole("switch", { name: t("sharing.ai.label") })).toBeInTheDocument();
  });
});

describe("roles", () => {
  it("lets an Owner change a role, and the list follows", async () => {
    const { user, dialog, client, spaceId } = await setup();
    await user.selectOptions(roleSelect(dialog, "Carol"), "editor");
    await waitFor(() => {
      expect(roleSelect(dialog, "Carol")).toHaveValue("editor");
    });
    const info = await client.spaceMembers(spaceId);
    expect(info.members.find((member) => member.name === "Carol")?.role).toBe("editor");
  });

  it("keeps one Owner: the last Owner's role and Leave are disabled and say why, until someone else is Owner", async () => {
    const { user, dialog } = await setup();
    expect(roleSelect(dialog, "Maya")).toBeDisabled();
    expect(roleSelect(dialog, "Maya")).toHaveAttribute("title", t("sharing.members.lastOwner"));
    const leave = within(dialog).getByRole("button", { name: t("sharing.leave.button") });
    expect(leave).toBeDisabled();
    expect(leave).toHaveAttribute("title", t("sharing.members.lastOwner"));
    // All Owners are equal: with Bob an Owner too, this account may step down or leave.
    await user.selectOptions(roleSelect(dialog, "Bob"), "owner");
    await waitFor(() => {
      expect(roleSelect(dialog, "Maya")).toBeEnabled();
    });
    expect(within(dialog).getByRole("button", { name: t("sharing.leave.button") })).toBeEnabled();
  });

  it("shows a refusal of the core under the list", async () => {
    const { user, dialog, memory } = await setup();
    memory.failNext({ code: "Forbidden", message: "this needs the owner role in the space" });
    await user.selectOptions(roleSelect(dialog, "Dan"), "editor");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      t("sharing.members.roleError", { message: "this needs the owner role in the space" }),
    );
    expect(roleSelect(dialog, "Dan")).toHaveValue("viewer");
  });
});

describe("removing a member", () => {
  it("asks first, says what a removed member keeps, then removes", async () => {
    const { user, dialog } = await setup();
    await user.click(within(memberRow(dialog, "Dan")).getByRole("button", { name: t("sharing.members.remove") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.members.removeTitle", { name: "Dan" }) });
    expect(question).toHaveTextContent(
      "Dan loses access to this space and cannot read anything written from now on. They keep what they already received: past pages and edits stay on their devices. Everyone else gets a new key.",
    );
    await user.click(within(question).getByRole("button", { name: t("sharing.members.removeConfirm", { name: "Dan" }) }));
    await waitFor(() => {
      expect(memberNames(dialog)).toEqual(["Maya", "Bob", "Carol"]);
    });
  });

  it("keeps the member when the question is cancelled, and offers no removal of this account", async () => {
    const { user, dialog } = await setup();
    await user.click(within(memberRow(dialog, "Dan")).getByRole("button", { name: t("sharing.members.remove") }));
    const question = await screen.findByRole("alertdialog");
    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    expect(memberNames(dialog)).toContain("Dan");
    expect(within(memberRow(dialog, "Maya")).queryByRole("button", { name: t("sharing.members.remove") })).not.toBeInTheDocument();
  });
});

describe("safety numbers", () => {
  it("shows a member's number in six groups of five digits and marks the person verified, and back", async () => {
    const { user, dialog, client, spaceId } = await setup();
    const carol = memberRow(dialog, "Carol");
    const info = await client.spaceMembers(spaceId);
    const number = info.members.find((member) => member.name === "Carol")?.safetyNumber ?? "";
    await user.click(within(carol).getByText(t("sharing.members.safety")));
    const group = within(carol).getByRole("group", { name: t("sharing.members.safety") });
    expect(Array.from(group.querySelectorAll("span"), (span) => span.textContent).join(" ")).toBe(number);
    expect(within(carol).getByText(t("sharing.members.safetyHint", { name: "Carol" }))).toBeInTheDocument();
    await user.click(within(carol).getByRole("button", { name: t("sharing.members.markVerified") }));
    expect(await within(memberRow(dialog, "Carol")).findByText(t("sharing.members.verified"))).toBeInTheDocument();
    await user.click(within(memberRow(dialog, "Carol")).getByRole("button", { name: t("sharing.members.unmarkVerified") }));
    expect(await within(memberRow(dialog, "Carol")).findByText(t("sharing.members.unverified"))).toBeInTheDocument();
    // This account has no number to compare with itself.
    expect(within(memberRow(dialog, "Maya")).queryByText(t("sharing.members.safety"))).not.toBeInTheDocument();
  });
});

describe("invitations", () => {
  const INVITES: MemorySharedSpaceSeed["invites"] = [
    { role: "editor" },
    { role: "viewer", status: "Used" },
    { role: "commenter", status: "Expired" },
    { role: "viewer", status: "Revoked", kind: "Code" },
  ];

  it("lists them with their status, and revokes an open one", async () => {
    const { user, dialog } = await setup({ sharing: { spaces: [{ space: "Work", invites: INVITES }] } });
    const list = within(dialog).getByRole("list", { name: t("sharing.invites.title") });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(4);
    expect(within(list).getByText(t("sharing.invites.status.open"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.status.used"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.status.expired"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.status.revoked"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.usedBy", { name: "Bob" }))).toBeInTheDocument();
    // Only an open invitation can be revoked.
    expect(within(list).getAllByRole("button", { name: t("sharing.invites.revoke") })).toHaveLength(1);
    await user.click(within(list).getByRole("button", { name: t("sharing.invites.revoke") }));
    await waitFor(() => {
      expect(within(list).queryByRole("button", { name: t("sharing.invites.revoke") })).not.toBeInTheDocument();
    });
    expect(within(list).getAllByText(t("sharing.invites.status.revoked"))).toHaveLength(2);
  });

  it("says when there are none", async () => {
    const { dialog } = await setup();
    expect(within(dialog).getByText(t("sharing.invites.none"))).toBeInTheDocument();
  });
});

describe("the settings of the space", () => {
  it("sets how long trashed pages stay, and whether Editors may create view links", async () => {
    const { user, dialog, client, spaceId } = await setup();
    const retention = within(dialog).getByRole("combobox", { name: t("sharing.settings.retention") });
    expect(retention).toHaveValue("30");
    expect(within(dialog).getByText(t("sharing.settings.retentionHint"))).toBeInTheDocument();
    await user.selectOptions(retention, "7");
    await waitFor(async () => {
      expect((await client.spaceMembers(spaceId)).settings.trashRetentionDays).toBe(7);
    });
    await user.selectOptions(retention, "0");
    await waitFor(async () => {
      expect((await client.spaceMembers(spaceId)).settings.trashRetentionDays).toBe(0);
    });
    const link = within(dialog).getByRole("checkbox", { name: t("sharing.settings.editorsLink") });
    expect(link).not.toBeChecked();
    await user.click(link);
    await waitFor(async () => {
      expect((await client.spaceMembers(spaceId)).settings.editorsMayLink).toBe(true);
    });
    expect(within(dialog).getByRole("checkbox", { name: t("sharing.settings.editorsLink") })).toBeChecked();
    expect(within(dialog).getByText(t("sharing.settings.editorsLinkHint"))).toBeInTheDocument();
  });

  it("offers the retention another Owner set, even when it is not one of the usual choices", async () => {
    const { dialog } = await setup({ sharing: { spaces: [{ space: "Work", settings: { trashRetentionDays: 2 } }] } });
    const retention = within(dialog).getByRole("combobox", { name: t("sharing.settings.retention") });
    expect(retention).toHaveValue("2");
    expect(within(retention).getByRole("option", { name: tp("sharing.days", 2) })).toBeInTheDocument();
  });
});

describe("AI in the space", () => {
  it("is off for a shared space until this account turns it on, and says what the other members may do", async () => {
    const { user, dialog, client, spaceId } = await setup();
    const toggle = within(dialog).getByRole("switch", { name: t("sharing.ai.label") });
    expect(toggle).not.toBeChecked();
    expect(within(dialog).getByText(new RegExp(t("sharing.ai.others").slice(0, 30), "u"))).toBeInTheDocument();
    await user.click(toggle);
    await waitFor(() => {
      expect(within(dialog).getByRole("switch", { name: t("sharing.ai.label") })).toBeChecked();
    });
    expect((await client.spaceList()).find((space) => space.id === spaceId)?.aiEnabled).toBeUndefined();
    await user.click(within(dialog).getByRole("switch", { name: t("sharing.ai.label") }));
    await waitFor(() => {
      expect(within(dialog).getByRole("switch", { name: t("sharing.ai.label") })).not.toBeChecked();
    });
    expect((await client.spaceList()).find((space) => space.id === spaceId)?.aiEnabled).toBe(false);
  });
});

describe("leaving and closing", () => {
  it("leaves after saying what happens: the space goes from this device", async () => {
    const { user, dialog, client } = await setup({ role: "editor" });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.leave.button") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.leave.title", { name: "Work" }) });
    expect(question).toHaveTextContent(t("sharing.leave.body"));
    await user.click(within(question).getByRole("button", { name: t("sharing.leave.confirm") }));
    await waitFor(async () => {
      expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal"]);
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("sharing.members.title") })).not.toBeInTheDocument();
    });
  });

  it("closes the space for everyone after saying so; it becomes a read-only copy with the way to remove it", async () => {
    const { user, dialog, client } = await setup();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.close.button") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.close.title", { name: "Work" }) });
    expect(question).toHaveTextContent(t("sharing.close.body"));
    await user.click(within(question).getByRole("button", { name: t("sharing.close.confirm") }));
    await waitFor(async () => {
      expect((await client.spaceList()).find((space) => space.name === "Work")?.shareState).toBe("Archived");
    });
    expect(await within(dialog).findByText(t("sharing.archive.closed"))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("sharing.archive.remove") })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.close.button") })).not.toBeInTheDocument();
    // Nothing can be changed in an archive: no invitations, no settings, no removal.
    expect(within(dialog).queryByRole("button", { name: t("sharing.invites.code") })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.members.remove") })).not.toBeInTheDocument();
  });

  it("removes a read-only copy from this device", async () => {
    const { user, dialog, client } = await setup({ role: "editor", archive: "Removed" });
    expect(within(dialog).getByText(t("sharing.archive.removed"))).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: t("sharing.leave.button") })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.archive.remove") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.archive.removeTitle", { name: "Work" }) });
    expect(question).toHaveTextContent(t("sharing.archive.removeBody"));
    await user.click(within(question).getByRole("button", { name: t("sharing.archive.removeConfirm") }));
    await waitFor(async () => {
      expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal"]);
    });
  });

  it("does not read the member list of the space it has just left, whose row the space list still shows for a moment", async () => {
    // Found by the real-app pass (drive-m13-share): between the core deleting the copy and the space list being read again, the member
    // list was asked for once more and the core answered NotFound, which the page logged as a console error.
    let left = false;
    let before: Response | null = null;
    const app = await renderSharing({
      role: "editor",
      wrap: (memory) =>
        new Proxy<Transport>(memory, {
          get(target, property) {
            if (property === "dispatch") {
              return async (request: Request): Promise<Response> => {
                if (request.type === "SpaceList" && left && before !== null) {
                  return before; // the list is not read again yet: it still shows the space
                }
                if (request.type === "SpaceMembers" && left) {
                  throw new CoreError({ code: "NotFound", message: "no such shared space" });
                }
                if (request.type === "SpaceLeave") {
                  left = true; // from here on the space list is stale, as it is until the event of the answer is handled
                }
                const response = await target.dispatch(request);
                if (request.type === "SpaceList" && !left) {
                  before = response;
                }
                return response;
              };
            }
            const value: unknown = Reflect.get(target, property);
            return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
          },
        }),
    });
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openSpace(app.user, "Work");
    const dialog = await openMembers(app.user);
    await app.user.click(within(dialog).getByRole("button", { name: t("sharing.leave.button") }));
    const question = await screen.findByRole("alertdialog", { name: t("sharing.leave.title", { name: "Work" }) });
    await app.user.click(within(question).getByRole("button", { name: t("sharing.leave.confirm") }));
    await waitFor(() => {
      expect(left).toBe(true);
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 150));
    });
    expect(logged.mock.calls.filter((call) => String(call[0]).includes("could not read the member list"))).toEqual([]);
  });

  it("shows a refusal of the core in the panel when closing fails", async () => {
    const { user, dialog, memory } = await setup();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.close.button") }));
    const question = await screen.findByRole("alertdialog");
    memory.failNext({ code: "Internal", message: "boom" });
    await user.click(within(question).getByRole("button", { name: t("sharing.close.confirm") }));
    expect(await within(dialog).findByText(t("sharing.close.error", { message: "boom" }))).toBeInTheDocument();
  });
});

describe("when the member list cannot be read", () => {
  it("says why, offers to try again, and reads the list when it can", async () => {
    let refuse = true;
    const app = await renderSharing({
      role: "owner",
      wrap: (memory) =>
        new Proxy<Transport>(memory, {
          get(target, property) {
            if (property === "dispatch") {
              return (request: Request): Promise<Response> =>
                refuse && request.type === "SpaceMembers"
                  ? Promise.reject(new CoreError({ code: "Internal", message: "no server" }))
                  : target.dispatch(request);
            }
            const value: unknown = Reflect.get(target, property);
            return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
          },
        }),
    });
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openSpace(app.user, "Work");
    await app.user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await app.user.click(await screen.findByRole("menuitem", { name: t("sharing.menu.members") }));
    const dialog = await screen.findByRole("dialog", { name: t("sharing.members.title") });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.members.error", { message: "no server" }));
    // Leaving and the other ways out are still there.
    expect(within(dialog).getByRole("button", { name: t("sharing.leave.button") })).toBeInTheDocument();
    refuse = false;
    await app.user.click(within(dialog).getByRole("button", { name: t("sharing.members.retry") }));
    await within(dialog).findByRole("list", { name: t("sharing.members.list") });
    expect(memberNames(dialog)).toEqual(["Maya", "Bob", "Carol", "Dan"]);
  });
});

describe("while the panel is open", () => {
  it("follows what other people do: a member leaves, a link is used, a role changes", async () => {
    const { dialog, memory, client, spaceId } = await setup();
    const carol = (await client.spaceMembers(spaceId)).members.find((member) => member.name === "Carol");
    act(() => {
      memory.receiveSharing({ type: "memberLeft", spaceId, accountId: carol?.accountId ?? "" });
    });
    await waitFor(() => {
      expect(memberNames(dialog)).toEqual(["Maya", "Bob", "Dan"]);
    });
    expect(await within(dialog).findByText(t("sharing.members.pendingRekey"))).toBeInTheDocument();
    act(() => {
      memory.receiveSharing({ type: "memberJoined", spaceId, name: "Eve", role: "commenter" });
    });
    await waitFor(() => {
      expect(memberNames(dialog)).toEqual(["Maya", "Bob", "Dan", "Eve"]);
    });
    expect(roleSelect(dialog, "Eve")).toHaveValue("commenter");
    const bob = (await client.spaceMembers(spaceId)).members.find((member) => member.name === "Bob");
    act(() => {
      memory.receiveSharing({ type: "roleSet", spaceId, accountId: bob?.accountId ?? "", role: "viewer" });
    });
    await waitFor(() => {
      expect(roleSelect(dialog, "Bob")).toHaveValue("viewer");
    });
  });

  it("counts a device this account just added in its own row, without the panel being reopened", async () => {
    // The device log of this account brings a device (a pairing, a recovery with the kit): the core says `DevicesChanged`, and the
    // lists that were read before are read again, as this account's row counts its devices.
    let extra = 0;
    let listener: ((event: CoreEvent) => void) | null = null;
    const app = await renderSharing({
      role: "owner",
      wrap: (memory) =>
        new Proxy<Transport>(memory, {
          get(target, property) {
            if (property === "dispatch") {
              return async (request: Request): Promise<Response> => {
                const response = await target.dispatch(request);
                if (request.type === "SpaceMembers" && response.type === "Members" && extra > 0) {
                  const info = response.data;
                  return { ...response, data: { ...info, members: info.members.map((m) => (m.isMe ? { ...m, deviceCount: m.deviceCount + extra } : m)) } };
                }
                return response;
              };
            }
            if (property === "subscribe") {
              return (onEvent: (event: CoreEvent) => void) => {
                listener = onEvent;
                return target.subscribe(onEvent);
              };
            }
            const value: unknown = Reflect.get(target, property);
            return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
          },
        }),
    });
    await openSpace(app.user, "Work");
    const dialog = await openMembers(app.user);
    expect(within(memberRow(dialog, "Maya")).getByText(tp("sharing.members.devices", 1))).toBeInTheDocument();
    extra = 1;
    act(() => {
      listener?.({ type: "DevicesChanged", data: { added: [], revoked: [], thisDeviceRevoked: false, rotated: false } });
    });
    expect(await within(memberRow(dialog, "Maya")).findByText(tp("sharing.members.devices", 2))).toBeInTheDocument();
  });

  it("lists an incomplete snapshot to Owners", async () => {
    const { dialog } = await setup({ sharing: { spaces: [{ space: "Work", disputes: [{ member: "Bob" }] }] } });
    const heading = within(dialog).getByRole("heading", { name: t("sharing.disputes.title") });
    expect(heading.closest("section")).toHaveTextContent(t("sharing.notice.disputedNoPage", { name: "Bob" }));
  });
});

describe("the member count of the space switcher", () => {
  it("follows a member who joined, which the core reports with `SpaceMembersChanged` alone", async () => {
    // The member count is part of the space as listed (`SpaceList`), and a member who joins does not come with `SpacesChanged`:
    // the badge beside the space must be read again with the members, or an Owner's switcher keeps saying "1 member".
    let extra = 0;
    let listener: ((event: CoreEvent) => void) | null = null;
    const app = await renderSharing({
      role: "owner",
      wrap: (memory) =>
        new Proxy<Transport>(memory, {
          get(target, property) {
            if (property === "dispatch") {
              return async (request: Request): Promise<Response> => {
                const response = await target.dispatch(request);
                if (request.type === "SpaceList" && response.type === "Spaces" && extra > 0) {
                  return { ...response, data: response.data.map((space) => (space.memberCount === undefined ? space : { ...space, memberCount: space.memberCount + extra })) };
                }
                return response;
              };
            }
            if (property === "subscribe") {
              return (onEvent: (event: CoreEvent) => void) => {
                listener = onEvent;
                return target.subscribe(onEvent);
              };
            }
            const value: unknown = Reflect.get(target, property);
            return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
          },
        }),
    });
    await openSpace(app.user, "Work");
    expect(await screen.findByText(tp("sharing.badge", 4))).toBeInTheDocument();
    extra = 1;
    act(() => {
      listener?.({ type: "SpaceMembersChanged", data: { spaceId: "any" } });
    });
    expect(await screen.findByText(tp("sharing.badge", 5))).toBeInTheDocument();
    expect(screen.queryByText(tp("sharing.badge", 4))).not.toBeInTheDocument();
  });
});
