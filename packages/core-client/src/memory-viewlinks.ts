// The in-memory twin of view-only links (Milestone 13, slice 4; ADR-0051, `docs/security/sharing.md` section 10, the contract in
// `docs/development/milestone-13.md` section 4.1 and `kb-core`'s `links.rs`), used by `MemoryTransport`. There is no server, no
// cryptography and no viewer here: a link is a record with a made-up key in its URL, and what the mirror keeps to is what the
// core answers:
//
// - **Who creates.** An Owner; an Editor only when the space's "Editors may create view links" setting is on (`Forbidden`
//   with `forbidden_role(Owner)` otherwise); a Commenter or a Viewer never (`forbidden_role(Editor)`); a read-only copy never
//   (`Forbidden` "archived"). Anyone in a personal space (there is no setting and nobody else there).
// - **The order of the checks of `ViewLinkCreate`:** the space (`NotFound`), the role, `expiresInDays` (absent, or a whole number
//   of days from 1 to `LINK_EXPIRY_MAX_DAYS`: `InvalidInput` for `expiresInDays`), the page (a live page of the space: `InvalidInput`
//   for `pageId`, whatever the reason), sync turned on with an account (`InvalidInput` for `serverUrl`). Not simulated: the
//   `LINK_MAX_CHUNKS` limit of a page's attachments.
// - **The URL is answered once.** `ViewLinkCreate` answers the URL (`<server>/view#<key>`); `ViewLinkList` answers every link with an
//   empty `url`, so a page that wants the URL again has to make a new link. (The real core rebuilds the URL from the `Link`
//   record for every member; the share dialog of `packages/ui` never shows it from a list either way.)
// - **Publishing, then Live.** A new link is `Publishing` on this device until its first upload succeeded, which takes `publishMs`
//   (`MemoryViewLinksOptions`; a lock finishes it at once, as it does a share), then it is `Live` with `lastPublishedAt`.
//   `ViewLinksChanged` fires for the creation and again for that change. A link whose time has come is `Expired` (and stays
//   listed: revoking it cleans it up); `Revoked` is never listed, a revoked link is gone.
// - **Listing.** Any member, and a read-only copy, reads the links of the space, newest first. `createdByName` is the member's name
//   in the space, or the last name the space knew a former member by (a link outlives its creator's membership).
// - **Revoking.** The link's creator or an Owner (`forbidden_role(Owner)` for another member; `Forbidden` "archived" for a
//   read-only copy; `InvalidInput` for `linkId` when the space has no such link); the answer is the links that remain.
// - **A space that goes takes its links:** closing it (by this account or another Owner) deletes the feed on the server, so
//   every link of the space is gone and `ViewLinksChanged` fires; leaving it or deleting its read-only copy drops the local
//   records. A read-only copy this account was removed from keeps listing the links the device knew.
//
// Seeds (`MemoryViewLinksOptions`, the dev page's `?links=`) start links without events, as the other seeds do, and draw no id from
// the transport's generator: the ids of the links are `00000000-0000-4000-8000-c...`.

import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { Event } from "./generated/Event";
import type { LinkId } from "./generated/LinkId";
import type { PageId } from "./generated/PageId";
import type { SpaceId } from "./generated/SpaceId";
import type { ViewLinkInfo } from "./generated/ViewLinkInfo";
import { LINK_EXPIRY_MAX_DAYS, VIEWER_PATH } from "./link";
import { forbiddenArchived, forbiddenRole } from "./sharing-mirror";
import type { MemorySharing } from "./sharing-mirror";
import { invalid } from "./validate";

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;

/** How long the first upload of a new link takes by default, in milliseconds. */
export const DEFAULT_LINK_PUBLISH_MS = 600;

/** The server a seeded link points at when sync is not configured. */
const SEED_SERVER = "https://sync.example.com";

