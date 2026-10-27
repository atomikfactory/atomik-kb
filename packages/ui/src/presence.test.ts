// The pure parts of presence (`presence.ts`; Milestone 13, slice 3; ADR-0050): the colour of a member by join order, their
// initials, the people the avatars show, and the queue that keeps this device to ten presence frames a second.

import { PRESENCE_UPDATES_PER_SECOND } from "@atomik-kb/core-client";
import type { PresenceInfo } from "@atomik-kb/core-client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { PRESENCE_PALETTE, PresenceSendQueue, initialsOf, peopleOf, presenceColour } from "./presence";
import type { PresenceFrame } from "./presence";
import type { AppState } from "./store";
import { presenceLive } from "./store-presence";

const PAGE = "22222222-2222-4222-8222-222222222222";
const OTHER_PAGE = "22222222-2222-4222-8222-222222222223";
const SPACE = "11111111-1111-4111-8111-111111111111";
const OTHER_SPACE = "11111111-1111-4111-8111-111111111112";
const MAYA = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const BOB = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const CAROL = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const DAN = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";

function device(accountId: string, name: string, color: number, overrides: Partial<PresenceInfo> = {}): PresenceInfo {
  return {
    accountId,
    deviceId: `${accountId.slice(0, 8)}-0000-4000-8000-000000000001`,
    name,
    color,
    pageId: PAGE,
    state: "Editing",
    updatedAt: 1,
    ...overrides,
  };
}

