// Invitations (Milestone 13, ADR-0046) over the in-memory core, which plays the other person and the server on timers: the Owner's
// code invitation (the code and the server address to give, six digits to compare, "You are inviting <name>" with the safety
// number, confirm second), the link invitation (shown once, copied, listed, revoked, used once), joining by code (the invitee
// types the server and the code, compares the digits, receives the space) and the preview of an invitation link, which the
// desktop shell delivers as an `inviteLink` message and which is held until the app is ready.

import {
  CoreClient,
  DEAD_LINK_SECRET,
  EXPIRED_INVITE_CODE,
  MEMORY_INVITE_CODE,
  MEMORY_INVITE_SAS,
  MemoryTransport,
} from "@atomik-kb/core-client";
import type { Request, Response, ShellEvent, Transport } from "@atomik-kb/core-client";
import { act, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "./i18n";
import { roleLabel } from "./sharing";
import { withShell } from "./test/shell";
import type { ShellTransport } from "./test/shell";
import { NOW, renderWith } from "./test/render";
import { SERVER, memberNames, memberRow, openMembers, openSpace, renderSharing, toasts } from "./test/sharing";

afterEach(() => {
  vi.restoreAllMocks();
});

const SECRET = "8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB";
const LINK = `atomik-kb://invite?server=${encodeURIComponent(SERVER)}#${SECRET}`;

/** The groups shown by a `role="group"` display (the code, the digits, the safety number). */
function groupsOf(group: HTMLElement): string[] {
  return Array.from(group.querySelectorAll("span"), (span) => span.textContent ?? "");
}

describe("an invitation by code, the Owner's side", () => {
  async function openInvite(role = "owner" as const) {
    const app = await renderSharing({ role });
    await openSpace(app.user, "Work");
    const panel = await openMembers(app.user);
    await app.user.click(within(panel).getByRole("button", { name: t("sharing.invites.code") }));
    const dialog = await screen.findByRole("dialog", { name: t("sharing.inviteCode.title") });
    return { ...app, panel, dialog };
  }

  it("shows the code and the server address, compares six digits, then confirms second, and the person joins verified", async () => {
    const { user, panel, dialog, client } = await openInvite();
    expect(within(dialog).getByText(t("sharing.inviteCode.formIntro"))).toBeInTheDocument();
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("sharing.invites.roleLabel") }), "viewer");
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") }));

    const code = await within(dialog).findByRole("group", { name: t("sharing.codeLabel") });
    expect(groupsOf(code)).toEqual(MEMORY_INVITE_CODE.split(" "));
    expect(dialog).toHaveTextContent(SERVER);
    expect(dialog).toHaveTextContent(t("sharing.inviteCode.intro"));

    const digits = await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    expect(groupsOf(digits)).toEqual([MEMORY_INVITE_SAS.slice(0, 3), MEMORY_INVITE_SAS.slice(3)]);
    // The other person confirms first: "You are inviting Dana", with their safety number.
    await within(dialog).findByText(t("sharing.inviteCode.inviting", { name: "Dana" }));
    const number = within(dialog).getByRole("group", { name: t("sharing.members.safety") });
    expect(groupsOf(number)).toHaveLength(6);
    expect(within(dialog).getByRole("group", { name: t("pair.digitsLabel") })).toHaveAccessibleDescription(t("sharing.pair.warning"));
    await user.click(within(dialog).getByRole("button", { name: t("pair.confirm") }));

    expect(
      await within(dialog).findByText(t("sharing.inviteCode.done", { name: "Dana", role: roleLabel("viewer") })),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    await waitFor(() => {
      expect(memberNames(panel)).toContain("Dana");
    });
    expect(within(memberRow(panel, "Dana")).getByText(t("sharing.members.verified"))).toBeInTheDocument();
    expect(within(memberRow(panel, "Dana")).getByRole("combobox", { name: t("sharing.roleFor", { name: "Dana" }) })).toHaveValue("viewer");
    // The invitation is in the list as used.
    const list = within(panel).getByRole("list", { name: t("sharing.invites.title") });
    expect(within(list).getByText(t("sharing.invites.status.used"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.usedBy", { name: "Dana" }))).toBeInTheDocument();
    expect((await client.inviteStatus()).state).toBe("Done");
  });

  it("only confirms once the other person did: until then there is no Confirm button", async () => {
    const { user, dialog } = await openInvite();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") }));
    await within(dialog).findByRole("group", { name: t("sharing.codeLabel") });
    expect(within(dialog).queryByRole("button", { name: t("pair.confirm") })).not.toBeInTheDocument();
    await within(dialog).findByRole("button", { name: t("pair.confirm") });
  });

  it("is cancelled by Cancel, and by closing the dialog, and then it can start again", async () => {
    const { user, dialog, client } = await openInvite();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") }));
    await within(dialog).findByRole("group", { name: t("sharing.codeLabel") });
    await user.click(within(dialog).getByRole("button", { name: t("pair.cancel") }));
    expect(await within(dialog).findByText(t("sharing.inviteCode.cancelled"))).toBeInTheDocument();
    expect((await client.inviteStatus()).state).toBe("Cancelled");
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") }));
    await within(dialog).findByRole("group", { name: t("sharing.codeLabel") });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("sharing.inviteCode.title") })).not.toBeInTheDocument();
    });
    expect((await client.inviteStatus()).state).toBe("Cancelled");
  });

  it("shows why it could not start", async () => {
    const { user, dialog, memory } = await openInvite();
    memory.failNext({ code: "Internal", message: "boom" });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") }));
    expect(await within(dialog).findByText(t("sharing.inviteCode.error.start", { message: "boom" }))).toBeInTheDocument();
    // The form is back, ready for another try.
    expect(within(dialog).getByRole("button", { name: t("sharing.inviteCode.start") })).toBeEnabled();
  });
});