/** A link the seeds start with. */
export interface MemoryViewLinkSeed {
  /** The name of a seeded space. */
  space: string;
  /** The title of a live page of that space. */
  page: string;
  /**
   * The member (by name) who made it; default this account. A name that is no member's is a former member's: the link outlives
   * its creator's membership.
   */
  createdBy?: string;
  /** How long ago it was made, in milliseconds (default one hour). */
  ageMs?: number;
  /** When it stops working, in milliseconds from now (negative: it stopped already); absent: never. */
  expiresInMs?: number;
  /** Its first upload has not finished: it is `Publishing` and turns `Live` after `publishMs`. Default: `Live`. */
  publishing?: boolean;
}

/** `MemoryTransportOptions.links`: the starting links and the pace of publishing. */
export interface MemoryViewLinksOptions {
  /** Links that exist from the start (the dev page: `?links=`). */
  links?: readonly MemoryViewLinkSeed[];
  /** How long the first upload of a new link takes, in milliseconds (default `DEFAULT_LINK_PUBLISH_MS`). */
  publishMs?: number;
}

/** What the view link mirror needs from its owner. */
export interface ViewLinksDeps {
  now: () => number;
  emit: (event: Event) => void;
  ready: () => boolean;
  sharing: MemorySharing;
  hasSpace: (spaceId: SpaceId) => boolean;
  findSpace: (name: string) => SpaceId | undefined;
  /** The live page of a space with this title. */
  pageByTitle: (spaceId: SpaceId, title: string) => PageId | undefined;
  /** A page the mirror still has (live or in the trash); `undefined` once purged. */
  page: (pageId: PageId) => { spaceId: SpaceId; trashed: boolean } | undefined;
  /** The sync server's address; `undefined` while sync is off. */
  syncServer: () => string | undefined;
  /** This account's id; `undefined` before the recovery kit exists. */
  accountId: () => AccountId | undefined;
}

interface LinkRecord {
  linkId: LinkId;
  spaceId: SpaceId;
  pageId: PageId;
  /** The URL, which only `ViewLinkCreate` answers. */
  url: string;
  createdBy: AccountId;
  /** The creator's name when the link was made, for a creator the space does not know any more. */
  createdByName: string;
  createdAt: number;
  expiresAt?: number;
  lastPublishedAt?: number;
  /** The first upload has not succeeded yet. */
  publishing: boolean;
}

/** 32 random bytes as base64url without padding: 43 characters, the form of `LinkKey::to_fragment`. */
function randomFragment(): string {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return globalThis.btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}

export class MemoryViewLinks {
  readonly #deps: ViewLinksDeps;
  readonly #publishMs: number;
  readonly #links: LinkRecord[] = [];
  /** The first uploads that have not finished, by link. */
  readonly #uploads = new Map<LinkId, ReturnType<typeof setTimeout>>();
  #nextId = 0;

  constructor(deps: ViewLinksDeps, options: MemoryViewLinksOptions = {}) {
    this.#deps = deps;
    this.#publishMs = options.publishMs ?? DEFAULT_LINK_PUBLISH_MS;
  }

  /** Starts the links of the options, without events (the spaces and pages they name are seeded already). */
  seed(options: MemoryViewLinksOptions): void {
    const seeds = options.links ?? [];
    if (seeds.length === 0) {
      // Reading the clock here would advance an injected test clock by one for every transport built without links.
      return;
    }
    const now = this.#deps.now();
    for (const seed of seeds) {
      const spaceId = this.#deps.findSpace(seed.space);
      const pageId = spaceId === undefined ? undefined : this.#deps.pageByTitle(spaceId, seed.page);
      if (spaceId === undefined || pageId === undefined) {
        throw new CoreError({ code: "Internal", message: "MemoryTransport: the view link's page is not a seeded page" });
      }
      const sharing = this.#deps.sharing;
      const creator = seed.createdBy === undefined ? sharing.meId() : sharing.accountByName(spaceId, seed.createdBy);
      const record: LinkRecord = {
        linkId: this.#mint(),
        spaceId,
        pageId,
        url: this.#urlOf(this.#deps.syncServer() ?? SEED_SERVER),
        createdBy: creator,
        createdByName: sharing.knownNameOf(spaceId, creator) ?? seed.createdBy ?? sharing.myName(),
        createdAt: now - (seed.ageMs ?? HOUR_MS),
        publishing: seed.publishing === true,
      };
      if (seed.expiresInMs !== undefined) {
        record.expiresAt = now + seed.expiresInMs;
      }
      if (!record.publishing) {
        record.lastPublishedAt = record.createdAt;
      }
      this.#links.push(record);
      if (record.publishing) {
        this.#startUpload(record);
      }
    }
  }

