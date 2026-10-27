// View-only links in the in-memory mirror (Milestone 13, slice 4, `./memory-viewlinks`): who may create (an Owner, an Editor when the
// space allows it, anyone in a personal space), the order of the checks, the URL that only the create answers, the first upload that
// turns `Publishing` into `Live`, expiry, the list for every member, revocation by the creator or an Owner, the links a closed space
// loses, the seeds and the events.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { MemberRole } from "./generated/MemberRole";
import type { SpaceInfo } from "./generated/SpaceInfo";
import { LINK_EXPIRY_MAX_DAYS } from "./link";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { DEFAULT_LINK_PUBLISH_MS } from "./memory-viewlinks";

const START = 1_700_000_000_000;
const DAY = 86_400_000;
const SERVER = "https://sync.example.com";

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

/**
 * Two seeded spaces, a kit, sync and the profile name "Maya"; `Work` is shared for `role` (Alice, Bob (Editor), Carol (Commenter) and
 * Dan (Viewer) are the other people) unless it is `"personal"`.
 */
function build(role: MemberRole | "personal" = "owner", options: MemoryTransportOptions = {}, settings?: { editorsMayLink: boolean }) {
  const memory = new MemoryTransport({
    workspaceName: "Home",
    spaces: SPACES,
    security: { kitCreatedAt: START },
    sync: { state: "idle", serverUrl: SERVER },
    syncStepMs: 5,
    sharing: {
      profileName: "Maya",
      ...(role === "personal" ? {} : { spaces: [{ space: "Work", role, ...(settings === undefined ? {} : { settings }) }] }),
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

async function spaceNamed(client: CoreClient, name: string): Promise<SpaceInfo> {
  const found = (await client.spaceList()).find((space) => space.name === name);
  if (found === undefined) {
    throw new Error(`no ${name} space`);
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

/** The Work space, its Plan page, and what the tests need around them. */
async function setup(role: MemberRole | "personal" = "owner", options: MemoryTransportOptions = {}, settings?: { editorsMayLink: boolean }) {
  const built = build(role, options, settings);
  const space = await spaceNamed(built.client, "Work");
  const plan = await pageId(built.client, space.id, "Plan");
  return { ...built, space, plan };
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

/** The view link events of a space. */
function linkEvents(events: readonly Event[]): Event[] {
  return events.filter((event) => event.type === "ViewLinksChanged");
}

function only<T>(items: readonly T[]): T {
  const [first] = items;
  if (first === undefined || items.length !== 1) {
    throw new Error(`expected one item, got ${String(items.length)}`);
  }
  return first;
}

describe("creating a link", () => {
  it("answers the link with its URL: the server, the viewer's path and a 43 character key in the fragment", async () => {
    const { client, space, plan } = await setup();
    const link = await client.viewLinkCreate(space.id, plan);
    expect(link.url).toMatch(/^https:\/\/sync\.example\.com\/view#[A-Za-z0-9_-]{43}$/u);
    expect(link).toMatchObject({ pageId: plan, createdByName: "Maya", createdAt: START, status: "Publishing" });
    // No expiry was asked for: the key is not there, and nothing was published yet.
    expect(Object.keys(link)).not.toContain("expiresAt");
    expect(Object.keys(link)).not.toContain("lastPublishedAt");
    // Another link has another key, and its own id.
    const other = await client.viewLinkCreate(space.id, plan);
    expect(other.url).not.toBe(link.url);
    expect(other.linkId).not.toBe(link.linkId);
  });

  it("keeps a trailing slash of the server out of the URL", async () => {
    const { client, space, plan } = await setup("owner", { sync: { state: "idle", serverUrl: `${SERVER}/` } });
    expect((await client.viewLinkCreate(space.id, plan)).url).toMatch(/^https:\/\/sync\.example\.com\/view#/u);
  });

  it("gives an expiry in days from now, one to the longest the contract allows", async () => {
    const { client, space, plan } = await setup();
    expect((await client.viewLinkCreate(space.id, plan, 7)).expiresAt).toBe(START + 7 * DAY);
    expect((await client.viewLinkCreate(space.id, plan, 1)).expiresAt).toBe(START + DAY);
    expect((await client.viewLinkCreate(space.id, plan, LINK_EXPIRY_MAX_DAYS)).expiresAt).toBe(START + LINK_EXPIRY_MAX_DAYS * DAY);
  });

  it("refuses an expiry that is not a whole number of days from 1 to 365", async () => {
    const { client, space, plan } = await setup();
    for (const days of [0, -1, 1.5, LINK_EXPIRY_MAX_DAYS + 1, Number.NaN]) {
      const error = await failure(client.viewLinkCreate(space.id, plan, days));
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toEqual({ field: "expiresInDays" });
    }
    expect(await client.viewLinkList(space.id)).toEqual([]);
  });

  it("refuses a page that is trashed, purged, another space's or unknown, with one answer for all", async () => {
    const { client, space, plan } = await setup();
    const notes = await pageId(client, space.id, "Notes");
    const diary = await pageId(client, (await spaceNamed(client, "Personal")).id, "Diary");
    await client.pageTrash(notes);
    const purged = await pageId(client, space.id, "Sub");
    await client.pageTrash(purged);
    await client.pagePurge(purged);
    for (const bad of [notes, purged, diary, "00000000-0000-4000-8000-000000000999"]) {
      const error = await failure(client.viewLinkCreate(space.id, bad));
      expect(error.code).toBe("InvalidInput");
      expect(error.details).toEqual({ field: "pageId" });
    }
    // The live page still works.
    expect((await client.viewLinkCreate(space.id, plan)).pageId).toBe(plan);
  });

  it("needs sync turned on and the recovery kit, which `serverUrl` says", async () => {
    const noSync = await setup("owner", { sync: undefined });
    const error = await failure(noSync.client.viewLinkCreate(noSync.space.id, noSync.plan));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "serverUrl" } });
    const noKit = await setup("owner", { security: {} });
    expect(await failure(noKit.client.viewLinkCreate(noKit.space.id, noKit.plan))).toMatchObject({
      code: "InvalidInput",
      details: { field: "serverUrl" },
    });
  });

  it("is NotFound for a space that does not exist", async () => {
    const { client, plan } = await setup();
    expect((await failure(client.viewLinkCreate("00000000-0000-4000-8000-000000000998", plan))).code).toBe("NotFound");
    expect((await failure(client.viewLinkList("00000000-0000-4000-8000-000000000998"))).code).toBe("NotFound");
    expect((await failure(client.viewLinkRevoke("00000000-0000-4000-8000-000000000998", "x"))).code).toBe("NotFound");
  });

  it("checks the role before the expiry and the page", async () => {
    const { client, space } = await setup("viewer");
    const error = await failure(client.viewLinkCreate(space.id, "00000000-0000-4000-8000-000000000999", 0));
    expect(error.code).toBe("Forbidden");
  });
});

describe("who may create", () => {
  it("lets an Owner, whatever the setting says", async () => {
    for (const editorsMayLink of [false, true]) {
      const { client, space, plan } = await setup("owner", {}, { editorsMayLink });
      expect((await client.viewLinkCreate(space.id, plan)).status).toBe("Publishing");
    }
  });

  it("lets an Editor only when the space allows it: forbidden_role(Owner) otherwise", async () => {
    const off = await setup("editor");
    const refused = await failure(off.client.viewLinkCreate(off.space.id, off.plan));
    expect(refused).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
    expect(await off.client.viewLinkList(off.space.id)).toEqual([]);
    const on = await setup("editor", {}, { editorsMayLink: true });
    expect(await on.client.viewLinkCreate(on.space.id, on.plan)).toMatchObject({ createdByName: "Maya" });
  });

  it("refuses a Commenter and a Viewer, setting or not: forbidden_role(Editor)", async () => {
    for (const role of ["commenter", "viewer"] as const) {
      const { client, space, plan } = await setup(role, {}, { editorsMayLink: true });
      const refused = await failure(client.viewLinkCreate(space.id, plan));
      expect(refused).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "editor" } });
    }
  });

  it("refuses a read-only copy: archived, for a removed account and for a closed space", async () => {
    for (const archive of ["Removed", "Closed"] as const) {
      const { client, space, plan } = await setup("owner", {
        sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "owner", archive }] },
      });
      expect(await failure(client.viewLinkCreate(space.id, plan))).toMatchObject({ code: "Forbidden", details: { reason: "archived" } });
    }
  });

  it("lets anyone in a personal space, which has no setting", async () => {
    const { client, space, plan } = await setup("personal");
    const link = await client.viewLinkCreate(space.id, plan, 30);
    expect(link).toMatchObject({ pageId: plan, createdByName: "Maya", status: "Publishing" });
    expect(only(await client.viewLinkList(space.id)).linkId).toBe(link.linkId);
  });

  it("makes the setting count at once: the Owner's change is what an Editor is held to", async () => {
    // The setting is the Owner's to change; the seed stands for the Owner having done so.
    const { client, space, plan } = await setup("editor", {}, { editorsMayLink: true });
    expect((await client.spaceMembers(space.id)).settings.editorsMayLink).toBe(true);
    expect((await client.viewLinkCreate(space.id, plan)).status).toBe("Publishing");
  });
});

