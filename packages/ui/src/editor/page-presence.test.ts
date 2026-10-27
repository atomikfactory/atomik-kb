// The presence of one open page (`page-presence.ts`; Milestone 13, slice 3; ADR-0050): what is sent (only while the space is live
// and shared and the window is the one in use, a state by what the device may do, a frame every ten seconds while nothing changes,
// one `Idle` frame when the window is left), and what the other devices' frames do to the page's cursors (the member's name and
// colour from membership, the page, the 30-second timeout, this account's own devices left out). The rate is `presence.test.ts`.

import { PRESENCE_UPDATE_MAX_BYTES } from "@atomik-kb/core-client";
import type { PresenceInfo, PresenceState, PresenceUpdate } from "@atomik-kb/core-client";
import { EphemeralStore, LoroMap, LoroText } from "loro-crdt";
import type { LoroList } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { loroDocFrom, paragraphs } from "../test/editor";
import { presenceColour } from "../presence";
import { PagePresence, PRESENCE_HEARTBEAT_MS, followWindowActivity } from "./page-presence";
import type { PagePresenceOptions, PagePresenceSettings } from "./page-presence";

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const OTHER_PAGE = "22222222-2222-4222-8222-222222222223";
const BOB = "33333333-3333-4333-8333-333333333333";
const MAYA = "33333333-3333-4333-8333-333333333334";

interface Sent {
  spaceId: string;
  pageId: string | undefined;
  state: PresenceState;
  update: Uint8Array;
}

const doc = loroDocFrom(paragraphs("Hello world"));

/** The first text of `doc`, for cursors. */
function text(): LoroText {
  const visit = (map: LoroMap): LoroText | undefined => {
    for (const child of (map.get("children") as LoroList).toArray()) {
      if (child instanceof LoroText) {
        return child;
      }
      if (child instanceof LoroMap) {
        const found = visit(child as LoroMap);
        if (found !== undefined) {
          return found;
        }
      }
    }
    return undefined;
  };
  const found = visit(doc.getMap("doc"));
  if (found === undefined) {
    throw new Error("the document has no text");
  }
  return found;
}

/** What a device sends: the Loro cursors of a selection in the page's text, as an `EphemeralStore` update. */
function update(from: number, to: number): Uint8Array {
  const store = new EphemeralStore(30_000);
  try {
    store.set("peer", { anchor: text().getCursor(from)?.encode(), focus: text().getCursor(to)?.encode() });
    return store.encode("peer");
  } finally {
    store.destroy();
  }
}

function info(overrides: Partial<PresenceInfo> = {}): PresenceInfo {
  return { accountId: BOB, deviceId: "44444444-4444-4444-8444-444444444444", name: "Bob", color: 1, pageId: PAGE, state: "Editing", updatedAt: 1, ...overrides };
}

const sent: Sent[] = [];
let deliver: ((message: PresenceUpdate) => void) | undefined;
let unsubscribed = 0;
const presences: PagePresence[] = [];

function make(overrides: Partial<PagePresenceOptions> = {}, settings: PagePresenceSettings | null = { enabled: true, canEdit: true }): PagePresence {
  const presence = new PagePresence({
    spaceId: SPACE,
    pageId: PAGE,
    peerKey: doc.peerIdStr,
    send: (spaceId, pageId, state, bytes) => {
      sent.push({ spaceId, pageId, state, update: bytes });
    },
    subscribe: (listener) => {
      deliver = listener;
      return () => {
        unsubscribed += 1;
        deliver = undefined;
      };
    },
    ...overrides,
  });
  presences.push(presence);
  if (settings !== null) {
    presence.configure(settings);
  }
  return presence;
}

/** The device sends this: a selection of the page's text as the cursor plugin writes it. */
function select(presence: PagePresence, from: number, to: number): void {
  presence.store.setLocal({ anchor: text().getCursor(from) ?? undefined, focus: text().getCursor(to) ?? undefined });
}

beforeEach(() => {
  vi.useFakeTimers();
  sent.length = 0;
  unsubscribed = 0;
  deliver = undefined;
});

