// Comments, person mentions and the Person property in the in-memory mirror (Milestone 13, slice 2, `./comments-mirror`): the
// rules of the comment stream's fold that the mirror enforces (Commenter and up post, only the author edits, the author or an
// Owner deletes and a delete is final, the thread's author or an Editor and up resolves), the inbox that only the other
// people raise, the people the `@` menu offers, the Person property with its filters, the text folds and the seeds.

import { Cursor, LoroDoc, LoroList, LoroMap } from "loro-crdt";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreClient } from "./client";
import { COMMENT_BODY_MAX_BYTES, COMMENT_QUOTE_MAX_CHARS, NOTIFICATIONS_MAX_ROWS } from "./comments";
import { CoreError } from "./errors";
import type { CommentBodyItem } from "./generated/CommentBodyItem";
import type { Event } from "./generated/Event";
import type { MemberRole } from "./generated/MemberRole";
import type { NotificationInfo } from "./generated/NotificationInfo";
import type { PersonInfo } from "./generated/PersonInfo";
import type { PropertyFilter } from "./generated/PropertyFilter";
import type { ThreadInfo } from "./generated/ThreadInfo";
import { fromBase64 } from "./backup";
import { MemoryTransport } from "./memory";
import type { MemoryTransportOptions } from "./memory";
import type { SeedSpace } from "./structure";
import type { MemoryThreadSeed } from "./comments";

const START = 1_700_000_000_000;
const SERVER = "https://sync.example.com";
const ANCHOR_CURSOR = "AQIDBAUGBwg=";

/** The clock moves only when a test says so, so that comments differ by a known time. */
const clock = { now: START };
const later = (ms = 1_000): void => {
  clock.now += ms;
};

const SPACES: readonly SeedSpace[] = [
  {
    name: "Work",
    properties: [{ name: "Owner", kind: "person" }],
    pages: [
      {
        title: "Plan",
        content: {
          type: "doc",
          content: [{ type: "paragraph", attrs: { id: "00000000-0000-4000-8000-000000000001" }, content: [{ type: "text", text: "Ship it on Friday" }] }],
        },
        properties: { Owner: ["Bob"] },
      },
      { title: "Notes" },
    ],
  },
  { name: "Personal", pages: [{ title: "Diary" }] },
];

/** Threads on the pages of `SPACES`: one anchored on words of Plan (three comments, the last this account's), a resolved page-level one, one on Notes. */
const THREADS: readonly MemoryThreadSeed[] = [
  {
    page: "Plan",
    quote: "Ship it",
    comments: [
      { author: "Carol", body: "Can we ship this on Friday?", minutesAgo: 180 },
      { author: "Bob", body: ["Friday works for me. ", { mention: "Maya" }, " can you confirm?"], minutesAgo: 120 },
      { body: "Confirmed: Friday.", minutesAgo: 90 },
    ],
  },
  {
    page: "Plan",
    resolved: "Bob",
    comments: [
      { author: "Carol", body: "The page needs an owner.", minutesAgo: 1_500 },
      { author: "Bob", body: "Added Maya.", minutesAgo: 1_400 },
    ],
  },
  { page: "Notes", comments: [{ author: "Bob", body: "Before Friday if you can.", minutesAgo: 50 }] },
];

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(START);
  clock.now = START;
});

afterEach(() => {
  vi.useRealTimers();
});

/** The people of the Work space by role: this account is Maya; the others are Alice (Owner, when this account is not), Bob (Editor), Carol (Commenter), Dan (Viewer). */
function build(role: MemberRole | "personal" = "owner", options: MemoryTransportOptions = {}) {
  const memory = new MemoryTransport({
    now: () => clock.now,
    workspaceName: "Home",
    spaces: SPACES,
    security: { kitCreatedAt: START },
    sync: { state: "idle", serverUrl: SERVER },
    syncStepMs: 5,
    sharing: {
      profileName: "Maya",
      ...(role === "personal" ? {} : { spaces: [{ space: "Work", role }] }),
    },
    ...options,
  });
  const client = new CoreClient(memory);
  const events: Event[] = [];
  memory.subscribe((event) => {
    events.push(event);
  });
  return { memory, client, events };
}

async function spaceId(client: CoreClient, name = "Work"): Promise<string> {
  const found = (await client.spaceList()).find((space) => space.name === name);
  if (found === undefined) {
    throw new Error(`no space ${name}`);
  }
  return found.id;
}

async function pageId(client: CoreClient, title: string, space = "Work"): Promise<string> {
  const tree = await client.treeGet(await spaceId(client, space));
  const found = tree.roots.find((node) => node.page.title === title);
  if (found === undefined) {
    throw new Error(`no page ${title}`);
  }
  return found.page.id;
}

async function accountOf(client: CoreClient, name: string, space = "Work"): Promise<string> {
  const person = (await client.peopleMentionable(await spaceId(client, space))).find((candidate) => candidate.name === name);
  if (person === undefined) {
    throw new Error(`no person ${name}`);
  }
  return person.accountId;
}

const text = (value: string): CommentBodyItem[] => [{ type: "text", text: value }];

async function failure(promise: Promise<unknown>): Promise<CoreError> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof CoreError) {
      return error;
    }
    throw error;
  }
  throw new Error("expected a rejection");
}

function bodyText(thread: ThreadInfo): string[] {
  return thread.comments.map((comment) => comment.body.map((item) => (item.type === "text" ? item.text : `<${item.type}>`)).join(""));
}

const names = (events: readonly Event[]): string[] => events.map((event) => event.type).filter((type) => !type.startsWith("Sync"));

describe("who may post", () => {
  for (const role of ["owner", "editor", "commenter"] as const) {
    it(`lets a ${role} start a thread and reply to it`, async () => {
      const { client } = build(role);
      const plan = await pageId(client, "Plan");
      const started = await client.commentPost({ pageId: plan, body: text("First") });
      expect(started.threadId).toBe(started.comments[0]?.commentId);
      expect(started).toMatchObject({ pageId: plan, resolved: false });
      later();
      const replied = await client.commentPost({ pageId: plan, threadId: started.threadId, body: text("Second") });
      expect(bodyText(replied)).toEqual(["First", "Second"]);
      expect(replied.comments.map((comment) => comment.isMine)).toEqual([true, true]);
      expect(replied.comments[0]).toMatchObject({ authorName: "Maya", authorFormer: false, deleted: false });
    });
  }

  it("refuses a Viewer, which still reads", async () => {
    const { client } = build("viewer");
    const plan = await pageId(client, "Plan");
    const error = await failure(client.commentPost({ pageId: plan, body: text("Hi") }));
    expect(error.code).toBe("Forbidden");
    expect(error.details).toEqual({ reason: "role", required: "commenter" });
    expect(await client.commentList(plan)).toEqual([]);
  });

  it("refuses anyone in a read-only archive, which still reads", async () => {
    const { client } = build("commenter", {
      sharing: { profileName: "Maya", spaces: [{ space: "Work", role: "commenter", archive: "Removed", comments: THREADS }] },
    });
    const plan = await pageId(client, "Plan");
    const error = await failure(client.commentPost({ pageId: plan, body: text("Hi") }));
    expect(error.details).toEqual({ reason: "archived" });
    expect((await client.commentList(plan)).length).toBeGreaterThan(0);
  });

  it("lets a personal space comment on its own", async () => {
    const { client } = build("personal");
    const plan = await pageId(client, "Plan");
    const thread = await client.commentPost({ pageId: plan, body: text("A note to myself") });
    expect(thread.comments[0]).toMatchObject({ authorName: "Maya", isMine: true });
    expect((await client.commentList(plan)).map((entry) => entry.threadId)).toEqual([thread.threadId]);
  });

  it("answers NotFound for a page that does not exist, before anything else", async () => {
    const { client } = build("viewer");
    const error = await failure(client.commentPost({ pageId: "00000000-0000-4000-8000-0000000000ff", body: text("Hi") }));
    expect(error.code).toBe("NotFound");
    expect((await failure(client.commentList("00000000-0000-4000-8000-0000000000ff"))).code).toBe("NotFound");
  });
});

