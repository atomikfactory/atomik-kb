// `CoreClient`'s shared-space requests (Milestone 13, slice 1 contract): what each method sends, what
// it returns, and that the contract's `Forbidden` error and the in-memory mirror answer as documented. What the
// mirror enforces (roles, invites, archives) is in `memory-sharing.test.ts`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError, isApiError } from "./errors";
import type { InviteLinkInfo } from "./generated/InviteLinkInfo";
import type { InviteSessionInfo } from "./generated/InviteSessionInfo";
import type { MembersInfo } from "./generated/MembersInfo";
import type { PersonInfo } from "./generated/PersonInfo";
import type { ProfileInfo } from "./generated/ProfileInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { SpaceSharePlan } from "./generated/SpaceSharePlan";
import { MemoryTransport } from "./memory";
import type { DocTransport, Transport } from "./transport";

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response | Error) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return response instanceof Error ? Promise.reject(response) : Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
  };
  return { requests, client: new CoreClient(transport) };
}

const SPACE = "11111111-1111-4111-8111-111111111111";
const ACCOUNT = "22222222-2222-4222-8222-222222222222";
const INVITE = "33333333-3333-4333-8333-333333333333";
const FEED = "44444444-4444-4444-8444-444444444444";

const space: SpaceInfo = {
  id: SPACE,
  name: "Project X",
  createdAt: 1_700_000_000_000,
  pageCount: 3,
  shareState: "Shared",
  role: "owner",
  memberCount: 2,
};

const members: MembersInfo = {
  spaceId: SPACE,
  feedId: FEED,
  myRole: "owner",
  epoch: 1,
  pendingRekey: false,
  members: [
    {
      accountId: ACCOUNT,
      name: "Bob",
      role: "editor",
      joinedAt: 1_700_000_000_000,
      isMe: false,
      verified: false,
      safetyNumber: "12345 67890 12345 67890 12345 67890",
      deviceCount: 1,
      refusedChanges: 0,
      nameShared: false,
    },
  ],
  formerMembers: [{ accountId: "33333333-3333-4333-8333-333333333333", name: "Dan" }],
  invites: [],
  settings: { trashRetentionDays: 30, editorsMayLink: false },
  disputes: [],
  refusedTotal: 0,
  serverFlagged: false,
};

const session: InviteSessionInfo = {
  flow: "Code",
  side: "Inviter",
  state: "Waiting",
  code: "ABCD EFGH JKMN",
  verified: false,
};