describe("the URL is answered once", () => {
  it("shows no URL in the list, for the creator and for everyone else", async () => {
    const { client, space, plan } = await setup("owner", { links: { links: [{ space: "Work", page: "Notes", createdBy: "Bob" }] } });
    const created = await client.viewLinkCreate(space.id, plan);
    expect(created.url).not.toBe("");
    const listed = await client.viewLinkList(space.id);
    expect(listed).toHaveLength(2);
    expect(listed.map((link) => link.url)).toEqual(["", ""]);
    // What the list says is otherwise the link as it was made.
    const again = listed.find((link) => link.linkId === created.linkId);
    expect(again).toEqual({ ...created, url: "" });
  });

  it("shows no URL in the answer of a revoke either", async () => {
    const { client, space, plan } = await setup();
    const first = await client.viewLinkCreate(space.id, plan);
    await client.viewLinkCreate(space.id, plan);
    const remaining = await client.viewLinkRevoke(space.id, first.linkId);
    expect(remaining.map((link) => link.url)).toEqual([""]);
  });
});

describe("publishing and expiry", () => {
  it("is Publishing until the first upload succeeded, then Live with the time of that upload, and says so in an event", async () => {
    const { client, space, plan, events } = await setup();
    const link = await client.viewLinkCreate(space.id, plan);
    expect(linkEvents(events)).toEqual([{ type: "ViewLinksChanged", data: { spaceId: space.id } }]);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Publishing");
    vi.advanceTimersByTime(DEFAULT_LINK_PUBLISH_MS - 1);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Publishing");
    vi.advanceTimersByTime(1);
    const live = only(await client.viewLinkList(space.id));
    expect(live).toMatchObject({ linkId: link.linkId, status: "Live", lastPublishedAt: START + DEFAULT_LINK_PUBLISH_MS });
    expect(linkEvents(events)).toHaveLength(2);
  });

  it("takes as long as the option says to publish", async () => {
    const { client, space, plan } = await setup("owner", { links: { publishMs: 50 } });
    await client.viewLinkCreate(space.id, plan);
    vi.advanceTimersByTime(49);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Publishing");
    vi.advanceTimersByTime(1);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Live");
  });

  it("is Expired once its time has come, and is still listed until someone revokes it", async () => {
    const { client, space, plan } = await setup();
    const link = await client.viewLinkCreate(space.id, plan, 2);
    vi.advanceTimersByTime(DEFAULT_LINK_PUBLISH_MS);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Live");
    vi.advanceTimersByTime(2 * DAY - DEFAULT_LINK_PUBLISH_MS - 1);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Live");
    vi.advanceTimersByTime(1);
    expect(only(await client.viewLinkList(space.id))).toMatchObject({ linkId: link.linkId, status: "Expired" });
    // Revoking an expired link cleans it up.
    expect(await client.viewLinkRevoke(space.id, link.linkId)).toEqual([]);
  });

  it("is Expired before it ever published when it expires first", async () => {
    const { client, space } = await setup("owner", {
      links: { publishMs: 5_000, links: [{ space: "Work", page: "Plan", publishing: true, expiresInMs: 1_000 }] },
    });
    vi.advanceTimersByTime(1_000);
    expect(only(await client.viewLinkList(space.id)).status).toBe("Expired");
  });

  it("finishes the uploads under way when the core locks, as it finishes a share", async () => {
    const { client, space, plan, memory, events } = await setup("owner", { security: { kitCreatedAt: START, appLockPassphrase: "correct horse" } });
    await client.viewLinkCreate(space.id, plan);
    await memory.lock();
    await memory.unlock("correct horse");
    expect(only(await client.viewLinkList(space.id)).status).toBe("Live");
    // The finish is silent: the lock stopped the events, and the timer is gone.
    const before = linkEvents(events).length;
    vi.advanceTimersByTime(10 * DEFAULT_LINK_PUBLISH_MS);
    expect(linkEvents(events)).toHaveLength(before);
  });
});

