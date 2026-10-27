// Presence of one open page of a shared space (Milestone 13, slice 3; ADR-0050): this device's cursor going out and the other
// members' cursors coming in, for the editor's cursor plugin (`presence-cursors.ts`).
//
// Out: the plugin writes this editor's selection, as Loro cursors, to the ephemeral store (`CursorEphemeralStore`); what goes to
// the transport is the store's update with the page and a state: `Editing` while the editor has a selection and may write,
// `Viewing` otherwise (a Viewer too: a read-only reader may send presence), and one `Idle` frame with no page when the window is
// no longer the one in use. Frames go only while the space is live and shared (`configure`) and the window is the active one
// (`setActive`); the page asks for them as often as it changes and the app's queue keeps the rate to ten a second
// (`PresenceSendQueue`). A frame is also sent every `PRESENCE_HEARTBEAT_MS` while nothing changes, so that the others do not time
// this device out (they do after `PRESENCE_TIMEOUT_SECONDS`).
//
// In: a frame names a device the core verified, with the member's name and colour from membership; its update holds the two
// cursors. The device's cursors are shown while it is in this page and not idle, and go `PRESENCE_TIMEOUT_SECONDS` after its latest
// frame (or when a frame says it left the page). This account's own other devices are not drawn: you are not shown to yourself.

import { PRESENCE_TIMEOUT_SECONDS, PRESENCE_UPDATE_MAX_BYTES } from "@atomik-kb/core-client";
import type { AccountId, PageId, PresenceState, PresenceUpdate, SpaceId } from "@atomik-kb/core-client";

import { presenceColour } from "../presence";
import { CursorEphemeralStore } from "./loro-binding";

/** How often a frame is sent while nothing changes (a third of the timeout, so that one lost frame does not time a device out). */
export const PRESENCE_HEARTBEAT_MS = 10_000;

export interface PagePresenceOptions {
  spaceId: SpaceId;
  pageId: PageId;
  /** The key of this device's own state in the store: the Loro peer id of the page's replica. */
  peerKey: string;
  /** Sends a frame (through the app's queue, which keeps the rate and drops what is not for a live shared space). */
  send: (spaceId: SpaceId, pageId: PageId | undefined, state: PresenceState, update: Uint8Array) => void;
  /** Listens to the frames of other devices. */
  subscribe: (listener: (message: PresenceUpdate) => void) => () => void;
  /** How long a device's cursors live after its latest frame (default `PRESENCE_TIMEOUT_SECONDS`). */
  timeoutMs?: number;
  /** How often a frame is sent while nothing changes (default `PRESENCE_HEARTBEAT_MS`). */
  heartbeatMs?: number;
}

/** What the page's presence follows from the app. */
export interface PagePresenceSettings {
  /** The space is live and shared: otherwise nothing is sent and nothing shown. */
  enabled: boolean;
  /** This account's role may write the page: a Viewer's or Commenter's device `Views` it. */
  canEdit: boolean;
  /** This account, so that its other devices are not drawn (unknown until the member list is read). */
  self?: AccountId;
}

export class PagePresence {
  /** The cursors of this page: the editor's cursor plugin reads and writes it. */
  readonly store: CursorEphemeralStore;
  readonly #options: PagePresenceOptions;
  readonly #heartbeatMs: number;
  readonly #remote = new Set<string>();
  readonly #stops: (() => void)[] = [];
  #settings: PagePresenceSettings = { enabled: false, canEdit: false };
  #active = false;
  #heartbeat: ReturnType<typeof setTimeout> | undefined;
  #destroyed = false;