describe("CoreClient shared spaces", () => {
  it("sends the plan, share, members, role, removal, leave, close, settings and AI requests", async () => {
    const plan: SpaceSharePlan = {
      spaceId: SPACE,
      pageCount: 3,
      trashedPageCount: 1,
      attachmentCount: 0,
      attachmentBytes: 0,
      historyBytes: 1024,
      serverConfigured: true,
      hasAccount: true,
      hasProfileName: true,
      alreadyShared: false,
    };
    const planned = stubTransport({ type: "SpaceSharePlan", data: plan });
    expect(await planned.client.spaceSharePlan(SPACE)).toEqual(plan);
    expect(planned.requests).toEqual([{ type: "SpaceSharePlan", data: { spaceId: SPACE } }]);

    const shared = stubTransport({ type: "Space", data: { ...space, shareState: "Pending" } });
    expect((await shared.client.spaceShare(SPACE)).shareState).toBe("Pending");
    expect(shared.requests).toEqual([{ type: "SpaceShare", data: { spaceId: SPACE } }]);

    const listed = stubTransport({ type: "Members", data: members });
    expect(await listed.client.spaceMembers(SPACE)).toEqual(members);
    expect(await listed.client.memberRoleSet(SPACE, ACCOUNT, "viewer")).toEqual(members);
    expect(await listed.client.memberRemove(SPACE, ACCOUNT)).toEqual(members);
    expect(await listed.client.inviteRevoke(SPACE, INVITE)).toEqual(members);
    expect(listed.requests).toEqual([
      { type: "SpaceMembers", data: { spaceId: SPACE } },
      { type: "MemberRoleSet", data: { spaceId: SPACE, accountId: ACCOUNT, role: "viewer" } },
      { type: "MemberRemove", data: { spaceId: SPACE, accountId: ACCOUNT } },
      { type: "InviteRevoke", data: { spaceId: SPACE, inviteId: INVITE } },
    ]);

    const left = stubTransport({ type: "Ok" });
    await left.client.spaceLeave(SPACE);
    expect(left.requests).toEqual([{ type: "SpaceLeave", data: { spaceId: SPACE } }]);

    const closed = stubTransport({ type: "Space", data: { ...space, shareState: "Archived", archiveReason: "Closed" } });
    expect((await closed.client.spaceClose(SPACE)).archiveReason).toBe("Closed");
    expect(await closed.client.spaceAiSet(SPACE, true)).toMatchObject({ shareState: "Archived" });
    expect(closed.requests).toEqual([
      { type: "SpaceClose", data: { spaceId: SPACE } },
      { type: "SpaceAiSet", data: { spaceId: SPACE, enabled: true } },
    ]);
  });

  it("sends only the settings that are named", async () => {
    const { client, requests } = stubTransport({ type: "Members", data: members });
    await client.spaceSettingsSet(SPACE, { trashRetentionDays: 0 });
    await client.spaceSettingsSet(SPACE, { editorsMayLink: true });
    await client.spaceSettingsSet(SPACE, { trashRetentionDays: 7, editorsMayLink: false });
    await client.spaceSettingsSet(SPACE, {});
    expect(requests).toEqual([
      { type: "SpaceSettingsSet", data: { spaceId: SPACE, trashRetentionDays: 0 } },
      { type: "SpaceSettingsSet", data: { spaceId: SPACE, editorsMayLink: true } },
      { type: "SpaceSettingsSet", data: { spaceId: SPACE, trashRetentionDays: 7, editorsMayLink: false } },
      { type: "SpaceSettingsSet", data: { spaceId: SPACE } },
    ]);
  });

  it("sends the profile and the people requests", async () => {
    const profile: ProfileInfo = { displayName: "Alice" };
    const profiles = stubTransport({ type: "Profile", data: profile });
    expect(await profiles.client.profileGet()).toEqual(profile);
    expect(await profiles.client.profileSet("Alice")).toEqual(profile);
    expect(profiles.requests).toEqual([
      { type: "ProfileGet" },
      { type: "ProfileSet", data: { displayName: "Alice" } },
    ]);

    const person: PersonInfo = { accountId: ACCOUNT, name: "Bob", verified: true, isMe: false };
    const people = stubTransport({ type: "Person", data: person });
    expect(await people.client.personGet(ACCOUNT)).toEqual(person);
    expect(await people.client.personVerifiedSet(ACCOUNT, true)).toEqual(person);
    expect(people.requests).toEqual([
      { type: "PersonGet", data: { accountId: ACCOUNT } },
      { type: "PersonVerifiedSet", data: { accountId: ACCOUNT, verified: true } },
    ]);
  });

  it("sends the invite requests of both flows", async () => {
    const link: InviteLinkInfo = {
      inviteId: INVITE,
      link: "atomik-kb://invite?server=https%3A%2F%2Fkb.example#ABCD",
      webLink: "https://kb.example/join#ABCD",
      role: "viewer",
      expiresAt: 1_700_000_000_000 + 7 * 86_400_000,
    };
    const links = stubTransport({ type: "InviteLink", data: link });
    expect(await links.client.inviteLinkCreate(SPACE, "viewer")).toEqual(link);
    expect(await links.client.inviteLinkCreate(SPACE, "editor", 30)).toEqual(link);
    expect(links.requests).toEqual([
      { type: "InviteLinkCreate", data: { spaceId: SPACE, role: "viewer" } },
      { type: "InviteLinkCreate", data: { spaceId: SPACE, role: "editor", expiresInDays: 30 } },
    ]);

    const sessions = stubTransport({ type: "Invite", data: session });
    expect(await sessions.client.inviteCodeStart(SPACE, "editor")).toEqual(session);
    expect(await sessions.client.inviteCodeJoin("https://kb.example", "ABCD EFGH JKMN")).toEqual(session);
    expect(await sessions.client.inviteLinkOpen("atomik-kb://invite?server=x#ABCD")).toEqual(session);
    expect(await sessions.client.inviteConfirm()).toEqual(session);
    expect(await sessions.client.inviteCancel()).toEqual(session);
    expect(await sessions.client.inviteStatus()).toEqual(session);
    expect(sessions.requests).toEqual([
      { type: "InviteCodeStart", data: { spaceId: SPACE, role: "editor" } },
      { type: "InviteCodeJoin", data: { serverUrl: "https://kb.example", code: "ABCD EFGH JKMN" } },
      { type: "InviteLinkOpen", data: { link: "atomik-kb://invite?server=x#ABCD" } },
      { type: "InviteConfirm" },
      { type: "InviteCancel" },
      { type: "InviteStatus" },
    ]);
  });

  it("surfaces a response of the wrong kind as Internal and keeps Forbidden with its details", async () => {
    const wrong = stubTransport({ type: "Ok" });
    const mismatch = await wrong.client.spaceMembers(SPACE).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(mismatch).toBeInstanceOf(CoreError);
    expect((mismatch as CoreError).code).toBe("Internal");

    const forbidden = {
      code: "Forbidden",
      message: "this needs the editor role in the space",
      details: { reason: "role", required: "editor" },
    } as const;
    expect(isApiError(forbidden)).toBe(true);
    const refused = stubTransport(new CoreError(forbidden));
    const error = await refused.client.spaceLeave(SPACE).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("Forbidden");
    expect((error as CoreError).details).toEqual({ reason: "role", required: "editor" });
  });

  it("is answered by the in-memory mirror: a personal space has no members, a shared one has", async () => {
    const memory = new MemoryTransport({
      workspaceName: "Home",
      spaces: [{ name: "Work", pages: [{ title: "Plan" }] }],
      sharing: { spaces: [{ space: "Work" }] },
    });
    const client = new CoreClient(memory);
    const [work] = await client.spaceList();
    expect(work).toMatchObject({ name: "Work", shareState: "Shared", role: "owner", memberCount: 4, aiEnabled: false });
    const listed = await client.spaceMembers(work?.id ?? "");
    expect(listed.myRole).toBe("owner");
    expect(listed.members.map((member) => member.role)).toEqual(["owner", "editor", "commenter", "viewer"]);
    // A space this account never shared is personal: `InvalidInput` for `spaceId`.
    const created = await client.spaceCreate("Home base");
    const error = await client.spaceMembers(created.id).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("InvalidInput");
    expect((error as CoreError).details).toEqual({ field: "spaceId" });
  });
});
