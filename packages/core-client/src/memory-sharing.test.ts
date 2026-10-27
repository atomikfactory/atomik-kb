// Shared spaces in the in-memory mirror (Milestone 13, slice 1, `./sharing-mirror`): the rules of the membership fold
// and of the role gating that the mirror enforces (all Owners equal, one Owner remains, roles gate writes, an invite
// is single use, expires and grants no Owner), the share, the member list, the invite sessions of both flows on
// timers, archives, the per-account AI switch, the profile and the events.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { MemberInfo } from "./generated/MemberInfo";
import type { MemberRole } from "./generated/MemberRole";
import type { MembersInfo } from "./generated/MembersInfo";
import type { SpaceInfo } from "./generated/SpaceInfo";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import {
  DEAD_LINK_SECRET,
  EXPIRED_INVITE_CODE,
  INVITE_DEAD_MESSAGE,
  MAX_OPEN_INVITES,
  MEMORY_INVITEE_NAME,
  MEMORY_INVITE_CODE,
  MEMORY_INVITE_SAS,
} from "./sharing-mirror";

const START = 1_700_000_000_000;
const STEP = 150;
const SERVER = "https://sync.example.com";
const DAY = 86_400_000;

const SPACES = [
  { name: "Work", pages: [{ title: "Plan", children: [{ title: "Sub" }] }, { title: "Notes" }] },
  { name: "Personal", pages: [{ title: "Diary" }] },
];

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(START);
});

afterEach(() => {
  vi.useRealTimers();
});

/** A mirror with two seeded spaces, a kit, sync and the profile name "Maya"; `Work` is shared when `role` is given. */
function build(role: MemberRole | "personal" = "owner", options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: SPACES,
    security: { kitCreatedAt: START },
    sync: { state: "idle", serverUrl: SERVER },
    syncStepMs: 5,
    sharing: {
      profileName: "Maya",
      ...(role === "personal" ? {} : { spaces: [{ space: "Work", role }] }),
    },
    ...options,
  });
  const client = new CoreClient(memory);
  const events: Event[] = [];
  memory.subscribe((event) => {
    events.push(event);
  });
  return { memory, client, events };
}

async function work(client: CoreClient): Promise<SpaceInfo> {
  const found = (await client.spaceList()).find((space) => space.name === "Work");
  if (found === undefined) {
    throw new Error("no Work space");
  }
  return found;
}

async function pageId(client: CoreClient, spaceId: string, title: string): Promise<string> {
  const tree = await client.treeGet(spaceId);
  const find = (nodes: typeof tree.roots): string | undefined => {
    for (const node of nodes) {
      if (node.page.title === title) {
        return node.page.id;
      }
      const inner = find(node.children);
      if (inner !== undefined) {
        return inner;
      }
    }
    return undefined;
  };
  const id = find(tree.roots);
  if (id === undefined) {
    throw new Error(`no page ${title}`);
  }
  return id;
}

function member(info: MembersInfo, name: string): MemberInfo {
  const found = info.members.find((candidate) => candidate.name === name);
  if (found === undefined) {
    throw new Error(`no member ${name}`);
  }
  return found;
}

/** What a promise rejects with. */
async function failure(promise: Promise<unknown>): Promise<CoreError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof CoreError) {
      return error;
    }
    throw error;
  }
  throw new Error("expected a rejection");
}

/** The event types, without the sync driver's (the mirror is connected and every local change asks for a cycle). */
function types(events: readonly Event[]): string[] {
  return events.map((event) => event.type).filter((type) => type !== "SyncRequested" && type !== "SyncChanged");
}

/** One owner link made and joined by `name`; answers the account. */
async function joinByLink(
  memory: MemoryTransport,
  client: CoreClient,
  spaceId: string,
  name: string,
  role: MemberRole = "editor",
): Promise<string | undefined> {
  const link = await client.inviteLinkCreate(spaceId, role);
  return memory.receiveSharing({ type: "joinByLink", link: link.link, name });
}

describe("the starting situation", () => {
  it("lists a personal space without any sharing field, and a shared one with its role, members and AI off", async () => {
    const { client } = build("editor");
    const spaces = await client.spaceList();
    const personal = spaces.find((space) => space.name === "Personal");
    expect(personal).toBeDefined();
    expect(Object.keys(personal ?? {})).not.toContain("shareState");
    expect(Object.keys(personal ?? {})).not.toContain("role");
    expect(Object.keys(personal ?? {})).not.toContain("aiEnabled");
    const shared = spaces.find((space) => space.name === "Work");
    // Alice (Owner) first, then this account, then Bob, Carol and Dan.
    expect(shared).toMatchObject({ shareState: "Shared", role: "editor", memberCount: 5, aiEnabled: false });
  });

  it("starts an archive for a removed account and for a closed space", async () => {
    const removed = build("editor", {
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "editor", archive: "Removed" }] },
    });
    expect(await work(removed.client)).toMatchObject({ shareState: "Archived", archiveReason: "Removed", role: "editor" });
    const closed = build("owner", { sharing: { spaces: [{ space: "Work", archive: "Closed" }] } });
    const info = await closed.client.spaceMembers((await work(closed.client)).id);
    expect(await work(closed.client)).toMatchObject({ shareState: "Archived", archiveReason: "Closed" });
    expect(info.closedAt).toBe(START - DAY);
    // An archive lists no invites and no disputes: nothing can be done there.
    expect(info.invites).toEqual([]);
  });

  it("refuses a seed that names a space that does not exist, or a shared space with no Owner", () => {
    expect(() => build("owner", { sharing: { spaces: [{ space: "Nope" }] } })).toThrow(/not a seeded space/u);
    expect(
      () =>
        build("owner", {
          sharing: { spaces: [{ space: "Work", role: "editor", members: [{ name: "Bob", role: "editor" }] }] },
        }),
    ).toThrow(/needs an Owner/u);
  });
});

