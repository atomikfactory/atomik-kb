// Comments, person mentions and notifications as the client sees them (Milestone 13, slice 2; ADR-0048 and ADR-0049,
// `docs/security/sharing.md` section 8): the limits of `kb_types::comments`, the pure rules of a comment's body and
// anchor (`kb-model`'s `validate_body` and `validate_anchor`), mirrored exactly so that a form can check what it is
// about to send, the plain text of a body, and the types of what the in-memory mirror (`./comments-mirror`) seeds
// and what simulated people do.
//
// Comments are signed records in a stream of their own, outside the page document: a Commenter comments without being
// able to write the page. The mirror keeps the folded result (threads, comments, who resolved) and enforces the fold's
// rules: Commenter and up post, only the author edits, the author or an Owner deletes (final), the thread's author or
// Editor and up resolves.

import type { AccountId } from "./generated/AccountId";
import type { CommentAnchor } from "./generated/CommentAnchor";
import type { CommentBodyItem } from "./generated/CommentBodyItem";
import type { NotificationKind } from "./generated/NotificationKind";
import type { PageId } from "./generated/PageId";
import { canonicalId } from "./page-doc";

/** The most a serialised comment body may weigh, in bytes (`COMMENT_BODY_MAX_BYTES`: 10 KB). */
export const COMMENT_BODY_MAX_BYTES = 10_240;
/** The most characters of an anchor's quote (`COMMENT_QUOTE_MAX_CHARS`). */
export const COMMENT_QUOTE_MAX_CHARS = 200;
/** Rows `NotificationList` answers when `limit` is left out (`NOTIFICATIONS_DEFAULT_LIMIT`). */
export const NOTIFICATIONS_DEFAULT_LIMIT = 50;
/** The most rows `NotificationList` answers (`NOTIFICATIONS_MAX_LIMIT`). */
export const NOTIFICATIONS_MAX_LIMIT = 200;
/** The most rows the device's inbox keeps (`NOTIFICATIONS_MAX_ROWS`): read rows go first, the newest never. */
export const NOTIFICATIONS_MAX_ROWS = 1_000;

const utf8 = new TextEncoder();

/** Whether `text` is non-empty standard base64 with correct padding (`kb-model`'s `is_base64`). */
export function isBase64(text: string): boolean {
  const body = text.replace(/=+$/u, "");
  const padding = text.length - body.length;
  return text.length > 0 && text.length % 4 === 0 && padding <= 2 && /^[A-Za-z0-9+/]*$/u.test(body);
}

/**
 * The message of the first rule a comment body breaks, or `undefined` (`validate_body`, `InvalidInput` for `body`): not
 * empty, every item is a text (not empty), a mention of an account or a page, at most {@link COMMENT_BODY_MAX_BYTES}
 * serialised.
 */
export function commentBodyProblem(body: readonly CommentBodyItem[]): string | undefined {
  if (!Array.isArray(body)) {
    return "the body is malformed";
  }
  if (body.length === 0) {
    return "a comment has a body";
  }
  for (const item of body as unknown[]) {
    if (typeof item !== "object" || item === null) {
      return "an item of the body is malformed";
    }
    const { type, text, accountId, id } = item as { type?: unknown; text?: unknown; accountId?: unknown; id?: unknown };
    if (type === "text") {
      if (typeof text !== "string") {
        return "an item of the body is malformed";
      }
      if (text === "") {
        return "a text item is empty";
      }
    } else if (type === "mention") {
      if (typeof accountId !== "string" || canonicalId(accountId) === undefined) {
        return "an item of the body is malformed";
      }
    } else if (type === "page") {
      if (typeof id !== "string" || canonicalId(id) === undefined) {
        return "an item of the body is malformed";
      }
    } else {
      return "an item of the body is malformed";
    }
  }
  if (utf8.encode(JSON.stringify(body)).length > COMMENT_BODY_MAX_BYTES) {
    return "the body is over 10 KB";
  }
  return undefined;
}

/**
 * The message of the first rule an anchor breaks, or `undefined` (`validate_anchor`, `InvalidInput` for `anchor`): the quote
 * has at most {@link COMMENT_QUOTE_MAX_CHARS} characters, and `start` and `end`, when present, are base64 text. What the
 * cursors mean is the editor's business, never the core's.
 */
export function commentAnchorProblem(anchor: CommentAnchor): string | undefined {
  if (typeof anchor.quote !== "string") {
    return "the anchor is malformed";
  }
  if (Array.from(anchor.quote).length > COMMENT_QUOTE_MAX_CHARS) {
    return "the quote is over 200 characters";
  }
  for (const cursor of [anchor.start, anchor.end]) {
    if (cursor !== undefined && (typeof cursor !== "string" || !isBase64(cursor))) {
      return "a cursor is not base64";
    }
  }
  if (anchor.blockId !== undefined && (typeof anchor.blockId !== "string" || canonicalId(anchor.blockId) === undefined)) {
    return "the block id is malformed";
  }
  return undefined;
}