describe("listing", () => {
  it("lists the links of a space newest first, for any member, a Viewer and a read-only copy included", async () => {
    const seeded: MemoryTransportOptions["links"] = {
      links: [
        { space: "Work", page: "Plan", ageMs: 3 * DAY, createdBy: "Alice" },
        { space: "Work", page: "Notes", ageMs: DAY, createdBy: "Bob" },
        { space: "Work", page: "Sub", ageMs: 2 * DAY },
      ],
    };
    for (const role of ["owner", "editor", "commenter", "viewer"] as const) {
      const { client, space } = await setup(role, { links: seeded });
      const listed = await client.viewLinkList(space.id);
      expect(listed.map((link) => link.createdByName)).toEqual(["Bob", "Maya", "Alice"]);
    }
    const archive = await setup("viewer", {
      links: seeded,
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "viewer", archive: "Removed" }] },
    });
    expect(await archive.client.viewLinkList(archive.space.id)).toHaveLength(3);
  });

  it("orders the links made at the same moment with the later one first", async () => {
    const { client, space, plan } = await setup();
    const first = await client.viewLinkCreate(space.id, plan);
    const second = await client.viewLinkCreate(space.id, plan);
    expect((await client.viewLinkList(space.id)).map((link) => link.linkId)).toEqual([second.linkId, first.linkId]);
  });

  it("lists only the links of the space asked about", async () => {
    const { client, space, plan } = await setup("personal");
    const personal = await spaceNamed(client, "Personal");
    const diary = await pageId(client, personal.id, "Diary");
    await client.viewLinkCreate(space.id, plan);
    await client.viewLinkCreate(personal.id, diary);
    expect(only(await client.viewLinkList(space.id)).pageId).toBe(plan);
    expect(only(await client.viewLinkList(personal.id)).pageId).toBe(diary);
  });

  it("names a creator who left the space by the last name it knew them by: a link outlives its creator", async () => {
    const { client, space } = await setup("owner", { links: { links: [{ space: "Work", page: "Plan", createdBy: "Bob" }] } });
    expect(only(await client.viewLinkList(space.id)).createdByName).toBe("Bob");
    const members = await client.spaceMembers(space.id);
    const bob = members.members.find((member) => member.name === "Bob");
    await client.memberRemove(space.id, bob?.accountId ?? "");
    expect(only(await client.viewLinkList(space.id))).toMatchObject({ createdByName: "Bob" });
  });
});