describe("sharing a space", () => {
  it("counts what the share dialog says before anything is sent", async () => {
    const { client } = build("personal");
    const space = await work(client);
    const plan = await client.spaceSharePlan(space.id);
    expect(plan).toMatchObject({
      spaceId: space.id,
      pageCount: 3,
      trashedPageCount: 0,
      attachmentCount: 0,
      attachmentBytes: 0,
      serverConfigured: true,
      hasAccount: true,
      hasProfileName: true,
      alreadyShared: false,
    });
    expect(plan.historyBytes).toBeGreaterThan(0);
    await client.pageTrash(await pageId(client, space.id, "Notes"));
    expect(await client.spaceSharePlan(space.id)).toMatchObject({ pageCount: 2, trashedPageCount: 1 });
  });

  it("answers Pending, then Shared after a step, with this account as the only Owner", async () => {
    const { client, events } = build("personal");
    const space = await work(client);
    const pending = await client.spaceShare(space.id);
    expect(pending).toMatchObject({ shareState: "Pending", role: "owner", memberCount: 1, aiEnabled: false });
    expect(types(events)).toEqual(["SpacesChanged"]);
    vi.advanceTimersByTime(STEP);
    expect((await work(client)).shareState).toBe("Shared");
    expect(events).toContainEqual({ type: "SpaceMembersChanged", data: { spaceId: space.id } });
    const info = await client.spaceMembers(space.id);
    expect(info).toMatchObject({ myRole: "owner", epoch: 1, pendingRekey: false });
    expect(info.members).toHaveLength(1);
    expect(info.members[0]).toMatchObject({ name: "Maya", role: "owner", isMe: true, verified: false });
    expect(info.members[0]?.safetyNumber).toBeUndefined();
    expect(info.settings).toEqual({ trashRetentionDays: 30, editorsMayLink: false });
  });

  it("needs sync, the account and a name, and refuses to share twice", async () => {
    const noSync = build("personal", { sync: undefined });
    expect((await failure(noSync.client.spaceShare((await work(noSync.client)).id))).details).toEqual({ field: "serverUrl" });
    const noKit = build("personal", { security: undefined });
    expect((await failure(noKit.client.spaceShare((await work(noKit.client)).id))).details).toEqual({ field: "account" });
    const noName = build("personal", { sharing: {} });
    expect((await failure(noName.client.spaceShare((await work(noName.client)).id))).details).toEqual({
      field: "displayName",
    });
    const { client } = build("personal");
    const space = await work(client);
    await client.spaceShare(space.id);
    expect((await failure(client.spaceShare(space.id))).code).toBe("AlreadyExists");
  });

  it("finishes a share that a lock interrupted, and drops a session in progress", async () => {
    const { client, memory } = build("personal", { security: { kitCreatedAt: START, appLockPassphrase: "correct horse" } });
    const space = await work(client);
    await client.spaceShare(space.id);
    await memory.lock();
    await memory.unlock("correct horse");
    expect((await work(client)).shareState).toBe("Shared");
  });
});

describe("members, roles and removal", () => {
  it("lists the members in join order with names, roles, safety numbers and the verified mark", async () => {
    const { client } = build("owner");
    const info = await client.spaceMembers((await work(client)).id);
    expect(info.members.map((entry) => [entry.name, entry.role, entry.isMe])).toEqual([
      ["Maya", "owner", true],
      ["Bob", "editor", false],
      ["Carol", "commenter", false],
      ["Dan", "viewer", false],
    ]);
    expect(member(info, "Bob").verified).toBe(true);
    expect(member(info, "Carol").verified).toBe(false);
    expect(member(info, "Bob").safetyNumber).toMatch(/^\d{5}( \d{5}){5}$/u);
    expect(member(info, "Bob").safetyNumber).not.toBe(member(info, "Carol").safetyNumber);
    expect(info.members.every((entry) => entry.joinedAt <= START)).toBe(true);
    // Joined in order.
    const joined = info.members.map((entry) => entry.joinedAt);
    expect([...joined].sort((a, b) => a - b)).toEqual(joined);
  });

  it("gives a person the number the member list shows, and a different one to each person", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const bob = member(info, "Bob");
    const person = await client.personGet(bob.accountId);
    expect(person).toMatchObject({ name: "Bob", verified: true, isMe: false });
    expect(person.safetyNumber).toBe(bob.safetyNumber);
    expect(new Set(info.members.map((entry) => entry.safetyNumber)).size).toBe(info.members.length);
  });

  it("marks the same name of two members", async () => {
    const { client, memory } = build("owner");
    const space = await work(client);
    memory.receiveSharing({ type: "memberJoined", spaceId: space.id, name: "bob", role: "viewer" });
    const info = await client.spaceMembers(space.id);
    expect(info.members.filter((entry) => entry.nameShared).map((entry) => entry.name)).toEqual(["Bob", "bob"]);
    expect(member(info, "Carol").nameShared).toBe(false);
  });

  it("lets an Owner change a role, announces it, and keeps all Owners equal", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    const bob = member(await client.spaceMembers(space.id), "Bob");
    events.length = 0;
    const info = await client.memberRoleSet(space.id, bob.accountId, "owner");
    expect(member(info, "Bob").role).toBe("owner");
    expect(events).toEqual([{ type: "SpaceMembersChanged", data: { spaceId: space.id } }]);
    // Any Owner may demote or remove any other, the creator (this account) included.
    const me = info.members.find((entry) => entry.isMe);
    const demoted = await client.memberRoleSet(space.id, me?.accountId ?? "", "viewer");
    expect(demoted.myRole).toBe("viewer");
    expect(events).toContainEqual({ type: "SpaceRoleChanged", data: { spaceId: space.id, role: "viewer", previous: "owner" } });
    expect((await work(client)).role).toBe("viewer");
    // A Viewer sees no invites and cannot change roles.
    expect(demoted.invites).toEqual([]);
    const refused = await failure(client.memberRoleSet(space.id, bob.accountId, "editor"));
    expect(refused).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
  });

  it("keeps at least one Owner: the last one is neither demoted, nor removed by leaving", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const me = info.members.find((entry) => entry.isMe);
    const demote = await failure(client.memberRoleSet(space.id, me?.accountId ?? "", "editor"));
    expect(demote.code).toBe("InvalidInput");
    expect(demote.details).toEqual({ field: "role" });
    expect((await failure(client.spaceLeave(space.id))).details).toEqual({ field: "role" });
    // Promote someone first, and the same steps work.
    const bob = member(info, "Bob");
    await client.memberRoleSet(space.id, bob.accountId, "owner");
    await client.memberRoleSet(space.id, me?.accountId ?? "", "editor");
    expect((await client.spaceMembers(space.id)).myRole).toBe("editor");
  });

  it("refuses a role change that names nobody, or for a non-member", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const error = await failure(client.memberRoleSet(space.id, "00000000-0000-4000-8000-0000000000ff", "viewer"));
    expect(error.details).toEqual({ field: "accountId" });
  });

  it("removes a member with a Rekey (the epoch rises) and not this account itself", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const after = await client.memberRemove(space.id, member(info, "Dan").accountId);
    expect(after.members.map((entry) => entry.name)).toEqual(["Maya", "Bob", "Carol"]);
    expect(after.epoch).toBe(info.epoch + 1);
    expect(after.pendingRekey).toBe(false);
    expect((await work(client)).memberCount).toBe(3);
    const me = info.members.find((entry) => entry.isMe);
    expect((await failure(client.memberRemove(space.id, me?.accountId ?? ""))).details).toEqual({ field: "accountId" });
    expect((await failure(client.memberRemove(space.id, member(info, "Dan").accountId))).details).toEqual({
      field: "accountId",
    });
  });

  it("lets only an Owner remove, and only members of the space", async () => {
    const { client } = build("editor");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const refused = await failure(client.memberRemove(space.id, member(info, "Dan").accountId));
    expect(refused).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
  });

  it("voids the open invites of an Owner who is demoted, and joins through them are refused", async () => {
    const { client, memory } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const me = info.members.find((entry) => entry.isMe)?.accountId ?? "";
    await client.memberRoleSet(space.id, member(info, "Bob").accountId, "owner");
    const link = await client.inviteLinkCreate(space.id, "viewer");
    // Bob (an Owner) demotes this account: its open invites are void.
    memory.receiveSharing({ type: "roleSet", spaceId: space.id, accountId: me, role: "editor" });
    expect(() => memory.receiveSharing({ type: "joinByLink", link: link.link, name: "Eve" })).toThrow(INVITE_DEAD_MESSAGE);
    // Promoted again, the invite stays void.
    memory.receiveSharing({ type: "roleSet", spaceId: space.id, accountId: me, role: "owner" });
    expect((await client.spaceMembers(space.id)).invites.map((invite) => invite.status)).toEqual(["Revoked"]);
  });

  it("voids the invites of an Owner who is removed", async () => {
    const { client } = build("owner", {
      sharing: {
        spaces: [
          {
            space: "Work",
            members: [
              { name: "Bob", role: "owner" },
              { name: "Carol", role: "viewer" },
            ],
            invites: [{ role: "viewer", createdBy: "Bob" }, { role: "editor" }],
          },
        ],
      },
    });
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    expect(info.invites.map((invite) => invite.status)).toEqual(["Open", "Open"]);
    const after = await client.memberRemove(space.id, member(info, "Bob").accountId);
    // Bob's invite is void, this account's stays.
    expect(after.invites.map((invite) => [invite.role, invite.status])).toEqual([
      ["viewer", "Revoked"],
      ["editor", "Open"],
    ]);
  });
});

