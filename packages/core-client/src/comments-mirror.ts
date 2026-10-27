// The in-memory twin of the core's comments, person mentions and notifications (Milestone 13, slice 2; ADR-0048 and
// ADR-0049, the contract in `docs/development/milestone-13.md` section 2.1, the design in `docs/security/sharing.md`
// section 8), used by `MemoryTransport`. There are no signed records here, no stream and no cryptography: the mirror keeps
// the folded result of a page's comment stream (threads and comments) and enforces the rules of the fold:
//
// - **Commenter and up post** (`CommentPost`): a Viewer, and anyone in a read-only archive, is `Forbidden`; a new thread when
//   `threadId` is absent (only then an anchor), a reply otherwise; the body (not empty, at most 10 KB, items of text,
//   mentions and pages) and the anchor (a quote of at most 200 characters, cursors that are base64) are `InvalidInput`
//   for `body` or `anchor` when they break a rule, a `threadId` that is not a thread of the page for `threadId`;
// - **only the author edits** (`CommentEdit`): `Forbidden` for anyone else, an Owner included, with `details` `{reason: "author"}`;
//   `NotFound` for a comment that does not exist or was deleted;
// - **the author or an Owner deletes** (`CommentDelete`; for another's comment the core asks for the Owner role, so `Forbidden` is
//   `{reason: "role", required: "owner"}`), and a delete is final: the comment cannot be edited or deleted
//   again. It stays as a placeholder (`deleted`, no body) while a later comment of its thread remains, else it is gone, and a
//   thread with no comment left is not listed; the answer of deleting the last one is the thread with no comments;
// - **the thread's author or Editor and up resolves** (`CommentResolve`), and reopens: `Forbidden` for anyone else, as
//   `{reason: "role", required: "editor"}`;
// - reads are open to every member (`CommentList`, an archive too); every write needs Commenter and up at the time, the
//   rules of the fold being judged by the role the author holds when the record is written;
// - a deleted root does not freeze its thread: replies stay, new replies and a resolve are still allowed (a thread whose comments
//   are all deleted is only left out of `CommentList`); a reply is never older than its root (its `createdAt` is clamped up to
//   the root's);
// - a removed or departed author's comments stay and read `authorFormer` with the last known name.
//
// Records of the other people (`MemoryTransport.receiveComments`) are judged like the core's fold judges them: one the rules do not
// allow is **ignored**, never an error (see `CommentChange`).
//
// **Notifications** are a local inbox raised by the other people only (`MemoryTransport.receiveComments`, `changePage` with an
// actor): a reply in a thread this account wrote in (`ThreadReply`), a mention of this account in a comment (`CommentMention`:
// a post, or an edit that adds the mention; a reply that also mentions this account is this kind, not a reply), a new mention of this account in a page (`PageMention`) and being added to a
// Person property (`PersonAssigned`). This account's own writes never notify. Each row is told as `NotificationRaised`, then
// `NotificationsChanged`; `NotificationList` pages newest first (`limit`, `beforeId`, `unreadOnly`), `NotificationsMarkRead`
// marks some rows or all; the inbox keeps at most 1,000 rows (read rows go first, the newest never).
//
// People (`PeopleMentionable`) are the members of the space in join order, this account among them; a personal space lists
// this account alone.

import {
  NOTIFICATIONS_DEFAULT_LIMIT,
  NOTIFICATIONS_MAX_LIMIT,
  NOTIFICATIONS_MAX_ROWS,
  commentAnchorProblem,
  commentBodyMentions,
  commentBodyProblem,
  commentBodyText,
} from "./comments";
import type { CommentChange, MemoryCommentBody, MemoryCommentSeed, MemoryNotificationSeed, MemoryThreadSeed } from "./comments";
import { CoreError } from "./errors";
import type { AccountId } from "./generated/AccountId";
import type { CommentAnchor } from "./generated/CommentAnchor";
import type { CommentBodyItem } from "./generated/CommentBodyItem";
import type { CommentId } from "./generated/CommentId";
import type { CommentInfo } from "./generated/CommentInfo";
import type { Event } from "./generated/Event";
import type { MemberRole } from "./generated/MemberRole";
import type { NotificationInfo } from "./generated/NotificationInfo";
import type { NotificationKind } from "./generated/NotificationKind";
import type { PageId } from "./generated/PageId";
import type { PersonInfo } from "./generated/PersonInfo";
import type { SpaceId } from "./generated/SpaceId";
import type { ThreadId } from "./generated/ThreadId";
import type { ThreadInfo } from "./generated/ThreadInfo";
import { canonicalId } from "./page-doc";
import type { PagePersonMention } from "./page-doc";
import { forbiddenArchived } from "./sharing-mirror";
import type { MemorySharing } from "./sharing-mirror";
import { invalid } from "./validate";