describe("revoking", () => {
  it("lets the creator revoke a link of their own, and answers the links that remain", async () => {
    const { client, space, plan, events } = await setup("editor", {}, { editorsMayLink: true });
    const first = await client.viewLinkCreate(space.id, plan);
    const second = await client.viewLinkCreate(space.id, plan);
    events.length = 0;
    const remaining = await client.viewLinkRevoke(space.id, first.linkId);
    expect(remaining.map((link) => link.linkId)).toEqual([second.linkId]);
    expect(linkEvents(events)).toEqual([{ type: "ViewLinksChanged", data: { spaceId: space.id } }]);
    expect((await client.viewLinkList(space.id)).map((link) => link.linkId)).toEqual([second.linkId]);
  });

  it("lets an Owner revoke the link of another member", async () => {
    const { client, space } = await setup("owner", { links: { links: [{ space: "Work", page: "Plan", createdBy: "Bob" }] } });
    const link = only(await client.viewLinkList(space.id));
    expect(await client.viewLinkRevoke(space.id, link.linkId)).toEqual([]);
  });

  it("refuses an Editor, a Commenter and a Viewer the link of another member: forbidden_role(Owner)", async () => {
    for (const role of ["editor", "commenter", "viewer"] as const) {
      const { client, space } = await setup(role, {
        links: { links: [{ space: "Work", page: "Plan", createdBy: "Alice" }] },
      }, { editorsMayLink: true });
      const link = only(await client.viewLinkList(space.id));
      const refused = await failure(client.viewLinkRevoke(space.id, link.linkId));
      expect(refused).toMatchObject({ code: "Forbidden", details: { reason: "role", required: "owner" } });
      expect(await client.viewLinkList(space.id)).toHaveLength(1);
    }
  });

  it("refuses a read-only copy: archived", async () => {
    const { client, space } = await setup("owner", {
      links: { links: [{ space: "Work", page: "Plan" }] },
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "owner", archive: "Removed" }] },
    });
    const link = only(await client.viewLinkList(space.id));
    expect(await failure(client.viewLinkRevoke(space.id, link.linkId))).toMatchObject({ code: "Forbidden", details: { reason: "archived" } });
  });

  it("is InvalidInput for `linkId` when the space has no such link: unknown, already revoked, or another space's", async () => {
    const { client, space, plan } = await setup("personal");
    const personal = await spaceNamed(client, "Personal");
    const diary = await pageId(client, personal.id, "Diary");
    const link = await client.viewLinkCreate(space.id, plan);
    const elsewhere = await client.viewLinkCreate(personal.id, diary);
    await client.viewLinkRevoke(space.id, link.linkId);
    for (const id of [link.linkId, elsewhere.linkId, "00000000-0000-4000-8000-c00000000999"]) {
      const error = await failure(client.viewLinkRevoke(space.id, id));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "linkId" } });
    }
    expect(only(await client.viewLinkList(personal.id)).linkId).toBe(elsewhere.linkId);
  });

  it("stops the first upload of a link that is revoked while it publishes", async () => {
    const { client, space, plan, events } = await setup();
    const link = await client.viewLinkCreate(space.id, plan);
    await client.viewLinkRevoke(space.id, link.linkId);
    const before = linkEvents(events).length;
    vi.advanceTimersByTime(10 * DEFAULT_LINK_PUBLISH_MS);
    expect(linkEvents(events)).toHaveLength(before);
    expect(await client.viewLinkList(space.id)).toEqual([]);
  });
});