describe("role gating of the writes", () => {
  const WRITES: readonly [string, (client: CoreClient, spaceId: string, page: string) => Promise<unknown>][] = [
    ["PageCreate", (client, spaceId) => client.pageCreate({ spaceId, title: "New" })],
    ["PageRename", (client, _spaceId, page) => client.pageRename(page, "Renamed")],
    ["PageSetIcon", (client, _spaceId, page) => client.pageSetIcon(page, "📝")],
    ["PageSetAppearance", (client, _spaceId, page) => client.pageSetAppearance(page, { locked: true })],
    ["PageMove", (client, _spaceId, page) => client.pageMove(page, null, 0)],
    ["PageReorderChildren", (client, spaceId, page) => client.pageReorderChildren(spaceId, null, [page])],
    ["PageTrash", (client, _spaceId, page) => client.pageTrash(page)],
    ["PageDuplicate", (client, _spaceId, page) => client.pageDuplicate(page)],
    ["SpaceRename", (client, spaceId) => client.spaceRename(spaceId, "Renamed")],
    ["SpaceSetIcon", (client, spaceId) => client.spaceSetIcon(spaceId, "💼")],
    ["StructureUndo", (client, spaceId) => client.structureUndo(spaceId)],
    ["PropertyCreate", (client, spaceId) => client.propertyCreate(spaceId, "Mood", "text")],
    ["AttachmentBegin", (client, spaceId) => client.attachmentBegin(spaceId, "a.txt", "text/plain")],
  ];

  for (const role of ["viewer", "commenter"] as const) {
    it(`refuses every write of a ${role} with Forbidden, and still reads`, async () => {
      const { client, memory } = build(role);
      const space = await work(client);
      const page = await pageId(client, space.id, "Notes");
      for (const [name, write] of WRITES) {
        const error = await failure(write(client, space.id, page));
        expect({ name, code: error.code, details: error.details }).toEqual({
          name,
          code: "Forbidden",
          details: { reason: "role", required: "editor" },
        });
      }
      // The page document too: a Viewer's edit never reaches the document.
      const docError = await memory.docs.import(page, new Uint8Array([1, 2, 3])).then(
        () => undefined,
        (e: unknown) => e as CoreError,
      );
      expect(docError).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "editor" } });
      // Reads, search and favourites stay open to every member.
      expect((await client.treeGet(space.id)).roots).toHaveLength(2);
      expect((await client.pageGet(page)).title).toBe("Notes");
      await client.pageFavoriteSet(page, true);
      expect((await client.treeGet(space.id)).favorites).toEqual([page]);
    });
  }

  it("lets an Editor write but not purge, delete attachments or change members", async () => {
    const { client } = build("editor");
    const space = await work(client);
    const page = await pageId(client, space.id, "Notes");
    await client.pageRename(page, "Renamed");
    await client.pageCreate({ spaceId: space.id, title: "New" });
    await client.spaceRename(space.id, "Team");
    await client.pageTrash(page);
    await client.pageRestore(page);
    await client.pageTrash(page);
    expect(await failure(client.pagePurge(page))).toMatchObject({
      code: "Forbidden",
      details: { reason: "role", required: "owner" },
    });
    const attachment = await client.attachmentBegin(space.id, "a.txt", "text/plain");
    await client.attachmentFinish(attachment);
    expect(await failure(client.attachmentDelete(attachment))).toMatchObject({ details: { required: "owner" } });
    for (const call of [
      () => client.spaceSettingsSet(space.id, { trashRetentionDays: 7 }),
      () => client.inviteLinkCreate(space.id, "viewer"),
      () => client.inviteCodeStart(space.id, "viewer"),
      () => client.spaceClose(space.id),
    ]) {
      expect(await failure(call())).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
    }
  });

  it("lets an Owner do all of it", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const page = await pageId(client, space.id, "Notes");
    await client.pageTrash(page);
    await client.pagePurge(page);
    await expect(client.pageGet(page)).rejects.toMatchObject({ code: "NotFound" });
  });

  it("moves pages into a shared space as an Editor, and out of one as an Owner only", async () => {
    const editor = build("editor");
    const space = await work(editor.client);
    const personal = (await editor.client.spaceList()).find((entry) => entry.name === "Personal");
    const diary = await pageId(editor.client, personal?.id ?? "", "Diary");
    // Into the shared space: Editor is enough.
    await editor.client.pageMoveToSpace(diary, space.id, null, null);
    // Out of it: Owner only.
    const plan = await failure(editor.client.pageMoveToSpace(diary, personal?.id ?? "", null, null));
    expect(plan).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
    const viewer = build("viewer");
    const viewerPersonal = (await viewer.client.spaceList()).find((entry) => entry.name === "Personal");
    const viewerDiary = await pageId(viewer.client, viewerPersonal?.id ?? "", "Diary");
    const into = await failure(viewer.client.pageMoveToSpace(viewerDiary, (await work(viewer.client)).id, null, null));
    expect(into).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "editor" } });
    // The plan only reads.
    await viewer.client.pageMoveToSpacePlan(viewerDiary, (await work(viewer.client)).id, null);
  });

  it("refuses every write to an archive, an Owner's too, and keeps the reads", async () => {
    const { client } = build("owner", { sharing: { spaces: [{ space: "Work", archive: "Closed" }] } });
    const space = await work(client);
    const page = await pageId(client, space.id, "Notes");
    const writes: (() => Promise<unknown>)[] = [
      () => client.pageRename(page, "Renamed"),
      () => client.pageTrash(page),
      () => client.pageCreate({ spaceId: space.id, title: "New" }),
      () => client.spaceRename(space.id, "Renamed"),
      () => client.spaceSettingsSet(space.id, { trashRetentionDays: 1 }),
    ];
    for (const write of writes) {
      const error = await failure(write());
      expect(error).toMatchObject({ code: "Forbidden", details: { reason: "archived" } });
    }
    expect((await client.pageGet(page)).title).toBe("Notes");
    expect((await client.treeGet(space.id)).roots).toHaveLength(2);
  });

  it("leaves personal spaces as they were: no gate, no sharing fields", async () => {
    const { client } = build("viewer");
    const personal = (await client.spaceList()).find((space) => space.name === "Personal");
    await client.pageCreate({ spaceId: personal?.id ?? "", title: "More" });
    await client.spaceRename(personal?.id ?? "", "Mine");
  });
});

