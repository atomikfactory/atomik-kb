// Presence in the in-memory mirror (Milestone 13, slice 3, `./memory-presence`): `PresenceList` and `PresenceChanged`, the
// frames the editor gets (`PresenceUpdate`), the colour by join order, the 30-second timeout, what removes a device, what this
// device sends (nothing in a personal space), and the scripted people with their cursors in the page's own text.

import { Cursor, EphemeralStore, LoroDoc } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import type { CoreError } from "./errors";
import type { Event } from "./generated/Event";
import type { MemberRole } from "./generated/MemberRole";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import { PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATE_MAX_BYTES } from "./presence";
import type { PresenceUpdate } from "./transport";
import type { SeedSpace } from "./structure";

const START = 1_700_000_000_000;
const SERVER = "https://sync.example.com";
const TIMEOUT_MS = PRESENCE_TIMEOUT_SECONDS * 1000;

const SPACES: readonly SeedSpace[] = [
  {
    name: "Work",
    pages: [
      {
        title: "Plan",
        content: {
          type: "doc",
          content: [
            { type: "paragraph", attrs: { id: "00000000-0000-4000-8000-000000000001" }, content: [{ type: "text", text: "Ship it on Friday" }] },
          ],
        },
      },
      { title: "Notes" },
    ],
  },
  { name: "Personal", pages: [{ title: "Diary" }] },
];

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(START);
});

afterEach(() => {
  vi.useRealTimers();
});

