// Pure helpers of the shared-space screens (Milestone 13, ADR-0041 to ADR-0047): the roles and what each may do, how a
// space's access decides what the app shows, and the words for roles and invites. The store part is `store-sharing.ts`.

import { INVITE_DEAD_MESSAGE } from "@atomik-kb/core-client";
import type {
  ArchiveReason,
  InviteSessionInfo,
  InviteSessionState,
  InviteStatus,
  MemberRole,
  SpaceInfo,
} from "@atomik-kb/core-client";

import { activeLocale, t } from "./i18n";
import type { MessageKey } from "./i18n";

/** The roles from the most to the least powerful: the order the share dialog explains them in. */
export const ROLES: readonly MemberRole[] = ["owner", "editor", "commenter", "viewer"];

/** The roles an invitation may grant: never Owner (an Owner is made by changing a role). */
export const INVITE_ROLES: readonly MemberRole[] = ["editor", "commenter", "viewer"];

const ROLE_KEYS: Readonly<Record<MemberRole, MessageKey>> = {
  owner: "sharing.role.owner",
  editor: "sharing.role.editor",
  commenter: "sharing.role.commenter",
  viewer: "sharing.role.viewer",
};

const ROLE_HINT_KEYS: Readonly<Record<MemberRole, MessageKey>> = {
  owner: "sharing.role.owner.hint",
  editor: "sharing.role.editor.hint",
  commenter: "sharing.role.commenter.hint",
  viewer: "sharing.role.viewer.hint",
};

/** The name of a role, as people read it. */
export function roleLabel(role: MemberRole): string {
  return t(ROLE_KEYS[role]);
}

/** What a role may do, in one sentence. */
export function roleHint(role: MemberRole): string {
  return t(ROLE_HINT_KEYS[role]);
}

/** Whether `value` is a role this build knows (a newer core may add one). */
export function isRole(value: string): value is MemberRole {
  return (ROLES as readonly string[]).includes(value);
}

const STATUS_KEYS: Readonly<Record<InviteStatus, MessageKey>> = {
  Pending: "sharing.invites.status.pending",
  Open: "sharing.invites.status.open",
  Used: "sharing.invites.status.used",
  Expired: "sharing.invites.status.expired",
  Revoked: "sharing.invites.status.revoked",
};

/** The word for where an invitation stands. */
export function inviteStatusLabel(status: InviteStatus): string {
  return t(STATUS_KEYS[status]);
}

/**
 * What a space's access lets this account do. A personal space (and a space the list does not know) is entirely this
 * account's; a shared one follows the role (ADR-0043: Editors and Owners write, Commenters and up comment, Owners alone delete
 * for good, move out and manage); an archive (a closed space, or one this account was removed from) is read-only for everybody.
 */
export interface SpaceAccess {
  /** A shared space, or an archive of one. */
  shared: boolean;
  /** This account's role; `null` for a personal space. */
  role: MemberRole | null;
  /** The space is a read-only copy. */
  archived: boolean;
  /** Why it is one. */
  archiveReason: ArchiveReason | null;
  /** Edit pages, the tree, properties, files, the space's name and icon. */
  canEdit: boolean;
  /**
   * Write comments (ADR-0048): Commenter and up in a space that is not a read-only copy, and everything in a personal one.
   * A Viewer reads them.
   */
  canComment: boolean;
  /** Delete for good, move pages out of the space, change members and settings, invite, close. */
  isOwner: boolean;
  /** The space is being shared: the server has not accepted it yet. */
  pending: boolean;
}

const PERSONAL: SpaceAccess = {
  shared: false,
  role: null,
  archived: false,
  archiveReason: null,
  canEdit: true,
  canComment: true,
  isOwner: true,
  pending: false,
};

/** The access of a space as the list reports it. */
export function accessOf(space: SpaceInfo | null | undefined): SpaceAccess {
  if (space === undefined || space === null) {
    return PERSONAL;
  }
  const state = space.shareState ?? "Personal";
  if (state === "Personal") {
    return PERSONAL;
  }
  const role = space.role ?? "viewer";
  const archived = state === "Archived";
  return {
    shared: true,
    role,
    archived,
    archiveReason: archived ? (space.archiveReason ?? "Removed") : null,
    canEdit: !archived && (role === "editor" || role === "owner"),
    canComment: !archived && role !== "viewer",
    isOwner: !archived && role === "owner",
    pending: state === "Pending",
  };
}