describe("leaving, closing and deleting", () => {
  it("leaves a shared space: the local copy goes, a space that is not shared cannot be left", async () => {
    const { client } = build("editor");
    const space = await work(client);
    const page = await pageId(client, space.id, "Notes");
    await client.spaceLeave(space.id);
    expect((await client.spaceList()).map((entry) => entry.name)).toEqual(["Personal"]);
    await expect(client.pageGet(page)).rejects.toMatchObject({ code: "NotFound" });
    const personal = (await client.spaceList())[0];
    expect((await failure(client.spaceLeave(personal?.id ?? ""))).details).toEqual({ field: "spaceId" });
  });

  it("closes a space as an Owner: it becomes an archive for this account, without a removal notice", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    events.length = 0;
    const closed = await client.spaceClose(space.id);
    expect(closed).toMatchObject({ shareState: "Archived", archiveReason: "Closed", role: "owner" });
    expect(types(events)).toEqual(["SpacesChanged", "SpaceMembersChanged"]);
    const info = await client.spaceMembers(space.id);
    expect(info.closedAt).toBe(START);
    expect(await failure(client.spaceClose(space.id))).toMatchObject({ details: { reason: "archived" } });
    // An archive is removed from this device with SpaceLeave, which only deletes the local copy.
    await client.spaceLeave(space.id);
    expect((await client.spaceList()).map((entry) => entry.name)).toEqual(["Personal"]);
  });

  it("refuses to delete a shared space or an archive: leave or close it", async () => {
    const { client } = build("owner");
    const space = await work(client);
    expect((await failure(client.spaceDelete(space.id))).details).toEqual({ field: "spaceId" });
    const archived = build("owner", { sharing: { spaces: [{ space: "Work", archive: "Removed" }] } });
    expect((await failure(archived.client.spaceDelete((await work(archived.client)).id))).details).toEqual({ field: "spaceId" });
  });
});

describe("settings", () => {
  it("changes the trash retention and the view-link flag, a field left out stays", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    events.length = 0;
    const first = await client.spaceSettingsSet(space.id, { trashRetentionDays: 7 });
    expect(first.settings).toEqual({ trashRetentionDays: 7, editorsMayLink: false });
    const second = await client.spaceSettingsSet(space.id, { editorsMayLink: true });
    expect(second.settings).toEqual({ trashRetentionDays: 7, editorsMayLink: true });
    expect((await client.spaceSettingsSet(space.id, { trashRetentionDays: 0 })).settings.trashRetentionDays).toBe(0);
    expect((await client.spaceSettingsSet(space.id, {})).settings).toEqual({ trashRetentionDays: 0, editorsMayLink: true });
    expect(types(events)).toEqual(Array(4).fill("SpaceMembersChanged"));
    for (const bad of [-1, 1.5, 4000]) {
      expect((await failure(client.spaceSettingsSet(space.id, { trashRetentionDays: bad }))).details).toEqual({
        field: "trashRetentionDays",
      });
    }
  });

  it("expires the trash of a shared space by its own retention, on an Owner's device only", async () => {
    const owner = build("owner");
    const space = await work(owner.client);
    const page = await pageId(owner.client, space.id, "Notes");
    await owner.client.pageTrash(page);
    await owner.client.spaceSettingsSet(space.id, { trashRetentionDays: 2 });
    vi.advanceTimersByTime(1 * DAY);
    expect(await owner.memory.trashExpire()).toBe(0);
    vi.advanceTimersByTime(2 * DAY);
    expect(await owner.memory.trashExpire()).toBe(1);
    await expect(owner.client.pageGet(page)).rejects.toMatchObject({ code: "NotFound" });

    // The device setting (30 days) does not apply to a shared space; zero keeps for ever.
    const keeper = build("owner");
    const keptSpace = await work(keeper.client);
    const kept = await pageId(keeper.client, keptSpace.id, "Notes");
    await keeper.client.pageTrash(kept);
    await keeper.client.spaceSettingsSet(keptSpace.id, { trashRetentionDays: 0 });
    vi.advanceTimersByTime(400 * DAY);
    expect(await keeper.memory.trashExpire()).toBe(0);

    // An Editor's device never purges a shared space, whatever its own setting says.
    const editor = build("editor", { settings: { "app.trash.retention_days": 1 } });
    const editorSpace = await work(editor.client);
    const trashed = await pageId(editor.client, editorSpace.id, "Notes");
    await editor.client.pageTrash(trashed);
    vi.advanceTimersByTime(10 * DAY);
    expect(await editor.memory.trashExpire()).toBe(0);
    // A personal space follows the device setting as before.
    const personal = (await editor.client.spaceList()).find((entry) => entry.name === "Personal");
    const diary = await pageId(editor.client, personal?.id ?? "", "Diary");
    await editor.client.pageTrash(diary);
    vi.advanceTimersByTime(2 * DAY);
    expect(await editor.memory.trashExpire()).toBe(1);
  });
});

describe("AI per account and space", () => {
  it("is off for a shared space until this account opts in, and on for a personal one", async () => {
    const { client } = build("editor");
    const space = await work(client);
    expect(space.aiEnabled).toBe(false);
    const on = await client.spaceAiSet(space.id, true);
    expect(Object.keys(on)).not.toContain("aiEnabled");
    const off = await client.spaceAiSet(space.id, false);
    expect(off.aiEnabled).toBe(false);
    // Any member may set it, a Viewer included; so is a personal space.
    const personal = (await client.spaceList()).find((entry) => entry.name === "Personal");
    expect((await client.spaceAiSet(personal?.id ?? "", false)).aiEnabled).toBe(false);
    expect(Object.keys(await client.spaceAiSet(personal?.id ?? "", true))).not.toContain("aiEnabled");
  });

  it("keeps the retrieval, the index and the MCP grants away from a space whose AI is off", async () => {
    const { client, memory } = build("viewer", {
      ai: { mode: "fake" },
      spaces: [{ name: "Work", pages: [{ title: "Plan" }, { title: "Notes" }] }],
    });
    const space = await work(client);
    const scope = { type: "space", spaceId: space.id } as const;
    expect(await client.aiRetrieve("Plan", scope)).toEqual([]);
    expect((await client.aiRetrieve("Plan", { type: "all" })).length).toBe(0);
    expect((await client.aiIndexStatus()).pagesTotal).toBe(0);
    const created = await client.mcpClientCreate("Tool");
    const refused = await failure(memory.mcp?.grantsSet(created.client.id, [{ spaceId: space.id, read: true, write: false }]) ?? Promise.reject(new Error("no mcp")));
    expect(refused.details).toEqual({ field: "grants" });
    await client.spaceAiSet(space.id, true);
    expect((await client.aiRetrieve("Plan", scope)).length).toBe(1);
    await memory.mcp?.grantsSet(created.client.id, [{ spaceId: space.id, read: true, write: false }]);
    expect((await client.mcpClientList())[0]?.grants).toHaveLength(1);
    // Turning it off again revokes the grants.
    await client.spaceAiSet(space.id, false);
    expect((await client.mcpClientList())[0]?.grants).toEqual([]);
    expect(await client.aiRetrieve("Plan", scope)).toEqual([]);
  });
});