const ROLE_RANK: Readonly<Record<MemberRole, number>> = { viewer: 1, commenter: 2, editor: 3, owner: 4 };
const MINUTE_MS = 60_000;

/** What the comments mirror needs from the transport. */
export interface CommentsDeps {
  /** Clock in Unix milliseconds. */
  now: () => number;
  /** Id generator (UUID text). */
  newId: () => string;
  /** Delivers an event to the transport's listeners. */
  emit: (event: Event) => void;
  /** Whether the core is ready (a toast is only raised then). */
  ready: () => boolean;
  /** The people and the roles. */
  sharing: MemorySharing;
  /** A page's space and title; `undefined` for an unknown or purged page. */
  page: (pageId: PageId) => { spaceId: SpaceId; title: string } | undefined;
  /** Whether a space exists on this device (the inbox of a space that was left is not listed). */
  hasSpace: (spaceId: SpaceId) => boolean;
  /** The id of the first live or trashed page of the space with this title (seeds). */
  pageByTitle: (spaceId: SpaceId, title: string) => PageId | undefined;
  /** Where `quote` sits in the page's seeded document, as a comment's anchor holds it (seeds); `undefined` when nowhere. */
  textAnchor: (pageId: PageId, quote: string) => { blockId: string; start: string; end: string } | undefined;
  /** The people a Person property of a page lists now (`[]` when none or unknown). */
  personValue: (pageId: PageId, propertyId: string) => AccountId[];
  /** Writes a Person property of a page as another person would (no role check here, the membership rule applies). */
  setPersonValue: (pageId: PageId, propertyId: string, accounts: AccountId[]) => void;
}

interface CommentRecord {
  commentId: CommentId;
  threadId: ThreadId;
  pageId: PageId;
  author: AccountId;
  /** The author's name when the comment was written or last seen: what "Former member" keeps. */
  authorName: string;
  body: CommentBodyItem[];
  createdAt: number;
  editedAt?: number;
  deleted: boolean;
}

interface ThreadRecord {
  threadId: ThreadId;
  pageId: PageId;
  anchor?: CommentAnchor;
  resolved: boolean;
  resolvedBy?: AccountId;
  comments: CommentRecord[];
}

interface NotificationRecord {
  id: number;
  kind: NotificationKind;
  spaceId: SpaceId;
  pageId?: PageId;
  blockId?: string;
  commentId?: CommentId;
  actor: AccountId;
  actorName: string;
  at: number;
  read: boolean;
}

function notFound(message: string): CoreError {
  return new CoreError({ code: "NotFound", message });
}

/** `Forbidden` for a change only the author of a comment may make (the core's `details`: `{reason: "author"}`). */
function forbiddenAuthor(): CoreError {
  return new CoreError({ code: "Forbidden", message: "only the author of a comment may change it", details: { reason: "author" } });
}

function atLeast(role: MemberRole | null, needed: MemberRole): boolean {
  return role === null || ROLE_RANK[role] >= ROLE_RANK[needed];
}

/** The comments part of `MemoryTransport`: see the file's comment. */
export class MemoryComments {
  readonly #deps: CommentsDeps;
  readonly #threads = new Map<PageId, ThreadRecord[]>();
  readonly #inbox: NotificationRecord[] = [];
  #lastRow = 0;
  #seedCounter = 0;
  #seeding = false;

  constructor(deps: CommentsDeps) {
    this.#deps = deps;
  }

  // ----- the requests -----------------------------------------------------------------------------------------------

