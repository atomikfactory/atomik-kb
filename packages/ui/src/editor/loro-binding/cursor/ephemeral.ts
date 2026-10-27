// Re-vendored from loro-prosemirror 0.4.4 (https://github.com/loro-dev/loro-prosemirror,
// commit 0e036a9), `src/cursor/ephemeral.ts`. MIT License: Copyright (c) 2019 Kevin Jahns,
// Copyright (c) 2024 LORO; see LICENSE-loro-prosemirror. Local changes are marked
// "atomik-kb patch" and listed in VENDORED.md.
//
// atomik-kb patch (c2), ADR-0050: the cursor store of upstream's `ephemeral.ts` is back, on
// Loro's `EphemeralStore` (the update of a frame is its encoding), with what presence between
// members of a shared space needs on top of it: a remote device's state is keyed by the device
// the core verified (never by a key the sender picked, so a sender cannot overwrite another
// device's cursor), whatever the update holds besides the two cursors is dropped (the name and
// the colour come from membership, never from the frame), the update is decoded on a store of its
// own whose long timeout does not depend on the sender's clock, and a state is removed exactly
// `ttl` after its latest frame (Loro's own cleanup only runs at half the timeout).

import { Cursor, EphemeralStore } from "loro-crdt";
import { PRESENCE_UPDATE_MAX_BYTES } from "@atomik-kb/core-client";

import { createCursorPlugin } from "./common";
import type { CursorPluginOptions, CursorState, PresenceStore, PresenceUser } from "./common";

/** How a store changed: by this device (`local`), by a frame of another (`import`), or by a state running out (`timeout`). */
export type EphemeralBy = "local" | "import" | "timeout";

/** What one device's state is in the store: the cursors as bytes, and (for a remote device) who it is. */
export type CursorWire = {
  anchor?: Uint8Array;
  focus?: Uint8Array;
  user?: PresenceUser;
};

/**
 * The clock of the sender is not ours: an update is decoded by a store whose timeout is this long, so that a clock that is
 * off by hours still decodes (Loro compares the sender's timestamp with the receiver's clock).
 */
const DECODE_TIMEOUT_MS = 7 * 24 * 3600 * 1000;
/** A Loro cursor is about 20 bytes; a larger one is not one. */
const MAX_CURSOR_BYTES = 256;

function cursorBytes(value: unknown): Uint8Array | undefined {
  return value instanceof Uint8Array && value.length > 0 && value.length <= MAX_CURSOR_BYTES ? value : undefined;
}

/**
 * The two cursors a frame's update holds, as bytes. `undefined` for an update that holds none, is empty, is over the limit of a
 * frame or is not an `EphemeralStore` update at all (it comes from another member's device: anything may arrive).
 */
export function decodeCursorUpdate(update: Uint8Array): CursorWire | undefined {
  if (update.length === 0 || update.length > PRESENCE_UPDATE_MAX_BYTES) {
    return undefined;
  }
  const decoder = new EphemeralStore(DECODE_TIMEOUT_MS);
  try {
    decoder.apply(update);
    // The key is the sender's own (upstream: its Loro peer id); only the state is read, and only from the first key.
    const [state] = Object.values(decoder.getAllStates()) as unknown[];
    if (typeof state !== "object" || state === null) {
      return undefined;
    }
    const anchor = cursorBytes((state as Record<string, unknown>).anchor);
    const focus = cursorBytes((state as Record<string, unknown>).focus);
    return anchor === undefined && focus === undefined
      ? undefined
      : { ...(anchor === undefined ? {} : { anchor }), ...(focus === undefined ? {} : { focus }) };
  } catch {
    return undefined;
  } finally {
    decoder.destroy();
  }
}

function decodeCursor(bytes: Uint8Array | undefined): Cursor | undefined {
  if (bytes === undefined) {
    return undefined;
  }
  try {
    return Cursor.decode(bytes);
  } catch {
    return undefined;
  }
}

/**
 * Where the people in a page have their cursors: this device's own (under `peer`, the Loro peer id of the replica, as upstream
 * keys it) and the other devices' (under the id of the device). `encodeLocal` is the update a presence frame carries.
 */
export class CursorEphemeralStore extends EphemeralStore<Record<string, CursorWire>> {
  /** The key of this device's own state, never drawn. */
  readonly peer: string;
  readonly #ttl: number;
  readonly #expiry = new Map<string, ReturnType<typeof setTimeout>>();
  readonly #listeners = new Set<(by: EphemeralBy) => void>();
  /** This device's state as given, and what of it goes into frames (the cursors only: the name and colour are not sent). */
  #local: CursorState | undefined;
  #localWire: CursorWire | undefined;

  /**
   * `ttl` is how long a remote device's state lives after its latest frame (`PRESENCE_TIMEOUT_SECONDS`, 30 s). Loro's own timeout
   * is the same, so a state is gone from `getAllStates` when it is due even if no timer ran; the removals and the announcements of
   * a removal are the timers of `setRemote`, not Loro's cleanup (which only runs every half timeout).
   */
  constructor(peer: string, ttl = 30_000) {
    super(ttl);
    this.peer = peer;
    this.#ttl = ttl;
  }