describe("the colour of a member", () => {
  it("is the palette entry of their place in join order, wrapping round", () => {
    const size = PRESENCE_PALETTE.length;
    expect(size).toBeGreaterThanOrEqual(8);
    for (let order = 0; order < size; order += 1) {
      expect(presenceColour(order)).toBe(PRESENCE_PALETTE[order]);
      expect(presenceColour(order + size)).toBe(PRESENCE_PALETTE[order]);
      expect(presenceColour(order + 2 * size)).toBe(PRESENCE_PALETTE[order]);
    }
  });

  it("gives the first ten members ten different colours, in the same order on every device", () => {
    expect(new Set(PRESENCE_PALETTE).size).toBe(PRESENCE_PALETTE.length);
    expect(PRESENCE_PALETTE.map((_, order) => presenceColour(order))).toEqual([...PRESENCE_PALETTE]);
    // The creator (0) always has the first colour.
    expect(presenceColour(0)).toBe(PRESENCE_PALETTE[0]);
  });

  it("is the first colour for an index that is none (a core that sends a fraction or a negative)", () => {
    expect(presenceColour(-1)).toBe(PRESENCE_PALETTE.at(-1));
    expect(presenceColour(1.5)).toBe(PRESENCE_PALETTE[0]);
    expect(presenceColour(Number.NaN)).toBe(PRESENCE_PALETTE[0]);
  });

  it("is dark enough for white text: every colour has a contrast of 4.5 to 1 or better", () => {
    const luminance = (hex: string): number => {
      const channel = (offset: number): number => {
        const value = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
        return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    };
    for (const colour of PRESENCE_PALETTE) {
      expect(colour).toMatch(/^#[0-9a-f]{6}$/u);
      expect(1.05 / (luminance(colour) + 0.05), colour).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("initials", () => {
  it("are the first letters of the first two words, in capitals", () => {
    expect(initialsOf("Maya Lin")).toBe("ML");
    expect(initialsOf("bob")).toBe("B");
    expect(initialsOf("  carol   ann   white ")).toBe("CA");
    expect(initialsOf("élodie")).toBe("É");
    expect(initialsOf("陳 小明")).toBe("陳小");
  });

  it("take a whole character, not half of one", () => {
    expect(initialsOf("😀 smile")).toBe("😀S");
  });

  it("are a question mark for a name with nothing in it", () => {
    expect(initialsOf("")).toBe("?");
    expect(initialsOf("   ")).toBe("?");
  });
});

describe("the people the avatars show", () => {
  const devices = [
    device(DAN, "Dan", 3, { pageId: OTHER_PAGE, state: "Viewing" }),
    device(BOB, "Bob", 1),
    device(CAROL, "Carol", 2, { state: "Idle" }),
    device(MAYA, "Maya", 0),
  ];
  const noPage = (info: PresenceInfo): PresenceInfo => {
    const copy = { ...info };
    delete copy.pageId;
    return copy;
  };

  it("lists each member once, in join order, with the colour of it", () => {
    const people = peopleOf(devices, undefined);
    expect(people.map((person) => [person.name, person.order, person.colour])).toEqual([
      ["Maya", 0, presenceColour(0)],
      ["Bob", 1, presenceColour(1)],
      ["Carol", 2, presenceColour(2)],
      ["Dan", 3, presenceColour(3)],
    ]);
  });

  it("never shows you", () => {
    expect(peopleOf(devices, MAYA).map((person) => person.name)).toEqual(["Bob", "Carol", "Dan"]);
    // This account's other devices are not shown either.
    expect(peopleOf([...devices, device(MAYA, "Maya", 0, { deviceId: "other" })], MAYA).map((person) => person.name)).toEqual(["Bob", "Carol", "Dan"]);
  });

  it("shows a page only those who are on it: not another page, and not an idle device", () => {
    expect(peopleOf(devices, MAYA, PAGE).map((person) => person.name)).toEqual(["Bob"]);
    expect(peopleOf(devices, MAYA, OTHER_PAGE).map((person) => person.name)).toEqual(["Dan"]);
    expect(peopleOf(devices, undefined, PAGE).map((person) => person.name)).toEqual(["Maya", "Bob"]);
    // A device in no page is on none.
    expect(peopleOf([noPage(device(BOB, "Bob", 1, { state: "Idle" }))], undefined, PAGE)).toEqual([]);
  });

  it("marks a member idle only when every device of theirs is", () => {
    const people = peopleOf(devices, MAYA);
    expect(people.map((person) => [person.name, person.idle])).toEqual([
      ["Bob", false],
      ["Carol", true],
      ["Dan", false],
    ]);
    const two = peopleOf(
      [device(CAROL, "Carol", 2, { state: "Idle" }), device(CAROL, "Carol", 2, { deviceId: "second", state: "Viewing" })],
      undefined,
    );
    expect(two).toHaveLength(1);
    expect(two[0]?.idle).toBe(false);
    // Whichever device is listed first.
    expect(
      peopleOf([device(CAROL, "Carol", 2, { deviceId: "second", state: "Viewing" }), device(CAROL, "Carol", 2, { state: "Idle" })], undefined)[0]?.idle,
    ).toBe(false);
  });

  it("shows a member whose page this device does not hold in the space, but on no page", () => {
    // The core leaves `pageId` out for a page that is not a page of the space on this device.
    const lost = noPage(device(BOB, "Bob", 1, { state: "Viewing" }));
    expect(peopleOf([lost], MAYA).map((person) => person.name)).toEqual(["Bob"]);
    expect(peopleOf([lost], MAYA, PAGE)).toEqual([]);
    expect(peopleOf([lost], MAYA)[0]?.idle).toBe(false);
  });

  it("is nobody for nobody", () => {
    expect(peopleOf([], MAYA)).toEqual([]);
  });

  it("puts members of the same colour in name order", () => {
    const people = peopleOf([device(DAN, "Zed", 1), device(BOB, "Abe", 1)], undefined);
    expect(people.map((person) => person.name)).toEqual(["Abe", "Zed"]);
  });
});

describe("the send queue", () => {
  const delivered: PresenceFrame[] = [];
  const frame = (n: number, spaceId = SPACE): PresenceFrame => ({ spaceId, pageId: PAGE, state: "Editing", update: Uint8Array.of(n) });
  const bytes = (): number[] => delivered.map((sent) => sent.update[0] ?? -1);
  let queue: PresenceSendQueue;

  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(1_700_000_000_000);
    delivered.length = 0;
    queue = new PresenceSendQueue((sent) => {
      delivered.push(sent);
      return Promise.resolve();
    });
  });

  afterEach(() => {
    queue.clear();
    vi.useRealTimers();
  });

  it("is ten frames a second, as the contract says", () => {
    expect(PRESENCE_UPDATES_PER_SECOND).toBe(10);
  });

  it("sends the first frame after a quiet time at once", () => {
    queue.push(frame(1));
    expect(bytes()).toEqual([1]);
  });

  it("holds the frames that follow within a tenth of a second and sends only the newest of them", () => {
    queue.push(frame(1));
    queue.push(frame(2));
    queue.push(frame(3));
    queue.push(frame(4));
    expect(bytes()).toEqual([1]);
    vi.advanceTimersByTime(99);
    expect(bytes()).toEqual([1]);
    vi.advanceTimersByTime(1);
    expect(bytes()).toEqual([1, 4]);
    vi.advanceTimersByTime(1_000);
    expect(bytes()).toEqual([1, 4]);
  });

  it("never sends more than ten frames in any second, however fast frames come", () => {
    const times: number[] = [];
    queue = new PresenceSendQueue(
      () => {
        times.push(Date.now());
        return Promise.resolve();
      },
      {},
    );
    // A frame every 5 ms for 3 seconds: a drag of the mouse.
    for (let at = 0; at < 3_000; at += 5) {
      queue.push(frame(at % 250));
      vi.advanceTimersByTime(5);
    }
    vi.advanceTimersByTime(500);
    expect(times.length).toBeGreaterThanOrEqual(25);
    expect(times.length).toBeLessThanOrEqual(31);
    for (let i = 0; i + 10 < times.length; i += 1) {
      // Eleven frames never fit in one second.
      expect((times[i + 10] ?? 0) - (times[i] ?? 0)).toBeGreaterThanOrEqual(1_000);
    }
  });

  it("keeps the last position: what was held when the drag ended is sent", () => {
    for (let n = 1; n <= 40; n += 1) {
      queue.push(frame(n));
    }
    vi.advanceTimersByTime(200);
    expect(bytes()).toEqual([1, 40]);
  });

  it("sends the next frame at once once a tenth of a second has gone quiet", () => {
    queue.push(frame(1));
    vi.advanceTimersByTime(100);
    queue.push(frame(2));
    expect(bytes()).toEqual([1, 2]);
  });

  it("keeps a frame for each space, one every tenth of a second, the oldest space first", () => {
    queue.push(frame(1, SPACE));
    queue.push(frame(2, OTHER_SPACE));
    queue.push(frame(3, SPACE));
    expect(bytes()).toEqual([1]);
    vi.advanceTimersByTime(100);
    // The other space's waits behind: this space's newer frame came after it.
    expect(bytes()).toEqual([1, 2]);
    vi.advanceTimersByTime(100);
    expect(bytes()).toEqual([1, 2, 3]);
  });

  it("forgets what waits when cleared", () => {
    queue.push(frame(1));
    queue.push(frame(2));
    queue.clear();
    vi.advanceTimersByTime(1_000);
    expect(bytes()).toEqual([1]);
    queue.push(frame(3));
    expect(bytes()).toEqual([1, 3]);
  });

  it("drops a frame that did not go, without a word and without trying again", async () => {
    let calls = 0;
    queue = new PresenceSendQueue(() => {
      calls += 1;
      return Promise.reject(new Error("the socket is down"));
    });
    queue.push(frame(1));
    await vi.advanceTimersByTimeAsync(1_000);
    expect(calls).toBe(1);
  });

  it("survives a delivery that throws", () => {
    let calls = 0;
    queue = new PresenceSendQueue(() => {
      calls += 1;
      throw new Error("not even a promise");
    });
    queue.push(frame(1));
    vi.advanceTimersByTime(100);
    queue.push(frame(2));
    expect(calls).toBe(2);
  });

  it("takes its rate and clock from its options", () => {
    let now = 0;
    const seen: number[] = [];
    queue = new PresenceSendQueue(
      (sent) => {
        seen.push(sent.update[0] ?? -1);
        return Promise.resolve();
      },
      { perSecond: 2, now: () => now },
    );
    queue.push(frame(1));
    now = 400;
    queue.push(frame(2));
    expect(seen).toEqual([1]);
    now = 500;
    queue.push(frame(3));
    vi.advanceTimersByTime(100);
    expect(seen).toEqual([1, 3]);
  });
});

describe("where presence is sent and listed", () => {
  const state = (shareState: string | undefined): AppState =>
    ({ spaces: [{ id: SPACE, name: "Work", ...(shareState === undefined ? {} : { shareState }) }] }) as unknown as AppState;

  it("only in a space that is shared and accepted by the server: not personal, not pending, not a read-only copy", () => {
    expect(presenceLive(state("Shared"), SPACE)).toBe(true);
    expect(presenceLive(state(undefined), SPACE)).toBe(false);
    expect(presenceLive(state("Personal"), SPACE)).toBe(false);
    expect(presenceLive(state("Pending"), SPACE)).toBe(false);
    expect(presenceLive(state("Archived"), SPACE)).toBe(false);
    // A space the list does not hold (left, not caught up) is none.
    expect(presenceLive(state("Shared"), OTHER_SPACE)).toBe(false);
  });
});