/** A body as plain text: text as it is, a mention as `@name` and a page as `[[title]]` (the names and titles come from the caller). */
export function commentBodyText(
  body: readonly CommentBodyItem[],
  nameOf: (accountId: AccountId) => string | undefined,
  titleOf: (pageId: PageId) => string | undefined = () => undefined,
): string {
  return body
    .map((item) => {
      switch (item.type) {
        case "text":
          return item.text;
        case "mention":
          return `@${nameOf(item.accountId) ?? ""}`;
        case "page":
          return `[[${titleOf(item.id) ?? ""}]]`;
      }
    })
    .join("");
}

/** The accounts a body mentions, in order, once each. */
export function commentBodyMentions(body: readonly CommentBodyItem[]): AccountId[] {
  const found: AccountId[] = [];
  for (const item of body) {
    if (item.type === "mention" && !found.includes(item.accountId)) {
      found.push(item.accountId);
    }
  }
  return found;
}

// ----- the mirror's seeds and the hooks of simulated people --------------------------------------------------------

/**
 * A body as a seed or a simulated person gives it: a string is one text item, a list mixes texts, `{ mention: name }` (a
 * member of the space by name, "Maya" being this account's profile name) and `{ page: title }` (a page of the space by
 * title).
 */
export type MemoryCommentBody = string | readonly (string | { mention: string } | { page: string })[];

/** One comment of a seeded thread. */
export interface MemoryCommentSeed {
  /** The member who wrote it, by name; absent: this account. */
  author?: string;
  body: MemoryCommentBody;
  /** How long before the seed it was written, in minutes; default: an hour apart, the last one a few minutes ago. */
  minutesAgo?: number;
  /** It was edited since (its `editedAt` is shortly after). */
  edited?: boolean;
  /** It was deleted (kept as a placeholder when a later comment of the thread remains). */
  deleted?: boolean;
}

/** A thread of a seeded shared space. */
export interface MemoryThreadSeed {
  /** The title of a page of the space (the first one with it). */
  page: string;
  /**
   * The words the first comment is about: found in the seeded page's text, the anchor then holds real Loro cursors and the
   * block id (the editor highlights it); when the page has no such words, the thread is a page-level one that quotes them.
   * Absent: a comment on the page as a whole.
   */
  quote?: string;
  /** The thread was resolved: by whom (a member's name), or `true` for this account. */
  resolved?: boolean | string;
  /** The comments in order; the first is the root. */
  comments: readonly MemoryCommentSeed[];
}

/** A row of a seeded inbox. */
export interface MemoryNotificationSeed {
  kind: NotificationKind;
  /** The member who did it, by name. */
  actor: string;
  /** The title of the page it is about. */
  page: string;
  /** The first comment of this thread (by its quote, or the first thread of the page) for the two comment kinds. */
  threadQuote?: string;
  read?: boolean;
  minutesAgo?: number;
}

/**
 * What a simulated person does with comments, as it arrives through sync (`MemoryTransport.receiveComments`). The fold judges
 * them by the rules of ADR-0048 and ignores a record the rules do not allow (nothing changes, nothing is told, `undefined` is
 * answered), as `kb-model` does: a Viewer's record of any op, an edit of someone else's comment, a delete by neither the author
 * nor an Owner, a resolve by neither the thread's author nor an Editor and up, a second post that claims the id of a comment
 * that exists, an edit or delete whose `threadId` is not the thread of its comment, a resolve whose `commentId` is not the
 * thread's root. Notifications for this account come out of these changes, never out of its own writes.
 */
export type CommentChange =
  /** A member posts a new thread, or a reply when `threadId` is given. */
  | {
      type: "post";
      pageId: PageId;
      /** The member, by name. */
      author: string;
      threadId?: string;
      anchor?: CommentAnchor;
      body: MemoryCommentBody;
      /** The id the record claims for its comment (default: a new one); a second post that claims an existing one is ignored. */
      commentId?: string;
    }
  /** A member edits one of their comments; a `threadId` that is not the comment's own makes the record void. */
  | { type: "edit"; pageId: PageId; author: string; commentId: string; threadId?: string; body: MemoryCommentBody }
  /** A member deletes a comment (their own, or any as an Owner); a `threadId` that is not the comment's own makes it void. */
  | { type: "delete"; pageId: PageId; author: string; commentId: string; threadId?: string }
  /** A member resolves or reopens a thread (the thread's author, or an Editor and up); it targets the root comment. */
  | { type: "resolve"; pageId: PageId; author: string; threadId: string; resolved: boolean; commentId?: string }
  /** A member sets a Person property of a page to people (by name): this account newly listed is told. */
  | { type: "personSet"; pageId: PageId; author: string; propertyId: string; people: readonly string[] };