/** This account is Maya, the creator and Owner of Work (join order 0); the others join in order: Bob (1), Carol (2), Dan (3). */
function build(role: MemberRole | "personal" = "owner", options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({
    now: () => Date.now(),
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
  const frames: PresenceUpdate[] = [];
  memory.presence.subscribe((frame) => frames.push(frame));
  return { memory, client, events, frames };
}

async function spaceId(client: CoreClient, name = "Work"): Promise<string> {
  const found = (await client.spaceList()).find((space) => space.name === name);
  if (found === undefined) {
    throw new Error(`no space ${name}`);
  }
  return found.id;
}

async function pageId(client: CoreClient, title: string, space = "Work"): Promise<string> {
  const found = (await client.treeGet(await spaceId(client, space))).roots.find((node) => node.page.title === title);
  if (found === undefined) {
    throw new Error(`no page ${title}`);
  }
  return found.page.id;
}

async function accountOf(client: CoreClient, name: string): Promise<string> {
  const person = (await client.peopleMentionable(await spaceId(client))).find((candidate) => candidate.name === name);
  if (person === undefined) {
    throw new Error(`no person ${name}`);
  }
  return person.accountId;
}

const changes = (events: readonly Event[]): Event[] => events.filter((event) => event.type === "PresenceChanged");

/** The cursors of an `EphemeralStore` update, as a receiver decodes them (the key the sender used is not read). */
function cursorsOf(update: Uint8Array): { anchor?: Cursor; focus?: Cursor } {
  const store = new EphemeralStore(7 * 24 * 3600 * 1000);
  try {
    store.apply(update);
    const [state] = Object.values(store.getAllStates()) as { anchor?: Uint8Array; focus?: Uint8Array }[];
    return {
      ...(state?.anchor === undefined ? {} : { anchor: Cursor.decode(state.anchor) }),
      ...(state?.focus === undefined ? {} : { focus: Cursor.decode(state.focus) }),
    };
  } finally {
    store.destroy();
  }
}

/** Where a page's text holds a cursor, in the text of the editor's own copy of the page. */
async function offsetsOf(memory: MemoryTransport, page: string, update: Uint8Array): Promise<[number, number] | undefined> {
  const doc = new LoroDoc();
  doc.import(await memory.docs.open(page));
  const { anchor, focus } = cursorsOf(update);
  if (anchor === undefined || focus === undefined) {
    return undefined;
  }
  const at = (cursor: Cursor): number => doc.getCursorPos(cursor)?.offset ?? -1;
  return [at(anchor), at(focus)];
}

/** The code of the error a call throws, `undefined` when it does not. */
function thrownCode(call: () => void): unknown {
  try {
    call();
  } catch (error) {
    return (error as CoreError).code;
  }
  return undefined;
}

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  try {
    await promise;
  } catch (error) {
    return error as CoreError;
  }
  throw new Error("expected a rejection");
}

describe("PresenceList", () => {
  it("is empty until somebody is present, and a personal space has none", async () => {
    const { client } = build();
    expect(await client.presenceList(await spaceId(client))).toEqual([]);
    expect(await client.presenceList(await spaceId(client, "Personal"))).toEqual([]);
  });

  it("answers NotFound for a space it does not know", async () => {
    const { client } = build();
    expect((await failure(client.presenceList("99999999-9999-4999-8999-999999999999"))).code).toBe("NotFound");
  });

  it("lists a device with the member's name, the colour of their join order and the clock of the frame", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Carol", pageId: plan });
    vi.advanceTimersByTime(4_000);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan, select: { quote: "Friday" } });
    const list = await client.presenceList(work);
    // Ordered by join order, Bob (1) before Carol (2); the time is the mirror's clock at the frame, never a claim of the sender.
    expect(list).toEqual([
      {
        accountId: await accountOf(client, "Bob"),
        deviceId: expect.stringMatching(/^[0-9a-f]{8}-/u) as string,
        name: "Bob",
        color: 1,
        pageId: plan,
        state: "Editing",
        updatedAt: START + 4_000,
      },
      {
        accountId: await accountOf(client, "Carol"),
        deviceId: expect.stringMatching(/^[0-9a-f]{8}-/u) as string,
        name: "Carol",
        color: 2,
        pageId: plan,
        state: "Viewing",
        updatedAt: START,
      },
    ]);
  });

  it("gives a member's colour by join order whoever joins in which role", async () => {
    const { memory, client } = build("editor");
    const work = await spaceId(client);
    // Alice (Owner) created the space (0); this account joined with the others: Maya, Carol, Dan after her.
    for (const member of ["Alice", "Bob", "Carol", "Dan"]) {
      memory.receivePresence({ type: "frame", spaceId: work, member });
    }
    const colours = Object.fromEntries((await client.presenceList(work)).map((info) => [info.name, info.color]));
    const order = (await client.peopleMentionable(work)).map((person) => person.name);
    expect(colours).toEqual(Object.fromEntries(["Alice", "Bob", "Carol", "Dan"].map((name) => [name, order.indexOf(name)])));
    expect(colours.Alice).toBe(0);
    expect(new Set(Object.values(colours)).size).toBe(4);
  });

  it("an idle device is in no page, and a device in a page without a cursor views it", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan, state: "Idle" });
    const [bob] = await client.presenceList(work);
    expect(bob).toMatchObject({ name: "Bob", state: "Idle" });
    expect(bob).not.toHaveProperty("pageId");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan });
    expect((await client.presenceList(work))[0]).toMatchObject({ state: "Viewing", pageId: plan });
  });

  it("keeps two devices of one member apart", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", device: 0, pageId: await pageId(client, "Plan") });
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", device: 1, pageId: await pageId(client, "Notes") });
    const list = await client.presenceList(work);
    expect(list).toHaveLength(2);
    expect(new Set(list.map((info) => info.deviceId)).size).toBe(2);
    expect(list.map((info) => info.color)).toEqual([1, 1]);
  });
});