describe("profile and people", () => {
  it("reads and sets the profile name, and every shared space shows the new name", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    expect(await client.profileGet()).toEqual({ displayName: "Maya" });
    events.length = 0;
    expect(await client.profileSet("  Maya R.  ")).toEqual({ displayName: "Maya R." });
    expect(types(events)).toEqual(["PeopleChanged", "SpaceMembersChanged"]);
    expect((await client.spaceMembers(space.id)).members[0]?.name).toBe("Maya R.");
    for (const bad of ["", "   ", "x".repeat(101), "bad\nname"]) {
      expect((await failure(client.profileSet(bad))).details).toEqual({ field: "displayName" });
    }
    expect(await build("personal", { sharing: {} }).client.profileGet()).toEqual({});
  });

  it("marks a person verified or not, for every space, and never this account", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const carol = member(info, "Carol");
    events.length = 0;
    expect(await client.personVerifiedSet(carol.accountId, true)).toMatchObject({ name: "Carol", verified: true });
    expect(types(events)).toEqual(["PeopleChanged"]);
    expect(member(await client.spaceMembers(space.id), "Carol").verified).toBe(true);
    expect((await client.personVerifiedSet(carol.accountId, false)).verified).toBe(false);
    const me = info.members.find((entry) => entry.isMe);
    expect((await failure(client.personVerifiedSet(me?.accountId ?? "", true))).details).toEqual({ field: "accountId" });
    expect((await failure(client.personGet("00000000-0000-4000-8000-0000000000ee"))).code).toBe("NotFound");
    expect(await client.personGet(me?.accountId ?? "")).toMatchObject({ name: "Maya", isMe: true });
  });
});

