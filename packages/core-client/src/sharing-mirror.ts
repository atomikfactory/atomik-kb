// The in-memory twin of the core's shared spaces (Milestone 13, slice 1; ADR-0041 to ADR-0047, the contract in
// `docs/development/milestone-13.md` section 1.1 and the design in `docs/security/sharing.md`), used by
// `MemoryTransport`. There is no server and no cryptography here: the other people (Alice, Bob, ...) are
// simulated, their account ids and safety numbers are made up, and the invite sessions of both flows run on timers
// of `stepMs` (like the pairing mirror of `./devices-mirror`) so that the UI can walk through every state.
//
// What it enforces is what the membership fold and the role gating of the real core enforce:
//
// - all Owners are equal (any Owner may change or remove any member, the creator included) and at least one Owner
//   remains: the last Owner can neither leave nor be demoted (`InvalidInput` for `role`);
// - roles gate writes (`MemorySharing.require`, called by `MemoryTransport` before each writing request and
//   page document import): `Forbidden` with `details` `{ reason: "role", required }` below the needed role, and
//   `{ reason: "archived" }` for any write to an archive (a closed space, or one this account was removed from);
// - an invite grants Editor, Commenter or Viewer, never Owner (`InvalidInput` for `role`); it is single use, it
//   expires (the clock of the mirror decides), it can be revoked, at most 20 are open at once, and the open invites
//   of an Owner who is demoted, removed or leaves are void;
// - a removed account can be invited again (it is not a member, a new `Join` is valid); removing a member, like a
//   Rekey, raises the epoch;
// - the space's settings, members, invites, disputes and profile names are Owner-only to change, and Owners alone
//   see the invites and disputes in `MembersInfo`.
//
// The simulated people act through `MemorySharing.receive` (`SharingChange`): they join by a link this account
// created, change roles, leave, remove this account, close the space, add a device with their kit, have changes
// refused, publish an incomplete snapshot. Seeds (`MemorySharingOptions`) start spaces shared, with members of each
// role, an archive, invites and disputes, or a session under way.

import type { MemoryNotificationSeed, MemoryThreadSeed } from "./comments";
import { Steps } from "./devices-mirror";
import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { ArchiveReason } from "./generated/ArchiveReason";
import type { DisputeInfo } from "./generated/DisputeInfo";
import type { Event } from "./generated/Event";
import type { FeedId } from "./generated/FeedId";
import type { FeedSyncInfo } from "./generated/FeedSyncInfo";
import type { InviteFlow } from "./generated/InviteFlow";
import type { InviteId } from "./generated/InviteId";
import type { InviteInfo } from "./generated/InviteInfo";
import type { InviteKind } from "./generated/InviteKind";
import type { InviteLinkInfo } from "./generated/InviteLinkInfo";
import type { InviteSessionInfo } from "./generated/InviteSessionInfo";
import type { InviteSessionState } from "./generated/InviteSessionState";
import type { InviteSide } from "./generated/InviteSide";
import type { InviteStatus } from "./generated/InviteStatus";
import type { MemberInfo } from "./generated/MemberInfo";
import type { MemberRole } from "./generated/MemberRole";
import type { FormerMemberInfo } from "./generated/FormerMemberInfo";
import type { MembersInfo } from "./generated/MembersInfo";
import type { PageId } from "./generated/PageId";
import type { PersonInfo } from "./generated/PersonInfo";
import type { ProfileInfo } from "./generated/ProfileInfo";
import type { SpaceId } from "./generated/SpaceId";
import type { SpaceInfo } from "./generated/SpaceInfo";
import type { SpaceSettings } from "./generated/SpaceSettings";
import type { SpaceSharePlan } from "./generated/SpaceSharePlan";
import type { SpaceShareState } from "./generated/SpaceShareState";
import { DEFAULT_TRASH_RETENTION_DAYS } from "./trash";
import { displayName, formatPairingCode, invalid, pairingCode, serverUrl as validServerUrl } from "./validate";

// ----- constants (`kb_types::sharing`) -----------------------------------------------------------------------

/** Most members of one shared space (`MAX_MEMBERS_PER_FEED`). */
export const MAX_MEMBERS = 50;
/** Most open invites of one space at a time (`MAX_OPEN_INVITES`). */
export const MAX_OPEN_INVITES = 20;
/** Longest life of an invite, in days (`INVITE_MAX_DAYS`). */
export const INVITE_MAX_DAYS = 30;
/** Default life of a link invite, in days (`INVITE_LINK_DEFAULT_DAYS`). */
export const INVITE_LINK_DEFAULT_DAYS = 7;
/** Life of the `Invite` a code flow writes after the digits match (`INVITE_CODE_TTL_SECONDS`), in milliseconds. */
export const INVITE_CODE_TTL_MS = 3600 * 1000;
/** Longest profile name, in characters (`PROFILE_NAME_MAX_CHARS`). */
export const PROFILE_NAME_MAX_CHARS = 100;
/** Longest trash retention an Owner may set, in days. */
export const MAX_TRASH_RETENTION_DAYS = 3650;
/** How long a code's mailbox lives (`PAIRING_TTL_SECONDS`), in milliseconds. */
const MAILBOX_TTL_MS = 600 * 1000;
const DAY_MS = 86_400_000;

/** Default length of one step of the simulated invite sessions and of the share (`MemoryTransportOptions.pairingStepMs`). */
export const DEFAULT_SHARING_STEP_MS = 150;
/** The code an inviter mirror shows (`InviteSessionInfo.code` while `Waiting`). */
export const MEMORY_INVITE_CODE = "7KQM X2TV R9HD";
/** The six digits both sides of a mirrored code invite show. */
export const MEMORY_INVITE_SAS = "735019";
/** The name of the imaginary person who types the code of an inviter mirror. */
export const MEMORY_INVITEE_NAME = "Dana";
/** A code the simulated server no longer knows: joining with it ends `Failed`, "this invite is invalid, ...". */
export const EXPIRED_INVITE_CODE = "DEAD DEAD DEAD";
/** The secret of a link the simulated server does not know (opening it ends `Failed`). */
export const DEAD_LINK_SECRET = "0".repeat(52);
/** What every dead invite says (one message for all of them: nobody can probe the state of an invite). */
export const INVITE_DEAD_MESSAGE = "this invite is invalid, expired or already used";
/** The account id of this account while no recovery kit has given it a real one. */
const FALLBACK_ACCOUNT: AccountId = "00000000-0000-4000-8000-a0000000000f";

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const ROLE_RANK: Readonly<Record<MemberRole, number>> = { viewer: 1, commenter: 2, editor: 3, owner: 4 };

// ----- options and hooks ---------------------------------------------------------------------------------------

/** A member of a seeded shared space (the people besides this account). */
export interface MemorySeedMember {
  name: string;
  role: MemberRole;
  /** The safety numbers were compared (`verifiedPeople`). */
  verified?: boolean;
  /** Active devices in the member's log (default 1). */
  deviceCount?: number;
  /** The member's account added a device with its kit at this time (a notice shows it). */
  viaKitAt?: number;
  /** Changes of this member that this device refused. */
  refusedChanges?: number;
}

/** An invite of a seeded shared space. */
export interface MemorySeedInvite {
  kind?: InviteKind;
  role: MemberRole;
  /** The member (by name) who made it; default this account. */
  createdBy?: string;
  /** Default `Open`. */
  status?: InviteStatus;
}

/** A space of the seeds that starts shared (or as an archive). */
export interface MemorySharedSpaceSeed {
  /** The name of a seeded space. */
  space: string;
  /** This account's role (default `owner`). */
  role?: MemberRole;
  /**
   * The other members, in join order. Default: Bob (Editor, verified), Carol (Commenter) and Dan (Viewer), after
   * Alice (Owner) when this account is not an Owner.
   */
  members?: readonly MemorySeedMember[];
  /** The space is an archive: this account was removed from it, or an Owner closed it. */
  archive?: ArchiveReason;
  settings?: Partial<SpaceSettings>;
  /** This account turned AI on for the space (default `false`). */
  aiEnabled?: boolean;
  invites?: readonly MemorySeedInvite[];
  /** Open snapshot disputes: the member (by name) whose device published an incomplete snapshot, and a page. */
  disputes?: readonly { member: string; pageId?: PageId }[];
  /** An item of a non-member was quarantined. */
  serverFlagged?: boolean;
  /**
   * Comment threads of the space's pages (Milestone 13, slice 2; seeded without events), written by this account and the space's
   * members, see `MemoryThreadSeed`.
   */
  comments?: readonly MemoryThreadSeed[];
  /** Rows of the inbox of this device (only for the other people of the space; `MemoryNotificationSeed`). */
  notifications?: readonly MemoryNotificationSeed[];
}

