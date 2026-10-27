// The in-memory twin of presence in shared spaces (Milestone 13, slice 3; ADR-0050, `docs/security/sharing.md` section 9, the
// contract in `docs/development/milestone-13.md` section 3.1), used by `MemoryTransport`. There is no socket and no
// cryptography here: the other people (the members of a shared space the sharing mirror simulates) have devices that
// appear, move through pages and go, and what the mirror hands the editor is what the shell's channel does: the
// accepted frame as `PresenceUpdate` (`info` and the sender's `EphemeralStore` update) and `Event.PresenceChanged`.
//
// What it enforces is what the core enforces of an accepted frame:
//
// - presence exists only in a live shared space (a personal space and a read-only copy have none: `PresenceList` answers
//   nothing, this device's `send` sends nothing and is no error), only between current members, and only for devices of
//   other accounts than this one;
// - a page that is not a page of the space on this device (another space's, one this device does not know) is no page: the
//   device is in the space and on no page (`PresenceInfo.pageId` absent), whatever its state says;
// - `PresenceInfo` is the table of "who is where now": the name is the member's name in the space, the colour the member's
//   place in join order (0 for the first member, kept for as long as the member's device is in the table), `updatedAt` is
//   the mirror's clock at the latest frame (never what a frame claims);
// - the table is in memory only: a lock drops it; a device is gone `PRESENCE_TIMEOUT_SECONDS` after its latest frame, when its
//   member leaves or is removed, or when the space stops being live (closed, this account removed), at once, and `PresenceChanged { spaceId }` fires when a device appears, changes page or state, or goes: not
//   for a cursor move (a frame from a device in the same page and state);
// - a Viewer's devices send like anyone's.
//
// The people act through `receive` (`PresenceChange`, `MemoryTransport.receivePresence`), and through scripts
// (`MemoryPresenceOptions.peers`): a person walks a route of pages, and in a page it edits it moves a caret through the words
// of the page (a Loro cursor of the page's own text, so that it resolves in the editor whatever the editor shows), alternately
// selecting a word and standing at its end. A script takes one step per `stepMs` (the dev page: `?presence=demo`), or only
// when `step` is called (tests).

import { EphemeralStore } from "loro-crdt";
import type { LoroDoc } from "loro-crdt";

import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { DeviceId } from "./generated/DeviceId";
import type { Event } from "./generated/Event";
import type { PageId } from "./generated/PageId";
import type { PresenceInfo } from "./generated/PresenceInfo";
import type { PresenceState } from "./generated/PresenceState";
import type { SpaceId } from "./generated/SpaceId";
import { textAnchorOf, textWordsOf } from "./page-doc";
import { PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATE_MAX_BYTES } from "./presence";
import type { MemorySharing } from "./sharing-mirror";
import { invalid } from "./validate";

/** The default length of one step of the scripted people, in milliseconds (`?presence=demo`). */
export const DEFAULT_PRESENCE_STEP_MS = 1_200;

/** The key the simulated devices put their cursors under in an `EphemeralStore` update (receivers never read it). */
const SENDER_KEY = "sim";

/** One place a scripted person stays for a number of steps. */
export interface MemoryPresenceStop {
  /** The title of a page of the space; absent: the device is in no page (idle). */
  page?: string;
  /** In the page: `"edit"` moves a caret through its words (state `Editing`), `"view"` only looks (state `Viewing`; the default). */
  does?: "edit" | "view";
  /** How many steps the person stays here (default 1). */
  steps?: number;
}

/** A simulated member who walks a route of pages, over and over. */
export interface MemoryPresencePeer {
  /** The name of a seeded space (it must be shared). */
  space: string;
  /** The name of a member of that space other than this account. */
  member: string;
  /** The places in turn; at least one. */
  route: readonly MemoryPresenceStop[];
}