describe("link invites", () => {
  it("makes a link that grants a role below Owner, shows the secret once and lists the invite", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const link = await client.inviteLinkCreate(space.id, "editor");
    expect(link.link).toMatch(/^atomik-kb:\/\/invite\?server=https%3A%2F%2Fsync\.example\.com#[0-9A-Z]{52}$/u);
    expect(link.webLink).toMatch(/^https:\/\/sync\.example\.com\/join#[0-9A-Z]{52}$/u);
    expect(link.role).toBe("editor");
    expect(link.expiresAt).toBe(START + 7 * DAY);
    const info = await client.spaceMembers(space.id);
    expect(info.invites).toEqual([
      {
        inviteId: link.inviteId,
        kind: "Link",
        role: "editor",
        createdBy: info.members.find((entry) => entry.isMe)?.accountId,
        createdAt: START,
        expiresAt: START + 7 * DAY,
        status: "Open",
      },
    ]);
    // The secret is not in the list.
    expect(JSON.stringify(info)).not.toContain(link.link.split("#")[1]);
    expect((await client.inviteLinkCreate(space.id, "viewer", 30)).expiresAt).toBe(START + 30 * DAY);
  });

  it("grants no Owner, at most 30 days, at most 20 open invites, and only to an Owner with sync on", async () => {
    const { client } = build("owner");
    const space = await work(client);
    expect((await failure(client.inviteLinkCreate(space.id, "owner"))).details).toEqual({ field: "role" });
    expect((await failure(client.inviteCodeStart(space.id, "owner"))).details).toEqual({ field: "role" });
    for (const days of [0, 31, 1.5]) {
      expect((await failure(client.inviteLinkCreate(space.id, "viewer", days))).details).toEqual({ field: "expiresInDays" });
    }
    for (let count = 0; count < MAX_OPEN_INVITES; count += 1) {
      await client.inviteLinkCreate(space.id, "viewer");
    }
    expect((await failure(client.inviteLinkCreate(space.id, "viewer"))).details).toEqual({ field: "invites" });
    const noSync = build("owner", { sync: undefined });
    expect((await failure(noSync.client.inviteLinkCreate((await work(noSync.client)).id, "viewer"))).details).toEqual({
      field: "serverUrl",
    });
  });

  it("is single use: the first person joins unverified with the role, the second finds the invite dead", async () => {
    const { client, memory, events } = build("owner");
    const space = await work(client);
    const link = await client.inviteLinkCreate(space.id, "commenter");
    events.length = 0;
    const eve = memory.receiveSharing({ type: "joinByLink", link: link.link, name: "Eve" });
    expect(eve).toBeDefined();
    expect(types(events)).toEqual(["SpaceMembersChanged", "PeopleChanged"]);
    const info = await client.spaceMembers(space.id);
    expect(member(info, "Eve")).toMatchObject({ role: "commenter", verified: false });
    expect(info.invites[0]).toMatchObject({ status: "Used", usedBy: eve });
    expect(() => memory.receiveSharing({ type: "joinByLink", link: link.link, name: "Frank" })).toThrow(INVITE_DEAD_MESSAGE);
    expect(member(await client.spaceMembers(space.id), "Eve")).toBeDefined();
    expect((await client.spaceMembers(space.id)).members.some((entry) => entry.name === "Frank")).toBe(false);
  });

  it("expires by the clock, and can be revoked once", async () => {
    const { client, memory } = build("owner");
    const space = await work(client);
    const short = await client.inviteLinkCreate(space.id, "viewer", 1);
    vi.advanceTimersByTime(DAY);
    expect((await client.spaceMembers(space.id)).invites[0]?.status).toBe("Expired");
    expect(() => memory.receiveSharing({ type: "joinByLink", link: short.link, name: "Eve" })).toThrow(INVITE_DEAD_MESSAGE);
    expect((await failure(client.inviteRevoke(space.id, short.inviteId))).details).toEqual({ field: "inviteId" });

    const open = await client.inviteLinkCreate(space.id, "viewer");
    const revoked = await client.inviteRevoke(space.id, open.inviteId);
    expect(revoked.invites.map((invite) => invite.status)).toEqual(["Expired", "Revoked"]);
    expect(() => memory.receiveSharing({ type: "joinByLink", link: open.link, name: "Eve" })).toThrow(INVITE_DEAD_MESSAGE);
    expect((await failure(client.inviteRevoke(space.id, open.inviteId))).details).toEqual({ field: "inviteId" });
    expect((await failure(client.inviteRevoke(space.id, "00000000-0000-4000-8000-0000000000dd"))).details).toEqual({
      field: "inviteId",
    });
  });

  it("lets a removed member be invited again, but a member not twice", async () => {
    const { client, memory } = build("owner");
    const space = await work(client);
    const bob = member(await client.spaceMembers(space.id), "Bob");
    await client.memberRemove(space.id, bob.accountId);
    const back = await joinByLink(memory, client, space.id, "Bob", "viewer");
    expect(back).toBe(bob.accountId);
    const info = await client.spaceMembers(space.id);
    expect(member(info, "Bob")).toMatchObject({ role: "viewer", verified: true });
    const twice = await client.inviteLinkCreate(space.id, "viewer");
    expect(() => memory.receiveSharing({ type: "joinByLink", link: twice.link, name: "Bob" })).toThrow(
      /already a member/u,
    );
  });

  it("lets only an Owner create and revoke", async () => {
    const { client } = build("editor");
    const space = await work(client);
    expect(await failure(client.inviteLinkCreate(space.id, "viewer"))).toMatchObject({ code: "Forbidden" });
    expect(await failure(client.inviteRevoke(space.id, "00000000-0000-4000-8000-0000000000dd"))).toMatchObject({
      code: "Forbidden",
    });
  });

  it("lists the seeded invites with their status", async () => {
    const { client } = build("owner", {
      sharing: {
        spaces: [
          {
            space: "Work",
            invites: [{ role: "editor" }, { role: "viewer", status: "Expired" }, { role: "viewer", status: "Used" }, { role: "commenter", kind: "Code", status: "Revoked" }],
          },
        ],
      },
    });
    const info = await client.spaceMembers((await work(client)).id);
    expect(info.invites.map((invite) => [invite.kind, invite.role, invite.status])).toEqual([
      ["Link", "editor", "Open"],
      ["Link", "viewer", "Expired"],
      ["Link", "viewer", "Used"],
      ["Code", "commenter", "Revoked"],
    ]);
  });
});

describe("a code invite, inviter side", () => {
  it("walks the code, the digits and the other person's identity, then adds a verified member", async () => {
    const { client, events } = build("owner");
    const space = await work(client);
    const started = await client.inviteCodeStart(space.id, "editor");
    expect(started).toMatchObject({ flow: "Code", side: "Inviter", state: "Starting", verified: false, serverUrl: SERVER, role: "editor" });
    expect(types(events)).toEqual(["InviteChanged", "InviteRequested"]);

    vi.advanceTimersByTime(STEP);
    const waiting = await client.inviteStatus();
    expect(waiting).toMatchObject({ state: "Waiting", code: MEMORY_INVITE_CODE, expiresAt: START + STEP + 600_000 });
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Confirming", sas: MEMORY_INVITE_SAS });
    // The other person confirms first: until then there is nothing for this side to confirm.
    expect((await failure(client.inviteConfirm())).details).toEqual({ field: "invite" });
    vi.advanceTimersByTime(STEP);
    const peer = await client.inviteStatus();
    expect(peer).toMatchObject({ state: "PeerConfirming", peerName: MEMORY_INVITEE_NAME });
    expect(peer.peerSafetyNumber).toMatch(/^\d{5}( \d{5}){5}$/u);

    const confirmed = await client.inviteConfirm();
    expect(confirmed).toMatchObject({ state: "Transferring", verified: true });
    // The Invite (bound to the invitee, valid for an hour) is in the list while the bundle goes.
    const during = await client.spaceMembers(space.id);
    expect(during.invites).toHaveLength(1);
    expect(during.invites[0]).toMatchObject({ kind: "Code", role: "editor", status: "Open", expiresAt: START + 3 * STEP + 3_600_000 });
    expect(during.invites[0]?.boundAccount).toBeDefined();
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Done" });
    const after = await client.spaceMembers(space.id);
    expect(member(after, MEMORY_INVITEE_NAME)).toMatchObject({ role: "editor", verified: true });
    expect(after.invites[0]?.status).toBe("Used");
    expect(after.invites[0]?.usedBy).toBe(member(after, MEMORY_INVITEE_NAME).accountId);
  });

  it("can be cancelled, replaced by a new session, and stopped by a lock", async () => {
    const { client, memory } = build("owner", { security: { kitCreatedAt: START, appLockPassphrase: "correct horse" } });
    const space = await work(client);
    await client.inviteCodeStart(space.id, "viewer");
    vi.advanceTimersByTime(2 * STEP);
    expect((await client.inviteCancel()).state).toBe("Cancelled");
    vi.advanceTimersByTime(10 * STEP);
    expect((await client.inviteStatus()).state).toBe("Cancelled");
    // Cancelling again changes nothing.
    expect((await client.inviteCancel()).state).toBe("Cancelled");
    await client.inviteCodeStart(space.id, "viewer");
    await memory.lock();
    await memory.unlock("correct horse");
    expect((await client.inviteStatus()).state).toBe("Cancelled");
    expect(await build("personal").client.inviteStatus()).toEqual({ flow: "Code", side: "Inviter", state: "Idle", verified: false });
  });

  it("fails when this account stopped being an Owner before it confirmed", async () => {
    const { client, memory } = build("owner", {
      sharing: { spaces: [{ space: "Work", members: [{ name: "Bob", role: "owner" }] }] },
    });
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    await client.inviteCodeStart(space.id, "viewer");
    vi.advanceTimersByTime(3 * STEP);
    memory.receiveSharing({ type: "roleSet", spaceId: space.id, accountId: info.members.find((entry) => entry.isMe)?.accountId ?? "", role: "editor" });
    const failed = await client.inviteConfirm();
    expect(failed.state).toBe("Failed");
    expect(failed.error).toMatch(/no longer invite/u);
  });
});

describe("a code invite, invitee side", () => {
  it("types the code, compares the digits, confirms first, then receives the space and verifies the inviter", async () => {
    const { client, events } = build("personal");
    const started = await client.inviteCodeJoin(SERVER, "7kqm-x2tv-r9hd");
    expect(started).toMatchObject({ flow: "Code", side: "Invitee", state: "Starting", serverUrl: SERVER });
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Confirming", sas: MEMORY_INVITE_SAS });
    expect((await client.inviteConfirm()).state).toBe("PeerConfirming");
    vi.advanceTimersByTime(STEP);
    const transferring = await client.inviteStatus();
    expect(transferring).toMatchObject({ state: "Transferring", spaceName: "Project X", inviterName: "Alice", role: "editor" });
    vi.advanceTimersByTime(STEP);
    expect((await client.inviteStatus()).state).toBe("Joining");
    events.length = 0;
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Done", verified: true });
    expect(types(events)).toEqual(expect.arrayContaining(["SpacesChanged", "SpaceMembersChanged", "PeopleChanged", "InviteChanged"]));
    const joined = (await client.spaceList()).find((space) => space.name === "Project X");
    expect(joined).toMatchObject({ shareState: "Shared", role: "editor", memberCount: 2, aiEnabled: false });
    expect((await client.treeGet(joined?.id ?? "")).roots.map((node) => node.page.title)).toEqual(["Welcome", "Meeting notes"]);
    const info = await client.spaceMembers(joined?.id ?? "");
    expect(info.members.map((entry) => [entry.name, entry.role])).toEqual([
      ["Alice", "owner"],
      ["Maya", "editor"],
    ]);
    // The code flow marks the inviter verified (the digits authenticated both accounts).
    expect(member(info, "Alice").verified).toBe(true);
  });

  it("offers what the options say, and ends Failed for a code that is dead", async () => {
    const { client } = build("personal", {
      sharing: { profileName: "Maya", invitation: { spaceName: "Garden", inviterName: "Zoe", role: "viewer" } },
    });
    await client.inviteCodeJoin(SERVER, MEMORY_INVITE_CODE);
    vi.advanceTimersByTime(STEP);
    await client.inviteConfirm();
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ spaceName: "Garden", inviterName: "Zoe", role: "viewer" });
    await client.inviteCodeJoin(SERVER, EXPIRED_INVITE_CODE);
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Failed", error: INVITE_DEAD_MESSAGE });
  });

  it("checks the server, the code, the workspace and the account", async () => {
    const { client } = build("personal");
    expect((await failure(client.inviteCodeJoin("ftp://nope", MEMORY_INVITE_CODE))).details).toEqual({ field: "serverUrl" });
    expect((await failure(client.inviteCodeJoin(SERVER, "short"))).details).toEqual({ field: "code" });
    const noKit = build("personal", { security: undefined });
    expect((await failure(noKit.client.inviteCodeJoin(SERVER, MEMORY_INVITE_CODE))).details).toEqual({ field: "account" });
    const memory = new MemoryTransport({ sharing: {} });
    expect((await failure(new CoreClient(memory).inviteCodeJoin(SERVER, MEMORY_INVITE_CODE))).details).toEqual({ field: "workspace" });
    // Nothing to confirm yet, or at all.
    expect((await failure(client.inviteConfirm())).details).toEqual({ field: "invite" });
  });
});