/** What the person on the other side of the invitee flows offers (`MemorySharingOptions.invitation`). */
export interface MemoryInvitation {
  spaceName: string;
  inviterName: string;
  role: MemberRole;
}

/** A session the dev page starts with (`?sharing=link-preview|code-inviter|code-invitee`). */
export type MemorySharingStart = "link-preview" | "code-inviter" | "code-invitee";

/** `MemoryTransportOptions.sharing`: the starting situation of shared spaces. */
export interface MemorySharingOptions {
  /** Spaces (by name) that start shared; they must exist among the seeded spaces. */
  spaces?: readonly MemorySharedSpaceSeed[];
  /** This account's profile name (the name it shows to co-members). */
  profileName?: string;
  /** What a code or a link the mirror's imaginary person gives offers (default: Alice invites to "Project X" as Editor). */
  invitation?: Partial<MemoryInvitation>;
  /** Starts a session: an inviter (the first shared space that this account owns), a code invitee or a link preview. */
  start?: MemorySharingStart;
  /** Length of one step of the simulated sessions and of the share, in milliseconds (default 150). */
  stepMs?: number;
}

/** What another person does, as it arrives through sync (`MemoryTransport.receiveSharing`). */
export type SharingChange =
  /** A person joined (the other side of a code invite finished). Answers the account. */
  | { type: "memberJoined"; spaceId: SpaceId; name: string; role?: MemberRole; verified?: boolean }
  /** A person opened a link this account created (`InviteLinkCreate`) and joined: the invite rules apply. Answers the account. */
  | { type: "joinByLink"; link: string; name: string }
  /** A member left. */
  | { type: "memberLeft"; spaceId: SpaceId; accountId: AccountId }
  /** Another Owner changed a role (this account's too: `SpaceRoleChanged`). */
  | { type: "roleSet"; spaceId: SpaceId; accountId: AccountId; role: MemberRole }
  /** An Owner removed a member; this account when `accountId` is left out (the space becomes an archive). */
  | { type: "removed"; spaceId: SpaceId; accountId?: AccountId; unsentPageIds?: readonly PageId[] }
  /** An Owner closed the space. */
  | { type: "closed"; spaceId: SpaceId }
  /** A member's account added a device with its recovery kit (`MemberDeviceViaKit`). */
  | { type: "viaKit"; spaceId: SpaceId; accountId: AccountId }
  /** This device refused changes of a member (`refusedChanges`). */
  | { type: "refused"; spaceId: SpaceId; accountId: AccountId; count?: number }
  /** A member's device published an incomplete snapshot (`SnapshotDisputed`); `pageId` is the page of a page document or of a comment stream. */
  | { type: "disputed"; spaceId: SpaceId; accountId: AccountId; pageId?: PageId }
  /** An item of a non-member was quarantined: the server misbehaved. */
  | { type: "serverFlagged"; spaceId: SpaceId }
  /** A member changed their profile name (`PeopleChanged`). */
  | { type: "renamed"; spaceId: SpaceId; accountId: AccountId; name: string };

/** What the sharing mirror needs from the transport. */
export interface SharingDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text). */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Whether the core is ready; the simulated sessions stop when it is not. */
  ready: () => boolean;
  /** Length of one step, in milliseconds. */
  stepMs: number;
  /** Whether this installation has a workspace (joining needs one). */
  hasWorkspace: () => boolean;
  /** Whether the space exists. */
  hasSpace: (spaceId: SpaceId) => boolean;
  /** The id of the first space with this name. */
  findSpace: (name: string) => SpaceId | undefined;
  /** The space as listed (decorated by this mirror). */
  spaceInfo: (spaceId: SpaceId) => SpaceInfo;
  /** The configured sync server. */
  syncServer: () => string | undefined;
  /** This account's id; `undefined` before the recovery kit. */
  accountId: () => AccountId | undefined;
  /** What the share dialog counts. */
  counts: (spaceId: SpaceId) => { pages: number; trashed: number; attachments: number; attachmentBytes: number };
  /** The local copy of the space goes (leaving, removing an archive from this device). */
  dropSpace: (spaceId: SpaceId) => void;
  /** The space an invitee receives with its history; answers its id. */
  joinedSpace: (name: string) => SpaceId;
  /** Fork-out: copies the unsent changes of pages into "Unsent changes - <title>" pages of a personal space. */
  copyUnsent: (pageIds: readonly PageId[]) => { targetSpaceId: SpaceId; pageIds: PageId[] };
  /** AI went off for the space: the MCP grants and the index rows of it go. */
  aiOff: (spaceId: SpaceId) => void;
}

// ----- state ---------------------------------------------------------------------------------------------------------

interface MemberRecord {
  accountId: AccountId;
  name: string;
  role: MemberRole;
  joinedAt: number;
  isMe: boolean;
  deviceCount: number;
  viaKitAt?: number;
  refused: number;
}

interface InviteRecord {
  inviteId: InviteId;
  kind: InviteKind;
  role: MemberRole;
  createdBy: AccountId;
  createdAt: number;
  expiresAt: number;
  boundAccount?: AccountId;
  usedBy?: AccountId;
  revoked: boolean;
  /** The secret of a link this account created (never listed). */
  secret?: string;
}

interface SharedSpace {
  spaceId: SpaceId;
  feedId: FeedId;
  state: SpaceShareState;
  archiveReason?: ArchiveReason;
  epoch: number;
  pendingRekey: boolean;
  closedAt?: number;
  members: MemberRecord[];
  /** People who were members and are not now, with the last name the space knew them by (`MembersInfo.formerMembers`). */
  former: FormerMemberInfo[];
  invites: InviteRecord[];
  settings: SpaceSettings;
  disputes: DisputeInfo[];
  refusedTotal: number;
  serverFlagged: boolean;
  /** This account turned AI on for the space. */
  ai: boolean;
}

/** What a link or a code offers the invitee. */
interface Offer {
  spaceName: string;
  inviterName: string;
  role: MemberRole;
  /** The invite is no good: opening it ends `Failed` with this message. */
  dead?: string;
}

interface Session {
  flow: InviteFlow;
  side: InviteSide;
  state: InviteSessionState;
  code?: string;
  sas?: string;
  peerName?: string;
  peerSafetyNumber?: string;
  spaceName?: string;
  inviterName?: string;
  role?: MemberRole;
  verified: boolean;
  serverUrl?: string;
  error?: string;
  expiresAt?: number;
  /** The space an inviter invites to. */
  spaceId?: SpaceId;
  /** What the invitee is offered. */
  offer?: Offer;
  /** The secret of the link being opened. */
  secret?: string;
}

const ACTIVE: ReadonlySet<InviteSessionState> = new Set([
  "Starting",
  "Waiting",
  "Confirming",
  "PeerConfirming",
  "Transferring",
  "Previewing",
  "Joining",
]);

// ----- helpers -------------------------------------------------------------------------------------------------------

export function forbiddenRole(required: MemberRole): CoreError {
  return new CoreError({
    code: "Forbidden",
    message: `this needs the ${required} role in the space`,
    details: { reason: "role", required },
  });
}

export function forbiddenArchived(): CoreError {
  return new CoreError({
    code: "Forbidden",
    message: "this space is a read-only archive",
    details: { reason: "archived" },
  });
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

function atLeast(role: MemberRole, needed: MemberRole): boolean {
  return ROLE_RANK[role] >= ROLE_RANK[needed];
}

/** Thirty digits in six groups of five, the same for both people of a pair (a stand-in for `kb_keys::safety`). */
function safetyNumber(a: AccountId, b: AccountId): string {
  const [low, high] = a < b ? [a, b] : [b, a];
  let hash = 0x811c9dc5;
  for (const char of `${low}|${high}`) {
    hash = Math.imul(hash ^ (char.codePointAt(0) ?? 0), 0x01000193) >>> 0;
  }
  let digits = "";
  let state = hash;
  while (digits.length < 30) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    digits += String((state >>> 16) % 10);
  }
  return [0, 5, 10, 15, 20, 25].map((at) => digits.slice(at, at + 5)).join(" ");
}

function randomSecret(): string {
  const bytes = new Uint8Array(52);
  globalThis.crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => CROCKFORD.charAt(byte % 32)).join("");
}

/** A link's secret as typed: upper case, `O` read as `0` and `I`/`L` as `1`; `undefined` unless 52 Crockford characters. */
function canonicalSecret(raw: string): string | undefined {
  let secret = "";
  for (const char of raw) {
    const upper = /^[a-z]$/u.test(char) ? char.toUpperCase() : char;
    const mapped = upper === "O" ? "0" : upper === "I" || upper === "L" ? "1" : upper;
    if (mapped.length !== 1 || !CROCKFORD.includes(mapped)) {
      return undefined;
    }
    secret += mapped;
  }
  return secret.length === 52 ? secret : undefined;
}