describe("what a comment holds", () => {
  it("refuses an empty body, an empty text item, a bad item and a body over 10 KB, for the field body", async () => {
    const { client } = build();
    const plan = await pageId(client, "Plan");
    const big = text("x".repeat(COMMENT_BODY_MAX_BYTES));
    for (const body of [[], text(""), [{ type: "mention", accountId: "not an id" }], [{ type: "other" }], big] as unknown as CommentBodyItem[][]) {
      const error = await failure(client.commentPost({ pageId: plan, body }));
      expect(error).toMatchObject({ code: "InvalidInput", details: { field: "body" } });
    }
    expect(await client.commentList(plan)).toEqual([]);
    // Just under the limit is fine.
    const fits = await client.commentPost({ pageId: plan, body: text("x".repeat(COMMENT_BODY_MAX_BYTES - 40)) });
    expect(fits.comments).toHaveLength(1);
  });

  it("refuses a quote over 200 characters and a cursor that is not base64, for the field anchor", async () => {
    const { client } = build();
    const plan = await pageId(client, "Plan");
    const long = await failure(client.commentPost({ pageId: plan, anchor: { quote: "é".repeat(COMMENT_QUOTE_MAX_CHARS + 1) }, body: text("Hi") }));
    expect(long).toMatchObject({ code: "InvalidInput", details: { field: "anchor" } });
    const badCursor = await failure(
      client.commentPost({ pageId: plan, anchor: { quote: "ok", start: "not base64!", end: ANCHOR_CURSOR }, body: text("Hi") }),
    );
    expect(badCursor).toMatchObject({ code: "InvalidInput", details: { field: "anchor" } });
    const fits = await client.commentPost({
      pageId: plan,
      anchor: { blockId: "00000000-0000-4000-8000-000000000001", start: ANCHOR_CURSOR, end: ANCHOR_CURSOR, quote: "é".repeat(COMMENT_QUOTE_MAX_CHARS) },
      body: text("Hi"),
    });
    expect(fits.anchor?.blockId).toBe("00000000-0000-4000-8000-000000000001");
  });

  it("refuses an anchor on a reply and a thread that is not the page's, and keeps the anchor of the root", async () => {
    const { client } = build();
    const plan = await pageId(client, "Plan");
    const notes = await pageId(client, "Notes");
    const root = await client.commentPost({ pageId: plan, anchor: { quote: "Ship it" }, body: text("Why?") });
    expect(root.anchor).toEqual({ quote: "Ship it" });
    const withAnchor = await failure(client.commentPost({ pageId: plan, threadId: root.threadId, anchor: { quote: "no" }, body: text("Hi") }));
    expect(withAnchor).toMatchObject({ code: "InvalidInput", details: { field: "anchor" } });
    const other = await failure(client.commentPost({ pageId: notes, threadId: root.threadId, body: text("Hi") }));
    expect(other).toMatchObject({ code: "InvalidInput", details: { field: "threadId" } });
    const unknown = await failure(client.commentPost({ pageId: plan, threadId: "00000000-0000-4000-8000-0000000000aa", body: text("Hi") }));
    expect(unknown).toMatchObject({ code: "InvalidInput", details: { field: "threadId" } });
    const reply = await client.commentPost({ pageId: plan, threadId: root.threadId, body: text("Because") });
    expect(reply.anchor).toEqual({ quote: "Ship it" });
  });

  it("keeps mentions of people and links to pages in a body, and lists the threads oldest root first", async () => {
    const { client } = build();
    const plan = await pageId(client, "Plan");
    const notes = await pageId(client, "Notes");
    const bob = await accountOf(client, "Bob");
    const first = await client.commentPost({
      pageId: plan,
      body: [{ type: "text", text: "See " }, { type: "page", id: notes }, { type: "text", text: " " }, { type: "mention", accountId: bob }],
    });
    later();
    const second = await client.commentPost({ pageId: plan, body: text("Another thread") });
    const listed = await client.commentList(plan);
    expect(listed.map((entry) => entry.threadId)).toEqual([first.threadId, second.threadId]);
    expect(listed[0]?.comments[0]?.body).toEqual([
      { type: "text", text: "See " },
      { type: "page", id: notes },
      { type: "text", text: " " },
      { type: "mention", accountId: bob },
    ]);
  });
});

describe("who may edit", () => {
  it("lets only the author edit, an Owner too being refused", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    const mine = await client.commentPost({ pageId: plan, body: text("Mine") });
    later();
    const bobs = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Bob's" });
    const bobsComment = bobs?.comments[0]?.commentId ?? "";
    const refused = await failure(client.commentEdit(plan, bobsComment, text("Rewritten")));
    expect(refused.code).toBe("Forbidden");
    expect(refused.details).toEqual({ reason: "author" });
    later();
    const edited = await client.commentEdit(plan, mine.comments[0]?.commentId ?? "", text("Mine, edited"));
    expect(bodyText(edited)).toEqual(["Mine, edited"]);
    expect(edited.comments[0]?.editedAt).toBe(clock.now);
    expect((await client.commentList(plan))[1]?.comments[0]?.body).toEqual(text("Bob's"));
  });

  it("ignores the edit of another by a simulated person, as the fold does, and refuses a stranger or this account as the author", async () => {
    const { client, memory, events } = build("owner");
    const plan = await pageId(client, "Plan");
    const carols = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "Carol's" });
    const id = carols?.comments[0]?.commentId ?? "";
    events.length = 0;
    // Bob (an Editor) edits Carol's comment: no error, no change, nothing told.
    expect(memory.receiveComments({ type: "edit", pageId: plan, author: "Bob", commentId: id, body: "Bob edits" })).toBeUndefined();
    expect(events).toEqual([]);
    expect(bodyText((await client.commentList(plan))[0] as ThreadInfo)).toEqual(["Carol's"]);
    expect(() => memory.receiveComments({ type: "post", pageId: plan, author: "Zed", body: "A stranger" })).toThrow(
      expect.objectContaining({ code: "InvalidInput", details: { field: "author" } }) as Error,
    );
    expect(() => memory.receiveComments({ type: "post", pageId: plan, author: "Maya", body: "Me" })).toThrow(
      expect.objectContaining({ code: "InvalidInput", details: { field: "author" } }) as Error,
    );
    expect((await client.commentList(plan))[0]?.comments).toHaveLength(1);
  });

  it("answers NotFound for an unknown comment and for the edit of one that was deleted", async () => {
    const { client } = build("commenter");
    const plan = await pageId(client, "Plan");
    expect((await failure(client.commentEdit(plan, "00000000-0000-4000-8000-0000000000bb", text("x")))).code).toBe("NotFound");
    const thread = await client.commentPost({ pageId: plan, body: text("Mine") });
    const id = thread.comments[0]?.commentId ?? "";
    await client.commentDelete(plan, id);
    expect((await failure(client.commentEdit(plan, id, text("x")))).code).toBe("NotFound");
  });

  it("checks the new body", async () => {
    const { client } = build("commenter");
    const plan = await pageId(client, "Plan");
    const thread = await client.commentPost({ pageId: plan, body: text("Mine") });
    const error = await failure(client.commentEdit(plan, thread.comments[0]?.commentId ?? "", []));
    expect(error).toMatchObject({ code: "InvalidInput", details: { field: "body" } });
  });

  it("refuses a Viewer who was demoted: its later records are refused", async () => {
    const { client, memory } = build("commenter");
    const plan = await pageId(client, "Plan");
    const thread = await client.commentPost({ pageId: plan, body: text("Written as a Commenter") });
    const me = (await client.spaceMembers(await spaceId(client))).members.find((member) => member.isMe);
    memory.receiveSharing({ type: "roleSet", spaceId: await spaceId(client), accountId: me?.accountId ?? "", role: "viewer" });
    expect((await failure(client.commentEdit(plan, thread.comments[0]?.commentId ?? "", text("x")))).details).toEqual({
      reason: "role",
      required: "commenter",
    });
    // What was written while the role allowed it stays.
    expect(bodyText((await client.commentList(plan))[0] as ThreadInfo)).toEqual(["Written as a Commenter"]);
  });
});