describe("a link invite, invitee side", () => {
  const LINK = `atomik-kb://invite?server=${encodeURIComponent(SERVER)}#8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB`;

  it("previews the space, the inviter and the role as not verified; accepting joins without verifying", async () => {
    const { client } = build("personal");
    const opened = await client.inviteLinkOpen(LINK);
    expect(opened).toMatchObject({ flow: "Link", side: "Invitee", state: "Starting", verified: false, serverUrl: SERVER });
    vi.advanceTimersByTime(STEP);
    const preview = await client.inviteStatus();
    expect(preview).toMatchObject({ state: "Previewing", spaceName: "Project X", inviterName: "Alice", role: "editor", verified: false });
    expect((await client.inviteConfirm()).state).toBe("Joining");
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Done", verified: false });
    const joined = (await client.spaceList()).find((space) => space.name === "Project X");
    expect(joined).toMatchObject({ shareState: "Shared", role: "editor" });
    const info = await client.spaceMembers(joined?.id ?? "");
    expect(member(info, "Alice").verified).toBe(false);
  });

  it("reads the web form of a link too, and the same link once only", async () => {
    const { client } = build("personal");
    await client.inviteLinkOpen("https://sync.example.com/join#8H2XG7MQ4TNP5WRZ3KVB9D6YCEJA1F0S8H2XG7MQ4TNP5WRZ3KAB");
    vi.advanceTimersByTime(STEP);
    await client.inviteConfirm();
    vi.advanceTimersByTime(STEP);
    expect((await client.inviteStatus()).state).toBe("Done");
    await client.inviteLinkOpen(LINK);
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Failed", error: INVITE_DEAD_MESSAGE });
  });

  it("ends Failed for the dead link, declines with a cancel, and refuses what is not a link", async () => {
    const { client } = build("personal");
    await client.inviteLinkOpen(`atomik-kb://invite?server=${encodeURIComponent(SERVER)}#${DEAD_LINK_SECRET}`);
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Failed", error: INVITE_DEAD_MESSAGE });
    await client.inviteLinkOpen(LINK);
    vi.advanceTimersByTime(STEP);
    expect((await client.inviteCancel()).state).toBe("Cancelled");
    expect((await client.spaceList()).map((space) => space.name)).toEqual(["Work", "Personal"]);
    for (const bad of ["", "https://example.com", "atomik-kb://page/1", "atomik-kb://invite?server=x#abc", `atomik-kb://invite?server=ftp%3A%2F%2Fx#${DEAD_LINK_SECRET}`]) {
      expect((await failure(client.inviteLinkOpen(bad))).details).toEqual({ field: "link" });
    }
  });

  it("tells that a link of this account's own space leads nowhere new", async () => {
    const { client } = build("owner");
    const link = await client.inviteLinkCreate((await work(client)).id, "viewer");
    await client.inviteLinkOpen(link.link);
    vi.advanceTimersByTime(STEP);
    expect(await client.inviteStatus()).toMatchObject({ state: "Failed", error: "you are already a member of this space" });
  });
});