  /** `ViewLinkCreate`: a link for a live page; answers the URL, which the list never shows again. */
  create(spaceId: SpaceId, pageId: PageId, expiresInDays: number | undefined): ViewLinkInfo {
    const deps = this.#deps;
    if (!deps.hasSpace(spaceId)) {
      throw new CoreError({ code: "NotFound", message: "space not found" });
    }
    // An archive and a role below Editor are refused here; an Editor needs the setting; a personal space passes.
    deps.sharing.require(spaceId, "editor");
    if (deps.sharing.access(spaceId).role === "editor" && !deps.sharing.editorsMayLink(spaceId)) {
      throw forbiddenRole("owner");
    }
    if (expiresInDays !== undefined && (!Number.isInteger(expiresInDays) || expiresInDays < 1 || expiresInDays > LINK_EXPIRY_MAX_DAYS)) {
      throw invalid("expiresInDays", `must be 1 to ${String(LINK_EXPIRY_MAX_DAYS)} days`);
    }
    const page = deps.page(pageId);
    if (page === undefined || page.spaceId !== spaceId || page.trashed) {
      throw invalid("pageId", "this page cannot be given a view link");
    }
    const server = deps.syncServer();
    if (server === undefined) {
      throw invalid("serverUrl", "turn sync on before creating a view link");
    }
    const me = deps.accountId();
    if (me === undefined) {
      throw invalid("serverUrl", "create your recovery kit before creating a view link");
    }
    const now = deps.now();
    const record: LinkRecord = {
      linkId: this.#mint(),
      spaceId,
      pageId,
      url: this.#urlOf(server),
      createdBy: me,
      createdByName: deps.sharing.myName(),
      createdAt: now,
      publishing: true,
    };
    if (expiresInDays !== undefined) {
      record.expiresAt = now + expiresInDays * DAY_MS;
    }
    this.#links.push(record);
    this.#startUpload(record);
    deps.emit({ type: "ViewLinksChanged", data: { spaceId } });
    return this.#info(record, true);
  }

  /** `ViewLinkList`: the space's links, newest first, without their URLs. */
  list(spaceId: SpaceId): ViewLinkInfo[] {
    if (!this.#deps.hasSpace(spaceId)) {
      throw new CoreError({ code: "NotFound", message: "space not found" });
    }
    return this.#listOf(spaceId);
  }

  /** `ViewLinkRevoke`: the creator or an Owner deletes a link; answers the links that remain. */
  revoke(spaceId: SpaceId, linkId: LinkId): ViewLinkInfo[] {
    const deps = this.#deps;
    if (!deps.hasSpace(spaceId)) {
      throw new CoreError({ code: "NotFound", message: "space not found" });
    }
    const access = deps.sharing.access(spaceId);
    if (access.archived) {
      throw forbiddenArchived();
    }
    const record = this.#links.find((candidate) => candidate.spaceId === spaceId && candidate.linkId === linkId);
    if (record === undefined) {
      throw invalid("linkId", "this space has no such view link");
    }
    if (access.role !== null && access.role !== "owner" && record.createdBy !== deps.sharing.meId()) {
      throw forbiddenRole("owner");
    }
    this.#remove(record);
    deps.emit({ type: "ViewLinksChanged", data: { spaceId } });
    return this.#listOf(spaceId);
  }