describe("who may delete, and what a delete leaves", () => {
  it("lets the author delete, an Owner delete anyone's, and an Editor or Commenter only their own", async () => {
    const owner = build("owner");
    const plan = await pageId(owner.client, "Plan");
    const carols = owner.memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "Carol's" });
    const ownerDelete = await owner.client.commentDelete(plan, carols?.comments[0]?.commentId ?? "");
    expect(ownerDelete.comments).toEqual([]);

    for (const role of ["editor", "commenter"] as const) {
      const { client, memory } = build(role);
      const page = await pageId(client, "Plan");
      const others = memory.receiveComments({ type: "post", pageId: page, author: "Carol", body: "Carol's" });
      const error = await failure(client.commentDelete(page, others?.comments[0]?.commentId ?? ""));
      expect(error.code).toBe("Forbidden");
      // The core asks for the Owner role to delete another's comment.
      expect(error.details).toEqual({ reason: "role", required: "owner" });
      const mine = await client.commentPost({ pageId: page, body: text("Mine") });
      expect((await client.commentDelete(page, mine.comments[0]?.commentId ?? "")).comments).toEqual([]);
    }
  });

  it("keeps a deleted comment as a placeholder while a later comment remains, and drops it after", async () => {
    const { client } = build("owner");
    const plan = await pageId(client, "Plan");
    const root = await client.commentPost({ pageId: plan, body: text("Root") });
    later();
    const reply = await client.commentPost({ pageId: plan, threadId: root.threadId, body: text("Reply") });
    later();
    const third = await client.commentPost({ pageId: plan, threadId: root.threadId, body: text("Third") });
    const [rootId, replyId, thirdId] = third.comments.map((comment) => comment.commentId);
    expect(reply.comments).toHaveLength(2);
    // Deleting the root leaves a placeholder: no body, kept for the replies after it.
    const afterRoot = await client.commentDelete(plan, rootId ?? "");
    expect(afterRoot.comments.map((comment) => [comment.deleted, comment.body.length])).toEqual([
      [true, 0],
      [false, 1],
      [false, 1],
    ]);
    // Deleting the last one drops it; the root's placeholder stays while a later live comment, the reply, remains.
    const afterThird = await client.commentDelete(plan, thirdId ?? "");
    expect(afterThird.comments.map((comment) => comment.commentId)).toEqual([rootId, replyId]);
    const afterReply = await client.commentDelete(plan, replyId ?? "");
    expect(afterReply.comments).toEqual([]);
    // A thread with nothing left is not listed, but it is still a thread: a deleted root does not freeze it.
    expect(await client.commentList(plan)).toEqual([]);
    later();
    const late = await client.commentPost({ pageId: plan, threadId: root.threadId, body: text("Late") });
    // Every deleted comment before the new one is a placeholder: it has a reply after it.
    expect(late.comments.map((comment) => [comment.deleted, comment.body.length])).toEqual([
      [true, 0],
      [true, 0],
      [true, 0],
      [false, 1],
    ]);
    expect((await client.commentList(plan)).map((entry) => entry.threadId)).toEqual([root.threadId]);
  });

  it("is final: the comment cannot be edited, deleted again or brought back", async () => {
    const { client } = build("owner");
    const plan = await pageId(client, "Plan");
    const root = await client.commentPost({ pageId: plan, body: text("Root") });
    later();
    await client.commentPost({ pageId: plan, threadId: root.threadId, body: text("Reply") });
    const id = root.comments[0]?.commentId ?? "";
    await client.commentDelete(plan, id);
    expect((await failure(client.commentEdit(plan, id, text("Back")))).code).toBe("NotFound");
    expect((await failure(client.commentDelete(plan, id))).code).toBe("NotFound");
    const listed = (await client.commentList(plan))[0];
    expect(listed?.comments[0]).toMatchObject({ commentId: id, deleted: true, body: [] });
  });
});

describe("who may resolve", () => {
  it("lets the thread's author, and Editor and up, resolve and reopen; nobody else", async () => {
    const { client, memory } = build("commenter");
    const plan = await pageId(client, "Plan");
    const mine = await client.commentPost({ pageId: plan, body: text("Mine") });
    const resolved = await client.commentResolve(plan, mine.threadId, true);
    const me = (await client.spaceMembers(await spaceId(client))).members.find((member) => member.isMe);
    expect(resolved).toMatchObject({ resolved: true, resolvedBy: me?.accountId });
    const reopened = await client.commentResolve(plan, mine.threadId, false);
    expect(reopened.resolved).toBe(false);
    expect(reopened.resolvedBy).toBeUndefined();

    const bobs = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Bob's thread" });
    const error = await failure(client.commentResolve(plan, bobs?.threadId ?? "", true));
    expect(error.code).toBe("Forbidden");
    // The core asks for the Editor role to resolve another's thread.
    expect(error.details).toEqual({ reason: "role", required: "editor" });
    // An Editor of the people resolves a Commenter's thread.
    const byBob = memory.receiveComments({ type: "resolve", pageId: plan, author: "Bob", threadId: mine.threadId, resolved: true });
    expect(byBob).toMatchObject({ resolved: true });
    // A Commenter who did not start it cannot: the fold ignores the record.
    expect(memory.receiveComments({ type: "resolve", pageId: plan, author: "Carol", threadId: bobs?.threadId ?? "", resolved: true })).toBeUndefined();
    expect((await client.commentList(plan)).find((entry) => entry.threadId === bobs?.threadId)?.resolved).toBe(false);
  });

  it("lets an Editor and an Owner resolve a thread of someone else", async () => {
    for (const role of ["editor", "owner"] as const) {
      const { client, memory } = build(role);
      const plan = await pageId(client, "Plan");
      const carols = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "Carol's thread" });
      expect(await client.commentResolve(plan, carols?.threadId ?? "", true)).toMatchObject({ resolved: true });
    }
  });

  it("refuses a Viewer, and answers NotFound for a thread that is not there", async () => {
    const { client, memory } = build("viewer");
    const plan = await pageId(client, "Plan");
    const carols = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "Carol's thread" });
    expect((await failure(client.commentResolve(plan, carols?.threadId ?? "", true))).details).toEqual({ reason: "role", required: "commenter" });
    const editor = build("editor");
    const page = await pageId(editor.client, "Plan");
    expect((await failure(editor.client.commentResolve(page, "00000000-0000-4000-8000-0000000000cc", true))).code).toBe("NotFound");
  });
});

