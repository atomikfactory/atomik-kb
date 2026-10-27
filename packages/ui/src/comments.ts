// Pure helpers of the comment screens (Milestone 13, slice 2; ADR-0048 and ADR-0049): who may do what to a comment, how a
// typed comment becomes a body of text and mentions and back, and the names people show under (a member's current name,
// "Former member"). The store part is `store-comments.ts`; the hooks are in `comments-hooks.ts`.

import { toCoreError } from "@atomik-kb/core-client";
import type { AccountId, CommentBodyItem, CommentInfo, MemberRole, PersonInfo, ThreadInfo } from "@atomik-kb/core-client";

import { t } from "./i18n";
import type { SpaceAccess } from "./sharing";
import { roleLabel } from "./sharing";

/** What was asked of a comment: the words of a refusal depend on it. */
export type CommentAction = "post" | "edit" | "delete" | "resolve";

const ROLES: readonly string[] = ["owner", "editor", "commenter", "viewer"];

/**
 * What a refusal of the core says to a person, from the shape of its `Forbidden` (ADR-0048): `{reason: "author"}` (only the author
 * edits), `{reason: "role", required}` (the role the space asks for: Owner to delete another's comment, Editor to resolve
 * another's thread, Commenter to write) and `{reason: "archived"}` (a read-only copy). Anything else says what the core said.
 */
export function refusalText(error: unknown, action: CommentAction): string {
  const failure = toCoreError(error);
  const details = failure.details;
  if (failure.code !== "Forbidden" || typeof details !== "object" || details === null || Array.isArray(details)) {
    return failure.message;
  }
  const { reason, required } = details;
  if (reason === "author") {
    return t("comments.refused.author");
  }
  if (reason === "archived") {
    return t("comments.readOnly.archive");
  }
  if (reason === "role" && typeof required === "string" && ROLES.includes(required)) {
    if (action === "delete" && required === "owner") {
      return t("comments.refused.delete");
    }
    if (action === "resolve" && required === "editor") {
      return t("comments.refused.resolve");
    }
    return t("comments.refused.role", { role: roleLabel(required as MemberRole) });
  }
  return failure.message;
}

/** Whether this account may edit the comment: its author, who can still comment (not a Viewer, not in a read-only copy). */
export function canEditComment(access: SpaceAccess, comment: CommentInfo): boolean {
  return access.canComment && comment.isMine && !comment.deleted;
}

/** Whether this account may delete the comment: its author, or an Owner (a delete is final). */
export function canDeleteComment(access: SpaceAccess, comment: CommentInfo): boolean {
  return access.canComment && !comment.deleted && (comment.isMine || access.isOwner);
}

/** Whether this account may resolve or reopen the thread: the thread's author, or Editor and up. */
export function canResolveThread(access: SpaceAccess, thread: ThreadInfo): boolean {
  return access.canComment && (thread.comments[0]?.isMine === true || access.canEdit);
}

/** "Former member (Bob)": what a person who left is called, with the last name the space knew for them; without one, "Former member". */
export function formerMemberName(label: string): string {
  return label.trim() === "" ? t("comments.formerMember") : t("comments.formerMember.label", { label: label.trim() });
}

/** What a member is called now; "Former member" (with the name they had, when known) for an account the space does not list. */
export function personName(people: readonly PersonInfo[] | undefined, accountId: AccountId, label = ""): string {
  if (people === undefined) {
    // Not read yet: what the mention says rather than a flash of "Former member".
    return label;
  }
  const found = people.find((person) => person.accountId === accountId);
  if (found !== undefined) {
    return found.name;
  }
  return formerMemberName(label);
}

/** Whether the account is a person the space does not list (the name shows as "Former member"). */
export function isFormerMember(people: readonly PersonInfo[] | undefined, accountId: AccountId): boolean {
  return people !== undefined && !people.some((person) => person.accountId === accountId);
}

/** The name a comment's author shows under: "Former member (name)" once they left (the core keeps the last name), else the name the core gives. */
export function authorName(comment: CommentInfo): string {
  return comment.authorFormer ? formerMemberName(comment.authorName) : comment.authorName;
}

/** The initial of a name, for the round mark beside it. */
export function initialOf(name: string): string {
  const first = Array.from(name.trim())[0];
  return first === undefined ? "?" : first.toUpperCase();
}

const WORD = /[\p{L}\p{N}_]/u;

/**
 * The body of a typed comment: the text, in which each `@name` of a registered mention (the people picked from the `@`
 * menu while typing: name to account) becomes a mention item, the rest text items. The text is trimmed; an empty one is no
 * body. A mention whose name was edited after it was picked is just text.
 */
export function bodyFromText(text: string, mentions: ReadonlyMap<string, AccountId>): CommentBodyItem[] {
  const trimmed = text.trim();
  if (trimmed === "") {
    return [];
  }
  const labels = [...mentions.keys()].filter((label) => label !== "").sort((a, b) => b.length - a.length);
  const items: CommentBodyItem[] = [];
  let run = "";
  const flush = (): void => {
    if (run !== "") {
      items.push({ type: "text", text: run });
      run = "";
    }
  };
  let at = 0;
  while (at < trimmed.length) {
    const char = trimmed.charAt(at);
    const before = at === 0 ? " " : trimmed.charAt(at - 1);
    let matched: string | undefined;
    if (char === "@" && !WORD.test(before)) {
      matched = labels.find((label) => {
        if (!trimmed.startsWith(label, at + 1)) {
          return false;
        }
        const after = trimmed.charAt(at + 1 + label.length);
        return after === "" || !WORD.test(after);
      });
    }
    const accountId = matched === undefined ? undefined : mentions.get(matched);
    if (matched !== undefined && accountId !== undefined) {
      flush();
      items.push({ type: "mention", accountId });
      at += 1 + matched.length;
    } else {
      run += char;
      at += 1;
    }
  }
  flush();
  return items;
}

/**
 * A body as the composer edits it: the text with `@name` where a mention is, and the registry of those names. A page link
 * reads as its title in brackets (the composer writes none, an older client may).
 */
export function textFromBody(
  body: readonly CommentBodyItem[],
  nameOf: (accountId: AccountId) => string,
  titleOf: (pageId: string) => string = () => "",
): { text: string; mentions: Map<string, AccountId> } {
  const mentions = new Map<string, AccountId>();
  const text = body
    .map((item) => {
      switch (item.type) {
        case "text":
          return item.text;
        case "mention": {
          const name = nameOf(item.accountId);
          mentions.set(name, item.accountId);
          return `@${name}`;
        }
        case "page":
          return `[[${titleOf(item.id)}]]`;
      }
    })
    .join("");
  return { text, mentions };
}

/** A comment's words as plain text (a mention as `@name`), for the label of a button or a title. */
export function commentPlainText(body: readonly CommentBodyItem[], nameOf: (accountId: AccountId) => string): string {
  return textFromBody(body, nameOf).text;
}

/** The words after the last `@` that starts a mention at `caret` in `text` (the `@` menu's query), `null` when the caret is not in one. */
export function mentionQuery(text: string, caret: number): { start: number; query: string } | null {
  const upToCaret = text.slice(0, caret);
  const at = upToCaret.lastIndexOf("@");
  if (at < 0) {
    return null;
  }
  const before = at === 0 ? " " : upToCaret.charAt(at - 1);
  const query = upToCaret.slice(at + 1);
  if (WORD.test(before) || query.includes("\n") || query.length > 60 || /^\s/u.test(query)) {
    return null;
  }
  return { start: at, query };
}