  /** `CommentList`: the threads of a page, oldest root first (any member; a thread with no comment left is left out). */
  list(pageId: PageId): ThreadInfo[] {
    const page = this.#page(pageId);
    return (this.#threads.get(pageId) ?? [])
      .map((thread) => this.#info(thread, page.spaceId))
      .filter((info) => info.comments.length > 0)
      .sort((a, b) => compareRoots(a, b));
  }

  /** `CommentPost`: Commenter and up; a new thread (with its anchor) or a reply; answers the thread. */
  post(pageId: PageId, threadId: ThreadId | undefined, anchor: CommentAnchor | undefined, body: readonly CommentBodyItem[]): ThreadInfo {
    const page = this.#page(pageId);
    this.#deps.sharing.require(page.spaceId, "commenter");
    const me = this.#deps.sharing.meId();
    const thread = this.#append(pageId, me, this.#deps.sharing.myName(), threadId, anchor, body, this.#deps.now(), undefined);
    this.#changed(pageId);
    return this.#info(thread, page.spaceId);
  }

  /** `CommentEdit`: the author only; the body is replaced. */
  edit(pageId: PageId, commentId: CommentId, body: readonly CommentBodyItem[]): ThreadInfo {
    const page = this.#page(pageId);
    this.#deps.sharing.require(page.spaceId, "commenter");
    const { thread, comment } = this.#comment(pageId, commentId);
    if (comment.author !== this.#deps.sharing.meId()) {
      throw forbiddenAuthor();
    }
    this.#replaceBody(comment, body, this.#deps.now());
    this.#changed(pageId);
    return this.#info(thread, page.spaceId);
  }

  /** `CommentDelete`: the author or an Owner; final. */
  delete(pageId: PageId, commentId: CommentId): ThreadInfo {
    const page = this.#page(pageId);
    this.#deps.sharing.require(page.spaceId, "commenter");
    const { thread, comment } = this.#comment(pageId, commentId);
    if (comment.author !== this.#deps.sharing.meId()) {
      // An Owner may delete anybody's comment: the core asks for the Owner role, so the refusal is the role's (`required: "owner"`).
      this.#deps.sharing.require(page.spaceId, "owner");
    }
    this.#remove(comment);
    this.#changed(pageId);
    return this.#info(thread, page.spaceId);
  }

  /** `CommentResolve`: the thread's author or Editor and up; resolves or reopens. */
  resolve(pageId: PageId, threadId: ThreadId, resolved: boolean): ThreadInfo {
    const page = this.#page(pageId);
    this.#deps.sharing.require(page.spaceId, "commenter");
    const thread = this.#thread(pageId, threadId);
    const me = this.#deps.sharing.meId();
    if (this.#rootAuthor(thread) !== me) {
      // The thread's author, or Editor and up: the core asks for the Editor role (`required: "editor"`).
      this.#deps.sharing.require(page.spaceId, "editor");
    }
    this.#setResolved(thread, resolved, me);
    this.#changed(pageId);
    return this.#info(thread, page.spaceId);
  }

  /** `NotificationList`: this device's inbox, newest first. */
  notificationList(input: { limit?: number; beforeId?: number; unreadOnly?: boolean }): NotificationInfo[] {
    const limit = Math.min(NOTIFICATIONS_MAX_LIMIT, Math.max(1, input.limit ?? NOTIFICATIONS_DEFAULT_LIMIT));
    const before = input.beforeId;
    return [...this.#inbox]
      .filter((row) => this.#deps.hasSpace(row.spaceId))
      .filter((row) => before === undefined || row.id < before)
      .filter((row) => input.unreadOnly !== true || !row.read)
      .sort((a, b) => b.id - a.id)
      .slice(0, limit)
      .map((row) => this.#notification(row));
  }

  /** `NotificationsMarkRead`: the listed rows, or every row when `ids` is left out; `NotificationsChanged` when one changed. */
  markRead(ids: readonly number[] | undefined): void {
    let changed = false;
    for (const row of this.#inbox) {
      if (!row.read && (ids === undefined || ids.includes(row.id))) {
        row.read = true;
        changed = true;
      }
    }
    if (changed) {
      this.#deps.emit({ type: "NotificationsChanged" });
    }
  }

  /**
   * What the text index holds for the comments of a page (`comment_fts`, ADR-0048): one text per comment that was not
   * deleted, a mention as `@name` and a page as `[[title]]`. A thread's own words, nothing of the authors.
   */
  searchTexts(spaceId: SpaceId, pageId: PageId): { commentId: CommentId; text: string }[] {
    const sharing = this.#deps.sharing;
    return (this.#threads.get(pageId) ?? []).flatMap((thread) =>
      thread.comments
        .filter((comment) => !comment.deleted)
        .map((comment) => ({
          commentId: comment.commentId,
          text: commentBodyText(
            comment.body,
            (accountId) => sharing.nameOf(spaceId, accountId),
            (id) => this.#deps.page(id)?.title,
          ),
        }))
        .filter(({ text }) => text.trim() !== ""),
    );
  }

  /** `PeopleMentionable`: the members of the space in join order (a personal space: this account alone). */
  people(spaceId: SpaceId): PersonInfo[] {
    return this.#deps.sharing.peopleOf(spaceId);
  }

  // ----- the other people -------------------------------------------------------------------------------------------

  /**
   * Another person acts, as it arrives through sync (see `CommentChange`). The fold ignores what the rules do not allow (a Viewer's
   * record of any op, an edit of someone else's comment, ...: nothing changes, nothing is told, `undefined` is answered);
   * `InvalidInput` for `author` when the name is not a member of the space (or is this account: its own writes are requests),
   * `NotFound` for a page, comment or thread that does not exist. Notifications for this account come from here. Answers the
   * thread the change touched (none for a Person property, nor for an ignored record).
   */
  receive(change: CommentChange): ThreadInfo | undefined {
    const page = this.#page(change.pageId);
    const access = this.#deps.sharing.access(page.spaceId);
    if (access.archived) {
      throw forbiddenArchived();
    }
    const member = this.#deps.sharing.memberByName(page.spaceId, change.author);
    if (member === undefined) {
      throw invalid("author", "this person is not a member of the space");
    }
    if (member.isMe) {
      throw invalid("author", "this account writes through its own requests");
    }
    // A record of a Viewer (a role below Commenter) is ignored, whatever its op.
    if (!atLeast(member.role, "commenter")) {
      return undefined;
    }
    const at = this.#deps.now();
    switch (change.type) {
      case "post": {
        // A second post that claims the id of a comment that exists is refused (the first one stands).
        if (change.commentId !== undefined && this.#hasComment(change.pageId, change.commentId)) {
          return undefined;
        }
        const body = this.#bodyOf(page.spaceId, change.body);
        const existing = change.threadId === undefined ? undefined : this.#findThread(change.pageId, change.threadId);
        const wroteIn = existing?.comments.some((comment) => !comment.deleted && comment.author === this.#deps.sharing.meId()) === true;
        const anchor = change.anchor;
        const thread = this.#append(change.pageId, member.accountId, member.name, change.threadId, anchor, body, at, undefined, change.commentId);
        const posted = thread.comments[thread.comments.length - 1];
        this.#changed(change.pageId);
        const blockId = thread.anchor?.blockId;
        const extra = { ...(blockId === undefined ? {} : { blockId }), ...(posted === undefined ? {} : { commentId: posted.commentId }) };
        // One row for a comment: a reply that also mentions this account is a `CommentMention`, not a `ThreadReply` (the core's rule).
        if (commentBodyMentions(body).includes(this.#deps.sharing.meId())) {
          this.#raise("CommentMention", page.spaceId, change.pageId, member.accountId, member.name, extra);
        } else if (wroteIn) {
          this.#raise("ThreadReply", page.spaceId, change.pageId, member.accountId, member.name, extra);
        }
        return this.#info(thread, page.spaceId);
      }
      case "edit": {
        const { thread, comment } = this.#comment(change.pageId, change.commentId);
        // Void: another's comment, or a record that names a thread that is not its comment's own.
        if (comment.author !== member.accountId || !this.#ownThread(comment, change.threadId)) {
          return undefined;
        }
        const before = commentBodyMentions(comment.body);
        const body = this.#bodyOf(page.spaceId, change.body);
        this.#replaceBody(comment, body, at);
        this.#changed(change.pageId);
        const me = this.#deps.sharing.meId();
        if (commentBodyMentions(comment.body).includes(me) && !before.includes(me)) {
          const blockId = thread.anchor?.blockId;
          this.#raise("CommentMention", page.spaceId, change.pageId, member.accountId, member.name, {
            ...(blockId === undefined ? {} : { blockId }),
            commentId: comment.commentId,
          });
        }
        return this.#info(thread, page.spaceId);
      }
      case "delete": {
        const { thread, comment } = this.#comment(change.pageId, change.commentId);
        if ((comment.author !== member.accountId && !atLeast(member.role, "owner")) || !this.#ownThread(comment, change.threadId)) {
          return undefined;
        }
        this.#remove(comment);
        this.#changed(change.pageId);
        return this.#info(thread, page.spaceId);
      }
      case "resolve": {
        const thread = this.#thread(change.pageId, change.threadId);
        // Void: a record that targets a comment that is not the thread's root, or by someone the rules do not allow.
        const target = change.commentId === undefined ? thread.threadId : (canonicalId(change.commentId) ?? change.commentId);
        if (target !== thread.threadId || (this.#rootAuthor(thread) !== member.accountId && !atLeast(member.role, "editor"))) {
          return undefined;
        }
        this.#setResolved(thread, change.resolved, member.accountId);
        this.#changed(change.pageId);
        return this.#info(thread, page.spaceId);
      }
      case "personSet": {
        // Only an Editor or an Owner writes properties: the change of anyone else is ignored.
        if (!atLeast(member.role, "editor")) {
          return undefined;
        }
        const accounts = change.people.map((name) => {
          const person = this.#deps.sharing.memberByName(page.spaceId, name);
          if (person === undefined) {
            throw invalid("value", "a person is not a member of the space");
          }
          return person.accountId;
        });
        const me = this.#deps.sharing.meId();
        const before = this.#deps.personValue(change.pageId, change.propertyId);
        this.#deps.setPersonValue(change.pageId, change.propertyId, accounts);
        if (accounts.includes(me) && !before.includes(me)) {
          this.#raise("PersonAssigned", page.spaceId, change.pageId, member.accountId, member.name, {});
        }
        return undefined;
      }
    }
  }

  /**
   * A page document changed as another person's change (`MemoryTransport.changePage` with an actor): a mention of this
   * account that was not in the page before is a `PageMention`, one row for each block it newly appears in.
   */
  pageEdited(pageId: PageId, actor: string, before: readonly PagePersonMention[], after: readonly PagePersonMention[]): void {
    const page = this.#page(pageId);
    const member = this.#deps.sharing.memberByName(page.spaceId, actor);
    const me = this.#deps.sharing.meId();
    if (member === undefined || member.isMe) {
      return;
    }
    const seen = new Set<string>();
    for (const mention of after) {
      if (mention.accountId !== me || seen.has(mention.blockId)) {
        continue;
      }
      seen.add(mention.blockId);
      if (!before.some((old) => old.accountId === me && old.blockId === mention.blockId)) {
        this.#raise("PageMention", page.spaceId, pageId, member.accountId, member.name, { blockId: mention.blockId });
      }
    }
  }

  // ----- seeds ----------------------------------------------------------------------------------------------------

  /** Starts the threads and inbox rows of a seeded shared space, without events and without drawing from the id generator. */
  seed(spaceId: SpaceId, threads: readonly MemoryThreadSeed[] | undefined, rows: readonly MemoryNotificationSeed[] | undefined): void {
    this.#seeding = true;
    try {
      const now = this.#deps.now();
      for (const seed of threads ?? []) {
        this.#seedThread(spaceId, seed, now);
      }
      // Oldest first, so that the ids (which a listing orders by) follow the times.
      for (const row of [...(rows ?? [])].sort((a, b) => (b.minutesAgo ?? 30) - (a.minutesAgo ?? 30))) {
        this.#seedRow(spaceId, row, now);
      }
    } finally {
      this.#seeding = false;
    }
  }

  #seedThread(spaceId: SpaceId, seed: MemoryThreadSeed, now: number): void {
    const pageId = this.#deps.pageByTitle(spaceId, seed.page);
    if (pageId === undefined) {
      throw new CoreError({ code: "Internal", message: `MemoryTransport: the seeded thread's page "${seed.page}" does not exist` });
    }
    let anchor: CommentAnchor | undefined;
    if (seed.quote !== undefined) {
      const found = this.#deps.textAnchor(pageId, seed.quote);
      const quote = Array.from(seed.quote).slice(0, 200).join("");
      anchor = found === undefined ? { quote } : { blockId: found.blockId, start: found.start, end: found.end, quote };
    }
    const sharing = this.#deps.sharing;
    const total = seed.comments.length;
    let thread: ThreadRecord | undefined;
    seed.comments.forEach((entry, index) => {
      const author = entry.author === undefined ? { accountId: sharing.meId(), name: sharing.myName() } : this.#seedAuthor(spaceId, entry.author);
      const at = now - (entry.minutesAgo ?? (total - index) * 20 + 5) * MINUTE_MS;
      const body = this.#bodyOf(spaceId, entry.body);
      thread = this.#append(pageId, author.accountId, author.name, thread?.threadId, index === 0 ? anchor : undefined, body, at, entry);
    });
    if (thread !== undefined && seed.resolved !== undefined && seed.resolved !== false) {
      const by = seed.resolved === true ? sharing.meId() : this.#seedAuthor(spaceId, seed.resolved).accountId;
      this.#setResolved(thread, true, by);
    }
  }

  #seedAuthor(spaceId: SpaceId, name: string): { accountId: AccountId; name: string } {
    const member = this.#deps.sharing.memberByName(spaceId, name);
    if (member !== undefined) {
      return { accountId: member.accountId, name: member.name };
    }
    return { accountId: this.#deps.sharing.accountByName(spaceId, name), name: name.trim() };
  }

  #seedRow(spaceId: SpaceId, seed: MemoryNotificationSeed, now: number): void {
    const pageId = this.#deps.pageByTitle(spaceId, seed.page);
    if (pageId === undefined) {
      throw new CoreError({ code: "Internal", message: `MemoryTransport: the seeded notification's page "${seed.page}" does not exist` });
    }
    const actor = this.#seedAuthor(spaceId, seed.actor);
    const threads = this.#threads.get(pageId) ?? [];
    const thread = seed.threadQuote === undefined ? threads[0] : threads.find((candidate) => candidate.anchor?.quote === seed.threadQuote);
    const last = thread?.comments[thread.comments.length - 1];
    const blockId = thread?.anchor?.blockId;
    this.#lastRow += 1;
    this.#inbox.push({
      id: this.#lastRow,
      kind: seed.kind,
      spaceId,
      pageId,
      ...(blockId === undefined ? {} : { blockId }),
      ...((seed.kind === "ThreadReply" || seed.kind === "CommentMention") && last !== undefined ? { commentId: last.commentId } : {}),
      actor: actor.accountId,
      actorName: actor.name,
      at: now - (seed.minutesAgo ?? 30) * MINUTE_MS,
      read: seed.read === true,
    });
  }

  // ----- internals --------------------------------------------------------------------------------------------------

  #page(pageId: PageId): { spaceId: SpaceId; title: string } {
    const page = this.#deps.page(pageId);
    if (page === undefined) {
      throw notFound("page not found");
    }
    return page;
  }

  #findThread(pageId: PageId, threadId: string): ThreadRecord | undefined {
    const wanted = canonicalId(threadId) ?? threadId;
    return (this.#threads.get(pageId) ?? []).find((thread) => thread.threadId === wanted);
  }

  /**
   * A thread of the page (`NotFound` otherwise). One whose comments are all deleted is still a thread: it is only left out of
   * `CommentList`, and a reply or a resolve still reaches it.
   */
  #thread(pageId: PageId, threadId: string): ThreadRecord {
    const thread = this.#findThread(pageId, threadId);
    if (thread === undefined) {
      throw notFound("thread not found");
    }
    return thread;
  }