describe("PresenceChanged and the frames", () => {
  it("fires when a device appears, changes page or state, or goes, and not for a cursor move", async () => {
    const { memory, client, events, frames } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    const notes = await pageId(client, "Notes");
    const frame = (select?: string, pageId = plan) =>
      memory.receivePresence({
        type: "frame",
        spaceId: work,
        member: "Bob",
        pageId,
        ...(select === undefined ? {} : { select: { quote: select } }),
      });

    frame("Ship");
    expect(changes(events)).toEqual([{ type: "PresenceChanged", data: { spaceId: work } }]);
    // A cursor move: the same page and state.
    frame("it");
    frame("Friday");
    expect(changes(events)).toHaveLength(1);
    expect(frames).toHaveLength(3);
    // A page, a state: the avatars change.
    frame("Ship", notes);
    expect(changes(events)).toHaveLength(2);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: notes, state: "Viewing" });
    expect(changes(events)).toHaveLength(3);
    memory.receivePresence({ type: "leave", spaceId: work, member: "Bob" });
    expect(changes(events)).toHaveLength(4);
    expect(await client.presenceList(work)).toEqual([]);
    // Leaving a device that is not there is nothing.
    memory.receivePresence({ type: "leave", spaceId: work, member: "Bob" });
    expect(changes(events)).toHaveLength(4);
  });

  it("hands the listener the accepted frame: who and where, and the update", async () => {
    const { memory, client, frames } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan, select: { quote: "Friday" } });
    const [frame] = frames;
    expect(frame?.info).toMatchObject({ name: "Bob", color: 1, pageId: plan, state: "Editing", updatedAt: START });
    expect(frame?.update.length).toBeGreaterThan(0);
    expect(frame?.update.length).toBeLessThanOrEqual(PRESENCE_UPDATE_MAX_BYTES);
    // The cursors are Loro cursors of the page's own text: the words are selected.
    expect(await offsetsOf(memory, plan, frame?.update ?? new Uint8Array())).toEqual([11, 17]);
  });

  it("puts a caret at one end of the words when asked", async () => {
    const { memory, client, frames } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan, select: { quote: "Friday", caret: "start" } });
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan, select: { quote: "Friday", caret: "end" } });
    expect(await offsetsOf(memory, plan, frames[0]?.update ?? new Uint8Array())).toEqual([11, 11]);
    expect(await offsetsOf(memory, plan, frames[1]?.update ?? new Uint8Array())).toEqual([17, 17]);
  });

  it("passes an update through as it is, garbage included, and a frame of a device in no page carries none", async () => {
    const { memory, client, frames } = build();
    const work = await spaceId(client);
    const garbage = Uint8Array.of(9, 9, 9);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Plan"), update: garbage });
    expect(frames[0]?.update).toEqual(garbage);
    expect(frames[0]?.info.state).toBe("Editing");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob" });
    expect(frames[1]?.update).toEqual(new Uint8Array());
    expect(frames[1]?.info.state).toBe("Idle");
  });

  it("a listener that throws does not stop the others, and a removed listener hears nothing", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    const heard: string[] = [];
    memory.presence.subscribe(() => {
      throw new Error("a bug in a listener");
    });
    const stop = memory.presence.subscribe((frame) => heard.push(frame.info.name));
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob" });
    stop();
    memory.receivePresence({ type: "frame", spaceId: work, member: "Carol" });
    expect(heard).toEqual(["Bob"]);
  });
});

describe("the timeout", () => {
  it("a device is gone 30 seconds after its latest frame, and PresenceChanged says so", async () => {
    const { memory, client, events } = build();
    const work = await spaceId(client);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Plan") });
    expect(changes(events)).toHaveLength(1);
    // The mirror's own timer fires when the device is due.
    vi.advanceTimersByTime(TIMEOUT_MS - 1_000);
    expect(await client.presenceList(work)).toHaveLength(1);
    vi.advanceTimersByTime(2_000);
    expect(changes(events)).toHaveLength(2);
    expect(await client.presenceList(work)).toEqual([]);
  });

  it("another frame starts the 30 seconds again", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan });
    vi.advanceTimersByTime(20_000);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: plan });
    vi.advanceTimersByTime(20_000);
    expect(await client.presenceList(work)).toHaveLength(1);
    vi.advanceTimersByTime(11_000);
    expect(await client.presenceList(work)).toEqual([]);
  });

  it("is asked for by a read and by presenceExpire when the clock is injected", async () => {
    let now = START;
    const { memory, client, events } = build("owner", { now: () => now });
    const work = await spaceId(client);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Plan") });
    now += TIMEOUT_MS - 1;
    expect(await client.presenceList(work)).toHaveLength(1);
    now += 1;
    memory.presenceExpire();
    expect(changes(events)).toHaveLength(2);
    expect(await client.presenceList(work)).toEqual([]);
  });
});

