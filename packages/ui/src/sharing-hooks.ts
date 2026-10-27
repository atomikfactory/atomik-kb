// React hooks of the shared-space screens (Milestone 13): the access of a space (what the role lets this account do) and its
// member list, read from the core when something asks for it.

import { toCoreError } from "@atomik-kb/core-client";
import type { MembersInfo, SpaceId, SpaceInfo } from "@atomik-kb/core-client";
import { useEffect, useEffectEvent, useMemo, useState } from "react";

import { accessOf } from "./sharing";
import type { SpaceAccess } from "./sharing";
import { useAppStore } from "./store-context";
import { selectCurrentSpace } from "./store-structure";

/** The space of this id as the sidebar lists it (`undefined` while unknown). */
export function useSpace(spaceId: SpaceId | null): SpaceInfo | undefined {
  return useAppStore((state) => (spaceId === null ? undefined : state.spaces.find((space) => space.id === spaceId)));
}

/** What the access of a space lets this account do (a personal space, and no space at all, is everything). */
export function useSpaceAccess(spaceId: SpaceId | null): SpaceAccess {
  const space = useSpace(spaceId);
  return useMemo(() => accessOf(space), [space]);
}

/** The access of the space in use (the active tab's). */
export function useCurrentAccess(): SpaceAccess {
  const space = useAppStore(selectCurrentSpace);
  return useMemo(() => accessOf(space), [space]);
}

/** What reading the member list of a space gave so far. */
export interface MembersRead {
  /** The list; `null` until it is there, for a personal space, and when the core refused. */
  info: MembersInfo | null;
  /** Why the core refused (a sentence for people); `null` while it has not. */
  error: string | null;
  /** Asks again after a refusal. */
  retry: () => void;
}

/**
 * The member list of a shared space (or archive), read when it was not read before. Events (`SpaceMembersChanged`,
 * `PeopleChanged`) keep it fresh.
 */
export function useMembersRead(spaceId: SpaceId | null): MembersRead {
  const shared = useSpaceAccess(spaceId).shared;
  const info = useAppStore((state) => (spaceId === null ? undefined : state.members[spaceId]));
  const loadMembers = useAppStore((state) => state.loadMembers);
  const [failure, setFailure] = useState<{ spaceId: SpaceId; message: string } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const missing = info === undefined;
  useEffect(() => {
    if (spaceId !== null && shared && missing) {
      loadMembers(spaceId).catch((error: unknown) => {
        const failed = toCoreError(error);
        if (failed.code === "NotFound") {
          // The space is gone from this device (left, or its read-only copy removed) and the space list has not caught up yet: the
          // core's answer to leaving reaches the page before the event that refreshes the list. The list follows; nothing to show.
          return;
        }
        console.error("could not read the member list", failed.code);
        setFailure({ spaceId, message: failed.message });
      });
    }
  }, [spaceId, shared, missing, loadMembers, attempt]);
  return {
    info: shared && info !== undefined ? info : null,
    error: failure !== null && failure.spaceId === spaceId && missing ? failure.message : null,
    retry: () => {
      setFailure(null);
      setAttempt((count) => count + 1);
    },
  };
}

/** How long "Open the space" waits for the row of a space that was just joined before it says so. */
export const OPEN_JOINED_WAIT_MS = 15_000;

/** What "Open the space" of a join dialog is doing. */
export interface OpenJoinedSpace {
  /** Opens the space now when its row is in the list, and otherwise waits for it. */
  open: () => void;
  /** Waiting for the row of the space: the button says "Opening...". */
  opening: boolean;
  /** The row did not come within `OPEN_JOINED_WAIT_MS`: the dialog says so and the button works again. */
  late: boolean;
}

/**
 * "Open the space" of the dialogs that say "You joined <space>" (by code, by link). The invite is done when the `Join` is posted; the
 * space itself arrives through the feed a moment later, and its row reaches the list with the core's next `SpacesChanged`. Pressed in
 * between, the button waits for the row (it is the newest shared space of that name) instead of closing the dialog with nothing opened,
 * and says so when the row does not come. `onOpened` closes the dialog.
 */
export function useOpenJoinedSpace(spaceName: string | undefined, onOpened: () => void): OpenJoinedSpace {
  const spaces = useAppStore((state) => state.spaces);
  const selectSpace = useAppStore((state) => state.selectSpace);
  const [phase, setPhase] = useState<"idle" | "waiting" | "late">("idle");
  const joinedId = spaceName === undefined ? undefined : [...spaces].reverse().find((space) => space.name === spaceName && space.shareState === "Shared")?.id;
  const opened = useEffectEvent(() => {
    onOpened();
  });

  useEffect(() => {
    if (phase !== "waiting") {
      return undefined;
    }
    if (joinedId !== undefined) {
      selectSpace(joinedId);
      opened();
      return undefined;
    }
    const timer = setTimeout(() => {
      setPhase("late");
    }, OPEN_JOINED_WAIT_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [phase, joinedId, selectSpace]);

  return {
    open: () => {
      if (joinedId !== undefined) {
        selectSpace(joinedId);
        onOpened();
      } else {
        setPhase("waiting");
      }
    },
    opening: phase === "waiting",
    late: phase === "late",
  };
}

/** The member list of a shared space (or archive): `null` until it is there, for a personal space, and when the core refused. */
export function useMembers(spaceId: SpaceId | null): MembersInfo | null {
  return useMembersRead(spaceId).info;
}
