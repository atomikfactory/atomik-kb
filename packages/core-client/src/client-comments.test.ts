// `CoreClient`'s comment, mention and notification requests (Milestone 13, slice 2 contract): what each
// method sends and what it returns, over a transport that answers with a canned response. What the in-memory mirror
// enforces (roles, threads, the inbox, the Person property) is in `memory-comments.test.ts`.

import { describe, expect, it } from "vitest";

import { CoreClient } from "./client";
import { CoreError } from "./errors";
import type { CommentBodyItem } from "./generated/CommentBodyItem";
import type { NotificationInfo } from "./generated/NotificationInfo";
import type { PersonInfo } from "./generated/PersonInfo";
import type { Request } from "./generated/Request";
import type { Response } from "./generated/Response";
import type { ThreadInfo } from "./generated/ThreadInfo";
import { PROPERTY_KINDS, MAX_PEOPLE } from "./properties";
import type { DocTransport, Transport } from "./transport";

/** Page documents are not used by these tests; touching them is a test bug. */
const noDocs: DocTransport = {
  open: () => Promise.reject(new Error("docs are not used here")),
  import: () => Promise.reject(new Error("docs are not used here")),
  export: () => Promise.reject(new Error("docs are not used here")),
  close: () => Promise.reject(new Error("docs are not used here")),
};

/** A transport that answers every request with one canned response and records the requests. */
function stubTransport(response: Response) {
  const requests: Request[] = [];
  const transport: Transport = {
    dispatch: (request) => {
      requests.push(request);
      return Promise.resolve(response);
    },
    subscribe: () => () => undefined,
    docs: noDocs,
  };
  return { requests, client: new CoreClient(transport) };
}

const SPACE = "11111111-1111-4111-8111-111111111111";
const PAGE = "22222222-2222-4222-8222-222222222222";
const ACCOUNT = "33333333-3333-4333-8333-333333333333";
const COMMENT = "44444444-4444-4444-8444-444444444444";
const BLOCK = "55555555-5555-4555-8555-555555555555";

const body: CommentBodyItem[] = [
  { type: "text", text: "Looks good, " },
  { type: "mention", accountId: ACCOUNT },
];

const thread: ThreadInfo = {
  threadId: COMMENT,
  pageId: PAGE,
  resolved: false,
  comments: [
    {
      commentId: COMMENT,
      threadId: COMMENT,
      pageId: PAGE,
      author: ACCOUNT,
      authorName: "Bob",
      authorFormer: false,
      body,
      createdAt: 1_700_000_000_000,
      deleted: false,
      isMine: false,
    },
  ],
};

describe("CoreClient comments", () => {
  it("lists the threads of a page and posts a new thread or a reply", async () => {
    const listed = stubTransport({ type: "Comments", data: [thread] });
    expect(await listed.client.commentList(PAGE)).toEqual([thread]);
    expect(listed.requests).toEqual([{ type: "CommentList", data: { pageId: PAGE } }]);

    const posted = stubTransport({ type: "Thread", data: thread });
    const anchor = { blockId: BLOCK, start: "AQID", end: "BAUG", quote: "the plan" };
    expect(await posted.client.commentPost({ pageId: PAGE, anchor, body })).toEqual(thread);
    expect(await posted.client.commentPost({ pageId: PAGE, threadId: COMMENT, body })).toEqual(thread);
    expect(posted.requests).toEqual([
      { type: "CommentPost", data: { pageId: PAGE, body, anchor } },
      { type: "CommentPost", data: { pageId: PAGE, body, threadId: COMMENT } },
    ]);
  });

  it("edits, deletes and resolves, and every answer is the thread", async () => {
    const { client, requests } = stubTransport({ type: "Thread", data: thread });
    expect(await client.commentEdit(PAGE, COMMENT, body)).toEqual(thread);
    expect(await client.commentDelete(PAGE, COMMENT)).toEqual(thread);
    expect(await client.commentResolve(PAGE, COMMENT, true)).toEqual(thread);
    expect(requests).toEqual([
      { type: "CommentEdit", data: { pageId: PAGE, commentId: COMMENT, body } },
      { type: "CommentDelete", data: { pageId: PAGE, commentId: COMMENT } },
      { type: "CommentResolve", data: { pageId: PAGE, threadId: COMMENT, resolved: true } },
    ]);
  });

  it("lists and marks notifications, sending only what is named", async () => {
    const row: NotificationInfo = {
      id: 7,
      kind: "ThreadReply",
      spaceId: SPACE,
      pageId: PAGE,
      actor: ACCOUNT,
      actorName: "Bob",
      at: 1_700_000_000_000,
      read: false,
    };
    const listed = stubTransport({ type: "Notifications", data: [row] });
    expect(await listed.client.notificationList()).toEqual([row]);
    expect(await listed.client.notificationList({ limit: 20, beforeId: 9, unreadOnly: true })).toEqual([row]);
    expect(listed.requests).toEqual([
      { type: "NotificationList", data: {} },
      { type: "NotificationList", data: { limit: 20, beforeId: 9, unreadOnly: true } },
    ]);

    const marked = stubTransport({ type: "Ok" });
    await marked.client.notificationsMarkRead();
    await marked.client.notificationsMarkRead([3, 4]);
    expect(marked.requests).toEqual([
      { type: "NotificationsMarkRead", data: {} },
      { type: "NotificationsMarkRead", data: { ids: [3, 4] } },
    ]);
  });

  it("asks for the people of a space", async () => {
    const person: PersonInfo = { accountId: ACCOUNT, name: "Bob", verified: false, isMe: false };
    const { client, requests } = stubTransport({ type: "People", data: [person] });
    expect(await client.peopleMentionable(SPACE)).toEqual([person]);
    expect(requests).toEqual([{ type: "PeopleMentionable", data: { spaceId: SPACE } }]);
  });

  it("surfaces a response of the wrong kind as Internal", async () => {
    const wrong = stubTransport({ type: "Ok" });
    const error = await wrong.client.commentList(PAGE).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(error).toBeInstanceOf(CoreError);
    expect((error as CoreError).code).toBe("Internal");
  });
});

describe("the Person property kind", () => {
  it("is listed after place, before relation, as the core lists it", () => {
    const at = PROPERTY_KINDS.indexOf("person");
    expect(at).toBeGreaterThan(-1);
    expect(PROPERTY_KINDS[at - 1]).toBe("place");
    expect(PROPERTY_KINDS[at + 1]).toBe("relation");
    expect(MAX_PEOPLE).toBe(20);
  });
});
