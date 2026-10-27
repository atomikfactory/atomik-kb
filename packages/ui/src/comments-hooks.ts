// React hooks of the comment screens (Milestone 13, slice 2): the people of a space (what a mention chip, the `@` menu, a
// comment's mention and the Person property show names from) and this account among them.

import type { AccountId, PersonInfo, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useMemo } from "react";

import { personName } from "./comments";
import { useMembers } from "./sharing-hooks";
import { useAppStore } from "./store-context";

/**
 * The people of a space, read once when something asks (`undefined` while they are not there yet, and for no space);
 * `PeopleChanged` and `SpaceMembersChanged` keep them fresh. In a personal space the list is this account alone.
 */
export function usePeople(spaceId: SpaceId | null): readonly PersonInfo[] | undefined {
  const people = useAppStore((state) => (spaceId === null ? undefined : state.people[spaceId]));
  const ensurePeople = useAppStore((state) => state.ensurePeople);
  const missing = people === undefined;
  useEffect(() => {
    if (spaceId !== null && missing) {
      ensurePeople(spaceId).catch((error: unknown) => {
        console.error("could not read the people of the space", error instanceof Error ? error.name : "unknown");
      });
    }
  }, [spaceId, missing, ensurePeople]);
  return people;
}

/**
 * The last names of the people who left a space (`MembersInfo.formerMembers`): the member list is read when asked for, and only of a
 * shared space; `null` reads nothing (nobody on the screen is a former member) and gives no names.
 */
export function useFormerNames(spaceId: SpaceId | null): ReadonlyMap<AccountId, string> {
  const info = useMembers(spaceId);
  return useMemo(() => new Map((info?.formerMembers.map((former) => [former.accountId, former.name] as const)) ?? []), [info]);
}

/**
 * What a screen calls the people it lists (`accounts`): a member by the name the space gives now, anyone the space does not list
 * as "Former member (name)" with the last name it knew them by (else the label the caller has, else just "Former member"). The
 * list of former members is read only when one of `accounts` is not a member.
 */
export function usePersonNames(spaceId: SpaceId | null, accounts: readonly AccountId[]): (accountId: AccountId, label?: string) => string {
  const people = usePeople(spaceId);
  const needsFormer = people !== undefined && accounts.some((accountId) => !people.some((person) => person.accountId === accountId));
  const former = useFormerNames(needsFormer ? spaceId : null);
  return (accountId, label = "") => personName(people, accountId, former.get(accountId) ?? label);
}

/** This account's id as the space lists it; `undefined` while the people are not read. */
export function useMyAccountId(spaceId: SpaceId | null): AccountId | undefined {
  return usePeople(spaceId)?.find((person) => person.isMe)?.accountId;
}
