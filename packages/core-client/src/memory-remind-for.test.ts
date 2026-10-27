// Who a reminder is for in the in-memory core (Milestone 13, slice 2; ADR-0047): the `reminderFor` of a date mention names the account
// whose devices fire the reminder of a shared space (every role, a Viewer included); a mention that names nobody fires on the devices
// of the space's Owners (the rule of slice 1); a personal space's reminders always fire; a read-only copy fires none. The mirror
// answers `RemindersDue` and ticks like the core: only the reminders this device fires.

import { LoroDoc, LoroList, LoroMap } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import type { Event } from "./generated/Event";
import type { MemberRole } from "./generated/MemberRole";
import type { ReminderDue } from "./generated/ReminderDue";
import { MemoryTransport } from "./memory";
import { extractDateMentions } from "./page-doc";
import type { SeedSpace } from "./structure";

const HOUR = 3600;

/** 2026-10-07 14:30 on the wall clock, in seconds since 1970-01-01T00:00:00. */
const OCT_7_1430 = Date.UTC(2026, 9, 7, 14, 30) / 1000;

/** The wall clock reading `seconds` (a local time) as an instant on this machine: what `now` returns. */
function atLocal(seconds: number): number {
  const wall = new Date(seconds * 1000);
  return new Date(wall.getUTCFullYear(), wall.getUTCMonth(), wall.getUTCDate(), wall.getUTCHours(), wall.getUTCMinutes(), wall.getUTCSeconds()).getTime();
}

const BLOCK = "00000000-0000-4000-8000-0000000000b1";

const SPACES: readonly SeedSpace[] = [
  {
    name: "Work",
    pages: [
      {
        title: "Standup",
        content: { type: "doc", content: [{ type: "paragraph", attrs: { id: BLOCK }, content: [{ type: "text", text: "Daily " }] }] },
      },
    ],
  },
  {
    name: "Personal",
    pages: [
      {
        title: "Diary",
        content: {
          type: "doc",
          content: [
            {
              type: "paragraph",
              attrs: { id: BLOCK },
              content: [{ type: "dateMention", attrs: { id: "00000000-0000-4000-8000-0000000000d1", date: "2026-10-07", time: "14:30", reminder: 15 } }],
            },
          ],
        },
      },
    ],
  },
];

function build(role: MemberRole | "personal") {
  const clock = { now: atLocal(OCT_7_1430 - 2 * HOUR) };
  const memory = new MemoryTransport({
    now: () => clock.now,
    workspaceName: "Home",
    spaces: SPACES,
    sharing: { profileName: "Maya", ...(role === "personal" ? {} : { spaces: [{ space: "Work", role }] }) },
  });
  const client = new CoreClient(memory);
  const events: Event[] = [];
  memory.subscribe((event) => {
    events.push(event);
  });
  return { memory, client, events, clock };
}

async function spaceId(client: CoreClient, name: string): Promise<string> {
  const found = (await client.spaceList()).find((space) => space.name === name);
  if (found === undefined) {
    throw new Error(`no space ${name}`);
  }
  return found.id;
}

async function pageId(client: CoreClient, title: string, space: string): Promise<string> {
  const found = (await client.treeGet(await spaceId(client, space))).roots.find((node) => node.page.title === title);
  if (found === undefined) {
    throw new Error(`no page ${title}`);
  }
  return found.page.id;
}

/** Adds a date mention at the end of the first paragraph: 2026-10-07 14:30 with a reminder 15 minutes before, for `reminderFor`. */
function addMention(node: string, reminderFor?: string): (doc: LoroDoc) => void {
  return (doc) => {
    const children = doc.getMap("doc").get("children") as LoroList;
    const paragraph = children.get(0) as LoroMap;
    const inline = paragraph.get("children") as LoroList;
    const mention = inline.insertContainer(inline.length, new LoroMap());
    mention.set("nodeName", "dateMention");
    const attributes = mention.setContainer("attributes", new LoroMap());
    attributes.set("id", node);
    attributes.set("date", "2026-10-07");
    attributes.set("time", "14:30");
    attributes.set("reminder", 15);
    if (reminderFor !== undefined) {
      attributes.set("reminderFor", reminderFor);
    }
    mention.setContainer("children", new LoroList());
  };
}