describe("the decisions of the fold for the records of the other people", () => {
  it("ignores a record of a Viewer of any op, and what was written while the role allowed stays", async () => {
    const { client, memory, events } = build("owner");
    const plan = await pageId(client, "Plan");
    const space = await spaceId(client);
    const bobs = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Written as an Editor" });
    const [rootId, threadId] = [bobs?.comments[0]?.commentId ?? "", bobs?.threadId ?? ""];
    memory.receiveSharing({ type: "roleSet", spaceId: space, accountId: await accountOf(client, "Bob"), role: "viewer" });
    events.length = 0;
    for (const change of [
      { type: "post", pageId: plan, author: "Bob", body: "A Viewer posts" },
      { type: "post", pageId: plan, author: "Dan", threadId, body: "A Viewer replies" },
      { type: "edit", pageId: plan, author: "Bob", commentId: rootId, body: "A Viewer edits" },
      { type: "delete", pageId: plan, author: "Bob", commentId: rootId },
      { type: "resolve", pageId: plan, author: "Bob", threadId, resolved: true },
    ] as const) {
      expect(memory.receiveComments(change)).toBeUndefined();
    }
    expect(events).toEqual([]);
    const [thread] = await client.commentList(plan);
    expect(bodyText(thread as ThreadInfo)).toEqual(["Written as an Editor"]);
    expect(thread?.resolved).toBe(false);
  });

  it("lets a thread go on after its root was deleted: the replies stay, and new replies and a resolve are allowed", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    const mine = await client.commentPost({ pageId: plan, body: text("My root") });
    later();
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", threadId: mine.threadId, body: "A reply" });
    await client.commentDelete(plan, mine.comments[0]?.commentId ?? "");
    later();
    const answered = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: mine.threadId, body: "Another reply" });
    expect(answered?.comments.map((comment) => [comment.deleted, comment.body.length])).toEqual([
      [true, 0],
      [false, 1],
      [false, 1],
    ]);
    // The root's author still resolves it, and so does an Editor.
    expect((await client.commentResolve(plan, mine.threadId, true)).resolved).toBe(true);
    expect(memory.receiveComments({ type: "resolve", pageId: plan, author: "Bob", threadId: mine.threadId, resolved: false })?.resolved).toBe(false);
  });

  it("clamps the time of a reply up to its root's", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    clock.now = START + 60_000;
    const root = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Root" });
    // A reply that claims to be older than its root, by a device clock that went back.
    clock.now = START;
    const reply = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: root?.threadId, body: "Backdated" });
    expect(reply?.comments.map((comment) => comment.createdAt)).toEqual([START + 60_000, START + 60_000]);
    expect(bodyText(reply as ThreadInfo)).toEqual(["Root", "Backdated"]);
    // A thread of its own is not clamped to anything.
    expect((await client.commentPost({ pageId: plan, body: text("Later thread") })).comments[0]?.createdAt).toBe(START);
  });

  it("refuses a second post that claims the id of a comment that exists", async () => {
    const { client, memory, events } = build("owner");
    const plan = await pageId(client, "Plan");
    const claimed = "00000000-0000-4000-8000-00000000c0de";
    const first = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "The first", commentId: claimed });
    expect(first?.threadId).toBe(claimed);
    events.length = 0;
    expect(memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "A copy", commentId: claimed })).toBeUndefined();
    expect(
      memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: claimed, body: "A reply that reuses the id", commentId: claimed }),
    ).toBeUndefined();
    expect(events).toEqual([]);
    const [thread] = await client.commentList(plan);
    expect(bodyText(thread as ThreadInfo)).toEqual(["The first"]);
    expect(thread?.comments[0]?.authorName).toBe("Bob");
    // The id of a deleted comment is taken too.
    await client.commentDelete(plan, claimed);
    expect(memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "After the delete", commentId: claimed })).toBeUndefined();
  });

  it("ignores an edit or a delete that names another thread than its comment's, and a resolve that does not target the root", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    const root = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Root" });
    later();
    const withReply = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: root?.threadId, body: "Reply" });
    const [rootId, replyId] = withReply?.comments.map((comment) => comment.commentId) ?? [];
    const other = "00000000-0000-4000-8000-0000000000ab";
    expect(memory.receiveComments({ type: "edit", pageId: plan, author: "Carol", commentId: replyId ?? "", threadId: other, body: "Edited" })).toBeUndefined();
    expect(memory.receiveComments({ type: "delete", pageId: plan, author: "Carol", commentId: replyId ?? "", threadId: other })).toBeUndefined();
    // A resolve that targets the reply, not the root.
    expect(
      memory.receiveComments({ type: "resolve", pageId: plan, author: "Bob", threadId: root?.threadId ?? "", resolved: true, commentId: replyId }),
    ).toBeUndefined();
    expect(bodyText((await client.commentList(plan))[0] as ThreadInfo)).toEqual(["Root", "Reply"]);
    expect((await client.commentList(plan))[0]?.resolved).toBe(false);
    // The same records that name their own thread and target the root count.
    const edited = memory.receiveComments({ type: "edit", pageId: plan, author: "Carol", commentId: replyId ?? "", threadId: root?.threadId, body: "Edited" });
    expect(bodyText(edited as ThreadInfo)).toEqual(["Root", "Edited"]);
    expect(
      memory.receiveComments({ type: "resolve", pageId: plan, author: "Bob", threadId: root?.threadId ?? "", resolved: true, commentId: rootId })?.resolved,
    ).toBe(true);
    expect(memory.receiveComments({ type: "delete", pageId: plan, author: "Carol", commentId: replyId ?? "", threadId: root?.threadId })?.comments).toHaveLength(1);
  });
});