  /** This device's cursors; none (no anchor and no focus) removes its state. */
  setLocal(state: CursorState): void {
    const anchor = state.anchor?.encode();
    const focus = state.focus?.encode();
    if (anchor === undefined && focus === undefined) {
      if (this.#local !== undefined) {
        this.#local = undefined;
        this.#localWire = undefined;
        this.delete(this.peer);
        this.#emit("local");
      }
      return;
    }
    this.#local = state;
    this.#localWire = { ...(anchor === undefined ? {} : { anchor }), ...(focus === undefined ? {} : { focus }) };
    this.set(this.peer, this.#localWire);
    this.#emit("local");
  }

  /** This device's cursors, `undefined` when it has none. */
  getLocal(): CursorState | undefined {
    return this.#local;
  }

  /** Every device's state, this device's included, by key. */
  getAll(): Record<string, CursorState> {
    const all: Record<string, CursorState> = {};
    for (const [key, wire] of Object.entries(this.getAllStates() as Record<string, CursorWire | undefined>)) {
      if (wire !== undefined && key !== this.peer) {
        all[key] = this.#state(wire);
      }
    }
    if (this.#local !== undefined) {
      all[this.peer] = this.#local;
    }
    return all;
  }

  /**
   * The update of a presence frame: this device's cursors as an `EphemeralStore` update, empty when it has none. Loro dates a
   * state by when it was set, so it is set again here: a caret that does not move still makes frames that say it is there.
   */
  encodeLocal(): Uint8Array {
    if (this.#localWire === undefined) {
      return new Uint8Array();
    }
    this.set(this.peer, this.#localWire);
    return this.encode(this.peer);
  }

  /**
   * A frame of another device (`key`: the id of the device the core verified): its cursors from `update`, shown as `user`
   * (the member's name and colour, from membership), gone `ttl` from now. An update without a cursor (the device views the page)
   * removes the device's cursors; the device's frame still counts, so this is not the end of its state's life when it is
   * followed by another. Anything the update holds besides the two cursors is dropped.
   */
  setRemote(key: string, user: PresenceUser, update: Uint8Array): void {
    const wire = decodeCursorUpdate(update);
    if (wire === undefined || key === this.peer) {
      this.removeRemote(key);
      return;
    }
    this.set(key, { ...wire, user });
    const pending = this.#expiry.get(key);
    if (pending !== undefined) {
      clearTimeout(pending);
    }
    this.#expiry.set(
      key,
      setTimeout(() => {
        this.#expiry.delete(key);
        this.#remove(key, "timeout");
      }, this.#ttl),
    );
    this.#emit("import");
  }

  /** The device's cursors go (it moved to another page, went idle or left). */
  removeRemote(key: string): void {
    const pending = this.#expiry.get(key);
    if (pending !== undefined) {
      clearTimeout(pending);
      this.#expiry.delete(key);
    }
    this.#remove(key, "import");
  }

  /** Listens to changes with what changed them (upstream `subscribeBy`). Returns the function that stops listening. */
  subscribeBy(listener: (by: EphemeralBy) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  /** Stops every timer; the store is not used afterwards. */
  override destroy(): void {
    for (const pending of this.#expiry.values()) {
      clearTimeout(pending);
    }
    this.#expiry.clear();
    this.#listeners.clear();
    super.destroy();
  }

  #remove(key: string, by: EphemeralBy): void {
    if (this.get(key) !== undefined) {
      this.delete(key);
      this.#emit(by);
    }
  }

  #state(wire: CursorWire): CursorState {
    const anchor = decodeCursor(wire.anchor);
    const focus = decodeCursor(wire.focus);
    return {
      ...(anchor === undefined ? {} : { anchor }),
      ...(focus === undefined ? {} : { focus }),
      ...(wire.user === undefined ? {} : { user: wire.user }),
    };
  }

  #emit(by: EphemeralBy): void {
    for (const listener of [...this.#listeners]) {
      listener(by);
    }
  }
}

/** The store as the cursor plugin reads it (upstream's `ephemeralStoreAdapter`). */
function ephemeralStoreAdapter(store: CursorEphemeralStore): PresenceStore {
  return {
    localKey: store.peer,
    getAll: () => store.getAll(),
    getLocal: () => store.getLocal(),
    setLocal: (state) => {
      store.setLocal(state);
    },
    subscribeBy: (listener) => store.subscribeBy(listener),
  };
}

/** The cursor plugin on an ephemeral store: remote carets and selections drawn, and this device's selection written to it. */
export function LoroEphemeralCursorPlugin(store: CursorEphemeralStore, options: CursorPluginOptions = {}) {
  return createCursorPlugin(ephemeralStoreAdapter(store), options);
}