const ME = "00000000-0000-4000-8000-0000000000c1";
const BOB = "00000000-0000-4000-8000-0000000000c2";
const NOBODY = "00000000-0000-4000-8000-0000000000c3";
const DAMAGED = "00000000-0000-4000-8000-0000000000c4";

/** The Standup page of the Work space with a mention for this account, Bob, nobody and a damaged `reminderFor`. */
async function standup(role: MemberRole | "personal") {
  const built = build(role);
  const { client, memory } = built;
  const page = await pageId(client, "Standup", "Work");
  if (role === "personal") {
    return { ...built, page, me: "", bob: "" };
  }
  const people = await client.peopleMentionable(await spaceId(client, "Work"));
  const me = people.find((person) => person.isMe)?.accountId ?? "";
  const bob = people.find((person) => person.name === "Bob")?.accountId ?? "";
  memory.changePage(page, addMention(ME, me));
  memory.changePage(page, addMention(BOB, bob));
  memory.changePage(page, addMention(NOBODY));
  memory.changePage(page, addMention(DAMAGED, "not-an-account"));
  return { ...built, page, me, bob };
}

const nodes = (due: ReminderDue[]): string[] => due.map((reminder) => reminder.nodeId).sort();

describe("who a reminder is for", () => {
  it("reads the account of reminderFor from the document, and a damaged value as none", async () => {
    const { memory, page, me } = await standup("owner");
    const loaded = new LoroDoc();
    loaded.import(await memory.docs.open(page));
    const found = extractDateMentions(loaded);
    expect(found.map((mention) => [mention.nodeId, mention.reminderFor])).toEqual([
      [ME, me],
      [BOB, found[1]?.reminderFor],
      [NOBODY, undefined],
      [DAMAGED, undefined],
    ]);
    expect(found[1]?.reminderFor).not.toBe(me);
  });

  it("fires on an Owner's device for the mentions that name this account or nobody, not for another account's", async () => {
    const { client } = await standup("owner");
    const due = await client.remindersDue(OCT_7_1430);
    // The Owner hears its own, the ones that name nobody and the damaged one (it names nobody), not Bob's.
    expect(nodes(due.filter((reminder) => reminder.pageTitle === "Standup"))).toEqual([ME, NOBODY, DAMAGED]);
  });

  it("fires on an Editor's device only for the mention that names it, never the ones that name nobody", async () => {
    const { client } = await standup("editor");
    expect(nodes(await client.remindersDue(OCT_7_1430))).toEqual([ME, "00000000-0000-4000-8000-0000000000d1"].sort());
  });

  it("fires on a Viewer's device for the mention that names it: the reminder is theirs", async () => {
    const { client } = await standup("viewer");
    const due = await client.remindersDue(OCT_7_1430);
    expect(nodes(due.filter((reminder) => reminder.pageTitle === "Standup"))).toEqual([ME]);
  });

  it("fires every reminder of a personal space, and of a space that is not shared", async () => {
    const { client } = await standup("personal");
    expect(nodes(await client.remindersDue(OCT_7_1430))).toEqual(["00000000-0000-4000-8000-0000000000d1"]);
    // The personal space of a shared workspace fires too, whatever the role in the shared one.
    const shared = await standup("viewer");
    expect(nodes((await shared.client.remindersDue(OCT_7_1430)).filter((reminder) => reminder.pageTitle === "Diary"))).toEqual([
      "00000000-0000-4000-8000-0000000000d1",
    ]);
  });

  it("fires nothing of a shared space that became a read-only copy", async () => {
    const { client, memory } = await standup("owner");
    const space = await spaceId(client, "Work");
    memory.receiveSharing({ type: "closed", spaceId: space });
    const due = await client.remindersDue(OCT_7_1430);
    expect(due.filter((reminder) => reminder.pageTitle === "Standup")).toEqual([]);
  });

  it("ticks only the reminders this device fires, once each", async () => {
    const { client, events, clock } = await standup("editor");
    clock.now = atLocal(OCT_7_1430 - 10 * 60);
    // 14:20: the reminders of 14:15 are due; the Editor's device fires its own and the Diary's, not Bob's or the Owners'.
    expect(await client.remindersTick()).toBe(2);
    const fired = events.flatMap((event) => (event.type === "ReminderDue" ? [event.data.nodeId] : [])).sort();
    expect(fired).toEqual([ME, "00000000-0000-4000-8000-0000000000d1"].sort());
    expect(await client.remindersTick()).toBe(0);
  });
});