describe("events and sync", () => {
  it("emits CommentsChanged for every write, one pending entry for each of this account's, none for another person's", async () => {
    const { client, memory, events } = build("commenter");
    const plan = await pageId(client, "Plan");
    const before = (await client.syncStatus()).pendingEntries;
    const thread = await client.commentPost({ pageId: plan, body: text("One") });
    await client.commentEdit(plan, thread.comments[0]?.commentId ?? "", text("Two"));
    await client.commentResolve(plan, thread.threadId, true);
    await client.commentDelete(plan, thread.comments[0]?.commentId ?? "");
    expect(events.filter((event) => event.type === "CommentsChanged")).toHaveLength(4);
    expect((await client.syncStatus()).pendingEntries - before).toBe(4);
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Bob" });
    expect(events.filter((event) => event.type === "CommentsChanged")).toHaveLength(5);
    expect((await client.syncStatus()).pendingEntries - before).toBe(4);
    expect(events.at(-1)).toEqual({ type: "CommentsChanged", data: { pageId: plan } });
  });

  it("emits nothing for a refused write", async () => {
    const { client, events } = build("viewer");
    const plan = await pageId(client, "Plan");
    await failure(client.commentPost({ pageId: plan, body: text("No") }));
    expect(names(events)).toEqual([]);
  });
});

describe("former members", () => {
  it("keeps the comments of a removed person, as a former member with the last known name", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    const space = await spaceId(client);
    const bob = await accountOf(client, "Bob");
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Before I go" });
    await client.memberRemove(space, bob);
    const [thread] = await client.commentList(plan);
    expect(thread?.comments[0]).toMatchObject({ author: bob, authorName: "Bob", authorFormer: true, deleted: false });
    expect(thread?.comments[0]?.body).toEqual(text("Before I go"));
    // An Owner still deletes it, a Person property still lists the account.
    expect((await client.commentDelete(plan, thread?.comments[0]?.commentId ?? "")).comments).toEqual([]);
    expect(await client.pageGet(plan).then((page) => page.properties?.find(([, value]) => value.kind === "person")?.[1])).toEqual({
      kind: "person",
      value: [bob],
    });
  });

  it("lists a removed person and one who left in `formerMembers` with the last name, in account order, and not one who came back", async () => {
    const { client, memory } = build("owner");
    const space = await spaceId(client);
    expect((await client.spaceMembers(space)).formerMembers).toEqual([]);
    const bob = await accountOf(client, "Bob");
    const carol = await accountOf(client, "Carol");
    memory.receiveSharing({ type: "renamed", spaceId: space, accountId: bob, name: "Robert" });
    await client.memberRemove(space, bob);
    memory.receiveSharing({ type: "memberLeft", spaceId: space, accountId: carol });
    const info = await client.spaceMembers(space);
    // The last name the space knew them by: Bob had been renamed Robert; Carol left under her name.
    expect(info.formerMembers).toEqual([bob, carol].sort().map((accountId) => ({ accountId, name: accountId === bob ? "Robert" : "Carol" })));
    expect(info.members.map((member) => member.accountId)).not.toContain(bob);
    // A person who joins again is a member, not a former one.
    const again = memory.receiveSharing({ type: "memberJoined", spaceId: space, name: "Carol" });
    expect((await client.spaceMembers(space)).formerMembers.map((former) => former.name)).toEqual(["Robert"]);
    expect(again).toBe(carol);
  });

  it("shows the name a member has now", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    const space = await spaceId(client);
    const bob = await accountOf(client, "Bob");
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Hi" });
    memory.receiveSharing({ type: "renamed", spaceId: space, accountId: bob, name: "Robert" });
    expect((await client.commentList(plan))[0]?.comments[0]).toMatchObject({ authorName: "Robert", authorFormer: false });
  });
});

describe("the people the @ menu offers", () => {
  it("lists the members of a shared space in join order, this account among them", async () => {
    const { client } = build("editor");
    const people = await client.peopleMentionable(await spaceId(client));
    expect(people.map((person) => person.name)).toEqual(["Alice", "Maya", "Bob", "Carol", "Dan"]);
    expect(people.find((person) => person.isMe)).toMatchObject({ name: "Maya", isMe: true });
    expect(people.find((person) => person.name === "Bob")).toMatchObject({ isMe: false, verified: true });
    expect(people.find((person) => person.name === "Bob")?.safetyNumber).toMatch(/^(\d{5} ){5}\d{5}$/u);
    expect(people.find((person) => person.isMe)?.safetyNumber).toBeUndefined();
  });

  it("lists this account alone in a personal space", async () => {
    const { client } = build("personal");
    const people: PersonInfo[] = await client.peopleMentionable(await spaceId(client, "Personal"));
    expect(people).toHaveLength(1);
    expect(people[0]).toMatchObject({ name: "Maya", isMe: true });
    expect((await client.peopleMentionable(await spaceId(client))).map((person) => person.name)).toEqual(["Maya"]);
  });

  it("follows a person who joins, leaves or is renamed, and answers NotFound for an unknown space", async () => {
    const { client, memory } = build("owner");
    const space = await spaceId(client);
    memory.receiveSharing({ type: "memberJoined", spaceId: space, name: "Erin", role: "commenter" });
    const bob = await accountOf(client, "Bob");
    memory.receiveSharing({ type: "memberLeft", spaceId: space, accountId: bob });
    memory.receiveSharing({ type: "renamed", spaceId: space, accountId: await accountOf(client, "Dan"), name: "Daniel" });
    expect((await client.peopleMentionable(space)).map((person) => person.name)).toEqual(["Maya", "Carol", "Daniel", "Erin"]);
    expect((await failure(client.peopleMentionable("00000000-0000-4000-8000-0000000000dd"))).code).toBe("NotFound");
  });
});