  /** A space was closed (by this account or another Owner): its feed is gone, and so are its links. Idempotent. */
  reconcile(): void {
    const closed = new Set(this.#links.filter((link) => this.#deps.sharing.isClosed(link.spaceId)).map((link) => link.spaceId));
    for (const spaceId of closed) {
      this.#dropSpace(spaceId);
      this.#deps.emit({ type: "ViewLinksChanged", data: { spaceId } });
    }
  }

  /** The local copy of a space goes (leaving it, deleting its read-only copy): so do its link records, silently. */
  spaceDeleted(spaceId: SpaceId): void {
    this.#dropSpace(spaceId);
  }

  /** The core locked: the uploads that were under way finish at once (the driver would meanwhile). */
  stop(): void {
    for (const [linkId, timer] of this.#uploads) {
      clearTimeout(timer);
      const record = this.#links.find((candidate) => candidate.linkId === linkId);
      if (record !== undefined) {
        this.#published(record, false);
      }
    }
    this.#uploads.clear();
  }

  // ----- internals --------------------------------------------------------------------------------------------------

  /** An id for a link: not drawn from the transport's generator (a link id is derived from the key, not random). */
  #mint(): LinkId {
    this.#nextId += 1;
    return `00000000-0000-4000-8000-c${this.#nextId.toString(16).padStart(11, "0")}`;
  }

  #urlOf(server: string): string {
    return `${server.replace(/\/+$/u, "")}${VIEWER_PATH}#${randomFragment()}`;
  }

  #listOf(spaceId: SpaceId): ViewLinkInfo[] {
    // Newest first; links made at the same time keep the order they were made in, the later one first.
    return this.#links
      .map((record, index) => ({ record, index }))
      .filter(({ record }) => record.spaceId === spaceId)
      .sort((a, b) => b.record.createdAt - a.record.createdAt || b.index - a.index)
      .map(({ record }) => this.#info(record, false));
  }

  #info(record: LinkRecord, withUrl: boolean): ViewLinkInfo {
    const expired = record.expiresAt !== undefined && record.expiresAt <= this.#deps.now();
    return {
      linkId: record.linkId,
      pageId: record.pageId,
      url: withUrl ? record.url : "",
      createdBy: record.createdBy,
      createdByName: this.#deps.sharing.knownNameOf(record.spaceId, record.createdBy) ?? record.createdByName,
      createdAt: record.createdAt,
      ...(record.expiresAt === undefined ? {} : { expiresAt: record.expiresAt }),
      ...(record.lastPublishedAt === undefined ? {} : { lastPublishedAt: record.lastPublishedAt }),
      status: expired ? "Expired" : record.publishing ? "Publishing" : "Live",
    };
  }

  /** The first upload of a link finishes after `publishMs`. */
  #startUpload(record: LinkRecord): void {
    const timer = setTimeout(() => {
      this.#uploads.delete(record.linkId);
      if (this.#deps.ready()) {
        this.#published(record, true);
      }
    }, this.#publishMs);
    this.#uploads.set(record.linkId, timer);
  }

  /** The first upload succeeded: the link is `Live`. */
  #published(record: LinkRecord, announce: boolean): void {
    if (!record.publishing || !this.#links.includes(record)) {
      return;
    }
    record.publishing = false;
    record.lastPublishedAt = this.#deps.now();
    if (announce) {
      this.#deps.emit({ type: "ViewLinksChanged", data: { spaceId: record.spaceId } });
    }
  }

  #remove(record: LinkRecord): void {
    const timer = this.#uploads.get(record.linkId);
    if (timer !== undefined) {
      clearTimeout(timer);
      this.#uploads.delete(record.linkId);
    }
    this.#links.splice(this.#links.indexOf(record), 1);
  }

  /** Removes every record of a space. */
  #dropSpace(spaceId: SpaceId): void {
    for (const record of this.#links.filter((candidate) => candidate.spaceId === spaceId)) {
      this.#remove(record);
    }
  }
}