/** `MemoryTransportOptions.presence`: the scripted people. */
export interface MemoryPresenceOptions {
  peers?: readonly MemoryPresencePeer[];
  /** Length of one step of the scripts, in milliseconds (default `DEFAULT_PRESENCE_STEP_MS`; 0: only `presenceStep` steps). */
  stepMs?: number;
}

/** What a frame's cursor holds: the first place in the page's text that has these words. */
export interface PresenceSelect {
  quote: string;
  /** Without it the words are selected; `"start"` or `"end"` puts a caret there. */
  caret?: "start" | "end";
}

/** What another person's device does, as it arrives through the socket (`MemoryTransport.receivePresence`). */
export type PresenceChange =
  /**
   * One accepted frame of a member's device. `state` defaults to `Idle` without a page, `Editing` in a page with a
   * cursor (`select`, or `update` as it is) and `Viewing` in a page without one. A frame of an `Idle` device is in no page.
   */
  | {
      type: "frame";
      spaceId: SpaceId;
      member: string;
      /** Which of the member's devices (default 0). */
      device?: number;
      pageId?: PageId;
      state?: PresenceState;
      select?: PresenceSelect;
      /** The `EphemeralStore` update as it is (garbage included), instead of `select`. */
      update?: Uint8Array;
    }
  /** The device went (closed the page, went offline): the core learns it by the timeout, a test by this. */
  | { type: "leave"; spaceId: SpaceId; member: string; device?: number };

/** One frame this device sent in a live shared space (`MemoryTransport.sentPresence`). */
export interface SentPresence {
  spaceId: SpaceId;
  pageId: PageId | undefined;
  state: PresenceState;
  update: Uint8Array;
}

/** What the presence mirror needs from its owner. */
export interface PresenceDeps {
  now: () => number;
  emit: (event: Event) => void;
  ready: () => boolean;
  sharing: MemorySharing;
  hasSpace: (spaceId: SpaceId) => boolean;
  findSpace: (name: string) => SpaceId | undefined;
  /** The live page of a space with this title. */
  pageByTitle: (spaceId: SpaceId, title: string) => PageId | undefined;
  /** The space a page is in. */
  pageSpace: (pageId: PageId) => SpaceId | undefined;
  /** The mirror's document of a page, when it has one. */
  doc: (pageId: PageId) => LoroDoc | undefined;
}

interface Device {
  spaceId: SpaceId;
  accountId: AccountId;
  deviceId: DeviceId;
  name: string;
  color: number;
  pageId: PageId | undefined;
  state: PresenceState;
  updatedAt: number;
}

interface ScriptedPeer {
  spaceId: SpaceId;
  accountId: AccountId;
  route: readonly MemoryPresenceStop[];
}

type Listener = (message: { info: PresenceInfo; update: Uint8Array }) => void;

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** An `EphemeralStore` update holding the cursors, as the editor encodes them. */
function ephemeralUpdate(anchor: Uint8Array, focus: Uint8Array): Uint8Array {
  const store = new EphemeralStore(PRESENCE_TIMEOUT_SECONDS * 1000);
  try {
    store.set(SENDER_KEY, { anchor, focus });
    return store.encode(SENDER_KEY);
  } finally {
    store.destroy();
  }
}

/** A device id that is stable for an account and the number of the device (`d0000000` is the first). */
function deviceIdOf(accountId: AccountId, device: number): DeviceId {
  return `${(0xd0000000 + device).toString(16)}${accountId.slice(8)}`;
}

function infoOf(device: Device): PresenceInfo {
  return {
    accountId: device.accountId,
    deviceId: device.deviceId,
    name: device.name,
    color: device.color,
    ...(device.pageId === undefined ? {} : { pageId: device.pageId }),
    state: device.state,
    updatedAt: device.updatedAt,
  };
}

/** The presence part of `MemoryTransport`: see the file's comment. */
export class MemoryPresence {
  readonly #deps: PresenceDeps;
  readonly #devices = new Map<string, Device>();
  readonly #listeners = new Set<Listener>();
  readonly #sent: SentPresence[] = [];
  readonly #peers: ScriptedPeer[] = [];
  #steps = 0;
  #expiry: ReturnType<typeof setTimeout> | undefined;
  #script: ReturnType<typeof setInterval> | undefined;
  #seeding = false;

