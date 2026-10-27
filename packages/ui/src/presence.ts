// Presence in shared spaces as the screens show it (Milestone 13, slice 3; ADR-0050, `docs/security/sharing.md` section 9): the
// colours of the members, their initials, the people the avatars show, and the queue that keeps this device to at most
// `PRESENCE_UPDATES_PER_SECOND` frames a second. The editor's side (the cursors) is `editor/page-presence.ts`; the store part is
// `store-presence.ts`.
//
// Names and colours come from membership: the core hands the UI the member's profile name and their place in join order
// (`PresenceInfo.name`, `PresenceInfo.color`), and the colour is that place modulo the palette below, so every device shows a
// member in one colour and a sender cannot pick one that mimics another member.

import { PRESENCE_UPDATES_PER_SECOND } from "@atomik-kb/core-client";
import type { AccountId, PageId, PresenceInfo, PresenceState, SpaceId } from "@atomik-kb/core-client";

/**
 * The colours of the members by join order: dark enough for white text (4.5:1 or better on every one), so that an avatar and
 * the name above a caret read in both themes. Ten, as the people of a page seldom outnumber them; the eleventh member takes the
 * first colour again.
 */
export const PRESENCE_PALETTE: readonly string[] = [
  "#c2410c",
  "#1d4ed8",
  "#15803d",
  "#a21caf",
  "#0e7490",
  "#b91c1c",
  "#6d28d9",
  "#854d0e",
  "#be185d",
  "#0f766e",
];

/** The colour of the member at this place in join order (0 for the creator), by the palette; it wraps, and a bad index is the first. */
export function presenceColour(index: number): string {
  const size = PRESENCE_PALETTE.length;
  const whole = Number.isInteger(index) ? index : 0;
  return PRESENCE_PALETTE[((whole % size) + size) % size] ?? "#c2410c";
}

/** One or two capital letters of a name: the first letter of its first two words ("Maya Lin" is "ML", "Bob" is "B"). */
export function initialsOf(name: string): string {
  const words = name.trim().split(/\s+/u).filter((word) => word !== "");
  if (words.length === 0) {
    return "?";
  }
  const first = (word: string): string => Array.from(word)[0]?.toLocaleUpperCase() ?? "";
  return words.length === 1 ? first(words[0] ?? "") : `${first(words[0] ?? "")}${first(words[1] ?? "")}`;
}

/** One person in the avatars: a member, however many devices of theirs are in the space. */
export interface PresencePerson {
  accountId: AccountId;
  name: string;
  /** The colour of their join order, a `presenceColour`. */
  colour: string;
  /** Their place in join order. */
  order: number;
  /** Every device of theirs is idle (in no page). */
  idle: boolean;
}

/**
 * The people in a list of devices, once each, in join order (then by name): the members other than this account (`me`, when it is
 * known: you are not shown to yourself), and with `pageId` only those with a device in that page. A device that is `Idle` is in
 * no page, so it counts for the space and not for a page.
 */
export function peopleOf(
  devices: readonly PresenceInfo[],
  me: AccountId | undefined,
  pageId?: PageId,
): PresencePerson[] {
  const people = new Map<AccountId, PresencePerson>();
  for (const device of devices) {
    if (device.accountId === me) {
      continue;
    }
    if (pageId !== undefined && (device.pageId !== pageId || device.state === "Idle")) {
      continue;
    }
    const known = people.get(device.accountId);
    const idle = device.state === "Idle";
    if (known === undefined) {
      people.set(device.accountId, {
        accountId: device.accountId,
        name: device.name,
        colour: presenceColour(device.color),
        order: device.color,
        idle,
      });
    } else if (!idle) {
      known.idle = false;
    }
  }
  return [...people.values()].sort((a, b) => a.order - b.order || a.name.localeCompare(b.name));
}

/** What the queue hands the transport: one presence frame of this device. */
export interface PresenceFrame {
  spaceId: SpaceId;
  pageId: PageId | undefined;
  state: PresenceState;
  update: Uint8Array;
}

export interface PresenceSendQueueOptions {
  /** The most frames a second (default `PRESENCE_UPDATES_PER_SECOND`). */
  perSecond?: number;
  /** The clock in milliseconds (default `Date.now`). */
  now?: () => number;
}

/**
 * Keeps this device to at most `PRESENCE_UPDATES_PER_SECOND` presence frames a second, whatever the editors ask (a drag of the
 * mouse changes the selection at every frame of the screen). The first frame after a quiet time goes at once; the ones that follow
 * within the same tenth of a second wait, and of those that wait only the newest of each space is sent (an older position is
 * worth nothing once a newer one exists), one every tenth of a second. Delivery is best effort: a frame that fails is dropped.
 */
export class PresenceSendQueue {
  readonly #deliver: (frame: PresenceFrame) => Promise<void>;
  readonly #gap: number;
  readonly #now: () => number;
  /** The frame waiting for each space, oldest space first (a `Map` keeps the order of insertion). */
  readonly #waiting = new Map<SpaceId, PresenceFrame>();
  #lastSentAt = Number.NEGATIVE_INFINITY;
  #timer: ReturnType<typeof setTimeout> | undefined;

  constructor(deliver: (frame: PresenceFrame) => Promise<void>, options: PresenceSendQueueOptions = {}) {
    this.#deliver = deliver;
    this.#gap = 1000 / (options.perSecond ?? PRESENCE_UPDATES_PER_SECOND);
    this.#now = options.now ?? Date.now;
  }

  /** Asks for a frame to be sent: now when the last one is a tenth of a second old, else as soon as it is. */
  push(frame: PresenceFrame): void {
    // The newest frame of a space goes behind the others, in the order the spaces were last heard from.
    this.#waiting.delete(frame.spaceId);
    this.#waiting.set(frame.spaceId, frame);
    this.#flush();
  }

  /** Forgets what waits (the app was locked, the page was left for good). */
  clear(): void {
    this.#waiting.clear();
    if (this.#timer !== undefined) {
      clearTimeout(this.#timer);
      this.#timer = undefined;
    }
  }

  #flush(): void {
    if (this.#timer !== undefined) {
      return;
    }
    const wait = this.#lastSentAt + this.#gap - this.#now();
    if (wait > 0) {
      this.#timer = setTimeout(() => {
        this.#timer = undefined;
        this.#flush();
      }, wait);
      return;
    }
    const next = this.#waiting.entries().next();
    if (next.done === true) {
      return;
    }
    const [spaceId, frame] = next.value;
    this.#waiting.delete(spaceId);
    this.#lastSentAt = this.#now();
    try {
      this.#deliver(frame).catch(() => {
        // Presence is best effort: nothing waits for it, and a frame that did not go is not sent again.
      });
    } catch {
      // A delivery that throws before it returns a promise is no different.
    }
    // Others wait for the next tenth of a second.
    if (this.#waiting.size > 0) {
      this.#flush();
    }
  }
}
