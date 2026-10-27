// The sharing part of the app store (Milestone 13, ADR-0041 to ADR-0047): the member lists of shared spaces, this
// account's profile name, the invite session of either flow and either side, and what the core's events say happened to
// this account in a shared space (a role change, a removal, a member's device added with its kit, unsent changes that were
// copied, an incomplete snapshot). `store.ts` composes it with the rest.
//
// As in the other slices, reads are "latest request wins" and events refresh: `SpaceMembersChanged` and `PeopleChanged`
// re-read the member lists that were read before, `InviteChanged` the session, and the space list itself (what the sidebar,
// the editor and the banners follow: the role, the share state, the member count, whether AI is on) is the structure
// slice's, refreshed by `SpacesChanged`.
//
// Names, safety numbers, invite codes and links are in memory only: a lock clears them (`EMPTY_SHARING`).

import { toCoreError } from "@atomik-kb/core-client";
import type {
  AccountId,
  CoreClient,
  Event as CoreEvent,
  InviteId,
  InviteLinkInfo,
  InviteSessionInfo,
  MemberRole,
  MembersInfo,
  PageId,
  ProfileInfo,
  SpaceId,
  SpaceInfo,
  SpaceSettings,
  SpaceSharePlan,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { t, tp } from "./i18n";
import { accessOf, inviteActive, roleLabel } from "./sharing";
import type { AppState } from "./store";
import { selectCurrentSpace } from "./store-structure";

export interface SharingData {
  /** The member lists read so far, by space; a space is present once its list was read. */
  members: Readonly<Record<SpaceId, MembersInfo>>;
  /** This account's profile; `null` before it was read. */
  profile: ProfileInfo | null;
  /** The invite session as the core reports it; `null` before it was read. */
  invite: InviteSessionInfo | null;
  /** How many sessions this store started (and one found under way at boot): a session is shown while it is newer than the last one dismissed. */
  inviteEpoch: number;
  /** The `inviteEpoch` of the last session whose dialog was closed. */
  inviteDismissed: number;
  /** The "Join a shared space" dialog is open. */
  joinOpen: boolean;
  /** The notices of the home screen that were dismissed (a notice's key says what it was about and how many). */
  dismissedNotices: readonly string[];
}

/**
 * The invite session whose dialog is to show: the latest one, until its dialog was closed (`dismissInvite`). A finished
 * session stays up to say how it ended; `null` when there is none, or the person closed it.
 */
export const selectVisibleInvite = (state: SharingData): InviteSessionInfo | null =>
  state.invite !== null && state.invite.state !== "Idle" && state.inviteDismissed < state.inviteEpoch ? state.invite : null;

/** Whether this account's role lets it edit the space in use (a personal space: always; a read-only copy: never). */
export const selectCurrentCanEdit = (state: AppState): boolean => accessOf(selectCurrentSpace(state)).canEdit;

export const EMPTY_SHARING: SharingData = {
  members: {},
  profile: null,
  invite: null,
  inviteEpoch: 0,
  inviteDismissed: 0,
  joinOpen: false,
  dismissedNotices: [],
};

/** What an Owner may change about a space's settings; a field left out stays. */
export type SpaceSettingsChange = Partial<SpaceSettings>;

/**
 * Every action rejects with a `CoreError` when the core refuses (`Forbidden` below the needed role); what follows comes
 * back through events, and the direct answers are applied right away.
 */
export interface SharingActions {
  /** What sharing a space would send, counted (nothing is written). */
  planShare: (spaceId: SpaceId) => Promise<SpaceSharePlan>;
  /** Shares a personal space: the core answers `Pending`, then the space is `Shared` once the server accepted it. */
  shareSpace: (spaceId: SpaceId) => Promise<SpaceInfo>;
  /** Reads the members of a shared space (or archive) and keeps them in `members`. */
  loadMembers: (spaceId: SpaceId) => Promise<MembersInfo>;
  setMemberRole: (spaceId: SpaceId, accountId: AccountId, role: MemberRole) => Promise<void>;
  removeMember: (spaceId: SpaceId, accountId: AccountId) => Promise<void>;
  /** Leaves a space; on an archive it only deletes the local copy. */
  leaveSpace: (spaceId: SpaceId) => Promise<void>;
  closeSpace: (spaceId: SpaceId) => Promise<void>;
  setSpaceSettings: (spaceId: SpaceId, change: SpaceSettingsChange) => Promise<void>;
  /** This account's own switch: whether local AI may use the space. */
  setSpaceAi: (spaceId: SpaceId, enabled: boolean) => Promise<void>;
  /** Makes an invitation link; the answer is shown once and not kept. */
  createInviteLink: (spaceId: SpaceId, role: MemberRole, expiresInDays?: number) => Promise<InviteLinkInfo>;
  revokeInvite: (spaceId: SpaceId, inviteId: InviteId) => Promise<void>;
  /** "Mark verified" (or not): per person, for every space. */
  setPersonVerified: (accountId: AccountId, verified: boolean) => Promise<void>;
  refreshProfile: () => Promise<void>;
  setProfileName: (name: string) => Promise<void>;
  refreshInvite: () => Promise<void>;
  /** The inviter's side of the code flow: shows a code once the session is on. */
  startCodeInvite: (spaceId: SpaceId, role: MemberRole) => Promise<InviteSessionInfo>;
  /** The invitee's side of the code flow (`InvalidInput` for `serverUrl`, `code`, `workspace` or `account`). */
  joinWithCode: (serverUrl: string, code: string) => Promise<InviteSessionInfo>;
  /**
   * Opens an invitation link (`atomik-kb://invite?...`, or the `/join` form): the session shows its preview. This is the
   * hook the desktop shell's deep links call (`store-deep-links.ts`); the link is never logged or kept beyond the session.
   */
  openInviteLink: (link: string) => Promise<InviteSessionInfo>;
  /** The digits match (code flow) or the preview is accepted (link flow). */
  confirmInvite: () => Promise<InviteSessionInfo>;
  cancelInvite: () => Promise<InviteSessionInfo>;
  /** The dialog of the session was closed: it is not shown again (a session under way is cancelled by the caller). */
  dismissInvite: () => void;
  openJoinDialog: () => void;
  closeJoinDialog: () => void;
  dismissSharingNotice: (key: string) => void;
}

export type SharingState = SharingData & SharingActions;

export interface SharingSlice {
  initial: SharingState;
  /** Reads the profile and the invite session for the boot sequence; a failure other than `Locked` leaves them unknown. */
  load: () => Promise<Partial<SharingData>>;
  onEvent: (event: CoreEvent) => void;
}

export function createSharingSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): SharingSlice {
  // Bumped by every read or action that answers the session or the profile; only the latest applies.
  let inviteTicket = 0;
  let profileTicket = 0;
  const memberTickets = new Map<SpaceId, number>();

  function spaceName(spaceId: SpaceId): string | undefined {
    return get().spaces.find((space) => space.id === spaceId)?.name;
  }

  function keepMembers(info: MembersInfo): void {
    set((state) => ({ members: { ...state.members, [info.spaceId]: info } }));
  }

  async function loadMembers(spaceId: SpaceId): Promise<MembersInfo> {
    const mine = (memberTickets.get(spaceId) ?? 0) + 1;
    memberTickets.set(spaceId, mine);
    const info = await client.spaceMembers(spaceId);
    if (memberTickets.get(spaceId) === mine) {
      keepMembers(info);
    }
    return info;
  }

  /** Re-reads a member list that was read before (or the current space's), quietly. */
  function reloadMembers(spaceId: SpaceId): void {
    if (get().members[spaceId] !== undefined || get().currentSpaceId === spaceId) {
      refreshQuietly(() => loadMembers(spaceId));
    }
  }

  async function inviteCall(call: () => Promise<InviteSessionInfo>): Promise<InviteSessionInfo> {
    const mine = (inviteTicket += 1);
    const invite = await call();
    if (mine === inviteTicket) {
      set({ invite });
    }
    return invite;
  }

  /**
   * Starts a session: like `inviteCall`, and the session is a new one for the dialogs to show. The event of the start
   * (`InviteChanged`) makes a read of its own that may be the latest, so the new session counts whichever read wins.
   */
  async function inviteStart(call: () => Promise<InviteSessionInfo>): Promise<InviteSessionInfo> {
    const mine = (inviteTicket += 1);
    const invite = await call();
    set((state) => ({ inviteEpoch: state.inviteEpoch + 1, ...(mine === inviteTicket ? { invite } : {}) }));
    return invite;
  }

  async function refreshInvite(): Promise<void> {
    await inviteCall(() => client.inviteStatus());
  }

  async function refreshProfile(): Promise<void> {
    const mine = (profileTicket += 1);
    const profile = await client.profileGet();
    if (mine === profileTicket) {
      set({ profile });
    }
  }

  async function quietly<T>(read: () => Promise<T>, what: string): Promise<T | null> {
    try {
      return await read();
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error(`could not read the ${what}`, failure.code);
      return null;
    }
  }

  async function load(): Promise<Partial<SharingData>> {
    const inviteMine = (inviteTicket += 1);
    const profileMine = (profileTicket += 1);
    const [profile, invite] = await Promise.all([
      quietly(() => client.profileGet(), "profile"),
      quietly(() => client.inviteStatus(), "invite status"),
    ]);
    return {
      ...(profileMine === profileTicket ? { profile } : {}),
      // A session found under way (the page was reloaded) shows its dialog again.
      ...(inviteMine === inviteTicket ? { invite, inviteEpoch: inviteActive(invite) ? 1 : 0, inviteDismissed: 0 } : {}),
    };
  }

  /** A member's name in a space, from the list when it was read, else the list is read first. */
  async function nameOfMember(spaceId: SpaceId, accountId: AccountId): Promise<string | null> {
    const known = get().members[spaceId]?.members.find((member) => member.accountId === accountId);
    if (known !== undefined) {
      return known.name;
    }
    const info = await quietly(() => loadMembers(spaceId), "member list");
    return info?.members.find((member) => member.accountId === accountId)?.name ?? null;
  }

  async function titleOfPage(pageId: PageId | undefined): Promise<string | null> {
    if (pageId === undefined) {
      return null;
    }
    const page = await quietly(() => client.pageGet(pageId), "page");
    return page === null ? null : page.title === "" ? t("page.untitled") : page.title;
  }

  const initial: SharingState = {
    ...EMPTY_SHARING,

    planShare: (spaceId) => client.spaceSharePlan(spaceId),
    shareSpace: (spaceId) => client.spaceShare(spaceId),
    loadMembers,

    setMemberRole: async (spaceId, accountId, role) => {
      keepMembers(await client.memberRoleSet(spaceId, accountId, role));
    },
    removeMember: async (spaceId, accountId) => {
      keepMembers(await client.memberRemove(spaceId, accountId));
    },
    leaveSpace: async (spaceId) => {
      await client.spaceLeave(spaceId);
      set((state) => ({ members: Object.fromEntries(Object.entries(state.members).filter(([id]) => id !== spaceId)) }));
    },
    closeSpace: async (spaceId) => {
      await client.spaceClose(spaceId);
      refreshQuietly(() => loadMembers(spaceId));
    },
    setSpaceSettings: async (spaceId, change) => {
      keepMembers(await client.spaceSettingsSet(spaceId, change));
    },
    setSpaceAi: async (spaceId, enabled) => {
      await client.spaceAiSet(spaceId, enabled);
    },
    createInviteLink: async (spaceId, role, expiresInDays) => {
      const link = await client.inviteLinkCreate(spaceId, role, expiresInDays);
      refreshQuietly(() => loadMembers(spaceId));
      return link;
    },
    revokeInvite: async (spaceId, inviteId) => {
      keepMembers(await client.inviteRevoke(spaceId, inviteId));
    },
    setPersonVerified: async (accountId, verified) => {
      await client.personVerifiedSet(accountId, verified);
    },
    refreshProfile,
    setProfileName: async (name) => {
      const mine = (profileTicket += 1);
      const profile = await client.profileSet(name);
      if (mine === profileTicket) {
        set({ profile });
      }
    },
    refreshInvite,
    startCodeInvite: (spaceId, role) => inviteStart(() => client.inviteCodeStart(spaceId, role)),
    joinWithCode: (serverUrl, code) => inviteStart(() => client.inviteCodeJoin(serverUrl, code)),
    openInviteLink: (link) => inviteStart(() => client.inviteLinkOpen(link)),
    confirmInvite: () => inviteCall(() => client.inviteConfirm()),
    cancelInvite: () => inviteCall(() => client.inviteCancel()),
    dismissInvite: () => {
      set((state) => ({ inviteDismissed: state.inviteEpoch }));
    },
    openJoinDialog: () => {
      set({ joinOpen: true });
    },
    closeJoinDialog: () => {
      set({ joinOpen: false });
    },
    dismissSharingNotice: (key) => {
      set((state) => (state.dismissedNotices.includes(key) ? state : { dismissedNotices: [...state.dismissedNotices, key] }));
    },
  };

  return {
    initial,
    load,
    onEvent: (event) => {
      if (get().phase === "locked") {
        // The next boot reads what it needs.
        return;
      }
      switch (event.type) {
        case "SpaceMembersChanged":
          reloadMembers(event.data.spaceId);
          // The member count of the space switcher's badge comes with the space (`SpaceList`), and no `SpacesChanged` comes
          // with a member who joined: the list is read again with the members.
          refreshQuietly(() => get().refreshSpaces());
          break;
        case "SpaceRoleChanged": {
          const { spaceId, role, previous } = event.data;
          const name = spaceName(spaceId);
          if (name !== undefined) {
            get().showToast(
              t("sharing.notice.roleChanged", { space: name, previous: roleLabel(previous), role: roleLabel(role) }),
            );
          }
          reloadMembers(spaceId);
          break;
        }
        case "SpaceRemoved": {
          const { spaceId, reason } = event.data;
          const name = spaceName(spaceId);
          if (name !== undefined) {
            get().showToast(t(reason === "Closed" ? "sharing.notice.closed" : "sharing.notice.removed", { space: name }));
          }
          reloadMembers(spaceId);
          break;
        }
        case "MemberDeviceViaKit":
          get().showToast(t("sharing.notice.viaKit", { name: event.data.name }));
          reloadMembers(event.data.spaceId);
          break;
        case "DevicesChanged":
          // This account's own row counts its devices and notes one added with the recovery kit: the lists read before are
          // stale once the device log brings a device (found by the real-app pass, a panel reopened after a kit recovery).
          for (const spaceId of Object.keys(get().members)) {
            refreshQuietly(() => loadMembers(spaceId));
          }
          break;
        case "PeopleChanged":
          refreshQuietly(refreshProfile);
          for (const spaceId of Object.keys(get().members)) {
            refreshQuietly(() => loadMembers(spaceId));
          }
          break;
        case "InviteChanged":
          refreshQuietly(refreshInvite);
          break;
        case "UnsentChangesCopied": {
          const { spaceId, targetSpaceId, pageIds } = event.data;
          const source = spaceName(spaceId);
          const target = spaceName(targetSpaceId);
          const first = pageIds[0];
          if (source !== undefined && target !== undefined && first !== undefined) {
            get().showToast(
              tp("sharing.notice.unsent", pageIds.length, { space: source, target }),
              "info",
              {
                label: t("sharing.notice.unsent.open"),
                run: () => {
                  get().navigateTo({ pageId: first, spaceId: targetSpaceId, disposition: "current" });
                },
              },
            );
          }
          break;
        }
        case "SnapshotDisputed": {
          const { spaceId, accountId, pageId } = event.data;
          void (async () => {
            const [member, page] = await Promise.all([nameOfMember(spaceId, accountId), titleOfPage(pageId)]);
            const name = member ?? t("sharing.notice.aMember");
            get().showToast(
              page === null
                ? t("sharing.notice.disputedNoPage", { name })
                : t("sharing.notice.disputed", { name, page }),
            );
          })();
          reloadMembers(spaceId);
          break;
        }
        // The other events are not about sharing.
      }
    },
  };
}