describe("an invitation by link, the Owner's side", () => {
  async function openLink() {
    const app = await renderSharing();
    await openSpace(app.user, "Work");
    const panel = await openMembers(app.user);
    await app.user.click(within(panel).getByRole("button", { name: t("sharing.invites.link") }));
    const dialog = await screen.findByRole("dialog", { name: t("sharing.inviteLink.title") });
    return { ...app, panel, dialog };
  }

  it("creates a link for a role and a lifetime, shows it once with Copy, and lists the invitation", async () => {
    const { user, panel, dialog, memory } = await openLink();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    expect(within(dialog).getByText(t("sharing.inviteLink.intro"))).toBeInTheDocument();
    // Never Owner: the roles offered are the three below it.
    const role = within(dialog).getByRole("combobox", { name: t("sharing.invites.roleLabel") });
    expect(within(role).getAllByRole("option").map((option) => option.textContent)).toEqual([
      roleLabel("editor"),
      roleLabel("commenter"),
      roleLabel("viewer"),
    ]);
    await user.selectOptions(role, "commenter");
    expect(within(dialog).getByRole("combobox", { name: t("sharing.inviteLink.expiry") })).toHaveValue("7");
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("sharing.inviteLink.expiry") }), "30");
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteLink.create") }));

    expect(await within(dialog).findByText(t("sharing.inviteLink.once"))).toBeInTheDocument();
    const box = within(dialog).getByRole("textbox", { name: t("sharing.inviteLink.linkLabel") });
    const link = (box as HTMLInputElement).value;
    expect(link).toMatch(/^atomik-kb:\/\/invite\?server=https%3A%2F%2Fsync\.example\.com#[0-9A-Z]{52}$/u);
    expect(within(dialog).getByRole<HTMLInputElement>("textbox", { name: t("sharing.inviteLink.webLabel") }).value).toMatch(
      /^https:\/\/sync\.example\.com\/join#[0-9A-Z]{52}$/u,
    );
    expect(box).toHaveAttribute("readonly");
    const [copyLink] = within(dialog).getAllByRole("button", { name: t("sharing.inviteLink.copy") });
    await user.click(copyLink as HTMLElement);
    expect(writeText).toHaveBeenCalledWith(link);
    expect(await within(dialog).findByRole("button", { name: t("sharing.inviteLink.copied") })).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    // The invitation is listed; the link itself is gone for good.
    const list = await within(panel).findByRole("list", { name: t("sharing.invites.title") });
    expect(within(list).getByText(t("sharing.invites.kind.link"))).toBeInTheDocument();
    expect(within(list).getByText(roleLabel("commenter"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.status.open"))).toBeInTheDocument();
    expect(document.body.textContent).not.toContain(link.split("#")[1]);

    // The first person who opens it joins with that role, unverified; the invitation is then used up.
    act(() => {
      memory.receiveSharing({ type: "joinByLink", link, name: "Eve" });
    });
    await waitFor(() => {
      expect(memberNames(panel)).toContain("Eve");
    });
    expect(within(memberRow(panel, "Eve")).getByText(t("sharing.members.unverified"))).toBeInTheDocument();
    expect(within(list).getByText(t("sharing.invites.usedBy", { name: "Eve" }))).toBeInTheDocument();
    expect(() => memory.receiveSharing({ type: "joinByLink", link, name: "Frank" })).toThrow(/invalid, expired or already used/u);
  });

  it("revokes an invitation: the link no longer works", async () => {
    const { user, panel, dialog, memory } = await openLink();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteLink.create") }));
    const box = await within(dialog).findByRole("textbox", { name: t("sharing.inviteLink.linkLabel") });
    const link = (box as HTMLInputElement).value;
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    const list = await within(panel).findByRole("list", { name: t("sharing.invites.title") });
    await user.click(within(list).getByRole("button", { name: t("sharing.invites.revoke") }));
    expect(await within(list).findByText(t("sharing.invites.status.revoked"))).toBeInTheDocument();
    expect(() => memory.receiveSharing({ type: "joinByLink", link, name: "Eve" })).toThrow(/invalid, expired or already used/u);
  });

  it("shows why a link could not be made", async () => {
    const { user, dialog, memory } = await openLink();
    memory.failNext({ code: "Forbidden", message: "this needs the owner role in the space" });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.inviteLink.create") }));
    expect(
      await within(dialog).findByText(t("sharing.inviteLink.error", { message: "this needs the owner role in the space" })),
    ).toBeInTheDocument();
  });
});

describe("joining a space with a code", () => {
  async function openJoin(options: Parameters<typeof renderSharing>[0] = {}) {
    const app = await renderSharing({ role: "personal", ...options });
    await app.user.click(screen.getByRole("button", { description: t("spaces.switchTitle") }));
    await app.user.click(await screen.findByRole("button", { name: t("sharing.menu.join") }));
    const dialog = await screen.findByRole("dialog", { name: t("sharing.join.title") });
    return { ...app, dialog };
  }

  async function fill(user: Awaited<ReturnType<typeof openJoin>>["user"], dialog: HTMLElement, server: string, code: string) {
    const serverField = within(dialog).getByRole("textbox", { name: t("sharing.join.serverLabel") });
    const codeField = within(dialog).getByRole("textbox", { name: t("sharing.join.codeLabel") });
    await user.clear(serverField);
    await user.clear(codeField);
    if (server !== "") {
      await user.type(serverField, server);
    }
    if (code !== "") {
      await user.type(codeField, code);
    }
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.submit") }));
  }

  it("walks the server, the code, the six digits, then receives the space and opens it", async () => {
    const { user, dialog, client } = await openJoin();
    expect(within(dialog).getByText(t("sharing.join.intro"))).toBeInTheDocument();
    await fill(user, dialog, SERVER, "7kqm-x2tv-r9hd");
    const digits = await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    expect(groupsOf(digits)).toEqual([MEMORY_INVITE_SAS.slice(0, 3), MEMORY_INVITE_SAS.slice(3)]);
    expect(dialog).toHaveTextContent(t("sharing.join.digitsIntro"));
    await user.click(within(dialog).getByRole("button", { name: t("pair.confirm") }));
    expect(
      await within(dialog).findByText(t("sharing.join.joiningSpace", { space: "Project X", role: roleLabel("editor") })),
    ).toBeInTheDocument();
    expect(await within(dialog).findByText(t("sharing.join.done", { space: "Project X" }), undefined, { timeout: 10_000 })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.open") }));
    await waitFor(() => {
      expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Project X");
    });
    expect(await screen.findByRole("treeitem", { name: "Welcome" })).toBeInTheDocument();
    const joined = (await client.spaceList()).find((space) => space.name === "Project X");
    expect(joined).toMatchObject({ shareState: "Shared", role: "editor" });
    const info = await client.spaceMembers(joined?.id ?? "");
    // The code flow verified the inviter.
    expect(info.members.find((member) => member.name === "Alice")?.verified).toBe(true);
  });

  it("waits for the row of the space when Open the space is pressed before the row has arrived, and opens it then", async () => {
    // Found by the real-app pass (drive-m13-share): the invite is done when the Join is posted, the space itself comes through the feed
    // a moment later. Pressed in between, the button closed the dialog and opened nothing.
    let hold = true;
    const { user, dialog, client } = await openJoin({
      wrap: (memory) =>
        new Proxy<Transport>(memory, {
          get(target, property) {
            if (property === "dispatch") {
              return async (request: Request): Promise<Response> => {
                const response = await target.dispatch(request);
                if (hold && request.type === "SpaceList" && response.type === "Spaces") {
                  return { ...response, data: response.data.filter((space) => space.name !== "Project X") };
                }
                return response;
              };
            }
            const value: unknown = Reflect.get(target, property);
            return typeof value === "function" ? (value as (...args: unknown[]) => unknown).bind(target) : value;
          },
        }),
    });
    await fill(user, dialog, SERVER, "7kqm-x2tv-r9hd");
    await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    await user.click(within(dialog).getByRole("button", { name: t("pair.confirm") }));
    expect(await within(dialog).findByText(t("sharing.join.done", { space: "Project X" }), undefined, { timeout: 10_000 })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.open") }));
    // The row is not in the list: the dialog stays and says it is opening.
    expect(await within(dialog).findByRole("button", { name: t("sharing.join.opening") })).toBeDisabled();
    expect(screen.getByRole("dialog", { name: t("sharing.join.title") })).toBeInTheDocument();
    expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).not.toHaveTextContent("Project X");
    // The row arrives with the core's next SpacesChanged: the space opens and the dialog goes.
    hold = false;
    await client.spaceCreate("Another");
    await waitFor(() => {
      expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Project X");
    });
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("sharing.join.title") })).not.toBeInTheDocument();
    });
  });

  it("asks for what is missing before it connects", async () => {
    const { user, dialog } = await openJoin();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.join.error.emptyServer"));
    await user.type(within(dialog).getByRole("textbox", { name: t("sharing.join.serverLabel") }), SERVER);
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.join.error.emptyCode"));
    await user.type(within(dialog).getByRole("textbox", { name: t("sharing.join.codeLabel") }), "ABC");
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.submit") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.join.error.code"));
  });

  it("says when the server address is not one", async () => {
    const { user, dialog } = await openJoin();
    await fill(user, dialog, "ftp://nope", MEMORY_INVITE_CODE);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/^This address cannot be used: /u);
  });

  it("says to create the recovery kit first when there is no account yet", async () => {
    const { user, dialog } = await openJoin({ memory: { security: undefined } });
    await fill(user, dialog, SERVER, MEMORY_INVITE_CODE);
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.join.error.account"));
  });

  it("ends with the reason when the code is dead, and lets the person try another", async () => {
    const { user, dialog } = await openJoin();
    await fill(user, dialog, SERVER, EXPIRED_INVITE_CODE);
    expect(
      await within(dialog).findByText(t("sharing.join.failed", { message: t("sharing.error.dead") })),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("textbox", { name: t("sharing.join.codeLabel") })).toBeInTheDocument();
  });

  it("cancels: nothing is joined, and closing the dialog cancels a session under way", async () => {
    const { user, dialog, client } = await openJoin();
    await fill(user, dialog, SERVER, MEMORY_INVITE_CODE);
    await within(dialog).findByRole("group", { name: t("pair.digitsLabel") });
    await user.click(within(dialog).getByRole("button", { name: t("pair.cancel") }));
    expect(await within(dialog).findByText(t("sharing.join.cancelled"))).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal", "Work"]);
  });
});