describe("the rules of an accepted frame", () => {
  it("presence exists only in a live shared space", async () => {
    const { memory, client } = build();
    const personal = await spaceId(client, "Personal");
    expect(thrownCode(() => { memory.receivePresence({ type: "frame", spaceId: personal, member: "Bob" }); })).toBe("InvalidInput");
    expect(await client.presenceList(personal)).toEqual([]);
  });

  it("refuses this account and a stranger", async () => {
    const { memory, client } = build();
    const work = await spaceId(client);
    const code = (change: Parameters<MemoryTransport["receivePresence"]>[0]): unknown =>
      thrownCode(() => {
        memory.receivePresence(change);
      });
    expect(code({ type: "frame", spaceId: work, member: "Maya" })).toBe("InvalidInput");
    expect(code({ type: "frame", spaceId: work, member: "Zed" })).toBe("InvalidInput");
    expect(code({ type: "frame", spaceId: work, member: "Bob", device: -1 })).toBe("InvalidInput");
  });

  it("a page this device does not hold in the space is no page: the member is in the space and on no page", async () => {
    const { memory, client, events } = build();
    const work = await spaceId(client);
    // A page of another space, and a page nobody knows.
    for (const page of [await pageId(client, "Diary", "Personal"), "99999999-9999-4999-8999-999999999999"]) {
      events.length = 0;
      memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: page, select: { quote: "Ship" } });
      const [bob] = await client.presenceList(work);
      expect(bob).toMatchObject({ name: "Bob", state: "Editing" });
      expect(bob).not.toHaveProperty("pageId");
      memory.receivePresence({ type: "leave", spaceId: work, member: "Bob" });
    }
    // Moving from a page to one that is none is a change of page: the avatars follow.
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Plan") });
    events.length = 0;
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Diary", "Personal") });
    expect(changes(events)).toHaveLength(1);
  });

  it("a member who leaves or is removed takes their devices with them", async () => {
    const { memory, client, events } = build();
    const work = await spaceId(client);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob" });
    memory.receivePresence({ type: "frame", spaceId: work, member: "Carol" });
    memory.receivePresence({ type: "frame", spaceId: work, member: "Dan" });
    events.length = 0;
    memory.receiveSharing({ type: "memberLeft", spaceId: work, accountId: await accountOf(client, "Bob") });
    expect((await client.presenceList(work)).map((info) => info.name)).toEqual(["Carol", "Dan"]);
    expect(changes(events)).toEqual([{ type: "PresenceChanged", data: { spaceId: work } }]);
    // This account removes one: its own request.
    await client.memberRemove(work, await accountOf(client, "Carol"));
    expect((await client.presenceList(work)).map((info) => info.name)).toEqual(["Dan"]);
  });

  it("a space that stops being live has no presence: this account was removed, or an Owner closed it", async () => {
    const first = build("editor");
    const work = await spaceId(first.client);
    first.memory.receivePresence({ type: "frame", spaceId: work, member: "Alice" });
    first.memory.receiveSharing({ type: "removed", spaceId: work });
    expect(await first.client.presenceList(work)).toEqual([]);
    expect(thrownCode(() => { first.memory.receivePresence({ type: "frame", spaceId: work, member: "Alice" }); })).toBe("InvalidInput");

    const second = build();
    const space = await spaceId(second.client);
    second.memory.receivePresence({ type: "frame", spaceId: space, member: "Bob" });
    await second.client.spaceClose(space);
    expect(await second.client.presenceList(space)).toEqual([]);
  });

  it("is in memory only: a lock drops it at once, with PresenceChanged, and unlocking does not bring it back", async () => {
    const { memory, client, events } = build("owner", {
      security: { kitCreatedAt: START, appLockPassphrase: "correct horse battery" },
    });
    const work = await spaceId(client);
    memory.receivePresence({ type: "frame", spaceId: work, member: "Bob", pageId: await pageId(client, "Plan") });
    events.length = 0;
    await memory.lock();
    expect(changes(events)).toEqual([{ type: "PresenceChanged", data: { spaceId: work } }]);
    await memory.unlock("correct horse battery");
    expect(await client.presenceList(work)).toEqual([]);
  });

  it("is not a local edit: a frame makes no pending sync entry", async () => {
    const { memory, client } = build();
    const before = (await client.syncStatus()).pendingEntries;
    memory.receivePresence({ type: "frame", spaceId: await spaceId(client), member: "Bob" });
    expect((await client.syncStatus()).pendingEntries).toBe(before);
  });
});