describe("notifications", () => {
  /** Posts as Maya in a thread of Plan that nobody else wrote in, and answers the page, the thread and Maya's account. */
  async function setup(role: MemberRole = "owner") {
    const built = build(role);
    const plan = await pageId(built.client, "Plan");
    const me = (await built.client.peopleMentionable(await spaceId(built.client))).find((person) => person.isMe)?.accountId ?? "";
    return { ...built, plan, me };
  }

  it("never raises one for this account's own writes", async () => {
    const { client, plan, me, events } = await setup();
    const thread = await client.commentPost({ pageId: plan, body: [{ type: "mention", accountId: me }] });
    await client.commentPost({ pageId: plan, threadId: thread.threadId, body: [{ type: "mention", accountId: me }] });
    await client.pagePropertySet(plan, (await client.propertySchemaGet(await spaceId(client))).find((def) => def.kind === "person")?.id ?? "", {
      kind: "person",
      value: [me],
    });
    expect(await client.notificationList()).toEqual([]);
    expect(events.some((event) => event.type === "NotificationRaised" || event.type === "NotificationsChanged")).toBe(false);
  });

  it("raises ThreadReply for a reply of another person in a thread this account wrote in", async () => {
    const { client, memory, plan, events } = await setup();
    const mine = await client.commentPost({ pageId: plan, anchor: { blockId: "00000000-0000-4000-8000-000000000001", quote: "Ship it" }, body: text("Mine") });
    later();
    const reply = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", threadId: mine.threadId, body: "A reply" });
    const bob = await accountOf(client, "Bob");
    const [row] = await client.notificationList();
    expect(row).toMatchObject({
      kind: "ThreadReply",
      pageId: plan,
      blockId: "00000000-0000-4000-8000-000000000001",
      commentId: reply?.comments[1]?.commentId,
      actor: bob,
      actorName: "Bob",
      pageTitle: "Plan",
      read: false,
      at: clock.now,
    });
    expect(row?.spaceId).toBe(await spaceId(client));
    // The toast carries the same row, then the counts change.
    const raised = events.filter((event) => event.type === "NotificationRaised" || event.type === "NotificationsChanged");
    expect(raised).toEqual([{ type: "NotificationRaised", data: row }, { type: "NotificationsChanged" }]);
    // The row names the page and the actor, never the words of the comment.
    expect(JSON.stringify(row)).not.toContain("A reply");
  });

  it("raises nothing for a reply in a thread this account did not write in, or for its own thread's deleted comments", async () => {
    const { client, memory, plan } = await setup();
    const bobs = memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: "Bob's thread" });
    memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: bobs?.threadId, body: "Carol replies to Bob" });
    expect(await client.notificationList()).toEqual([]);
    const mine = await client.commentPost({ pageId: plan, body: text("Mine") });
    await client.commentDelete(plan, mine.comments[0]?.commentId ?? "");
    expect(await client.notificationList()).toEqual([]);
  });

  it("raises CommentMention for a post that mentions this account, and for an edit that adds the mention only", async () => {
    const { client, memory, plan, me } = await setup();
    const posted = memory.receiveComments({
      type: "post",
      pageId: plan,
      author: "Bob",
      body: ["Over to you, ", { mention: "Maya" }],
    });
    expect((await client.notificationList()).map((row) => row.kind)).toEqual(["CommentMention"]);
    expect(posted?.comments[0]?.body).toContainEqual({ type: "mention", accountId: me });
    const id = posted?.comments[0]?.commentId ?? "";
    // An edit that keeps the mention is not news; one that adds it to a comment without is.
    memory.receiveComments({ type: "edit", pageId: plan, author: "Bob", commentId: id, body: ["Still ", { mention: "Maya" }] });
    expect(await client.notificationList()).toHaveLength(1);
    const plain = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: "No mention yet" });
    memory.receiveComments({
      type: "edit",
      pageId: plan,
      author: "Carol",
      commentId: plain?.comments[0]?.commentId ?? "",
      body: [{ mention: "Maya" }, " after all"],
    });
    expect((await client.notificationList()).map((row) => row.kind)).toEqual(["CommentMention", "CommentMention"]);
  });

  it("raises one CommentMention, not a ThreadReply too, for a reply that mentions this account in its own thread", async () => {
    const { client, memory, plan, events } = await setup();
    const mine = await client.commentPost({ pageId: plan, body: text("Mine") });
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", threadId: mine.threadId, body: [{ mention: "Maya" }, " yes"] });
    expect((await client.notificationList()).map((row) => row.kind)).toEqual(["CommentMention"]);
    expect(events.filter((event) => event.type === "NotificationRaised")).toHaveLength(1);
    // A reply that does not mention it is still a reply.
    memory.receiveComments({ type: "post", pageId: plan, author: "Carol", threadId: mine.threadId, body: "Agreed" });
    expect((await client.notificationList()).map((row) => row.kind)).toEqual(["ThreadReply", "CommentMention"]);
  });

  it("raises PageMention for a change of another person that adds a mention of this account to a page", async () => {
    const { client, memory, plan, me } = await setup();
    const mention = (doc: LoroDoc): void => {
      const children = doc.getMap("doc").get("children") as LoroList;
      const paragraph = children.get(0) as LoroMap;
      const inline = paragraph.get("children") as LoroList;
      const node = inline.insertContainer(inline.length, new LoroMap());
      node.set("nodeName", "personMention");
      const attributes = node.setContainer("attributes", new LoroMap());
      attributes.set("accountId", me);
      attributes.set("label", "Maya");
      node.setContainer("children", new LoroList());
    };
    // Without an actor, a remote change notifies nobody.
    memory.changePage(await pageId(client, "Notes"), mention);
    expect(await client.notificationList()).toEqual([]);
    memory.changePage(plan, mention, "Remote", "Bob");
    const rows = await client.notificationList();
    expect(rows.map((row) => row.kind)).toEqual(["PageMention"]);
    expect(rows[0]).toMatchObject({ pageId: plan, blockId: "00000000-0000-4000-8000-000000000001", actorName: "Bob" });
    // The block already held a mention (two now): the second change adds none.
    memory.changePage(plan, () => undefined, "Remote", "Bob");
    expect(await client.notificationList()).toHaveLength(1);
    // The mention is text now: the page reads "Ship it on Friday" and the label.
    const content = await client.pageContentGet(plan);
    expect(JSON.stringify(content.content)).toContain("personMention");
  });

  it("raises PersonAssigned for being added to a Person property by another person, not for staying on it", async () => {
    const { client, memory, plan } = await setup();
    const propertyId = (await client.propertySchemaGet(await spaceId(client))).find((def) => def.kind === "person")?.id ?? "";
    memory.receiveComments({ type: "personSet", pageId: plan, author: "Bob", propertyId, people: ["Bob", "Maya"] });
    const [row] = await client.notificationList();
    expect(row).toMatchObject({ kind: "PersonAssigned", pageId: plan, actorName: "Bob", pageTitle: "Plan" });
    memory.receiveComments({ type: "personSet", pageId: plan, author: "Bob", propertyId, people: ["Maya", "Carol"] });
    expect(await client.notificationList()).toHaveLength(1);
    // Only an Editor or an Owner writes properties: the change of a Commenter is ignored.
    expect(memory.receiveComments({ type: "personSet", pageId: plan, author: "Carol", propertyId, people: ["Dan"] })).toBeUndefined();
    const value = (await client.pageGet(plan)).properties?.find(([id]) => id === propertyId)?.[1];
    expect(value?.kind === "person" ? value.value.length : 0).toBe(2);
    expect(() => memory.receiveComments({ type: "personSet", pageId: plan, author: "Bob", propertyId, people: ["Zed"] })).toThrow(
      expect.objectContaining({ code: "InvalidInput", details: { field: "value" } }) as Error,
    );
  });

  it("lists newest first with a limit, a cursor and the unread ones only", async () => {
    const { client, memory, plan } = await setup();
    for (let n = 1; n <= 5; n += 1) {
      later();
      memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }, ` ${String(n)}`] });
    }
    const all = await client.notificationList();
    expect(all.map((row) => row.id)).toEqual([5, 4, 3, 2, 1]);
    expect((await client.notificationList({ limit: 2 })).map((row) => row.id)).toEqual([5, 4]);
    expect((await client.notificationList({ limit: 2, beforeId: 4 })).map((row) => row.id)).toEqual([3, 2]);
    await client.notificationsMarkRead([2, 4]);
    expect((await client.notificationList({ unreadOnly: true })).map((row) => row.id)).toEqual([5, 3, 1]);
    expect((await client.notificationList({ unreadOnly: true, limit: 0 })).length).toBe(1);
  });

  it("marks some rows or all as read, and tells the counts changed only when something did", async () => {
    const { client, memory, plan, events } = await setup();
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }] });
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }, " again"] });
    events.length = 0;
    await client.notificationsMarkRead([1]);
    expect(events).toEqual([{ type: "NotificationsChanged" }]);
    expect((await client.notificationList()).map((row) => row.read)).toEqual([false, true]);
    await client.notificationsMarkRead([1]);
    expect(events).toHaveLength(1);
    await client.notificationsMarkRead();
    expect(events).toHaveLength(2);
    expect((await client.notificationList()).every((row) => row.read)).toBe(true);
  });

  it("keeps at most 1,000 rows, the read ones going first and the newest never", async () => {
    const { client, memory, plan } = await setup();
    const post = (n: number): void => {
      memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }, ` ${String(n)}`] });
    };
    for (let n = 1; n <= NOTIFICATIONS_MAX_ROWS; n += 1) {
      post(n);
    }
    await client.notificationsMarkRead([10, 20]);
    post(NOTIFICATIONS_MAX_ROWS + 1);
    const rows = await Promise.all([
      client.notificationList({ limit: 200 }),
      client.notificationList({ limit: 200, beforeId: 802 }),
      client.notificationList({ limit: 200, beforeId: 602 }),
      client.notificationList({ limit: 200, beforeId: 402 }),
      client.notificationList({ limit: 200, beforeId: 202 }),
    ]);
    const ids = rows.flat().map((row) => row.id);
    expect(ids).toHaveLength(NOTIFICATIONS_MAX_ROWS);
    expect(ids).toContain(NOTIFICATIONS_MAX_ROWS + 1);
    // Row 10 was read, so it went first; row 1, unread, stays.
    expect(ids).not.toContain(10);
    expect(ids).toContain(1);
  });

  it("holds the toast while the core is not ready, and lists the row afterwards", async () => {
    const { client, memory, plan, events } = await setup();
    memory.setCoreState("opening");
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }] });
    expect(events.some((event) => event.type === "NotificationRaised")).toBe(false);
    memory.setCoreState("ready");
    expect(await client.notificationList()).toHaveLength(1);
  });

  it("does not list the rows of a space that was left", async () => {
    const { client, memory, plan } = await setup("editor");
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }] });
    expect(await client.notificationList()).toHaveLength(1);
    await client.spaceLeave(await spaceId(client));
    expect(await client.notificationList()).toEqual([]);
  });
});