describe("a space that goes", () => {
  it("loses its links when this account closes it, and says so", async () => {
    const { client, space, plan, events } = await setup();
    const personal = await spaceNamed(client, "Personal");
    await client.viewLinkCreate(space.id, plan);
    events.length = 0;
    await client.spaceClose(space.id);
    expect(await client.viewLinkList(space.id)).toEqual([]);
    expect(linkEvents(events)).toEqual([{ type: "ViewLinksChanged", data: { spaceId: space.id } }]);
    // Another space's links stay.
    expect(await client.viewLinkList(personal.id)).toEqual([]);
  });

  it("loses its links when another Owner closes it", async () => {
    const { client, space, memory, events } = await setup("editor", { links: { links: [{ space: "Work", page: "Plan", createdBy: "Alice" }] } });
    expect(await client.viewLinkList(space.id)).toHaveLength(1);
    events.length = 0;
    memory.receiveSharing({ type: "closed", spaceId: space.id });
    expect(await client.viewLinkList(space.id)).toEqual([]);
    expect(linkEvents(events)).toHaveLength(1);
  });

  it("keeps the links of a space it closed in another one: only the closed space loses them", async () => {
    const { client, space, memory } = await setup("owner", {
      links: {
        links: [
          { space: "Work", page: "Plan" },
          { space: "Personal", page: "Diary" },
        ],
      },
    });
    const personal = await spaceNamed(client, "Personal");
    memory.receiveSharing({ type: "closed", spaceId: space.id });
    expect(await client.viewLinkList(space.id)).toEqual([]);
    expect(await client.viewLinkList(personal.id)).toHaveLength(1);
  });

  it("keeps listing the links of a read-only copy this account was removed from, and drops them with the copy", async () => {
    const { client, space, memory } = await setup("editor", { links: { links: [{ space: "Work", page: "Plan", createdBy: "Alice" }] } });
    memory.receiveSharing({ type: "removed", spaceId: space.id });
    expect(await client.viewLinkList(space.id)).toHaveLength(1);
    await client.spaceLeave(space.id);
    expect((await failure(client.viewLinkList(space.id))).code).toBe("NotFound");
  });
});