describe("what this device sends", () => {
  it("sends nothing in a personal space, and that is no error", async () => {
    const { memory, client } = build();
    await memory.presence.send(await spaceId(client, "Personal"), await pageId(client, "Diary", "Personal"), "Editing", Uint8Array.of(1, 2));
    expect(memory.sentPresence()).toEqual([]);
  });

  it("sends in a live shared space, a Viewer's included", async () => {
    for (const role of ["owner", "viewer"] as const) {
      const { memory, client } = build(role);
      const work = await spaceId(client);
      const plan = await pageId(client, "Plan");
      await memory.presence.send(work, plan, "Viewing", Uint8Array.of(1, 2, 3));
      await memory.presence.send(work, undefined, "Idle", new Uint8Array());
      expect(memory.sentPresence()).toEqual([
        { spaceId: work, pageId: plan, state: "Viewing", update: Uint8Array.of(1, 2, 3) },
        { spaceId: work, pageId: undefined, state: "Idle", update: new Uint8Array() },
      ]);
    }
  });

  it("sends nothing in a read-only copy, and refuses an update over the limit", async () => {
    const copy = build("editor");
    const work = await spaceId(copy.client);
    copy.memory.receiveSharing({ type: "removed", spaceId: work });
    await copy.memory.presence.send(work, undefined, "Idle", new Uint8Array());
    expect(copy.memory.sentPresence()).toEqual([]);

    const { memory, client } = build();
    const space = await spaceId(client);
    const error = await failure(memory.presence.send(space, undefined, "Viewing", new Uint8Array(PRESENCE_UPDATE_MAX_BYTES + 1)));
    expect(error.code).toBe("InvalidInput");
    await memory.presence.send(space, undefined, "Viewing", new Uint8Array(PRESENCE_UPDATE_MAX_BYTES));
    expect(memory.sentPresence()).toHaveLength(1);
  });

  it("is refused like any call while the workspace is locked", async () => {
    const memory = new MemoryTransport({
      now: () => Date.now(),
      workspaceName: "Home",
      spaces: SPACES,
      security: { kitCreatedAt: START, startLocked: true, appLockPassphrase: "correct horse battery" },
    });
    expect((await failure(memory.presence.send("99999999-9999-4999-8999-999999999999", undefined, "Idle", new Uint8Array()))).code).toBe("Locked");
  });
});