  constructor(deps: PresenceDeps) {
    this.#deps = deps;
  }

  // ----- seeds -------------------------------------------------------------------------------------------------

  /**
   * Starts the scripted people: the first step is taken at once, without events (nothing listens yet), and one more every
   * `stepMs` unless that is 0. A person who is not a member of a shared space of the seeds is a programming error.
   */
  seed(options: MemoryPresenceOptions): void {
    for (const peer of options.peers ?? []) {
      const spaceId = this.#deps.findSpace(peer.space);
      if (spaceId === undefined || !this.#deps.sharing.isLive(spaceId)) {
        throw new CoreError({ code: "Internal", message: "MemoryTransport: a simulated person needs a shared space of the seeds" });
      }
      const member = this.#deps.sharing.memberByName(spaceId, peer.member);
      if (member === undefined || member.isMe) {
        throw new CoreError({ code: "Internal", message: "MemoryTransport: a simulated person must be a member other than this account" });
      }
      if (peer.route.length === 0) {
        throw new CoreError({ code: "Internal", message: "MemoryTransport: a simulated person needs a route" });
      }
      for (const stop of peer.route) {
        if (stop.page !== undefined && this.#deps.pageByTitle(spaceId, stop.page) === undefined) {
          throw new CoreError({ code: "Internal", message: `MemoryTransport: the page "${stop.page}" of a simulated person is not in the space` });
        }
      }
      this.#peers.push({ spaceId, accountId: member.accountId, route: peer.route });
    }
    if (this.#peers.length === 0) {
      return;
    }
    this.#seeding = true;
    try {
      this.#walk();
    } finally {
      this.#seeding = false;
    }
    const stepMs = options.stepMs ?? DEFAULT_PRESENCE_STEP_MS;
    if (stepMs > 0) {
      this.#script = setInterval(() => {
        if (this.#deps.ready()) {
          this.step();
        }
      }, stepMs);
      (this.#script as { unref?: () => void }).unref?.();
    }
  }

  // ----- the requests ---------------------------------------------------------------------------------------------

  /** `PresenceList`: who is where in a space now; nothing for a personal space and a read-only copy. */
  list(spaceId: SpaceId): PresenceInfo[] {
    if (!this.#deps.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
    this.expire();
    return [...this.#devices.values()]
      .filter((device) => device.spaceId === spaceId)
      .sort((a, b) => a.color - b.color || (a.deviceId < b.deviceId ? -1 : 1))
      .map(infoOf);
  }

  /**
   * This device's presence (`PresenceTransport.send`): `InvalidInput` for an update over `PRESENCE_UPDATE_MAX_BYTES`;
   * nothing is sent, and nothing is wrong, for a space that is not live and shared. What was sent is `sent`.
   */
  send(spaceId: SpaceId, pageId: PageId | undefined, state: PresenceState, update: Uint8Array): void {
    if (update.length > PRESENCE_UPDATE_MAX_BYTES) {
      throw invalid("update", `must be at most ${String(PRESENCE_UPDATE_MAX_BYTES)} bytes`);
    }
    if (!this.#deps.hasSpace(spaceId) || !this.#deps.sharing.isLive(spaceId)) {
      return;
    }
    this.#sent.push({ spaceId, pageId, state, update: new Uint8Array(update) });
  }

  /** The frames of `send`, oldest first (for tests). */
  sent(): readonly SentPresence[] {
    return this.#sent;
  }

  /** Registers a listener for the accepted frames; returns the function that removes it. */
  subscribe(listener: Listener): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  // ----- what the other people do ---------------------------------------------------------------------------------

  /** One thing another person's device did; `InvalidInput` when the rules above do not allow it. */
  receive(change: PresenceChange): void {
    this.expire();
    const { spaceId } = change;
    if (!this.#deps.hasSpace(spaceId) || !this.#deps.sharing.isLive(spaceId)) {
      throw invalid("spaceId", "is not a shared space that is live");
    }
    const member = this.#deps.sharing.memberByName(spaceId, change.member);
    if (member === undefined || member.isMe) {
      throw invalid("member", "is not a member of the space other than this account");
    }
    const index = change.device ?? 0;
    if (!Number.isInteger(index) || index < 0 || index > 0xfffff) {
      throw invalid("device", "must be a small whole number");
    }
    const deviceId = deviceIdOf(member.accountId, index);
    if (change.type === "leave") {
      this.#remove([`${spaceId}/${deviceId}`]);
      return;
    }
    const state: PresenceState =
      change.state ?? (change.pageId === undefined ? "Idle" : change.select !== undefined || change.update !== undefined ? "Editing" : "Viewing");
    // A device that is idle is in no page, and neither is one in a page this device does not hold in the space.
    const pageId = state !== "Idle" && change.pageId !== undefined && this.#deps.pageSpace(change.pageId) === spaceId ? change.pageId : undefined;
    let update = change.update ?? new Uint8Array();
    if (change.select !== undefined && pageId !== undefined) {
      update = this.#cursorOf(pageId, change.select) ?? update;
    }
    this.#accept({ spaceId, accountId: member.accountId, deviceId, name: member.name, pageId, state }, update);
  }

  /**
   * Takes `count` steps of the scripted people: each one moves to its next place (or, in a page it edits, its caret to the
   * next word) and sends a frame, which also keeps it from timing out.
   */
  step(count = 1): void {
    for (let i = 0; i < count; i += 1) {
      this.#walk();
    }
  }

  /** Devices whose latest frame is `PRESENCE_TIMEOUT_SECONDS` old are gone; `PresenceChanged` says so. */
  expire(): void {
    const limit = this.#deps.now() - PRESENCE_TIMEOUT_SECONDS * 1000;
    this.#remove([...this.#devices.entries()].filter(([, device]) => device.updatedAt <= limit).map(([key]) => key));
  }

  /** The table goes (a lock: presence is in memory only); `PresenceChanged` says so for each space that had a device. */
  drop(): void {
    this.#remove([...this.#devices.keys()]);
  }

  /** Devices of a member who is no longer one (left, removed), or of a space that is no longer live, are gone. */
  reconcile(): void {
    const gone: string[] = [];
    for (const [key, device] of this.#devices) {
      const member = this.#deps.sharing.memberOf(device.spaceId, device.accountId);
      if (!this.#deps.sharing.isLive(device.spaceId) || member === undefined || member.isMe) {
        gone.push(key);
      }
    }
    this.#remove(gone);
  }

  // ----- inside ---------------------------------------------------------------------------------------------------

  #walk(): void {
    const n = this.#steps;
    this.#steps += 1;
    for (const peer of this.#peers) {
      const member = this.#deps.sharing.memberOf(peer.spaceId, peer.accountId);
      if (member === undefined || !this.#deps.sharing.isLive(peer.spaceId)) {
        continue;
      }
      const total = peer.route.reduce((sum, stop) => sum + Math.max(1, stop.steps ?? 1), 0);
      let at = n % total;
      const stop = peer.route.find((candidate) => {
        at -= Math.max(1, candidate.steps ?? 1);
        return at < 0;
      });
      const pageId = stop?.page === undefined ? undefined : this.#deps.pageByTitle(peer.spaceId, stop.page);
      const edits = pageId !== undefined && stop?.does === "edit";
      const state: PresenceState = pageId === undefined ? "Idle" : edits ? "Editing" : "Viewing";
      const update = edits ? (this.#caretAt(pageId, n) ?? new Uint8Array()) : new Uint8Array();
      this.#accept(
        { spaceId: peer.spaceId, accountId: peer.accountId, deviceId: deviceIdOf(peer.accountId, 0), name: member.name, pageId, state },
        update,
      );
    }
  }

  /** The caret of step `n` in a page: it alternately selects the next word and stands at its end. */
  #caretAt(pageId: PageId, n: number): Uint8Array | undefined {
    const doc = this.#deps.doc(pageId);
    const words = doc === undefined ? [] : textWordsOf(doc);
    const at = n % (2 * Math.max(1, words.length));
    const word = words[at >> 1];
    if (word === undefined) {
      return undefined;
    }
    const end = word.text.getCursor(word.end);
    const start = at % 2 === 0 ? word.text.getCursor(word.start) : end;
    return start === undefined || end === undefined ? undefined : ephemeralUpdate(start.encode(), end.encode());
  }

  /** The cursors of a quote in a page (a caret at one end of it, or the words selected). */
  #cursorOf(pageId: PageId, select: PresenceSelect): Uint8Array | undefined {
    const doc = this.#deps.doc(pageId);
    const found = doc === undefined ? undefined : textAnchorOf(doc, select.quote);
    if (found === undefined) {
      return undefined;
    }
    if (select.caret === "start") {
      return ephemeralUpdate(found.start, found.start);
    }
    return ephemeralUpdate(select.caret === "end" ? found.end : found.start, found.end);
  }

  /** The frame is accepted: the table changes, `PresenceChanged` says so when it must, and the listeners get the frame. */
  #accept(next: Omit<Device, "color" | "updatedAt">, update: Uint8Array): void {
    const key = `${next.spaceId}/${next.deviceId}`;
    const before = this.#devices.get(key);
    const device: Device = {
      ...next,
      // The colour is the member's place in join order and stays while the device is in the table.
      color: before?.color ?? this.#joinOrder(next.spaceId, next.accountId),
      updatedAt: this.#deps.now(),
    };
    this.#devices.set(key, device);
    this.#armExpiry();
    if (!this.#seeding && (before === undefined || before.pageId !== device.pageId || before.state !== device.state)) {
      this.#deps.emit({ type: "PresenceChanged", data: { spaceId: next.spaceId } });
    }
    if (this.#seeding) {
      return;
    }
    for (const listener of [...this.#listeners]) {
      try {
        listener({ info: infoOf(device), update: new Uint8Array(update) });
      } catch {
        // A listener that throws is the UI's bug, and no other listener's loss.
      }
    }
  }

  /** The member's place in join order, 0 for the first (the creator). */
  #joinOrder(spaceId: SpaceId, accountId: AccountId): number {
    const at = this.#deps.sharing.peopleOf(spaceId).findIndex((person) => person.accountId === accountId);
    return Math.max(0, at);
  }

  #remove(keys: readonly string[]): void {
    const spaces = new Set<SpaceId>();
    for (const key of keys) {
      const device = this.#devices.get(key);
      if (device !== undefined) {
        spaces.add(device.spaceId);
        this.#devices.delete(key);
      }
    }
    for (const spaceId of spaces) {
      this.#deps.emit({ type: "PresenceChanged", data: { spaceId } });
    }
  }

  /** A timer for the oldest device's timeout, so that `PresenceChanged` comes when it is due and not when someone asks. */
  #armExpiry(): void {
    if (this.#expiry !== undefined) {
      clearTimeout(this.#expiry);
      this.#expiry = undefined;
    }
    let oldest: number | undefined;
    for (const device of this.#devices.values()) {
      oldest = oldest === undefined ? device.updatedAt : Math.min(oldest, device.updatedAt);
    }
    if (oldest === undefined) {
      return;
    }
    const wait = Math.max(0, oldest + PRESENCE_TIMEOUT_SECONDS * 1000 - this.#deps.now());
    this.#expiry = setTimeout(() => {
      this.#expiry = undefined;
      const before = this.#devices.size;
      this.expire();
      // Re-armed only when something went (a clock that did not move, in a test, must not loop); the next frame arms it too.
      if (this.#devices.size < before) {
        this.#armExpiry();
      }
    }, wait + 1);
    (this.#expiry as { unref?: () => void }).unref?.();
  }
}
