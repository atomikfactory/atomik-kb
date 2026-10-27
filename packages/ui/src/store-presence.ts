// The presence part of the app store (Milestone 13, slice 3; ADR-0050): who is where in the shared spaces (what the avatars of the
// page header and the space switcher show), this device's presence going out through a queue that keeps it to ten frames a second,
// and the frames of the other devices coming in (what the editor draws its cursors from).
//
// As in the other slices, reads are "latest request wins" and events refresh: `Event.PresenceChanged` (someone joined, left or
// changed page; not a cursor move) re-reads the list of that space once it was read. The cursors themselves never enter the store:
// the editor listens to the frames directly (`subscribePresence`), as each frame is meant for the page it names.
//
// Presence is in memory only, and best effort: a lock clears it (`EMPTY_PRESENCE`), a space that is not live (personal, a read-only
// copy, still being shared) has none, and nothing is sent for it.

import { toCoreError } from "@atomik-kb/core-client";
import type {
  CoreClient,
  Event as CoreEvent,
  PageId,
  PresenceInfo,
  PresenceState,
  PresenceUpdate,
  SpaceId,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { PresenceSendQueue } from "./presence";
import { accessOf } from "./sharing";
import type { AppState } from "./store";

export interface PresenceData {
  /** The devices of the other members in each shared space whose list was read, as the core lists them (this account's own other devices too). */
  presence: Readonly<Record<SpaceId, readonly PresenceInfo[]>>;
}

export const EMPTY_PRESENCE: PresenceData = { presence: {} };

export interface PresenceActions {
  /** Whether the transport carries presence (the desktop shell and the in-memory core do); without it the UI shows no avatars and no cursors. */
  presenceAvailable: boolean;
  /** Reads who is where in a space (nothing for a space that is not live and shared) and keeps it in `presence`. */
  loadPresence: (spaceId: SpaceId) => Promise<void>;
  /**
   * Sends this device's presence: the page (`undefined`: in no page), what it does and the editor's ephemeral-store update. Nothing
   * is sent for a space that is not live and shared, and never more than ten frames a second (the newest of a space wins).
   */
  sendPresence: (spaceId: SpaceId, pageId: PageId | undefined, state: PresenceState, update: Uint8Array) => void;
  /** Listens to the frames other members' devices sent (the editor's cursors); returns the function that stops listening. */
  subscribePresence: (listener: (message: PresenceUpdate) => void) => () => void;
}

export type PresenceSliceState = PresenceData & PresenceActions;

export interface PresenceSlice {
  initial: PresenceSliceState;
  onEvent: (event: CoreEvent) => void;
}

/** Whether presence is sent and listed for a space: shared, accepted by the server and not a read-only copy. */
export function presenceLive(state: AppState, spaceId: SpaceId): boolean {
  const access = accessOf(state.spaces.find((space) => space.id === spaceId));
  return access.shared && !access.archived && !access.pending;
}

export function createPresenceSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): PresenceSlice {
  const available = client.hasPresence();
  const tickets = new Map<SpaceId, number>();
  let failureLogged = false;

  const queue = new PresenceSendQueue((frame) =>
    client.presenceSend(frame.spaceId, frame.pageId, frame.state, frame.update).catch((error: unknown) => {
      // Once, and the code only: presence is best effort, and a frame holds nothing the log may keep.
      if (!failureLogged) {
        failureLogged = true;
        console.error("could not send presence", toCoreError(error).code);
      }
    }),
  );

  function drop(spaceId: SpaceId): void {
    tickets.set(spaceId, (tickets.get(spaceId) ?? 0) + 1);
    if (get().presence[spaceId] !== undefined) {
      set((state) => ({ presence: Object.fromEntries(Object.entries(state.presence).filter(([id]) => id !== spaceId)) }));
    }
  }

  async function loadPresence(spaceId: SpaceId): Promise<void> {
    if (!available) {
      return;
    }
    if (!presenceLive(get(), spaceId)) {
      drop(spaceId);
      return;
    }
    const mine = (tickets.get(spaceId) ?? 0) + 1;
    tickets.set(spaceId, mine);
    let devices: PresenceInfo[];
    try {
      devices = await client.presenceList(spaceId);
    } catch (error) {
      const failure = toCoreError(error);
      // A locked core and a space that is gone (left, the list not caught up) are not worth a line.
      if (failure.code !== "Locked" && failure.code !== "NotFound") {
        console.error("could not read who is in the space", failure.code);
      }
      return;
    }
    if (tickets.get(spaceId) === mine && presenceLive(get(), spaceId)) {
      set((state) => ({ presence: { ...state.presence, [spaceId]: devices } }));
    }
  }

  const initial: PresenceSliceState = {
    ...EMPTY_PRESENCE,
    presenceAvailable: available,
    loadPresence,
    sendPresence: (spaceId, pageId, state, update) => {
      if (available && presenceLive(get(), spaceId)) {
        queue.push({ spaceId, pageId, state, update });
      }
    },
    subscribePresence: (listener) => client.onPresence(listener),
  };

  return {
    initial,
    onEvent: (event) => {
      if (get().phase === "locked") {
        queue.clear();
        return;
      }
      switch (event.type) {
        case "PresenceChanged": {
          const { spaceId } = event.data;
          if (get().presence[spaceId] !== undefined || get().currentSpaceId === spaceId) {
            refreshQuietly(() => loadPresence(spaceId));
          }
          break;
        }
        case "SpaceRemoved":
          drop(event.data.spaceId);
          break;
        // The other events are not about presence.
      }
    },
  };
}