describe("the seeds", () => {
  it("starts live, publishing, expiring and expired links without events, with the creators and ages asked for", async () => {
    const { client, space, events } = await setup("owner", {
      links: {
        publishMs: 100,
        links: [
          { space: "Work", page: "Plan", createdBy: "Bob", ageMs: 5 * DAY },
          { space: "Work", page: "Plan", ageMs: 4 * DAY, expiresInMs: 3 * DAY },
          { space: "Work", page: "Notes", ageMs: 3 * DAY, expiresInMs: -DAY },
          { space: "Work", page: "Sub", ageMs: DAY, publishing: true },
        ],
      },
    });
    expect(linkEvents(events)).toEqual([]);
    const listed = await client.viewLinkList(space.id);
    expect(listed.map((link) => [link.createdByName, link.status])).toEqual([
      ["Maya", "Publishing"],
      ["Maya", "Expired"],
      ["Maya", "Live"],
      ["Bob", "Live"],
    ]);
    expect(listed.map((link) => link.expiresAt)).toEqual([undefined, START - DAY, START + 3 * DAY, undefined]);
    expect(listed[3]?.lastPublishedAt).toBe(START - 5 * DAY);
    expect(listed[0]?.lastPublishedAt).toBeUndefined();
    vi.advanceTimersByTime(100);
    expect((await client.viewLinkList(space.id))[0]).toMatchObject({ status: "Live", lastPublishedAt: START + 100 });
    expect(linkEvents(events)).toHaveLength(1);
  });

  it("draws no id from the transport's generator, so the seeded spaces and pages keep theirs", async () => {
    const drawn = { plain: 0, linked: 0 };
    const ids = (key: "plain" | "linked") => () => {
      drawn[key] += 1;
      return `00000000-0000-4000-8000-${String(drawn[key]).padStart(12, "0")}`;
    };
    const plain = build("owner", { newId: ids("plain") });
    const linked = build("owner", { newId: ids("linked"), links: { links: [{ space: "Work", page: "Plan" }] } });
    expect(drawn.linked).toBe(drawn.plain);
    expect((await plain.client.spaceList()).map((space) => space.id)).toEqual((await linked.client.spaceList()).map((space) => space.id));
  });

  it("refuses a seed that names a page that is not a seeded page", () => {
    expect(() => build("owner", { links: { links: [{ space: "Work", page: "Nope" }] } })).toThrow(/not a seeded page/u);
    expect(() => build("owner", { links: { links: [{ space: "Nope", page: "Plan" }] } })).toThrow(/not a seeded page/u);
  });
});