describe("the Person property", () => {
  async function schema(client: CoreClient) {
    return (await client.propertySchemaGet(await spaceId(client))).find((def) => def.kind === "person");
  }

  it("is a kind a space can add, listed with the others", async () => {
    const { client } = build("owner");
    const added = await client.propertyCreate(await spaceId(client), "Reviewer", "person");
    expect(added).toMatchObject({ name: "Reviewer", kind: "person", options: [] });
  });

  it("holds members of the space, at most 20, without duplicates, and clears with an empty list", async () => {
    const { client } = build("owner");
    const notes = await pageId(client, "Notes");
    const def = await schema(client);
    const carol = await accountOf(client, "Carol");
    const dan = await accountOf(client, "Dan");
    const set = (value: string[]) => client.pagePropertySet(notes, def?.id ?? "", { kind: "person", value });
    expect(await set([carol, dan])).toEqual([[def?.id, { kind: "person", value: [carol, dan] }]]);
    expect(await failure(set([carol, carol]))).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
    expect(await failure(set(["00000000-0000-4000-8000-0000000000ee"]))).toMatchObject({
      code: "InvalidInput",
      details: { field: "value" },
    });
    expect(await failure(set(["nobody"]))).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
    expect(await set([])).toEqual([]);
    const tooMany = Array.from({ length: 21 }, (_, n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`);
    expect(await failure(set(tooMany))).toMatchObject({ code: "InvalidInput", details: { field: "value" } });
  });

  it("is the person themself in a personal space", async () => {
    const { client } = build("personal", { spaces: [{ name: "Work", properties: [{ name: "Owner", kind: "person" }], pages: [{ title: "Plan" }] }] });
    const plan = await pageId(client, "Plan");
    const def = await schema(client);
    const me = (await client.peopleMentionable(await spaceId(client)))[0]?.accountId ?? "";
    await client.pagePropertySet(plan, def?.id ?? "", { kind: "person", value: [me] });
    expect(await failure(client.pagePropertySet(plan, def?.id ?? "", { kind: "person", value: ["00000000-0000-4000-8000-0000000000ee"] }))).toMatchObject({
      details: { field: "value" },
    });
  });

  it("answers the filters HasValue, ContainsPerson and ContainsMe, and refuses the others", async () => {
    const { client } = build("owner");
    const space = await spaceId(client);
    const plan = await pageId(client, "Plan");
    const notes = await pageId(client, "Notes");
    const def = await schema(client);
    const bob = await accountOf(client, "Bob");
    const me = (await client.peopleMentionable(space)).find((person) => person.isMe)?.accountId ?? "";
    await client.pagePropertySet(notes, def?.id ?? "", { kind: "person", value: [me] });
    // Plan lists Bob (seeded by name), Notes lists this account.
    // By title: Notes, then Plan.
    expect(await client.pagesByProperty(space, def?.id ?? "", { type: "HasValue" })).toEqual([notes, plan]);
    expect(await client.pagesByProperty(space, def?.id ?? "", { type: "ContainsPerson", data: bob })).toEqual([plan]);
    expect(await client.pagesByProperty(space, def?.id ?? "", { type: "ContainsMe" })).toEqual([notes]);
    const refused: PropertyFilter[] = [
      { type: "Equals", data: { kind: "person", value: [bob] } },
      { type: "ContainsOption", data: "x" },
      { type: "TextContains", data: "x" },
      { type: "RelatesTo", data: plan },
    ];
    for (const filter of refused) {
      expect(await failure(client.pagesByProperty(space, def?.id ?? "", filter))).toMatchObject({ code: "InvalidInput", details: { field: "filter" } });
    }
    // The person filters belong to the Person kind.
    const tags = (await client.propertySchemaGet(space))[0];
    expect(await failure(client.pagesByProperty(space, tags?.id ?? "", { type: "ContainsMe" }))).toMatchObject({ details: { field: "filter" } });
  });

  it("keeps the accounts of people who left, and indexes the names of the others", async () => {
    const { client, memory } = build("owner");
    const space = await spaceId(client);
    const plan = await pageId(client, "Plan");
    expect((await client.search("Bob", { spaceId: space })).map((hit) => hit.pageId)).toEqual([plan]);
    await client.memberRemove(space, await accountOf(client, "Bob"));
    expect(await client.search("Bob", { spaceId: space })).toEqual([]);
    void memory;
  });

  it("exports the names, and Former member for an account the space does not list", async () => {
    const { client } = build("owner");
    const space = await spaceId(client);
    const plan = await pageId(client, "Plan");
    const bob = await accountOf(client, "Bob");
    const def = await schema(client);
    const csvOf = (): Promise<string> =>
      client.exportCsv(space, [plan], [{ type: "Title" }, { type: "Property", data: { propertyId: def?.id ?? "" } }]);
    expect(await csvOf()).toContain("Bob");
    await client.memberRemove(space, bob);
    const csv = await csvOf();
    expect(csv).toContain("Former member");
    expect(csv).not.toContain(bob);
  });
});

describe("the text folds", () => {
  it("reads a person mention as its label in the page text, and as @label in Markdown", async () => {
    const { client } = build("owner", {
      spaces: [
        {
          name: "Work",
          pages: [
            {
              title: "Plan",
              content: {
                type: "doc",
                content: [
                  {
                    type: "paragraph",
                    attrs: { id: "00000000-0000-4000-8000-000000000001" },
                    content: [
                      { type: "text", text: "Ask " },
                      { type: "personMention", attrs: { accountId: "00000000-0000-4000-8000-a00000000001", label: "Bob" } },
                      { type: "text", text: " about it" },
                    ],
                  },
                ],
              },
            },
          ],
        },
      ],
    });
    const space = await spaceId(client);
    const plan = await pageId(client, "Plan");
    expect(await client.pageExportMarkdown(plan)).toBe("Ask @Bob about it\n");
    const hits = await client.search("ask bob", { spaceId: space });
    expect(hits[0]).toMatchObject({ pageId: plan, snippet: "Ask Bob about it" });
  });
});

describe("the seeds", () => {
  it("starts threads on pages with real anchors, a resolution and inbox rows, and no events", async () => {
    const { client, events, memory } = build("owner", {
      sharing: { profileName: "Maya", spaces: [{ space: "Work", comments: THREADS, notifications: [{ kind: "ThreadReply", actor: "Bob", page: "Plan", threadQuote: "Ship it" }] }] },
    });
    const plan = await pageId(client, "Plan");
    const threads = await client.commentList(plan);
    expect(threads).toHaveLength(2);
    const anchored = threads.find((thread) => thread.anchor !== undefined);
    const resolved = threads.find((thread) => thread.resolved);
    expect(anchored?.anchor).toMatchObject({ blockId: "00000000-0000-4000-8000-000000000001", quote: "Ship it" });
    // The anchor holds the Loro cursors of the words, as the editor makes them.
    const doc = new LoroDoc();
    doc.import(await memory.docs.open(plan));
    const start = Cursor.decode(fromBase64(anchored?.anchor?.start ?? ""));
    const end = Cursor.decode(fromBase64(anchored?.anchor?.end ?? ""));
    expect(doc.getCursorPos(start)?.offset).toBe(0);
    expect(doc.getCursorPos(end)?.offset).toBe("Ship it".length);
    expect(anchored?.comments.map((comment) => comment.authorName)).toEqual(["Carol", "Bob", "Maya"]);
    expect(anchored?.comments[1]?.body).toContainEqual({ type: "mention", accountId: (await accountOf(client, "Maya")) });
    expect(resolved?.resolved).toBe(true);
    expect(resolved?.anchor).toBeUndefined();
    expect(resolved?.resolvedBy).toBe(await accountOf(client, "Bob"));
    const rows = await client.notificationList();
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ kind: "ThreadReply", actorName: "Bob", pageTitle: "Plan", read: false });
    expect(events.filter((event) => event.type === "CommentsChanged" || event.type === "NotificationRaised")).toEqual([]);
  });

  it("starts threads of its own with an author, a resolution, an edit, a delete and an age", async () => {
    const { client } = build("commenter", {
      sharing: {
        profileName: "Maya",
        spaces: [
          {
            space: "Work",
            role: "commenter",
            comments: [
              {
                page: "Notes",
                resolved: true,
                comments: [
                  { author: "Bob", body: "One", minutesAgo: 30, edited: true },
                  { author: "Carol", body: ["Two ", { page: "Plan" }], minutesAgo: 20, deleted: true },
                  { body: "Three", minutesAgo: 10 },
                ],
              },
            ],
          },
        ],
      },
    });
    const notes = await pageId(client, "Notes");
    const [thread] = await client.commentList(notes);
    expect(thread?.resolved).toBe(true);
    expect(thread?.resolvedBy).toBe((await client.peopleMentionable(await spaceId(client))).find((person) => person.isMe)?.accountId);
    expect(thread?.comments.map((comment) => [comment.authorName, comment.deleted, comment.isMine, comment.createdAt])).toEqual([
      ["Bob", false, false, START - 30 * 60_000],
      ["Carol", true, false, START - 20 * 60_000],
      ["Maya", false, true, START - 10 * 60_000],
    ]);
    expect(thread?.comments[0]?.editedAt).toBe(START - 25 * 60_000);
    // The roles apply to what is written next: this account is a Commenter, the thread is Bob's.
    expect((await failure(client.commentResolve(notes, thread?.threadId ?? "", false))).code).toBe("Forbidden");
  });

  it("refuses a seed that names a page that is not there", () => {
    expect(() =>
      build("owner", { sharing: { profileName: "Maya", spaces: [{ space: "Work", comments: [{ page: "Nope", comments: [{ body: "x" }] }] }] } }),
    ).toThrow(/does not exist/u);
  });

  it("seeds Person values by name: a member's, this account's, and a made-up person's", async () => {
    const { client } = build("owner", {
      spaces: [
        {
          name: "Work",
          properties: [{ name: "Owner", kind: "person" }],
          pages: [{ title: "Plan", properties: { Owner: ["Bob", "me", "Zed"] } }],
        },
      ],
    });
    const plan = await client.pageGet(await pageId(client, "Plan"));
    const value = plan.properties?.find(([, entry]) => entry.kind === "person")?.[1];
    expect(value?.kind === "person" ? value.value : []).toHaveLength(3);
    const people = await client.peopleMentionable(await spaceId(client));
    const ids = value?.kind === "person" ? value.value : [];
    expect(ids[0]).toBe(people.find((person) => person.name === "Bob")?.accountId);
    expect(ids[1]).toBe(people.find((person) => person.isMe)?.accountId);
    expect(people.some((person) => person.accountId === ids[2])).toBe(false);
  });
});

describe("search", () => {
  it("finds the words of a comment, labelled with the comment, after the page's own hits", async () => {
    const { client, memory } = build("owner");
    const space = await spaceId(client);
    const plan = await pageId(client, "Plan");
    const thread = memory.receiveComments({ type: "post", pageId: plan, author: "Carol", body: ["The friday release ", { mention: "Maya" }] });
    const hits = await client.search("release", { spaceId: space });
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatchObject({ pageId: plan, title: "Plan", commentId: thread?.comments[0]?.commentId, snippet: "The friday release @Maya" });
    expect(hits[0]?.blockId).toBeUndefined();
    await client.commentDelete(plan, thread?.comments[0]?.commentId ?? "");
    expect(await client.search("release", { spaceId: space })).toEqual([]);
  });
});

describe("the notification row's types", () => {
  it("is a NotificationInfo that names the page and the actor", async () => {
    const { client, memory } = build("owner");
    const plan = await pageId(client, "Plan");
    memory.receiveComments({ type: "post", pageId: plan, author: "Bob", body: [{ mention: "Maya" }] });
    const [row]: NotificationInfo[] = await client.notificationList();
    expect(Object.keys(row ?? {}).sort()).toEqual(["actor", "actorName", "at", "commentId", "id", "kind", "pageId", "pageTitle", "read", "spaceId"]);
  });
});