describe("the preview of an invitation link", () => {
  const INVITE_LINK: ShellEvent = { type: "inviteLink", data: { link: LINK } } as unknown as ShellEvent;

  /** The app over a core with a shell around it, which delivers links as the desktop shell does. */
  async function withShellLinks(held: readonly ShellEvent[] = [], options: Parameters<typeof renderSharing>[0] = {}) {
    let shell: ShellTransport | undefined;
    const app = await renderSharing({
      role: "personal",
      wrap: (memory) => {
        shell = withShell(memory, { held });
        return shell.transport;
      },
      ...options,
    });
    if (shell === undefined) {
      throw new Error("no shell");
    }
    return { ...app, shell };
  }

  const preview = (): Promise<HTMLElement> => screen.findByRole("dialog", { name: t("sharing.preview.title") });

  it("shows who invites you, to what, as which role, that you receive the whole history and that it is not verified", async () => {
    const { shell } = await withShellLinks();
    act(() => {
      shell.emit(INVITE_LINK);
    });
    const dialog = await preview();
    expect(await within(dialog).findByText(t("sharing.preview.invites", { inviter: "Alice", space: "Project X", role: roleLabel("editor") }))).toBeInTheDocument();
    expect(dialog).toHaveTextContent(t("sharing.preview.history"));
    expect(within(dialog).getByRole("note")).toHaveTextContent(t("sharing.preview.unverified"));
    // The link carries a secret: it is nowhere on screen.
    expect(document.body.textContent).not.toContain(SECRET);
  });

  it("joins on Join the space, receives the space, and opens it", async () => {
    const { user, shell, client } = await withShellLinks();
    act(() => {
      shell.emit(INVITE_LINK);
    });
    const dialog = await preview();
    await user.click(await within(dialog).findByRole("button", { name: t("sharing.preview.accept") }));
    expect(await within(dialog).findByText(t("sharing.join.done", { space: "Project X" }), undefined, { timeout: 10_000 })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("sharing.join.open") }));
    await waitFor(() => {
      expect(screen.getByRole("button", { description: t("spaces.switchTitle") })).toHaveTextContent("Project X");
    });
    const joined = (await client.spaceList()).find((space) => space.name === "Project X");
    // A link does not verify anyone: the inviter is still unverified.
    expect((await client.spaceMembers(joined?.id ?? "")).members.find((member) => member.name === "Alice")?.verified).toBe(false);
  });

  it("declines: nothing is joined and the dialog goes", async () => {
    const { user, shell, client } = await withShellLinks();
    act(() => {
      shell.emit(INVITE_LINK);
    });
    const dialog = await preview();
    await within(dialog).findByRole("button", { name: t("sharing.preview.accept") });
    await user.click(within(dialog).getByRole("button", { name: t("sharing.preview.decline") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect((await client.inviteStatus()).state).toBe("Cancelled");
    expect((await client.spaceList()).map((space) => space.name)).toEqual(["Personal", "Work"]);
  });

  it("says that a dead invitation is invalid, expired or already used, whatever made it dead", async () => {
    const { shell } = await withShellLinks();
    act(() => {
      shell.emit({ type: "inviteLink", data: { link: `atomik-kb://invite?server=${encodeURIComponent(SERVER)}#${DEAD_LINK_SECRET}` } } as unknown as ShellEvent);
    });
    const dialog = await preview();
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(t("sharing.preview.failed", { message: t("sharing.error.dead") }));
    expect(within(dialog).queryByRole("button", { name: t("sharing.preview.accept") })).not.toBeInTheDocument();
  });

  it("is used once: the same link a second time is dead", async () => {
    const { user, shell } = await withShellLinks();
    act(() => {
      shell.emit(INVITE_LINK);
    });
    const dialog = await preview();
    await user.click(await within(dialog).findByRole("button", { name: t("sharing.preview.accept") }));
    await within(dialog).findByText(t("sharing.join.done", { space: "Project X" }), undefined, { timeout: 10_000 });
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    act(() => {
      shell.emit(INVITE_LINK);
    });
    const again = await preview();
    expect(await within(again).findByRole("alert")).toHaveTextContent(t("sharing.error.dead"));
  });

  it("says only that the link could not be opened when the core refuses it, without showing the link", async () => {
    const { shell } = await withShellLinks();
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    act(() => {
      shell.emit({ type: "inviteLink", data: { link: "atomik-kb://invite?server=x#SECRETSECRET" } } as unknown as ShellEvent);
    });
    await within(toasts()).findByText(t("sharing.preview.openError"));
    expect(screen.queryByRole("dialog", { name: t("sharing.preview.title") })).not.toBeInTheDocument();
    for (const call of vi.mocked(console.error).mock.calls) {
      expect(JSON.stringify(call)).not.toContain("SECRETSECRET");
    }
  });

  it("is held while the app opens, and shown when the workspace is there", async () => {
    await withShellLinks([INVITE_LINK]);
    const dialog = await preview();
    expect(await within(dialog).findByText(t("sharing.preview.invites", { inviter: "Alice", space: "Project X", role: roleLabel("editor") }))).toBeInTheDocument();
  });

  it("is dropped by a first-run screen: there is no workspace to join with", async () => {
    const memory = new MemoryTransport({ now: () => NOW });
    const shell = withShell(memory, { held: [INVITE_LINK] });
    renderWith(shell.transport);
    await screen.findByRole("region", { name: t("onboarding.title") });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    // The link was dropped, not kept for later: no session was started.
    expect((await new CoreClient(memory).inviteStatus()).state).toBe("Idle");
  });

  it("is not a page link: the page links of the shell still open pages", async () => {
    const { shell } = await withShellLinks();
    act(() => {
      shell.emit({ type: "deepLink", data: { pageId: "99999999-0000-4000-8000-000000000009" } });
    });
    await within(toasts()).findByText(t("deepLink.unknown"));
    expect(screen.queryByRole("dialog", { name: t("sharing.preview.title") })).not.toBeInTheDocument();
  });
});