/** Whether local AI may use the space on this account's devices: always for a personal space, by opt-in for a shared one. */
export function spaceAiOn(space: SpaceInfo | null | undefined): boolean {
  return space?.aiEnabled !== false;
}

/** The ways an invitation session can be over. */
const ENDED: readonly InviteSessionState[] = ["Idle", "Cancelled", "Failed", "Done"];

/** Whether a session is still going (its steps show instead of the way in). */
export function inviteActive(invite: InviteSessionInfo | null): invite is InviteSessionInfo {
  return invite !== null && !ENDED.includes(invite.state);
}

/**
 * The session of this flow and side when one is under way, else `null`.
 */
export function inviteUnderWay(
  invite: InviteSessionInfo | null,
  flow: InviteSessionInfo["flow"],
  side: InviteSessionInfo["side"],
): InviteSessionInfo | null {
  return inviteActive(invite) && invite.flow === flow && invite.side === side ? invite : null;
}

/** An error message of the core as people read it: the one message of every dead invitation is translated. */
export function inviteErrorText(message: string | undefined): string {
  if (message === undefined || message === "") {
    return "";
  }
  return message === INVITE_DEAD_MESSAGE ? t("sharing.error.dead") : message;
}

/** The days a trash retention offers; a value outside this list (set by another Owner) is added to it. */
export const RETENTION_DAYS: readonly number[] = [0, 7, 14, 30, 90, 365];

/** The options of the trash retention select: the usual ones, and the current value when it is not one of them. */
export function retentionOptions(current: number): readonly number[] {
  return RETENTION_DAYS.includes(current) ? RETENTION_DAYS : [...RETENTION_DAYS, current].sort((a, b) => a - b);
}

/** The days an invitation link may live: a day, a week (the default) or a month. */
export const LINK_DAYS: readonly number[] = [1, 7, 30];
export const LINK_DEFAULT_DAYS = 7;

/** A day (a Unix-millisecond time) as people read it, in the locale in use; empty for a value that is not a date. */
export function formatDay(unixMs: number | bigint): string {
  const date = new Date(Number(unixMs));
  return Number.isNaN(date.getTime()) ? "" : new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium" }).format(date);
}

/** A moment (a Unix-millisecond time) as people read it, date and time, in the locale in use; empty for a value that is not a date. */
export function formatMoment(unixMs: number | bigint): string {
  const date = new Date(Number(unixMs));
  return Number.isNaN(date.getTime())
    ? ""
    : new Intl.DateTimeFormat(activeLocale(), { dateStyle: "medium", timeStyle: "short" }).format(date);
}

/** The lifetimes a view link offers, in days (a link may also never expire, which is not a number of days). */
export const VIEW_LINK_DAYS: readonly number[] = [1, 7, 30, 90, 365];

/**
 * What a role lets this account do about view links of the pages of a space (ADR-0051): `allowed` for an Owner, for an Editor
 * when the space's "Editors may create view links" setting is on (or not read yet: the core decides) and for everything in a
 * personal space; `ownerOnly` for an Editor when the setting is off; `none` for a Commenter, a Viewer and a read-only copy.
 */
export type ViewLinkRight = "allowed" | "ownerOnly" | "none";

export function viewLinkRight(access: SpaceAccess, editorsMayLink: boolean | undefined): ViewLinkRight {
  if (!access.shared) {
    return "allowed";
  }
  if (access.archived || access.role === "commenter" || access.role === "viewer") {
    return "none";
  }
  return access.role === "owner" || editorsMayLink !== false ? "allowed" : "ownerOnly";
}

/** The safety number as six groups of five digits, however it came (a core that sends it without spaces). */
export function safetyGroups(safetyNumber: string): string[] {
  const digits = safetyNumber.replace(/\D/gu, "");
  const groups: string[] = [];
  for (let at = 0; at < digits.length; at += 5) {
    groups.push(digits.slice(at, at + 5));
  }
  return groups;
}