describe("the scripted people", () => {
  const peers = (stepMs = 0): MemoryTransportOptions["presence"] => ({
    stepMs,
    peers: [
      { space: "Work", member: "Bob", route: [{ page: "Plan", does: "edit" }] },
      { space: "Work", member: "Carol", route: [{ page: "Plan", steps: 2 }, { page: "Notes", steps: 2 }, { steps: 1 }] },
    ],
  });

  it("start in place at once, without an event, and are in the list", async () => {
    const { client, events, frames } = build("owner", { presence: peers() });
    const work = await spaceId(client);
    expect(events).toEqual([]);
    expect(frames).toEqual([]);
    const list = await client.presenceList(work);
    expect(list.map((info) => [info.name, info.state, info.pageId])).toEqual([
      ["Bob", "Editing", await pageId(client, "Plan")],
      ["Carol", "Viewing", await pageId(client, "Plan")],
    ]);
  });

  it("move a caret through the words of the page they edit: select a word, stand at its end, next word", async () => {
    const { memory, client, frames } = build("owner", { presence: peers() });
    const plan = await pageId(client, "Plan");
    const seen: [number, number][] = [];
    for (let i = 0; i < 9; i += 1) {
      frames.length = 0;
      memory.presenceStep();
      const bob = frames.find((frame) => frame.info.name === "Bob");
      const offsets = await offsetsOf(memory, plan, bob?.update ?? new Uint8Array());
      if (offsets === undefined) {
        throw new Error("Bob sent no cursor");
      }
      seen.push(offsets);
    }
    // Steps 1 to 9 of "Ship it on Friday": the script began with step 0 (the word "Ship" selected).
    expect(seen).toEqual([
      [4, 4], // step 1: at the end of "Ship"
      [5, 7], // step 2: "it" selected
      [7, 7],
      [8, 10], // "on"
      [10, 10],
      [11, 17], // "Friday"
      [17, 17],
      [0, 4], // round again
      [4, 4],
    ]);
  });

  it("walk their route, and a person in no page is idle", async () => {
    const { memory, client, events } = build("owner", { presence: peers() });
    const work = await spaceId(client);
    const plan = await pageId(client, "Plan");
    const notes = await pageId(client, "Notes");
    const where = async () => (await client.presenceList(work)).find((info) => info.name === "Carol");
    // Steps 0 and 1: Plan; 2 and 3: Notes; 4: nowhere.
    expect(await where()).toMatchObject({ pageId: plan, state: "Viewing" });
    memory.presenceStep();
    expect(await where()).toMatchObject({ pageId: plan });
    events.length = 0;
    memory.presenceStep();
    expect(await where()).toMatchObject({ pageId: notes });
    expect(changes(events)).toHaveLength(1);
    memory.presenceStep(2);
    const idle = await where();
    expect(idle).toMatchObject({ state: "Idle" });
    expect(idle).not.toHaveProperty("pageId");
    memory.presenceStep();
    expect(await where()).toMatchObject({ pageId: plan });
  });

  it("keep their devices alive: a step is a frame, so they do not time out", async () => {
    const { memory, client } = build("owner", { presence: peers() });
    const work = await spaceId(client);
    for (let i = 0; i < 4; i += 1) {
      vi.advanceTimersByTime(20_000);
      memory.presenceStep();
    }
    expect(await client.presenceList(work)).toHaveLength(2);
  });

  it("step on a timer when the dev page asks, and not otherwise", async () => {
    const { client, frames } = build("owner", { presence: peers(1_000) });
    await spaceId(client);
    vi.advanceTimersByTime(3_000);
    expect(frames.length).toBe(6);
    const idle = build("owner", { presence: peers(0) });
    vi.advanceTimersByTime(10_000);
    expect(idle.frames).toEqual([]);
  });

  it("must be members of a shared space of the seeds, with a route and pages that exist", () => {
    const options = (peer: NonNullable<MemoryTransportOptions["presence"]>["peers"]): MemoryTransportOptions => ({
      presence: { stepMs: 0, ...(peer === undefined ? {} : { peers: peer }) },
    });
    expect(() => build("owner", options([{ space: "Personal", member: "Bob", route: [{}] }]))).toThrow(/shared space/u);
    expect(() => build("owner", options([{ space: "Work", member: "Zed", route: [{}] }]))).toThrow(/member/u);
    expect(() => build("owner", options([{ space: "Work", member: "Maya", route: [{}] }]))).toThrow(/member/u);
    expect(() => build("owner", options([{ space: "Work", member: "Bob", route: [] }]))).toThrow(/route/u);
    expect(() => build("owner", options([{ space: "Work", member: "Bob", route: [{ page: "Nowhere" }] }]))).toThrow(/Nowhere/u);
  });

  it("go when their member leaves", async () => {
    const { memory, client } = build("owner", { presence: peers() });
    const work = await spaceId(client);
    memory.receiveSharing({ type: "memberLeft", spaceId: work, accountId: await accountOf(client, "Bob") });
    expect((await client.presenceList(work)).map((info) => info.name)).toEqual(["Carol"]);
    memory.presenceStep(3);
    expect((await client.presenceList(work)).map((info) => info.name)).toEqual(["Carol"]);
  });
});