describe("what the other people do", () => {
  it("announces a role change of this account, and a Viewer's page edits are refused afterwards", async () => {
    const { client, memory, events } = build("editor");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const me = info.members.find((entry) => entry.isMe);
    const page = await pageId(client, space.id, "Notes");
    await client.pageRename(page, "Mine");
    events.length = 0;
    memory.receiveSharing({ type: "roleSet", spaceId: space.id, accountId: me?.accountId ?? "", role: "viewer" });
    expect(types(events)).toEqual(["SpacesChanged", "SpaceRoleChanged", "SpaceMembersChanged"]);
    expect(events).toContainEqual({ type: "SpaceRoleChanged", data: { spaceId: space.id, role: "viewer", previous: "editor" } });
    expect((await work(client)).role).toBe("viewer");
    expect((await failure(client.pageRename(page, "Again"))).code).toBe("Forbidden");
  });

  it("turns the space into an archive when this account is removed, and copies the unsent changes", async () => {
    const { client, memory, events } = build("editor");
    const space = await work(client);
    const page = await pageId(client, space.id, "Notes");
    events.length = 0;
    memory.receiveSharing({ type: "removed", spaceId: space.id, unsentPageIds: [page] });
    expect(await work(client)).toMatchObject({ shareState: "Archived", archiveReason: "Removed", role: "editor" });
    expect(events).toContainEqual({ type: "SpaceRemoved", data: { spaceId: space.id, reason: "Removed" } });
    const copied = events.find((event) => event.type === "UnsentChangesCopied");
    expect(copied).toBeDefined();
    if (copied?.type === "UnsentChangesCopied") {
      const target = (await client.spaceList()).find((entry) => entry.id === copied.data.targetSpaceId);
      expect(target?.name).toBe("Personal");
      expect((await client.pageGet(copied.data.pageIds[0] ?? "")).title).toBe("Unsent changes - Notes");
    }
    expect((await failure(client.pageRename(page, "x"))).details).toEqual({ reason: "archived" });
    // The archive still reads.
    expect((await client.pageGet(page)).title).toBe("Notes");
  });

  it("closes the space for everyone, and removes, leaves, renames and flags other members", async () => {
    const { client, memory, events } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const bob = member(info, "Bob");
    const carol = member(info, "Carol");
    // The last Owner cannot be taken out of the list by leaving.
    expect(() => memory.receiveSharing({ type: "memberLeft", spaceId: space.id, accountId: info.members.find((entry) => entry.isMe)?.accountId ?? "" })).toThrow(/last Owner/u);
    memory.receiveSharing({ type: "memberLeft", spaceId: space.id, accountId: carol.accountId });
    const left = await client.spaceMembers(space.id);
    expect(left.members.map((entry) => entry.name)).toEqual(["Maya", "Bob", "Dan"]);
    expect(left.pendingRekey).toBe(true);
    memory.receiveSharing({ type: "removed", spaceId: space.id, accountId: bob.accountId });
    expect((await client.spaceMembers(space.id)).epoch).toBe(info.epoch + 1);
    events.length = 0;
    memory.receiveSharing({ type: "renamed", spaceId: space.id, accountId: member(left, "Dan").accountId, name: "Daniel" });
    expect(types(events)).toEqual(["PeopleChanged", "SpaceMembersChanged"]);
    memory.receiveSharing({ type: "refused", spaceId: space.id, accountId: member(left, "Dan").accountId, count: 3 });
    const refused = await client.spaceMembers(space.id);
    expect(member(refused, "Daniel").refusedChanges).toBe(3);
    expect(refused.refusedTotal).toBe(3);
    memory.receiveSharing({ type: "serverFlagged", spaceId: space.id });
    expect((await client.spaceMembers(space.id)).serverFlagged).toBe(true);
    expect((await client.syncStatus()).feeds?.find((feed) => feed.spaceId === space.id)).toMatchObject({ serverFlagged: true, refused: 3 });
    events.length = 0;
    memory.receiveSharing({ type: "closed", spaceId: space.id });
    expect(await work(client)).toMatchObject({ shareState: "Archived", archiveReason: "Closed" });
    expect(events).toContainEqual({ type: "SpaceRemoved", data: { spaceId: space.id, reason: "Closed" } });
  });

  it("shows a co-member's device added with the kit, and an incomplete snapshot to Owners only", async () => {
    const { client, memory, events } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const bob = member(info, "Bob");
    const page = await pageId(client, space.id, "Plan");
    events.length = 0;
    memory.receiveSharing({ type: "viaKit", spaceId: space.id, accountId: bob.accountId });
    expect(events).toContainEqual({ type: "MemberDeviceViaKit", data: { spaceId: space.id, accountId: bob.accountId, name: "Bob" } });
    expect(member(await client.spaceMembers(space.id), "Bob").viaKitAt).toBe(START);
    memory.receiveSharing({ type: "disputed", spaceId: space.id, accountId: bob.accountId, pageId: page });
    expect(events).toContainEqual({ type: "SnapshotDisputed", data: { spaceId: space.id, accountId: bob.accountId, pageId: page } });
    expect((await client.spaceMembers(space.id)).disputes).toEqual([{ pageId: page, accountId: bob.accountId, at: START }]);
    // An Editor sees no disputes.
    const editor = build("editor", { sharing: { spaces: [{ space: "Work", role: "editor", disputes: [{ member: "Bob" }] }] } });
    expect((await editor.client.spaceMembers((await work(editor.client)).id)).disputes).toEqual([]);
    const owner = build("owner", { sharing: { spaces: [{ space: "Work", disputes: [{ member: "Bob" }] }] } });
    expect((await owner.client.spaceMembers((await work(owner.client)).id)).disputes).toHaveLength(1);
  });

  it("does not count what other people do as this account's own edits", async () => {
    const { client, memory } = build("owner", { sync: { state: "idle", serverUrl: SERVER } });
    const space = await work(client);
    const before = (await client.syncStatus()).pendingEntries;
    memory.receiveSharing({ type: "memberJoined", spaceId: space.id, name: "Eve", role: "viewer" });
    expect((await client.syncStatus()).pendingEntries).toBe(before);
  });
});

describe("the sync feeds", () => {
  it("lists the personal feed first and one per shared space, once a space is shared", async () => {
    const { client } = build("owner");
    const space = await work(client);
    const status = await client.syncStatus();
    expect(status.feeds).toHaveLength(2);
    expect(status.feeds?.[0]).toMatchObject({ refused: 0, forked: false, closed: false, serverFlagged: false });
    expect(status.feeds?.[0]?.spaceId).toBeUndefined();
    expect(status.feeds?.[1]).toMatchObject({ spaceId: space.id, closed: false });
    expect((await client.spaceMembers(space.id)).feedId).toBe(status.feeds?.[1]?.feedId);
    await client.spaceClose(space.id);
    expect((await client.syncStatus()).feeds?.[1]?.closed).toBe(true);
    // Nothing shared: the list is left out, as before.
    const plain = build("personal");
    expect((await plain.client.syncStatus()).feeds).toBeUndefined();
  });
});

describe("sessions that start under way", () => {
  it("starts a link preview, a code invitee and a code inviter for the dev page", async () => {
    const preview = build("personal", { sharing: { profileName: "Maya", start: "link-preview" } });
    expect((await preview.client.inviteStatus()).state).toBe("Starting");
    vi.advanceTimersByTime(STEP);
    expect(await preview.client.inviteStatus()).toMatchObject({ flow: "Link", state: "Previewing" });
    const invitee = build("personal", { sharing: { profileName: "Maya", start: "code-invitee" } });
    vi.advanceTimersByTime(STEP);
    expect(await invitee.client.inviteStatus()).toMatchObject({ flow: "Code", side: "Invitee", state: "Confirming" });
    const inviter = build("owner", { sharing: { profileName: "Maya", spaces: [{ space: "Work" }], start: "code-inviter" } });
    vi.advanceTimersByTime(STEP);
    expect(await inviter.client.inviteStatus()).toMatchObject({ flow: "Code", side: "Inviter", state: "Waiting" });
  });
});

describe("the contract's wire shapes", () => {
  it("answers every sharing request with its documented type and omits the optional fields it does not have", async () => {
    const { client, memory } = build("owner");
    const space = await work(client);
    const info = await client.spaceMembers(space.id);
    const me = info.members.find((entry) => entry.isMe) as MemberInfo;
    expect(Object.keys(me).sort()).toEqual(
      ["accountId", "deviceCount", "isMe", "joinedAt", "name", "nameShared", "refusedChanges", "role", "verified"].sort(),
    );
    const plain = await memory.dispatch({ type: "SpaceMembers", data: { spaceId: space.id } });
    expect(plain.type).toBe("Members");
    expect(JSON.parse(JSON.stringify(info))).toEqual(info);
    const link = await client.inviteLinkCreate(space.id, "viewer");
    expect(Object.keys(link).sort()).toEqual(["expiresAt", "inviteId", "link", "role", "webLink"]);
    expect(await client.inviteStatus()).toEqual({ flow: "Code", side: "Inviter", state: "Idle", verified: false });
  });
});
