// React hooks of the presence avatars (Milestone 13, slice 3): the people in a shared space and on a page, read from the core when
// something asks for them and kept fresh by `Event.PresenceChanged` (`store-presence.ts`). You are never one of them.

import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useMemo } from "react";

import { peopleOf } from "./presence";
import type { PresencePerson } from "./presence";
import { useMembers, useSpaceAccess } from "./sharing-hooks";
import { useAppStore } from "./store-context";

/** The members other than this account who have a device in the space (idle ones too, marked so), in join order. */
export function useSpacePeople(spaceId: SpaceId | null): PresencePerson[] {
  return usePeople(spaceId, undefined);
}

/** The members other than this account who are on this page now (a device that is idle is in no page). */
export function usePagePeople(spaceId: SpaceId | null, pageId: PageId): PresencePerson[] {
  return usePeople(spaceId, pageId);
}

function usePeople(spaceId: SpaceId | null, pageId: PageId | undefined): PresencePerson[] {
  const access = useSpaceAccess(spaceId);
  const live = access.shared && !access.archived && !access.pending;
  const available = useAppStore((state) => state.presenceAvailable);
  const loadPresence = useAppStore((state) => state.loadPresence);
  const devices = useAppStore((state) => (spaceId === null ? undefined : state.presence[spaceId]));
  // Who "you" are in this space: the member list says (it is read for the space switcher already). Until it is there nobody is
  // shown, so that this account's own other devices never flash by.
  const members = useMembers(spaceId);
  const me = members?.members.find((member) => member.isMe)?.accountId;
  const known = members !== null;
  const unread = devices === undefined;
  useEffect(() => {
    if (available && live && spaceId !== null && unread) {
      void loadPresence(spaceId);
    }
  }, [available, live, spaceId, unread, loadPresence]);
  return useMemo(
    () => (available && live && known && devices !== undefined ? peopleOf(devices, me, pageId) : []),
    [available, live, known, devices, me, pageId],
  );
}