const APP_LINK = /^atomik-kb:\/\/invite\?server=([^#\s&]+)#([^\s#]+)$/iu;
const WEB_LINK = /^(https?:\/\/[^\s/#?]+(?:\/[^\s#?]*)?)\/join#([^\s#]+)$/iu;

/** The server and the secret of an invite link (`atomik-kb://invite?server=<url>#<secret>` or `<server>/join#<secret>`). */
function parseInviteLink(raw: string): { server: string; secret: string } {
  const text = raw.trim();
  const app = APP_LINK.exec(text);
  const web = app === null ? WEB_LINK.exec(text) : null;
  const server = app?.[1] ?? web?.[1];
  const secret = canonicalSecret(app?.[2] ?? web?.[2] ?? "");
  if (server === undefined || secret === undefined) {
    throw invalid("link", "this is not an invite link");
  }
  try {
    return { server: validServerUrl(app === null ? server : decodeURIComponent(server)), secret };
  } catch {
    throw invalid("link", "this is not an invite link");
  }
}

function defaultInvitation(partial: Partial<MemoryInvitation> | undefined): MemoryInvitation {
  return { spaceName: "Project X", inviterName: "Alice", role: "editor", ...partial };
}

/** The sharing part of `MemoryTransport`: see the file's comment. */
export class MemorySharing {
  readonly #deps: SharingDeps;
  readonly #steps: Steps;
  readonly #spaces = new Map<SpaceId, SharedSpace>();
  /** Personal spaces this account turned AI off for (a shared one keeps its flag in the record). */
  readonly #personalAiOff = new Set<SpaceId>();
  /** The simulated people by name: the same name is the same account in every space. */
  readonly #people = new Map<string, AccountId>();
  /** `verifiedPeople`: account to the time the safety numbers were compared. */
  readonly #verified = new Map<AccountId, number>();
  readonly #used = new Set<string>();
  readonly #invitation: MemoryInvitation;
  #meId: AccountId | undefined;
  #nextPerson = 0;
  #nextSeedId = 0;
  #profileName: string | undefined;
  /** The timers of the shares waiting for the server to accept their Create. */
  readonly #shares = new Set<ReturnType<typeof setTimeout>>();
  #session: Session | undefined;
  #seeding = false;

  constructor(deps: SharingDeps, options: MemorySharingOptions = {}) {
    this.#deps = deps;
    this.#steps = new Steps(deps);
    this.#invitation = defaultInvitation(options.invitation);
    // Known before the seeds run, so that a seeded Person value can name this account by its profile name.
    if (options.profileName !== undefined) {
      this.#profileName = displayName(options.profileName, PROFILE_NAME_MAX_CHARS, "displayName");
    }
  }

  // ----- seeds ----------------------------------------------------------------------------------------------------

  /**
   * Starts the shared spaces, the profile name and the session of the options, without events. Draws no id from the
   * transport's generator (the seeded spaces and pages must keep theirs).
   */
  seed(options: MemorySharingOptions): void {
    this.#seeding = true;
    try {
      if (options.profileName !== undefined) {
        this.#profileName = displayName(options.profileName, PROFILE_NAME_MAX_CHARS, "displayName");
      }
      for (const seed of options.spaces ?? []) {
        const spaceId = this.#deps.findSpace(seed.space);
        if (spaceId === undefined) {
          throw new CoreError({ code: "Internal", message: "MemoryTransport: the shared space is not a seeded space" });
        }
        this.#seedSpace(spaceId, seed);
      }
      if (options.start !== undefined) {
        this.#seedSession(options.start);
      }
    } finally {
      this.#seeding = false;
    }
  }

  #seedSpace(spaceId: SpaceId, seed: MemorySharedSpaceSeed): void {
    const now = this.#deps.now();
    const role = seed.role ?? "owner";
    const others: MemorySeedMember[] = [...(seed.members ?? this.#defaultMembers(role))];
    const space = this.#newSpace(spaceId, "Shared");
    // Join order: the creator first (this account when it is an Owner, else the first Owner listed).
    const mine = this.#member(this.#me(), this.#profileName ?? "You", role, now - 7 * DAY_MS, true);
    mine.deviceCount = 1;
    const first = others.findIndex((member) => member.role === "owner");
    const records = others.map((member, index) => {
      const record = this.#member(this.#peer(member.name), member.name, member.role, now - (6 - Math.min(index, 5)) * DAY_MS, false);
      record.deviceCount = member.deviceCount ?? 1;
      record.refused = member.refusedChanges ?? 0;
      if (member.viaKitAt !== undefined) {
        record.viaKitAt = member.viaKitAt;
      }
      if (member.verified === true) {
        this.#verified.set(record.accountId, now - DAY_MS);
      }
      return record;
    });
    if (role === "owner" || first < 0) {
      space.members = [mine, ...records];
    } else {
      space.members = [...records.slice(0, first + 1), mine, ...records.slice(first + 1)];
    }
    if (!space.members.some((member) => member.role === "owner")) {
      throw new CoreError({ code: "Internal", message: "MemoryTransport: a shared space needs an Owner" });
    }
    space.refusedTotal = space.members.reduce((total, member) => total + member.refused, 0);
    space.settings = { ...space.settings, ...seed.settings };
    space.ai = seed.aiEnabled === true;
    space.serverFlagged = seed.serverFlagged === true;
    for (const invite of seed.invites ?? []) {
      space.invites.push(this.#seedInvite(space, invite, now));
    }
    for (const dispute of seed.disputes ?? []) {
      const member = space.members.find((candidate) => candidate.name === dispute.member);
      if (member !== undefined) {
        space.disputes.push({
          ...(dispute.pageId === undefined ? {} : { pageId: dispute.pageId }),
          accountId: member.accountId,
          at: now - 3600_000,
        });
      }
    }
    if (seed.archive !== undefined) {
      space.state = "Archived";
      space.archiveReason = seed.archive;
      if (seed.archive === "Closed") {
        space.closedAt = now - DAY_MS;
      }
    }
    this.#spaces.set(spaceId, space);
  }

  #defaultMembers(role: MemberRole): MemorySeedMember[] {
    const people: MemorySeedMember[] = [
      { name: "Bob", role: "editor", verified: true },
      { name: "Carol", role: "commenter" },
      { name: "Dan", role: "viewer" },
    ];
    return role === "owner" ? people : [{ name: "Alice", role: "owner", verified: true }, ...people];
  }

  #seedInvite(space: SharedSpace, seed: MemorySeedInvite, now: number): InviteRecord {
    const status = seed.status ?? "Open";
    const record: InviteRecord = {
      inviteId: this.#mint(),
      kind: seed.kind ?? "Link",
      role: seed.role,
      createdBy: this.#seededCreator(space, seed.createdBy),
      createdAt: now - 3600_000,
      expiresAt: status === "Expired" ? now - 60_000 : now + INVITE_LINK_DEFAULT_DAYS * DAY_MS,
      revoked: status === "Revoked",
    };
    if (status === "Used") {
      record.usedBy = space.members.find((member) => !member.isMe)?.accountId ?? this.#me();
    }
    return record;
  }

  #seededCreator(space: SharedSpace, name: string | undefined): AccountId {
    const creator = name === undefined ? undefined : space.members.find((member) => member.name === name);
    return creator?.accountId ?? this.#me();
  }

  #seedSession(start: MemorySharingStart): void {
    if (start === "code-invitee") {
      this.codeJoin("https://sync.example.com", MEMORY_INVITE_CODE);
      return;
    }
    if (start === "link-preview") {
      this.linkOpen(`atomik-kb://invite?server=https%3A%2F%2Fsync.example.com#${randomSecret()}`);
      return;
    }
    const owned = [...this.#spaces.values()].find((space) => this.#roleOf(space) === "owner" && space.state === "Shared");
    if (owned === undefined) {
      throw new CoreError({ code: "Internal", message: "MemoryTransport: a code inviter needs a shared space this account owns" });
    }
    this.codeStart(owned.spaceId, "editor");
  }

  // ----- listing ----------------------------------------------------------------------------------------------------

  /** Adds what sharing says to a listed space; a personal space comes back as it is (or `aiEnabled: false`). */
  decorate(info: SpaceInfo): SpaceInfo {
    const space = this.#spaces.get(info.id);
    if (space === undefined) {
      return this.#personalAiOff.has(info.id) ? { ...info, aiEnabled: false } : info;
    }
    const decorated: SpaceInfo = {
      ...info,
      shareState: space.state,
      role: this.#roleOf(space),
      memberCount: space.members.length,
    };
    if (space.state === "Archived" && space.archiveReason !== undefined) {
      decorated.archiveReason = space.archiveReason;
    }
    if (!space.ai) {
      decorated.aiEnabled = false;
    }
    return decorated;
  }

  /** Whether local AI and MCP may use the space on this account's devices (ADR-0047). */
  aiEnabled(spaceId: SpaceId): boolean {
    const space = this.#spaces.get(spaceId);
    return space === undefined ? !this.#personalAiOff.has(spaceId) : space.ai;
  }

  /** `SyncInfo.feeds`: the personal feed first, then one per shared space. */
  feeds(pendingEntries: number): FeedSyncInfo[] {
    const personal: FeedSyncInfo = {
      feedId: this.#me(),
      pendingEntries,
      refused: 0,
      forked: false,
      closed: false,
      serverFlagged: false,
    };
    const shared = [...this.#spaces.values()].map(
      (space): FeedSyncInfo => ({
        feedId: space.feedId,
        spaceId: space.spaceId,
        pendingEntries: 0,
        refused: space.refusedTotal,
        forked: false,
        closed: space.closedAt !== undefined,
        serverFlagged: space.serverFlagged,
      }),
    );
    return [personal, ...shared];
  }

  /**
   * How the trash of a space expires: `undefined` for a personal space (the device setting decides), `"skip"` where
   * this device must not purge (not an Owner device, or an archive), else the space's retention in milliseconds
   * (`null` keeps for ever). ADR-0047, rule 2.
   */
  trashPolicy(spaceId: SpaceId): "skip" | number | null | undefined {
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      return undefined;
    }
    if (space.state === "Archived" || this.#roleOf(space) !== "owner") {
      return "skip";
    }
    return space.settings.trashRetentionDays <= 0 ? null : space.settings.trashRetentionDays * DAY_MS;
  }

  // ----- the gate ---------------------------------------------------------------------------------------------------

  /**
   * The gate of every writing request: `Forbidden` when this account's role in the space is below `needed`, or the
   * space is an archive. A personal space and an unknown space pass (the request itself then answers).
   */
  require(spaceId: SpaceId | undefined, needed: MemberRole): void {
    const space = spaceId === undefined ? undefined : this.#spaces.get(spaceId);
    if (space !== undefined) {
      this.#allow(space, needed);
    }
  }

  /** Whether the space is shared (or an archive): `SpaceDelete` is refused there. */
  isShared(spaceId: SpaceId): boolean {
    return this.#spaces.has(spaceId);
  }

  /**
   * Whether the space is shared and live: the server accepted it and it is not a read-only copy. Presence (Milestone 13, slice 3)
   * is sent and received only there.
   */
  isLive(spaceId: SpaceId): boolean {
    return this.#spaces.get(spaceId)?.state === "Shared";
  }

  /** The space of the sharing mirror goes with its local copy (a hook of the transport's own deletes). */
  spaceDeleted(spaceId: SpaceId): void {
    this.#spaces.delete(spaceId);
    this.#personalAiOff.delete(spaceId);
  }

  // ----- people and roles, for comments, mentions and Person values (slice 2) --------------------------------------

  /** This account's id (the recovery kit's, else a stand-in). */
  meId(): AccountId {
    return this.#me();
  }

  /** The account of a simulated person by name (the same name is the same account everywhere), made on first use. */
  peer(name: string): AccountId {
    return this.#peer(name);
  }

  /**
   * What this account may do in a space: its role (`null` in a personal space, where it is everything) and whether the
   * space is a read-only archive (a closed one, or one this account was removed from).
   */
  access(spaceId: SpaceId): { role: MemberRole | null; archived: boolean } {
    const space = this.#spaces.get(spaceId);
    return space === undefined ? { role: null, archived: false } : { role: this.#roleOf(space), archived: space.state === "Archived" };
  }

  /**
   * The space's "Editors may create view links" setting (`false` for a personal space, which has no such setting: this
   * account may link there whatever it says). The view links of Milestone 13, slice 4, read it.
   */
  editorsMayLink(spaceId: SpaceId): boolean {
    return this.#spaces.get(spaceId)?.settings.editorsMayLink ?? false;
  }

  /** Whether an Owner closed the space: its feed is deleted on the server, and with it the space's view links. */
  isClosed(spaceId: SpaceId): boolean {
    return this.#spaces.get(spaceId)?.archiveReason === "Closed";
  }

  /**
   * The name a person goes by in a space: a member's, else the last name the space knew a former member by (a view link
   * outlives its creator's membership), `undefined` for anyone else.
   */
  knownNameOf(spaceId: SpaceId, accountId: AccountId): string | undefined {
    return this.nameOf(spaceId, accountId) ?? this.#spaces.get(spaceId)?.former.find((former) => former.accountId === accountId)?.name;
  }

  /**
   * Whether this device fires a reminder of a date mention in a space (ADR-0047). A personal space's always fire here. A shared
   * space's fire while the space is live (not an archive) and this account is a member: on the devices of `reminderFor` when the
   * mention names an account (every role, a Viewer included: it is theirs), and on an Owner's device when it names nobody.
   */
  firesReminder(spaceId: SpaceId, reminderFor: AccountId | undefined): boolean {
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      return true;
    }
    if (space.state === "Archived") {
      return false;
    }
    const me = this.#meOf(space);
    return reminderFor === undefined ? me.role === "owner" : reminderFor === me.accountId;
  }

  /**
   * The people of a space as the `@` menu and a Person property offer them (`PeopleMentionable`): its members in join
   * order, this account among them; in a personal space this account alone.
   */
  peopleOf(spaceId: SpaceId): PersonInfo[] {
    this.#known(spaceId);
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      return [this.#person(this.#me())];
    }
    const me = this.#meOf(space);
    return space.members.map(
      (member): PersonInfo => ({
        accountId: member.accountId,
        name: member.name,
        verified: this.#verified.has(member.accountId),
        ...(member.isMe ? {} : { safetyNumber: safetyNumber(me.accountId, member.accountId) }),
        isMe: member.isMe,
      }),
    );
  }

  /** A current member of the space (this account in a personal space), `undefined` for a former member or a stranger. */
  memberOf(spaceId: SpaceId, accountId: AccountId): { accountId: AccountId; name: string; role: MemberRole; isMe: boolean } | undefined {
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      return accountId === this.#me()
        ? { accountId, name: this.#profileName ?? "You", role: "owner", isMe: true }
        : undefined;
    }
    const member = space.members.find((candidate) => candidate.accountId === accountId);
    return member === undefined ? undefined : { accountId, name: member.name, role: member.role, isMe: member.isMe };
  }

  /** A current member by name; "me" (any case) and this account's profile name stand for this account. */
  memberByName(spaceId: SpaceId, name: string): { accountId: AccountId; name: string; role: MemberRole; isMe: boolean } | undefined {
    const wanted = name.trim();
    const mine = this.memberOf(spaceId, this.#me());
    if (mine !== undefined && (wanted.toLowerCase() === "me" || wanted === mine.name)) {
      return mine;
    }
    const space = this.#spaces.get(spaceId);
    const member = space?.members.find((candidate) => candidate.name === wanted);
    return member === undefined ? undefined : this.memberOf(spaceId, member.accountId);
  }

  /** The account a seeded name stands for in a space: a member's, this account's ("me", the profile name), else a made-up person's. */
  accountByName(spaceId: SpaceId | undefined, name: string): AccountId {
    const member = spaceId === undefined ? undefined : this.memberByName(spaceId, name);
    if (member !== undefined) {
      return member.accountId;
    }
    const wanted = name.trim();
    return wanted.toLowerCase() === "me" || wanted === this.#profileName ? this.#me() : this.#peer(wanted);
  }

  /** The name an account goes by in a space now (this account's profile name too), `undefined` for a former member. */
  nameOf(spaceId: SpaceId, accountId: AccountId): string | undefined {
    return this.memberOf(spaceId, accountId)?.name;
  }

  /** The name this account shows to co-members. */
  myName(): string {
    return this.#profileName ?? "You";
  }

  // ----- the requests -----------------------------------------------------------------------------------------------

  /** `SpaceSharePlan`: counts only, nothing is written. */
  plan(spaceId: SpaceId): SpaceSharePlan {
    this.#known(spaceId);
    const counts = this.#deps.counts(spaceId);
    return {
      spaceId,
      pageCount: counts.pages,
      trashedPageCount: counts.trashed,
      attachmentCount: counts.attachments,
      attachmentBytes: counts.attachmentBytes,
      historyBytes: 1024 + 4096 * (counts.pages + counts.trashed) + counts.attachments * 256,
      serverConfigured: this.#deps.syncServer() !== undefined,
      hasAccount: this.#deps.accountId() !== undefined,
      hasProfileName: this.#profileName !== undefined,
      alreadyShared: this.#spaces.has(spaceId),
    };
  }

  /** `SpaceShare`: this account becomes the Owner of the space, which is `Pending` for a step, then `Shared`. */
  share(spaceId: SpaceId): SpaceInfo {
    this.#known(spaceId);
    if (this.#spaces.has(spaceId)) {
      throw new CoreError({ code: "AlreadyExists", message: "the space is already shared" });
    }
    if (this.#deps.syncServer() === undefined) {
      throw invalid("serverUrl", "turn sync on before sharing a space");
    }
    if (this.#deps.accountId() === undefined) {
      throw invalid("account", "create your recovery kit before sharing a space");
    }
    if (this.#profileName === undefined) {
      throw invalid("displayName", "set the name your co-members see before sharing a space");
    }
    const space = this.#newSpace(spaceId, "Pending");
    space.members = [this.#member(this.#me(), this.#profileName, "owner", this.#deps.now(), true)];
    this.#spaces.set(spaceId, space);
    this.#deps.emit({ type: "SpacesChanged" });
    // The server accepts the feed's Create after a step; a lock finishes it at once (the driver would meanwhile).
    const timer = setTimeout(() => {
      this.#shares.delete(timer);
      if (this.#deps.ready()) {
        this.#finishShare(space);
      }
    }, this.#deps.stepMs);
    this.#shares.add(timer);
    return this.#deps.spaceInfo(spaceId);
  }

  /** `SpaceMembers`. */
  members(spaceId: SpaceId): MembersInfo {
    return this.#membersInfo(this.#shared(spaceId));
  }

  /** `MemberRoleSet`: an Owner changes a role; the last Owner cannot be demoted. */
  roleSet(spaceId: SpaceId, accountId: AccountId, role: MemberRole): MembersInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    this.#setRole(space, this.#target(space, accountId), role);
    return this.#membersInfo(space);
  }

  /** `MemberRemove`: an Owner removes another member; the removal carries its Rekey (the epoch rises). */
  remove(spaceId: SpaceId, accountId: AccountId): MembersInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    const target = this.#target(space, accountId);
    if (target.isMe) {
      throw invalid("accountId", "you cannot remove yourself: leave the space instead");
    }
    this.#removeMember(space, target);
    return this.#membersInfo(space);
  }

  /** `SpaceLeave`: a member leaves and the local copy goes; on an archive only the local copy goes. */
  leave(spaceId: SpaceId): void {
    const space = this.#shared(spaceId);
    if (space.state !== "Archived") {
      const me = this.#meOf(space);
      if (me.role === "owner" && this.#ownerCount(space) === 1) {
        throw invalid("role", "you are the last Owner: promote someone first, or close the space");
      }
    }
    this.#spaces.delete(spaceId);
    this.#deps.dropSpace(spaceId);
  }

  /** `SpaceClose`: an Owner closes the space; it becomes an archive for everyone. */
  close(spaceId: SpaceId): SpaceInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    space.state = "Archived";
    space.archiveReason = "Closed";
    space.closedAt = this.#deps.now();
    this.#deps.emit({ type: "SpacesChanged" });
    this.#membersChanged(space);
    return this.#deps.spaceInfo(spaceId);
  }

  /** `SpaceSettingsSet`: an Owner changes the space's settings; a field left out stays. */
  settingsSet(spaceId: SpaceId, trashRetentionDays: number | undefined, editorsMayLink: boolean | undefined): MembersInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    if (
      trashRetentionDays !== undefined &&
      (!Number.isInteger(trashRetentionDays) || trashRetentionDays < 0 || trashRetentionDays > MAX_TRASH_RETENTION_DAYS)
    ) {
      throw invalid("trashRetentionDays", `must be 0 (keep) or a number of days up to ${String(MAX_TRASH_RETENTION_DAYS)}`);
    }
    space.settings = {
      trashRetentionDays: trashRetentionDays ?? space.settings.trashRetentionDays,
      editorsMayLink: editorsMayLink ?? space.settings.editorsMayLink,
    };
    this.#membersChanged(space);
    return this.#membersInfo(space);
  }

  /** `SpaceAiSet`: this account's own opt-in (any member, an archive too); turning it off drops the grants and index rows. */
  aiSet(spaceId: SpaceId, enabled: boolean): SpaceInfo {
    this.#known(spaceId);
    const space = this.#spaces.get(spaceId);
    const before = this.aiEnabled(spaceId);
    if (space === undefined) {
      if (enabled) {
        this.#personalAiOff.delete(spaceId);
      } else {
        this.#personalAiOff.add(spaceId);
      }
    } else {
      space.ai = enabled;
    }
    if (before !== enabled) {
      if (!enabled) {
        this.#deps.aiOff(spaceId);
      }
      this.#deps.emit({ type: "SpacesChanged" });
    }
    return this.#deps.spaceInfo(spaceId);
  }

  /** `ProfileGet`. */
  profileGet(): ProfileInfo {
    return this.#profileName === undefined ? {} : { displayName: this.#profileName };
  }

  /** `ProfileSet`: appends a `Profile` entry to every shared space (the member lists show the new name). */
  profileSet(rawName: string): ProfileInfo {
    const name = displayName(rawName, PROFILE_NAME_MAX_CHARS, "displayName");
    this.#profileName = name;
    for (const space of this.#spaces.values()) {
      const me = space.members.find((member) => member.isMe);
      if (me !== undefined) {
        me.name = name;
      }
    }
    this.#deps.emit({ type: "PeopleChanged" });
    for (const space of this.#spaces.values()) {
      this.#membersChanged(space);
    }
    return { displayName: name };
  }

  /** `PersonGet`. */
  personGet(accountId: AccountId): PersonInfo {
    return this.#person(accountId);
  }

  /** `PersonVerifiedSet`: "Mark verified" writes `verifiedPeople` (per person, for every space). */
  personVerifiedSet(accountId: AccountId, verified: boolean): PersonInfo {
    const person = this.#person(accountId);
    if (person.isMe) {
      throw invalid("accountId", "you cannot verify yourself");
    }
    if (verified) {
      this.#verified.set(accountId, this.#deps.now());
    } else {
      this.#verified.delete(accountId);
    }
    this.#deps.emit({ type: "PeopleChanged" });
    return this.#person(accountId);
  }

  /** `InviteLinkCreate`: an Owner makes a link that grants a role below Owner; returned once. */
  linkCreate(spaceId: SpaceId, role: MemberRole, expiresInDays: number | undefined): InviteLinkInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    this.#checkInviteRole(role);
    const days = expiresInDays ?? INVITE_LINK_DEFAULT_DAYS;
    if (!Number.isInteger(days) || days < 1 || days > INVITE_MAX_DAYS) {
      throw invalid("expiresInDays", `must be 1 to ${String(INVITE_MAX_DAYS)} days`);
    }
    const server = this.#deps.syncServer();
    if (server === undefined) {
      throw invalid("serverUrl", "turn sync on before inviting people");
    }
    this.#checkRoom(space);
    const now = this.#deps.now();
    const secret = randomSecret();
    const record: InviteRecord = {
      inviteId: this.#deps.newId(),
      kind: "Link",
      role,
      createdBy: this.#me(),
      createdAt: now,
      expiresAt: now + days * DAY_MS,
      revoked: false,
      secret,
    };
    space.invites.push(record);
    this.#membersChanged(space);
    return {
      inviteId: record.inviteId,
      link: `atomik-kb://invite?server=${encodeURIComponent(server)}#${secret}`,
      webLink: `${server.replace(/\/+$/u, "")}/join#${secret}`,
      role,
      expiresAt: record.expiresAt,
    };
  }

  /** `InviteRevoke`: an Owner revokes an open invite. */
  revoke(spaceId: SpaceId, inviteId: InviteId): MembersInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    const invite = space.invites.find((candidate) => candidate.inviteId === inviteId);
    if (invite === undefined) {
      throw invalid("inviteId", "this invite does not exist");
    }
    if (this.#statusOf(invite) !== "Open") {
      throw invalid("inviteId", "this invite is not open");
    }
    invite.revoked = true;
    this.#membersChanged(space);
    return this.#membersInfo(space);
  }

  // ----- the invite sessions --------------------------------------------------------------------------------------

  /** `InviteCodeStart`: the inviter side of the code flow (see the file's comment of `./sharing-mirror` for the steps). */
  codeStart(spaceId: SpaceId, role: MemberRole): InviteSessionInfo {
    const space = this.#shared(spaceId);
    this.#allow(space, "owner");
    this.#checkInviteRole(role);
    const server = this.#deps.syncServer();
    if (server === undefined) {
      throw invalid("serverUrl", "turn sync on before inviting people");
    }
    this.#checkRoom(space);
    const session: Session = { flow: "Code", side: "Inviter", state: "Starting", verified: false, serverUrl: server, spaceId, role };
    this.#begin(session);
    this.#next(session, () => {
      this.#change(session, {
        state: "Waiting",
        code: MEMORY_INVITE_CODE,
        expiresAt: this.#deps.now() + MAILBOX_TTL_MS,
      });
      // The imaginary invitee types the code.
      this.#next(session, () => {
        this.#change(session, { state: "Confirming", sas: MEMORY_INVITE_SAS });
        // It confirmed first and sent its identity: "You are inviting <name>".
        this.#next(session, () => {
          this.#change(session, {
            state: "PeerConfirming",
            peerName: MEMORY_INVITEE_NAME,
            peerSafetyNumber: safetyNumber(this.#me(), this.#peer(MEMORY_INVITEE_NAME)),
          });
        });
      });
    });
    return this.#info(session);
  }

  /** `InviteCodeJoin`: the invitee side of the code flow; the imaginary inviter offers `MemoryInvitation`. */
  codeJoin(rawServer: string, rawCode: string): InviteSessionInfo {
    const server = validServerUrl(rawServer);
    const code = pairingCode(rawCode);
    this.#needsAccount();
    const offer: Offer = { ...this.#invitation };
    if (code === pairingCode(EXPIRED_INVITE_CODE)) {
      offer.dead = INVITE_DEAD_MESSAGE;
    }
    const session: Session = { flow: "Code", side: "Invitee", state: "Starting", verified: false, serverUrl: server, offer };
    this.#begin(session);
    this.#next(session, () => {
      if (offer.dead !== undefined) {
        this.#fail(session, offer.dead);
        return;
      }
      this.#change(session, { state: "Confirming", sas: MEMORY_INVITE_SAS, expiresAt: this.#deps.now() + MAILBOX_TTL_MS });
    });
    return this.#info(session);
  }

  /** `InviteLinkOpen`: the invitee side of the link flow: the preview comes after one step. */
  linkOpen(link: string): InviteSessionInfo {
    const { server, secret } = parseInviteLink(link);
    this.#needsAccount();
    const offer = this.#offerFor(secret);
    const session: Session = { flow: "Link", side: "Invitee", state: "Starting", verified: false, serverUrl: server, secret, offer };
    this.#begin(session);
    this.#next(session, () => {
      if (offer.dead !== undefined) {
        this.#fail(session, offer.dead);
        return;
      }
      this.#change(session, {
        state: "Previewing",
        spaceName: offer.spaceName,
        inviterName: offer.inviterName,
        role: offer.role,
      });
    });
    return this.#info(session);
  }

  /** `InviteConfirm`: the digits match (code flow) or the preview is accepted (link flow). */
  confirm(): InviteSessionInfo {
    const session = this.#session;
    if (session === undefined || !ACTIVE.has(session.state)) {
      throw invalid("invite", "there is nothing to confirm");
    }
    if (session.flow === "Link" && session.state === "Previewing") {
      this.#join(session);
    } else if (session.flow === "Code" && session.side === "Invitee" && session.state === "Confirming") {
      this.#change(session, { state: "PeerConfirming", verified: false });
      this.#next(session, () => {
        const offer = session.offer ?? this.#invitation;
        this.#change(session, {
          state: "Transferring",
          spaceName: offer.spaceName,
          inviterName: offer.inviterName,
          peerName: offer.inviterName,
          peerSafetyNumber: safetyNumber(this.#me(), this.#peer(offer.inviterName)),
          role: offer.role,
        });
        this.#next(session, () => {
          this.#join(session);
        });
      });
    } else if (session.flow === "Code" && session.side === "Inviter" && session.state === "PeerConfirming") {
      this.#inviterConfirms(session);
    } else if (session.flow === "Code" && session.side === "Inviter" && session.state === "Confirming") {
      throw invalid("invite", "the other person has not confirmed yet");
    } else {
      throw invalid("invite", "there is nothing to confirm");
    }
    return this.#info(session);
  }

  /** `InviteCancel`: a session in progress ends `Cancelled`; otherwise nothing changes. */
  cancel(): InviteSessionInfo {
    const session = this.#session;
    if (session !== undefined && ACTIVE.has(session.state)) {
      this.#steps.cancel();
      this.#change(session, { state: "Cancelled" });
    }
    return this.status();
  }

  /** `InviteStatus`. Optional fields are left out when unset, as the core's serialisation does. */
  status(): InviteSessionInfo {
    return this.#session === undefined ? { flow: "Code", side: "Inviter", state: "Idle", verified: false } : this.#info(this.#session);
  }

  /** The core locked: the session in progress ends `Cancelled`, silently, and a share in progress finishes. */
  stop(): void {
    this.#steps.cancel();
    for (const timer of this.#shares) {
      clearTimeout(timer);
    }
    this.#shares.clear();
    if (this.#session !== undefined && ACTIVE.has(this.#session.state)) {
      this.#session.state = "Cancelled";
    }
    for (const space of this.#spaces.values()) {
      if (space.state === "Pending") {
        space.state = "Shared";
      }
    }
  }

  // ----- the other people -------------------------------------------------------------------------------------------

  /** Another person acts, as it arrives through sync. See {@link SharingChange}. Answers the account of a join. */
  receive(change: SharingChange): AccountId | undefined {
    switch (change.type) {
      case "memberJoined": {
        const space = this.#shared(change.spaceId);
        const member = this.#addMember(space, change.name, change.role ?? "editor");
        if (change.verified === true) {
          this.#verified.set(member.accountId, this.#deps.now());
        }
        this.#membersChanged(space);
        this.#deps.emit({ type: "PeopleChanged" });
        return member.accountId;
      }
      case "joinByLink":
        return this.#joinByLink(change.link, change.name);
      case "memberLeft": {
        const space = this.#shared(change.spaceId);
        const member = this.#target(space, change.accountId);
        if (member.role === "owner" && this.#ownerCount(space) === 1) {
          throw invalid("role", "the last Owner cannot leave");
        }
        this.#removeMember(space, member, true);
        return undefined;
      }
      case "roleSet": {
        const space = this.#shared(change.spaceId);
        this.#setRole(space, this.#target(space, change.accountId), change.role);
        return undefined;
      }
      case "removed":
        this.#removedBy(change);
        return undefined;
      case "closed": {
        const space = this.#shared(change.spaceId);
        space.state = "Archived";
        space.archiveReason = "Closed";
        space.closedAt = this.#deps.now();
        this.#deps.emit({ type: "SpacesChanged" });
        this.#membersChanged(space);
        this.#deps.emit({ type: "SpaceRemoved", data: { spaceId: space.spaceId, reason: "Closed" } });
        return undefined;
      }
      case "viaKit": {
        const space = this.#shared(change.spaceId);
        const member = this.#target(space, change.accountId);
        member.viaKitAt = this.#deps.now();
        this.#membersChanged(space);
        this.#deps.emit({
          type: "MemberDeviceViaKit",
          data: { spaceId: space.spaceId, accountId: member.accountId, name: member.name },
        });
        return undefined;
      }
      case "refused": {
        const space = this.#shared(change.spaceId);
        const member = this.#target(space, change.accountId);
        const count = change.count ?? 1;
        member.refused += count;
        space.refusedTotal += count;
        this.#membersChanged(space);
        return undefined;
      }
      case "disputed": {
        const space = this.#shared(change.spaceId);
        const member = this.#target(space, change.accountId);
        space.disputes.push({
          ...(change.pageId === undefined ? {} : { pageId: change.pageId }),
          accountId: member.accountId,
          at: this.#deps.now(),
        });
        this.#membersChanged(space);
        this.#deps.emit({
          type: "SnapshotDisputed",
          data: {
            spaceId: space.spaceId,
            ...(change.pageId === undefined ? {} : { pageId: change.pageId }),
            accountId: member.accountId,
          },
        });
        return undefined;
      }
      case "serverFlagged": {
        const space = this.#shared(change.spaceId);
        space.serverFlagged = true;
        this.#deps.emit({ type: "SyncChanged" });
        this.#membersChanged(space);
        return undefined;
      }
      case "renamed": {
        const space = this.#shared(change.spaceId);
        this.#target(space, change.accountId).name = displayName(change.name, PROFILE_NAME_MAX_CHARS, "displayName");
        this.#deps.emit({ type: "PeopleChanged" });
        this.#membersChanged(space);
        return undefined;
      }
    }
  }

  // ----- internals: membership rules ------------------------------------------------------------------------------

  /** The account of a simulated person (the same name is the same account everywhere). */
  #peer(name: string): AccountId {
    let id = this.#people.get(name);
    if (id === undefined) {
      this.#nextPerson += 1;
      id = `00000000-0000-4000-8000-a${this.#nextPerson.toString(16).padStart(11, "0")}`;
      this.#people.set(name, id);
    }
    return id;
  }

  #me(): AccountId {
    this.#meId ??= this.#deps.accountId() ?? FALLBACK_ACCOUNT;
    return this.#meId;
  }

  /** An id for something a seed creates: not drawn from the transport's generator. */
  #mint(): string {
    this.#nextSeedId += 1;
    return `00000000-0000-4000-8000-b${this.#nextSeedId.toString(16).padStart(11, "0")}`;
  }

  #newSpace(spaceId: SpaceId, state: SpaceShareState): SharedSpace {
    return {
      spaceId,
      feedId: this.#seeding ? this.#mint() : this.#deps.newId(),
      state,
      epoch: 1,
      pendingRekey: false,
      members: [],
      former: [],
      invites: [],
      settings: { trashRetentionDays: DEFAULT_TRASH_RETENTION_DAYS, editorsMayLink: false },
      disputes: [],
      refusedTotal: 0,
      serverFlagged: false,
      ai: false,
    };
  }

  #member(accountId: AccountId, name: string, role: MemberRole, joinedAt: number, isMe: boolean): MemberRecord {
    return { accountId, name, role, joinedAt, isMe, deviceCount: 1, refused: 0 };
  }

  /** The space exists (`NotFound`). */
  #known(spaceId: SpaceId): void {
    if (!this.#deps.hasSpace(spaceId)) {
      throw notFound("space not found");
    }
  }

  /** The shared space (or archive) of `spaceId`: `NotFound` when unknown, `InvalidInput` for a personal one. */
  #shared(spaceId: SpaceId): SharedSpace {
    this.#known(spaceId);
    const space = this.#spaces.get(spaceId);
    if (space === undefined) {
      throw invalid("spaceId", "this space is not shared");
    }
    return space;
  }

  #meOf(space: SharedSpace): MemberRecord {
    const me = space.members.find((member) => member.isMe);
    if (me === undefined) {
      throw new CoreError({ code: "Internal", message: "this account is not in the member list" });
    }
    return me;
  }

  #roleOf(space: SharedSpace): MemberRole {
    return space.members.find((member) => member.isMe)?.role ?? "viewer";
  }

  #allow(space: SharedSpace, needed: MemberRole): void {
    if (space.state === "Archived") {
      throw forbiddenArchived();
    }
    if (!atLeast(this.#roleOf(space), needed)) {
      throw forbiddenRole(needed);
    }
  }

  #target(space: SharedSpace, accountId: AccountId): MemberRecord {
    const member = space.members.find((candidate) => candidate.accountId === accountId);
    if (member === undefined) {
      throw invalid("accountId", "this person is not a member of the space");
    }
    return member;
  }

  #ownerCount(space: SharedSpace): number {
    return space.members.filter((member) => member.role === "owner").length;
  }

  #checkInviteRole(role: MemberRole): void {
    if (role === "owner") {
      throw invalid("role", "an invite cannot grant the Owner role: promote a member instead");
    }
  }

  #checkRoom(space: SharedSpace): void {
    if (space.invites.filter((invite) => this.#statusOf(invite) === "Open").length >= MAX_OPEN_INVITES) {
      throw invalid("invites", `a space has at most ${String(MAX_OPEN_INVITES)} open invites: revoke one first`);
    }
    if (space.members.length >= MAX_MEMBERS) {
      throw invalid("spaceId", `a space has at most ${String(MAX_MEMBERS)} members`);
    }
  }

  #statusOf(invite: InviteRecord): InviteStatus {
    if (invite.usedBy !== undefined) {
      return "Used";
    }
    if (invite.revoked) {
      return "Revoked";
    }
    return this.#deps.now() >= invite.expiresAt ? "Expired" : "Open";
  }

  /** The open invites of an Owner who is demoted, removed or leaves are void (ADR-0043). */
  #voidInvites(space: SharedSpace, creator: AccountId): void {
    for (const invite of space.invites) {
      if (invite.createdBy === creator && this.#statusOf(invite) === "Open") {
        invite.revoked = true;
      }
    }
  }

  /** `RoleSet`: at least one Owner remains; this account's own change is `SpaceRoleChanged`. */
  #setRole(space: SharedSpace, target: MemberRecord, role: MemberRole): void {
    const previous = target.role;
    if (previous === role) {
      return;
    }
    if (previous === "owner" && this.#ownerCount(space) === 1) {
      throw invalid("role", "a space needs at least one Owner: promote someone else first");
    }
    target.role = role;
    if (previous === "owner") {
      this.#voidInvites(space, target.accountId);
    }
    if (target.isMe) {
      this.#deps.emit({ type: "SpacesChanged" });
      this.#deps.emit({ type: "SpaceRoleChanged", data: { spaceId: space.spaceId, role, previous } });
    }
    this.#membersChanged(space);
  }

  /** `Remove` (or a `Leave`): the member goes; a removal carries the Rekey inline, a leave waits for one. */
  #removeMember(space: SharedSpace, member: MemberRecord, left = false): void {
    space.members = space.members.filter((candidate) => candidate !== member);
    // The space remembers the last name it knew them by (a comment's author, a Person value and a mention read "Former member (name)").
    space.former = [...space.former.filter((former) => former.accountId !== member.accountId), { accountId: member.accountId, name: member.name }];
    if (member.role === "owner") {
      this.#voidInvites(space, member.accountId);
    }
    if (left) {
      space.pendingRekey = true;
    } else {
      space.epoch += 1;
    }
    this.#deps.emit({ type: "SpacesChanged" });
    this.#membersChanged(space);
  }

  /** A person joins: a removed account is not a member, so it may be invited again; a member may not join twice. */
  #addMember(space: SharedSpace, name: string, role: MemberRole): MemberRecord {
    this.#checkInviteRole(role);
    if (space.members.length >= MAX_MEMBERS) {
      throw invalid("spaceId", `a space has at most ${String(MAX_MEMBERS)} members`);
    }
    const trimmed = displayName(name, PROFILE_NAME_MAX_CHARS, "displayName");
    const accountId = this.#peer(trimmed);
    if (space.members.some((member) => member.accountId === accountId)) {
      throw new CoreError({ code: "AlreadyExists", message: "this person is already a member" });
    }
    const member = this.#member(accountId, trimmed, role, this.#deps.now(), false);
    space.members.push(member);
    // A person who joins again is a member, not a former one.
    space.former = space.former.filter((former) => former.accountId !== accountId);
    // A Join after a Rekey needs a Grant, which any member device posts: the epoch is unchanged.
    return member;
  }

  /** Another person uses a link this account created: the invite rules apply, and every dead invite is one `NotFound`. */
  #joinByLink(link: string, name: string): AccountId {
    const { secret } = parseInviteLink(link);
    for (const space of this.#spaces.values()) {
      const invite = space.invites.find((candidate) => candidate.secret === secret);
      if (invite === undefined) {
        continue;
      }
      if (space.state === "Archived" || this.#statusOf(invite) !== "Open") {
        throw notFound(INVITE_DEAD_MESSAGE);
      }
      const member = this.#addMember(space, name, invite.role);
      invite.usedBy = member.accountId;
      this.#membersChanged(space);
      this.#deps.emit({ type: "PeopleChanged" });
      return member.accountId;
    }
    throw notFound(INVITE_DEAD_MESSAGE);
  }

  /** An Owner removed a member of the space: another one, or this account (the space becomes an archive). */
  #removedBy(change: Extract<SharingChange, { type: "removed" }>): void {
    const space = this.#shared(change.spaceId);
    const member = change.accountId === undefined ? this.#meOf(space) : this.#target(space, change.accountId);
    if (!member.isMe) {
      this.#removeMember(space, member);
      return;
    }
    space.state = "Archived";
    space.archiveReason = "Removed";
    space.epoch += 1;
    this.#deps.emit({ type: "SpacesChanged" });
    this.#membersChanged(space);
    this.#deps.emit({ type: "SpaceRemoved", data: { spaceId: space.spaceId, reason: "Removed" } });
    if (change.unsentPageIds !== undefined && change.unsentPageIds.length > 0) {
      const copied = this.#deps.copyUnsent(change.unsentPageIds);
      this.#deps.emit({
        type: "UnsentChangesCopied",
        data: { spaceId: space.spaceId, targetSpaceId: copied.targetSpaceId, pageIds: copied.pageIds },
      });
    }
  }

  #finishShare(space: SharedSpace): void {
    if (space.state === "Pending") {
      space.state = "Shared";
      this.#deps.emit({ type: "SpacesChanged" });
      this.#membersChanged(space);
    }
  }

  #membersChanged(space: SharedSpace): void {
    this.#deps.emit({ type: "SpaceMembersChanged", data: { spaceId: space.spaceId } });
  }

  // ----- internals: lists ---------------------------------------------------------------------------------------------

  #membersInfo(space: SharedSpace): MembersInfo {
    const me = this.#meOf(space);
    const owner = me.role === "owner" && space.state !== "Archived";
    const members = space.members.map((member): MemberInfo => {
      const info: MemberInfo = {
        accountId: member.accountId,
        name: member.name,
        role: member.role,
        joinedAt: member.joinedAt,
        isMe: member.isMe,
        verified: this.#verified.has(member.accountId),
        deviceCount: member.deviceCount,
        refusedChanges: member.refused,
        nameShared: space.members.some(
          (other) => other !== member && other.name.toLowerCase() === member.name.toLowerCase(),
        ),
      };
      if (!member.isMe) {
        info.safetyNumber = safetyNumber(me.accountId, member.accountId);
      }
      if (member.viaKitAt !== undefined) {
        info.viaKitAt = member.viaKitAt;
      }
      return info;
    });
    const info: MembersInfo = {
      spaceId: space.spaceId,
      feedId: space.feedId,
      myRole: me.role,
      epoch: space.epoch,
      pendingRekey: space.pendingRekey,
      members,
      // In account order, as the core lists them.
      formerMembers: [...space.former].sort((a, b) => (a.accountId < b.accountId ? -1 : a.accountId > b.accountId ? 1 : 0)).map((former) => ({ ...former })),
      invites: owner ? space.invites.map((invite) => this.#inviteInfo(invite)) : [],
      settings: { ...space.settings },
      disputes: owner ? space.disputes.map((dispute) => ({ ...dispute })) : [],
      refusedTotal: space.refusedTotal,
      serverFlagged: space.serverFlagged,
    };
    if (space.closedAt !== undefined) {
      info.closedAt = space.closedAt;
    }
    return info;
  }

  #inviteInfo(invite: InviteRecord): InviteInfo {
    const info: InviteInfo = {
      inviteId: invite.inviteId,
      kind: invite.kind,
      role: invite.role,
      createdBy: invite.createdBy,
      createdAt: invite.createdAt,
      expiresAt: invite.expiresAt,
      status: this.#statusOf(invite),
    };
    if (invite.boundAccount !== undefined) {
      info.boundAccount = invite.boundAccount;
    }
    if (invite.usedBy !== undefined) {
      info.usedBy = invite.usedBy;
    }
    return info;
  }

  #person(accountId: AccountId): PersonInfo {
    if (accountId === this.#me()) {
      return { accountId, name: this.#profileName ?? "You", verified: false, isMe: true };
    }
    for (const space of this.#spaces.values()) {
      const member = space.members.find((candidate) => candidate.accountId === accountId);
      if (member !== undefined) {
        const me = this.#meOf(space);
        return {
          accountId,
          name: member.name,
          verified: this.#verified.has(accountId),
          safetyNumber: safetyNumber(me.accountId, accountId),
          isMe: false,
        };
      }
    }
    throw notFound("person not found");
  }

  // ----- internals: sessions ----------------------------------------------------------------------------------------

  /** Joining needs a workspace and the recovery kit (the account id exists). */
  #needsAccount(): void {
    if (!this.#deps.hasWorkspace()) {
      throw invalid("workspace", "create a workspace before joining a shared space");
    }
    if (this.#deps.accountId() === undefined) {
      throw invalid("account", "create your recovery kit before joining a shared space");
    }
  }

  /** What a link offers: a link of this account's own spaces is dead for it, the dead secret and a used one too. */
  #offerFor(secret: string): Offer {
    const offer: Offer = { ...this.#invitation };
    if (secret === DEAD_LINK_SECRET || this.#used.has(secret)) {
      offer.dead = INVITE_DEAD_MESSAGE;
    } else if ([...this.#spaces.values()].some((space) => space.invites.some((invite) => invite.secret === secret))) {
      offer.dead = "you are already a member of this space";
    }
    return offer;
  }

  #begin(session: Session): void {
    this.#steps.cancel();
    this.#session = session;
    this.#deps.emit({ type: "InviteChanged" });
    this.#deps.emit({ type: "InviteRequested" });
  }

  #change(session: Session, change: Partial<Session>): void {
    Object.assign(session, change);
    this.#deps.emit({ type: "InviteChanged" });
  }

  #fail(session: Session, message: string): void {
    this.#change(session, { state: "Failed", error: message });
  }

  /** The next step of `session`, unless it was replaced, cancelled or stopped meanwhile. */
  #next(session: Session, step: () => void): void {
    this.#steps.next(
      () => {
        if (this.#session === session && ACTIVE.has(session.state)) {
          step();
        }
      },
      () => {
        this.stop();
      },
    );
  }

  /** The inviter confirmed after the invitee: the `Invite` is appended, the bundle goes, the invitee's `Join` follows. */
  #inviterConfirms(session: Session): void {
    const space = session.spaceId === undefined ? undefined : this.#spaces.get(session.spaceId);
    const role = session.role ?? "editor";
    if (space === undefined || space.state === "Archived" || !atLeast(this.#roleOf(space), "owner")) {
      this.#fail(session, "you can no longer invite people to this space");
      return;
    }
    const now = this.#deps.now();
    const name = session.peerName ?? MEMORY_INVITEE_NAME;
    const peer = this.#peer(name);
    const invite: InviteRecord = {
      inviteId: this.#deps.newId(),
      kind: "Code",
      role,
      createdBy: this.#me(),
      createdAt: now,
      expiresAt: now + INVITE_CODE_TTL_MS,
      boundAccount: peer,
      revoked: false,
    };
    space.invites.push(invite);
    this.#change(session, { state: "Transferring", verified: true });
    this.#membersChanged(space);
    this.#next(session, () => {
      try {
        const member = this.#addMember(space, name, role);
        invite.usedBy = member.accountId;
        // The digits authenticated both accounts: the code flow marks them verified (ADR-0042).
        this.#verified.set(member.accountId, this.#deps.now());
      } catch (error) {
        this.#fail(session, error instanceof CoreError ? error.message : "the other person could not join");
        return;
      }
      this.#membersChanged(space);
      this.#deps.emit({ type: "PeopleChanged" });
      this.#change(session, { state: "Done" });
    });
  }

  /** The invitee's side after the preview or the bundle: the account is opened, the Join posted, the space arrives. */
  #join(session: Session): void {
    this.#change(session, { state: "Joining" });
    this.#next(session, () => {
      const offer = session.offer ?? this.#invitation;
      const name = offer.spaceName;
      const spaceId = this.#deps.joinedSpace(name);
      const now = this.#deps.now();
      const space = this.#newSpace(spaceId, "Shared");
      const inviter = this.#member(this.#peer(offer.inviterName), offer.inviterName, "owner", now - 2 * DAY_MS, false);
      const mine = this.#member(this.#me(), this.#profileName ?? "You", offer.role, now, true);
      space.members = [inviter, mine];
      this.#spaces.set(spaceId, space);
      if (session.flow === "Code") {
        this.#verified.set(inviter.accountId, now);
      }
      if (session.secret !== undefined) {
        this.#used.add(session.secret);
      }
      this.#deps.emit({ type: "SpacesChanged" });
      this.#membersChanged(space);
      this.#deps.emit({ type: "PeopleChanged" });
      this.#change(session, { state: "Done", verified: session.flow === "Code" });
    });
  }

  #info(session: Session): InviteSessionInfo {
    const info: InviteSessionInfo = { flow: session.flow, side: session.side, state: session.state, verified: session.verified };
    if (session.code !== undefined) {
      info.code = formatPairingCode(session.code);
    }
    if (session.sas !== undefined) {
      info.sas = session.sas;
    }
    if (session.peerName !== undefined) {
      info.peerName = session.peerName;
    }
    if (session.peerSafetyNumber !== undefined) {
      info.peerSafetyNumber = session.peerSafetyNumber;
    }
    if (session.spaceName !== undefined) {
      info.spaceName = session.spaceName;
    }
    if (session.inviterName !== undefined) {
      info.inviterName = session.inviterName;
    }
    if (session.role !== undefined) {
      info.role = session.role;
    }
    if (session.serverUrl !== undefined) {
      info.serverUrl = session.serverUrl;
    }
    if (session.error !== undefined) {
      info.error = session.error;
    }
    if (session.expiresAt !== undefined) {
      info.expiresAt = session.expiresAt;
    }
    return info;
  }
}