  /** Whether a comment with this id exists on the page (deleted ones too: the id is taken). */
  #hasComment(pageId: PageId, commentId: string): boolean {
    const wanted = canonicalId(commentId) ?? commentId;
    return (this.#threads.get(pageId) ?? []).some((thread) => thread.comments.some((comment) => comment.commentId === wanted));
  }

  /** Whether a record's `threadId`, when it names one, is the thread of the comment it targets. */
  #ownThread(comment: CommentRecord, threadId: string | undefined): boolean {
    return threadId === undefined || (canonicalId(threadId) ?? threadId) === comment.threadId;
  }

  /** A comment that was not deleted (`NotFound` otherwise: a delete is final). */
  #comment(pageId: PageId, commentId: string): { thread: ThreadRecord; comment: CommentRecord } {
    const wanted = canonicalId(commentId) ?? commentId;
    for (const thread of this.#threads.get(pageId) ?? []) {
      const comment = thread.comments.find((candidate) => candidate.commentId === wanted);
      if (comment !== undefined) {
        if (comment.deleted) {
          break;
        }
        return { thread, comment };
      }
    }
    throw notFound("comment not found");
  }

  #rootAuthor(thread: ThreadRecord): AccountId | undefined {
    return thread.comments.find((comment) => comment.commentId === thread.threadId)?.author ?? thread.comments[0]?.author;
  }

  /** Posts a comment into a page's stream (a new thread, or a reply to `threadId`); `InvalidInput` for the rules of a body and an anchor. */
  #append(
    pageId: PageId,
    author: AccountId,
    authorName: string,
    threadId: ThreadId | undefined,
    anchor: CommentAnchor | undefined,
    body: readonly CommentBodyItem[],
    at: number,
    seed: MemoryCommentSeed | undefined,
    claimedId?: string,
  ): ThreadRecord {
    const problem = commentBodyProblem(body);
    if (problem !== undefined) {
      throw invalid("body", problem);
    }
    if (threadId !== undefined && anchor !== undefined) {
      throw invalid("anchor", "only the first comment of a thread has an anchor");
    }
    if (anchor !== undefined) {
      const anchorProblem = commentAnchorProblem(anchor);
      if (anchorProblem !== undefined) {
        throw invalid("anchor", anchorProblem);
      }
    }
    let thread: ThreadRecord | undefined;
    if (threadId !== undefined) {
      thread = this.#findThread(pageId, threadId);
      if (thread === undefined) {
        throw invalid("threadId", "this is not a thread of the page");
      }
    }
    const commentId =
      claimedId !== undefined ? (canonicalId(claimedId) ?? claimedId) : this.#seeding ? this.#mint() : this.#deps.newId();
    if (thread === undefined) {
      thread = {
        threadId: commentId,
        pageId,
        ...(anchor === undefined ? {} : { anchor: copyAnchor(anchor) }),
        resolved: false,
        comments: [],
      };
      const threads = this.#threads.get(pageId) ?? [];
      threads.push(thread);
      this.#threads.set(pageId, threads);
    }
    // A reply is never older than its root: a record that claims an earlier time is clamped up to the root's.
    const root = thread.comments.find((candidate) => candidate.commentId === thread.threadId);
    const comment: CommentRecord = {
      commentId,
      threadId: thread.threadId,
      pageId,
      author,
      authorName,
      body: body.map((item) => ({ ...item })),
      createdAt: root === undefined ? at : Math.max(at, root.createdAt),
      deleted: seed?.deleted === true,
    };
    if (seed?.edited === true) {
      comment.editedAt = Math.min(this.#deps.now(), at + 5 * MINUTE_MS);
    }
    if (seed?.deleted === true) {
      comment.body = [];
    }
    thread.comments.push(comment);
    return thread;
  }

  #replaceBody(comment: CommentRecord, body: readonly CommentBodyItem[], at: number): void {
    const problem = commentBodyProblem(body);
    if (problem !== undefined) {
      throw invalid("body", problem);
    }
    comment.body = body.map((item) => ({ ...item }));
    comment.editedAt = at;
  }

  /** A delete is final: the body is gone for good. */
  #remove(comment: CommentRecord): void {
    comment.deleted = true;
    comment.body = [];
  }

  #setResolved(thread: ThreadRecord, resolved: boolean, by: AccountId): void {
    thread.resolved = resolved;
    if (resolved) {
      thread.resolvedBy = by;
    } else {
      delete thread.resolvedBy;
    }
  }

  /** A body as a simulated person or a seed gives it, resolved against the space: names to accounts, titles to pages. */
  #bodyOf(spaceId: SpaceId, body: MemoryCommentBody): CommentBodyItem[] {
    const items = typeof body === "string" ? [body] : body;
    return items.map((item): CommentBodyItem => {
      if (typeof item === "string") {
        return { type: "text", text: item };
      }
      if ("mention" in item) {
        return { type: "mention", accountId: this.#deps.sharing.accountByName(spaceId, item.mention) };
      }
      const id = this.#deps.pageByTitle(spaceId, item.page);
      if (id === undefined) {
        throw invalid("body", `a page titled "${item.page}" does not exist`);
      }
      return { type: "page", id };
    });
  }

  /** The thread as the UI reads it: names as the space says now, placeholders only where a later comment remains. */
  #info(thread: ThreadRecord, spaceId: SpaceId): ThreadInfo {
    const sharing = this.#deps.sharing;
    const me = sharing.meId();
    const ordered = [...thread.comments].sort((a, b) => {
      if ((a.commentId === thread.threadId) !== (b.commentId === thread.threadId)) {
        return a.commentId === thread.threadId ? -1 : 1;
      }
      return a.createdAt - b.createdAt || (a.commentId < b.commentId ? -1 : a.commentId > b.commentId ? 1 : 0);
    });
    const lastLive = ordered.reduce((found, comment, at) => (comment.deleted ? found : at), -1);
    const comments = ordered
      .filter((_, at) => at <= lastLive)
      .map((comment): CommentInfo => {
        const member = sharing.memberOf(spaceId, comment.author);
        if (member !== undefined) {
          comment.authorName = member.name;
        }
        return {
          commentId: comment.commentId,
          threadId: comment.threadId,
          pageId: comment.pageId,
          author: comment.author,
          authorName: comment.authorName,
          authorFormer: member === undefined,
          body: comment.deleted ? [] : comment.body.map((item) => ({ ...item })),
          createdAt: comment.createdAt,
          ...(comment.editedAt === undefined ? {} : { editedAt: comment.editedAt }),
          deleted: comment.deleted,
          isMine: comment.author === me,
        };
      });
    return {
      threadId: thread.threadId,
      pageId: thread.pageId,
      ...(thread.anchor === undefined ? {} : { anchor: copyAnchor(thread.anchor) }),
      resolved: thread.resolved,
      ...(thread.resolvedBy === undefined ? {} : { resolvedBy: thread.resolvedBy }),
      comments,
    };
  }

  #changed(pageId: PageId): void {
    this.#deps.emit({ type: "CommentsChanged", data: { pageId } });
  }

  /** Adds a row of the inbox and tells it (the toast), unless the core is not ready; the oldest read rows go past the limit. */
  #raise(
    kind: NotificationKind,
    spaceId: SpaceId,
    pageId: PageId,
    actor: AccountId,
    actorName: string,
    extra: { blockId?: string; commentId?: CommentId },
  ): void {
    this.#lastRow += 1;
    const row: NotificationRecord = {
      id: this.#lastRow,
      kind,
      spaceId,
      pageId,
      ...extra,
      actor,
      actorName,
      at: this.#deps.now(),
      read: false,
    };
    this.#inbox.push(row);
    while (this.#inbox.length > NOTIFICATIONS_MAX_ROWS) {
      const victim = this.#inbox.findIndex((candidate) => candidate.read && candidate !== row);
      this.#inbox.splice(victim >= 0 ? victim : 0, 1);
    }
    if (this.#deps.ready()) {
      this.#deps.emit({ type: "NotificationRaised", data: this.#notification(row) });
      this.#deps.emit({ type: "NotificationsChanged" });
    }
  }

  /** A row as listed: the actor's name as the space says now, the page's title read now (absent when the page is gone). */
  #notification(row: NotificationRecord): NotificationInfo {
    const member = this.#deps.sharing.memberOf(row.spaceId, row.actor);
    if (member !== undefined) {
      row.actorName = member.name;
    }
    const title = row.pageId === undefined ? undefined : this.#deps.page(row.pageId)?.title;
    return {
      id: row.id,
      kind: row.kind,
      spaceId: row.spaceId,
      ...(row.pageId === undefined ? {} : { pageId: row.pageId }),
      ...(row.blockId === undefined ? {} : { blockId: row.blockId }),
      ...(row.commentId === undefined ? {} : { commentId: row.commentId }),
      actor: row.actor,
      actorName: row.actorName,
      ...(title === undefined ? {} : { pageTitle: title }),
      at: row.at,
      read: row.read,
    };
  }

  /** An id for something a seed creates: not drawn from the transport's generator. */
  #mint(): string {
    this.#seedCounter += 1;
    return `00000000-0000-4000-8000-c${this.#seedCounter.toString(16).padStart(11, "0")}`;
  }
}

function copyAnchor(anchor: CommentAnchor): CommentAnchor {
  return {
    ...(anchor.blockId === undefined ? {} : { blockId: anchor.blockId }),
    ...(anchor.start === undefined ? {} : { start: anchor.start }),
    ...(anchor.end === undefined ? {} : { end: anchor.end }),
    quote: anchor.quote,
  };
}

/** Roots oldest first, then by thread id. */
function compareRoots(a: ThreadInfo, b: ThreadInfo): number {
  const first = a.comments[0]?.createdAt ?? 0;
  const second = b.comments[0]?.createdAt ?? 0;
  return first - second || (a.threadId < b.threadId ? -1 : a.threadId > b.threadId ? 1 : 0);
}