  constructor(options: PagePresenceOptions) {
    this.#options = options;
    this.#heartbeatMs = options.heartbeatMs ?? PRESENCE_HEARTBEAT_MS;
    this.store = new CursorEphemeralStore(options.peerKey, options.timeoutMs ?? PRESENCE_TIMEOUT_SECONDS * 1000);
    this.#stops.push(
      this.store.subscribeBy((by) => {
        // A move of this device's own cursor is a frame to send; what the others do is for the plugin to draw.
        if (by === "local") {
          this.#push();
        }
      }),
      options.subscribe((message) => {
        this.#receive(message);
      }),
    );
  }

  /** Follows the app: the space's access, this account's role and who this account is. */
  configure(settings: PagePresenceSettings): void {
    const was = this.#settings.enabled;
    this.#settings = settings;
    if (settings.enabled && !was) {
      this.#push();
    } else if (!settings.enabled && was) {
      this.#stopHeartbeat();
      for (const key of [...this.#remote]) {
        this.#drop(key);
      }
    }
  }

  /**
   * The window is the one in use (focused and visible), or it is not: presence goes only while it is, and the end of it sends one
   * `Idle` frame, in no page, so that the others do not wait for the timeout to see this device go.
   */
  setActive(active: boolean): void {
    if (active === this.#active || this.#destroyed) {
      return;
    }
    this.#active = active;
    if (!this.#settings.enabled) {
      return;
    }
    if (active) {
      this.#push();
    } else {
      this.#stopHeartbeat();
      this.#options.send(this.#options.spaceId, undefined, "Idle", new Uint8Array());
    }
  }

  /** The page is left: one `Idle` frame if the device was sending, and nothing is kept. */
  destroy(): void {
    if (this.#destroyed) {
      return;
    }
    this.#destroyed = true;
    this.#stopHeartbeat();
    for (const stop of this.#stops) {
      stop();
    }
    if (this.#settings.enabled && this.#active) {
      this.#options.send(this.#options.spaceId, undefined, "Idle", new Uint8Array());
    }
    this.store.destroy();
  }

  #push(): void {
    if (this.#destroyed || !this.#settings.enabled || !this.#active) {
      return;
    }
    let update = this.store.encodeLocal();
    if (update.length > PRESENCE_UPDATE_MAX_BYTES) {
      // Never reached by two cursors (about 70 bytes); a frame over the limit is refused, and presence is not worth an error.
      update = new Uint8Array();
    }
    const state: PresenceState = update.length > 0 && this.#settings.canEdit ? "Editing" : "Viewing";
    this.#options.send(this.#options.spaceId, this.#options.pageId, state, update);
    this.#stopHeartbeat();
    this.#heartbeat = setTimeout(() => {
      this.#heartbeat = undefined;
      this.#push();
    }, this.#heartbeatMs);
  }

  #stopHeartbeat(): void {
    if (this.#heartbeat !== undefined) {
      clearTimeout(this.#heartbeat);
      this.#heartbeat = undefined;
    }
  }

  #receive({ info, update }: PresenceUpdate): void {
    if (this.#destroyed || !this.#settings.enabled) {
      return;
    }
    const here = info.pageId === this.#options.pageId && info.state !== "Idle" && info.accountId !== this.#settings.self;
    if (!here) {
      this.#drop(info.deviceId);
      return;
    }
    // The name and the colour are the membership's (the core's `PresenceInfo`), never what the frame says.
    this.store.setRemote(info.deviceId, { name: info.name, color: presenceColour(info.color) }, update);
    this.#remote.add(info.deviceId);
  }

  #drop(deviceId: string): void {
    this.#remote.delete(deviceId);
    this.store.removeRemote(deviceId);
  }
}

/**
 * Tells a page's presence whether the window is the one in use: focused and visible. Returns the function that stops listening.
 * The first answer is given at once.
 */
export function followWindowActivity(presence: PagePresence, win: Window = window): () => void {
  const doc = win.document;
  const update = (): void => {
    presence.setActive(doc.visibilityState === "visible" && doc.hasFocus());
  };
  win.addEventListener("focus", update);
  win.addEventListener("blur", update);
  doc.addEventListener("visibilitychange", update);
  update();
  return () => {
    win.removeEventListener("focus", update);
    win.removeEventListener("blur", update);
    doc.removeEventListener("visibilitychange", update);
  };
}