afterEach(() => {
  for (const presence of presences.splice(0)) {
    presence.destroy();
  }
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("what is sent", () => {
  it("nothing until the space is live and shared and the window is the one in use", () => {
    const presence = make({}, null);
    select(presence, 1, 3);
    expect(sent).toEqual([]);
    presence.configure({ enabled: true, canEdit: true });
    expect(sent).toEqual([]); // not active yet
    presence.setActive(true);
    expect(sent).toHaveLength(1);
  });

  it("a personal space sends nothing: not for the window, not for a cursor, not for the heartbeat", () => {
    const presence = make({}, { enabled: false, canEdit: true });
    presence.setActive(true);
    select(presence, 1, 3);
    vi.advanceTimersByTime(60_000);
    presence.setActive(false);
    presence.destroy();
    expect(sent).toEqual([]);
  });

  it("views the page with no cursor, and edits it with one", () => {
    const presence = make();
    presence.setActive(true);
    expect(sent).toEqual([{ spaceId: SPACE, pageId: PAGE, state: "Viewing", update: new Uint8Array() }]);
    select(presence, 1, 3);
    expect(sent[1]).toMatchObject({ spaceId: SPACE, pageId: PAGE, state: "Editing" });
    expect(sent[1]?.update.length).toBeGreaterThan(0);
    expect(sent[1]?.update.length).toBeLessThanOrEqual(PRESENCE_UPDATE_MAX_BYTES);
    presence.store.setLocal({});
    expect(sent[2]).toEqual({ spaceId: SPACE, pageId: PAGE, state: "Viewing", update: new Uint8Array() });
  });

  it("a device that may not write the page views it, with its cursor: a Viewer may send presence", () => {
    const presence = make({}, { enabled: true, canEdit: false });
    presence.setActive(true);
    select(presence, 1, 3);
    expect(sent[1]).toMatchObject({ state: "Viewing" });
    expect(sent[1]?.update.length).toBeGreaterThan(0);
  });

  it("sends a frame every ten seconds while nothing changes, and a change starts the ten seconds again", () => {
    const presence = make();
    presence.setActive(true);
    expect(PRESENCE_HEARTBEAT_MS).toBe(10_000);
    vi.advanceTimersByTime(9_999);
    expect(sent).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(sent).toHaveLength(2);
    vi.advanceTimersByTime(25_000);
    expect(sent).toHaveLength(4);
    vi.advanceTimersByTime(4_000);
    select(presence, 1, 2);
    expect(sent).toHaveLength(5);
    vi.advanceTimersByTime(9_000);
    expect(sent).toHaveLength(5);
    vi.advanceTimersByTime(1_000);
    expect(sent).toHaveLength(6);
  });

  it("the heartbeat repeats the cursor, so that a caret that does not move does not time out for the others", () => {
    const presence = make();
    presence.setActive(true);
    select(presence, 1, 3);
    const first = sent.at(-1);
    for (let i = 0; i < 4; i += 1) {
      vi.advanceTimersByTime(PRESENCE_HEARTBEAT_MS);
      expect(sent.at(-1)?.state).toBe("Editing");
      expect(sent.at(-1)?.update.length).toBe(first?.update.length);
    }
  });

  it("leaving the window sends one Idle frame in no page and stops; coming back sends the page again", () => {
    const presence = make();
    presence.setActive(true);
    sent.length = 0;
    presence.setActive(false);
    expect(sent).toEqual([{ spaceId: SPACE, pageId: undefined, state: "Idle", update: new Uint8Array() }]);
    vi.advanceTimersByTime(60_000);
    select(presence, 1, 3);
    expect(sent).toHaveLength(1);
    presence.setActive(false);
    expect(sent).toHaveLength(1);
    presence.setActive(true);
    expect(sent[1]).toMatchObject({ pageId: PAGE, state: "Editing" });
  });

  it("the end of sharing stops it, the start of it begins", () => {
    const presence = make();
    presence.setActive(true);
    presence.configure({ enabled: false, canEdit: true });
    sent.length = 0;
    vi.advanceTimersByTime(30_000);
    select(presence, 1, 3);
    expect(sent).toEqual([]);
    presence.configure({ enabled: true, canEdit: true });
    expect(sent).toHaveLength(1);
  });

  it("leaving the page sends Idle if the device was sending, and nothing otherwise", () => {
    const sending = make();
    sending.setActive(true);
    sent.length = 0;
    sending.destroy();
    expect(sent).toEqual([{ spaceId: SPACE, pageId: undefined, state: "Idle", update: new Uint8Array() }]);
    expect(unsubscribed).toBe(1);

    sent.length = 0;
    const quiet = make();
    quiet.destroy();
    expect(sent).toEqual([]);
    vi.advanceTimersByTime(60_000);
    expect(sent).toEqual([]);
  });
});

describe("the other devices' frames", () => {
  it("draw a cursor in the member's name and the colour of their join order, never what the frame says", () => {
    const presence = make();
    deliver?.({ info: info({ color: 12 }), update: update(1, 4) });
    const state = Object.entries(presence.store.getAll());
    expect(state).toHaveLength(1);
    expect(state[0]?.[0]).toBe("44444444-4444-4444-8444-444444444444");
    expect(state[0]?.[1].user).toEqual({ name: "Bob", color: presenceColour(12) });
    expect(presenceColour(12)).toBe(presenceColour(2));
  });

  it("go when the device is in another page, idle, or gone from the page", () => {
    const presence = make();
    deliver?.({ info: info(), update: update(1, 4) });
    expect(Object.keys(presence.store.getAll())).toHaveLength(1);
    deliver?.({ info: info({ pageId: OTHER_PAGE }), update: update(1, 4) });
    expect(presence.store.getAll()).toEqual({});

    deliver?.({ info: info(), update: update(1, 4) });
    deliver?.({ info: info({ state: "Idle" }), update: new Uint8Array() });
    expect(presence.store.getAll()).toEqual({});

    deliver?.({ info: info(), update: update(1, 4) });
    const inNoPage = info();
    delete inNoPage.pageId;
    deliver?.({ info: inNoPage, update: new Uint8Array() });
    expect(presence.store.getAll()).toEqual({});
  });

  it("go 30 seconds after the latest frame of the device", () => {
    const presence = make();
    deliver?.({ info: info(), update: update(1, 4) });
    vi.advanceTimersByTime(29_000);
    deliver?.({ info: info(), update: update(2, 5) });
    vi.advanceTimersByTime(29_000);
    expect(Object.keys(presence.store.getAll())).toHaveLength(1);
    vi.advanceTimersByTime(1_001);
    expect(presence.store.getAll()).toEqual({});
  });

  it("keep each device apart, two of one member's included", () => {
    const presence = make();
    deliver?.({ info: info({ deviceId: "44444444-4444-4444-8444-44444444444a" }), update: update(1, 2) });
    deliver?.({ info: info({ deviceId: "44444444-4444-4444-8444-44444444444b" }), update: update(3, 5) });
    expect(Object.keys(presence.store.getAll())).toHaveLength(2);
  });

  it("do not draw this account's own other devices: you are not shown to yourself", () => {
    const presence = make({}, { enabled: true, canEdit: true, self: MAYA });
    deliver?.({ info: info({ accountId: MAYA, name: "Maya" }), update: update(1, 4) });
    expect(presence.store.getAll()).toEqual({});
    deliver?.({ info: info(), update: update(1, 4) });
    expect(Object.keys(presence.store.getAll())).toHaveLength(1);
  });

  it("are ignored in a space that is not shared, and what was drawn goes when sharing ends", () => {
    const presence = make({}, { enabled: false, canEdit: true });
    deliver?.({ info: info(), update: update(1, 4) });
    expect(presence.store.getAll()).toEqual({});
    presence.configure({ enabled: true, canEdit: true });
    deliver?.({ info: info(), update: update(1, 4) });
    expect(Object.keys(presence.store.getAll())).toHaveLength(1);
    presence.configure({ enabled: false, canEdit: true });
    expect(presence.store.getAll()).toEqual({});
  });

  it("are no longer heard once the page is left", () => {
    const presence = make();
    presence.destroy();
    expect(deliver).toBeUndefined();
  });

  it("draw nothing for an update that is not a cursor", () => {
    const presence = make();
    deliver?.({ info: info(), update: Uint8Array.of(1, 2, 3) });
    deliver?.({ info: info({ deviceId: "44444444-4444-4444-8444-44444444444b" }), update: new Uint8Array() });
    expect(presence.store.getAll()).toEqual({});
  });
});

describe("followWindowActivity", () => {
  it("says whether the window is focused and visible, now and when it changes, and stops listening", () => {
    const presence = make();
    const focus = vi.spyOn(document, "hasFocus").mockReturnValue(true);
    const stop = followWindowActivity(presence);
    expect(sent).toHaveLength(1); // active at once: the page is announced
    sent.length = 0;
    focus.mockReturnValue(false);
    window.dispatchEvent(new Event("blur"));
    expect(sent).toEqual([{ spaceId: SPACE, pageId: undefined, state: "Idle", update: new Uint8Array() }]);
    focus.mockReturnValue(true);
    window.dispatchEvent(new Event("focus"));
    expect(sent.at(-1)).toMatchObject({ pageId: PAGE });
    sent.length = 0;
    stop();
    window.dispatchEvent(new Event("blur"));
    expect(sent).toEqual([]);
  });

  it("a window that is hidden is not in use, whatever has focus", () => {
    const presence = make();
    vi.spyOn(document, "hasFocus").mockReturnValue(true);
    const visible = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    const stop = followWindowActivity(presence);
    expect(sent).toEqual([]);
    visible.mockReturnValue("visible");
    document.dispatchEvent(new Event("visibilitychange"));
    expect(sent).toHaveLength(1);
    stop();
  });
});
