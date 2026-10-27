// The comments part of the app store (Milestone 13, slice 2; ADR-0048 and ADR-0049): the comment threads of the open page and the
// panel that shows them, the comment being written (a draft with its anchor), the people of the spaces (what the `@` menu, the
// chips of a mention and the Person property show), and the inbox of notifications with its toasts.
//
// As in the other slices, reads are "latest request wins" and events refresh: `CommentsChanged` re-reads the threads of the open
// page, `NotificationRaised` shows a toast and `NotificationsChanged` re-reads the inbox, `PeopleChanged` and
// `SpaceMembersChanged` re-read the people that were read before. Writes answer with the thread, which is applied at once.
//
// Comments, names and notifications are in memory only: a lock clears them (`EMPTY_COMMENTS`).

import { toCoreError } from "@atomik-kb/core-client";
import type {
  CommentAnchor,
  CommentBodyItem,
  CommentId,
  CoreClient,
  Event as CoreEvent,
  NotificationInfo,
  NotificationKind,
  PageId,
  PersonInfo,
  SpaceId,
  ThreadId,
  ThreadInfo,
} from "@atomik-kb/core-client";
import type { StoreApi } from "zustand/vanilla";

import { personName } from "./comments";
import { t } from "./i18n";
import type { MessageKey } from "./i18n";
import type { AppState } from "./store";

/** Most rows the inbox asks for at once (the core's own limit). */
export const INBOX_LIMIT = 200;

/** The threads of the open page. */
export interface PageComments {
  pageId: PageId;
  status: "loading" | "ready" | "failed";
  /** Oldest root first, as the core lists them. */
  threads: readonly ThreadInfo[];
  /** Why the core refused (a sentence for people), when `status` is `failed`. */
  error?: string;
}

/** A comment being written: on the page as a whole, or anchored to the text that was selected. */
export interface CommentDraft {
  pageId: PageId;
  anchor?: CommentAnchor;
}

/** Where the panel scrolls to: a thread, or the thread that holds a comment (a search hit, a notification). */
export interface CommentFocus {
  threadId?: ThreadId;
  commentId?: CommentId;
  /** Distinguishes two requests for the same thread. */
  seq: number;
}

/** The device's inbox as read: the newest rows and how many of all are unread. */
export interface InboxState {
  status: "idle" | "loading" | "ready" | "failed";
  items: readonly NotificationInfo[];
  unread: number;
}

export interface CommentsData {
  /** The threads of the open page; `null` with no page open. */
  pageComments: PageComments | null;
  /** The comments panel is open (beside the page). */
  commentsOpen: boolean;
  /** Resolved threads are listed too. */
  showResolved: boolean;
  /** The thread in focus (a click on its text or its card, a notification): the panel scrolls to it, the page draws its text stronger. */
  commentFocus: CommentFocus | null;
  commentDraft: CommentDraft | null;
  /** The people of the spaces read so far (`PeopleMentionable`): a space is present once its list was read. */
  people: Readonly<Record<SpaceId, readonly PersonInfo[]>>;
  inbox: InboxState;
  /** The inbox popover is open. */
  inboxOpen: boolean;
}

export const EMPTY_COMMENTS: CommentsData = {
  pageComments: null,
  commentsOpen: false,
  showResolved: false,
  commentFocus: null,
  commentDraft: null,
  people: {},
  inbox: { status: "idle", items: [], unread: 0 },
  inboxOpen: false,
};

/** What `openComments` may point at. */
export interface OpenComments {
  threadId?: ThreadId;
  commentId?: CommentId;
  /** Starts a comment: on the page as a whole when there is no anchor. */
  draft?: CommentDraft;
}

/** Every write rejects with a `CoreError` when the core refuses (`Forbidden` below the needed role, `InvalidInput`). */
export interface CommentsActions {
  /** Loads the threads of a page (the open one), or forgets them with `null`. */
  loadComments: (pageId: PageId | null) => void;
  openComments: (target?: OpenComments) => void;
  closeComments: () => void;
  toggleComments: () => void;
  setShowResolved: (show: boolean) => void;
  /** Brings a thread into focus: the panel scrolls to it, and the page draws its text stronger. */
  focusThread: (threadId: ThreadId) => void;
  /** Starts a comment on a page (anchored when it has an anchor) and opens the panel on it. */
  beginComment: (draft: CommentDraft) => void;
  cancelComment: () => void;
  /** Posts a thread, or a reply when `threadId` is given. */
  postComment: (input: { pageId: PageId; threadId?: ThreadId; anchor?: CommentAnchor; body: CommentBodyItem[] }) => Promise<ThreadInfo>;
  editComment: (pageId: PageId, commentId: CommentId, body: CommentBodyItem[]) => Promise<ThreadInfo>;
  deleteComment: (pageId: PageId, commentId: CommentId) => Promise<ThreadInfo>;
  resolveThread: (pageId: PageId, threadId: ThreadId, resolved: boolean) => Promise<ThreadInfo>;
  /** Reads the people of a space (its members; in a personal space this account alone) and keeps them in `people`. */
  loadPeople: (spaceId: SpaceId) => Promise<readonly PersonInfo[]>;
  /** The people of a space: the list read before, else one read shared by every caller that asks while it is under way. */
  ensurePeople: (spaceId: SpaceId) => Promise<readonly PersonInfo[]>;
  /** Reads the inbox again. */
  loadInbox: () => Promise<void>;
  /** Marks rows read, or every row without ids. */
  markNotificationsRead: (ids?: number[]) => Promise<void>;
  setInboxOpen: (open: boolean) => void;
  /** Opens what a row is about (its page at its block, and the comments panel for the two comment kinds) and marks it read. */
  openNotification: (row: NotificationInfo) => void;
}

export type CommentsState = CommentsData & CommentsActions;

export interface CommentsSlice {
  initial: CommentsState;
  /** Reads the inbox for the boot sequence; a failure other than `Locked` leaves it unread. */
  load: () => Promise<Partial<CommentsData>>;
  onEvent: (event: CoreEvent) => void;
}

const NOTIFICATION_KEYS: Readonly<Record<NotificationKind, MessageKey>> = {
  ThreadReply: "comments.notify.threadReply",
  CommentMention: "comments.notify.commentMention",
  PageMention: "comments.notify.pageMention",
  PersonAssigned: "comments.notify.personAssigned",
};

/**
 * What a row says: who did what, on which page, from the translated phrase of its kind (`{actor}`, `{page}`). The core sends no
 * text, only the kind, the names and the page's title; never the words of a comment. `actor` is the actor as this device shows
 * them (a person who left reads "Former member (name)"); the name of the row when it is left out.
 */
export function notificationText(row: NotificationInfo, actor: string = row.actorName): string {
  const page = row.pageTitle === undefined || row.pageTitle.trim() === "" ? t("page.untitled") : row.pageTitle.trim();
  return t(NOTIFICATION_KEYS[row.kind], { actor, page });
}

/** The thread list with `thread` in it (replacing the one it answers for), oldest root first; one without comments is gone. */
function withThread(threads: readonly ThreadInfo[], thread: ThreadInfo): ThreadInfo[] {
  const others = threads.filter((entry) => entry.threadId !== thread.threadId);
  const next = thread.comments.length === 0 ? others : [...others, thread];
  return next.sort((a, b) => (a.comments[0]?.createdAt ?? 0) - (b.comments[0]?.createdAt ?? 0) || (a.threadId < b.threadId ? -1 : 1));
}

export function createCommentsSlice(
  client: CoreClient,
  set: StoreApi<AppState>["setState"],
  get: StoreApi<AppState>["getState"],
  refreshQuietly: (refresh: () => Promise<unknown>) => void,
): CommentsSlice {
  // Bumped by every read that answers the open page's threads or the inbox; only the latest applies.
  let threadsTicket = 0;
  let inboxTicket = 0;
  let focusSeq = 0;
  const peopleTickets = new Map<SpaceId, number>();
  const peopleReads = new Map<SpaceId, Promise<readonly PersonInfo[]>>();

  async function fetchComments(pageId: PageId): Promise<void> {
    const mine = (threadsTicket += 1);
    try {
      const threads = await client.commentList(pageId);
      if (mine === threadsTicket && get().pageComments?.pageId === pageId) {
        set({ pageComments: { pageId, status: "ready", threads } });
      }
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error("could not read the comments", failure.code);
      if (mine === threadsTicket && get().pageComments?.pageId === pageId) {
        set((state) => ({
          pageComments: { pageId, status: "failed", threads: state.pageComments?.threads ?? [], error: failure.message },
        }));
      }
    }
  }

  /** Puts the core's answer for one thread into the open page's list, when it is that page's. */
  function applyThread(thread: ThreadInfo): ThreadInfo {
    set((state) =>
      state.pageComments?.pageId === thread.pageId
        ? { pageComments: { ...state.pageComments, threads: withThread(state.pageComments.threads, thread) } }
        : state,
    );
    return thread;
  }

  async function fetchInbox(): Promise<void> {
    const mine = (inboxTicket += 1);
    const [items, unread] = await Promise.all([
      client.notificationList({ limit: INBOX_LIMIT }),
      client.notificationList({ limit: INBOX_LIMIT, unreadOnly: true }),
    ]);
    if (mine === inboxTicket) {
      set({ inbox: { status: "ready", items, unread: unread.length } });
    }
  }

  async function loadPeople(spaceId: SpaceId): Promise<readonly PersonInfo[]> {
    const mine = (peopleTickets.get(spaceId) ?? 0) + 1;
    peopleTickets.set(spaceId, mine);
    const people = await client.peopleMentionable(spaceId);
    if (peopleTickets.get(spaceId) === mine) {
      set((state) => ({ people: { ...state.people, [spaceId]: people } }));
    }
    return people;
  }

  const initial: CommentsState = {
    ...EMPTY_COMMENTS,

    loadComments: (pageId) => {
      if (pageId === null) {
        threadsTicket += 1;
        set({ pageComments: null, commentDraft: null });
        return;
      }
      set((state) => ({
        pageComments: state.pageComments?.pageId === pageId ? state.pageComments : { pageId, status: "loading", threads: [] },
        // A draft belongs to its page.
        commentDraft: state.commentDraft?.pageId === pageId ? state.commentDraft : null,
      }));
      refreshQuietly(() => fetchComments(pageId));
    },

    openComments: (target) => {
      set({
        commentsOpen: true,
        ...(target?.draft === undefined ? {} : { commentDraft: target.draft }),
        ...(target?.threadId === undefined && target?.commentId === undefined
          ? {}
          : {
              commentFocus: {
                ...(target.threadId === undefined ? {} : { threadId: target.threadId }),
                ...(target.commentId === undefined ? {} : { commentId: target.commentId }),
                seq: (focusSeq += 1),
              },
            }),
      });
    },
    closeComments: () => {
      set({ commentsOpen: false, commentFocus: null });
    },
    toggleComments: () => {
      set((state) => (state.commentsOpen ? { commentsOpen: false, commentFocus: null } : { commentsOpen: true }));
    },
    setShowResolved: (show) => {
      set({ showResolved: show });
    },
    focusThread: (threadId) => {
      set({ commentFocus: { threadId, seq: (focusSeq += 1) } });
    },
    beginComment: (draft) => {
      set({ commentsOpen: true, commentDraft: draft });
    },
    cancelComment: () => {
      set({ commentDraft: null });
    },

    postComment: async ({ pageId, threadId, anchor, body }) => {
      const thread = applyThread(
        await client.commentPost({
          pageId,
          body,
          ...(threadId === undefined ? {} : { threadId }),
          ...(anchor === undefined ? {} : { anchor }),
        }),
      );
      // A new thread ends the draft that made it.
      if (threadId === undefined) {
        set((state) => (state.commentDraft?.pageId === pageId ? { commentDraft: null } : state));
      }
      return thread;
    },
    editComment: async (pageId, commentId, body) => applyThread(await client.commentEdit(pageId, commentId, body)),
    deleteComment: async (pageId, commentId) => applyThread(await client.commentDelete(pageId, commentId)),
    resolveThread: async (pageId, threadId, resolved) => applyThread(await client.commentResolve(pageId, threadId, resolved)),

    loadPeople,
    ensurePeople: (spaceId) => {
      const known = get().people[spaceId];
      if (known !== undefined) {
        return Promise.resolve(known);
      }
      let read = peopleReads.get(spaceId);
      if (read === undefined) {
        read = loadPeople(spaceId).finally(() => {
          peopleReads.delete(spaceId);
        });
        peopleReads.set(spaceId, read);
      }
      return read;
    },
    loadInbox: fetchInbox,
    markNotificationsRead: async (ids) => {
      await client.notificationsMarkRead(ids);
      await fetchInbox();
    },
    setInboxOpen: (open) => {
      set({ inboxOpen: open });
    },
    openNotification: (row) => {
      set({ inboxOpen: false });
      refreshQuietly(() => get().markNotificationsRead([row.id]));
      const { pageId } = row;
      if (pageId === undefined) {
        return;
      }
      get().navigateTo({ pageId, spaceId: row.spaceId, ...(row.blockId === undefined ? {} : { blockId: row.blockId }) });
      if (row.kind === "ThreadReply" || row.kind === "CommentMention") {
        get().openComments({ ...(row.commentId === undefined ? {} : { commentId: row.commentId }) });
      }
    },
  };

  async function load(): Promise<Partial<CommentsData>> {
    try {
      const [items, unread] = await Promise.all([
        client.notificationList({ limit: INBOX_LIMIT }),
        client.notificationList({ limit: INBOX_LIMIT, unreadOnly: true }),
      ]);
      return { inbox: { status: "ready", items, unread: unread.length } };
    } catch (error) {
      const failure = toCoreError(error);
      if (failure.code === "Locked") {
        throw failure;
      }
      console.error("could not read the inbox", failure.code);
      return {};
    }
  }

  return {
    initial,
    load,
    onEvent: (event) => {
      if (get().phase === "locked") {
        // The next boot reads what it needs.
        return;
      }
      switch (event.type) {
        case "CommentsChanged":
          if (get().pageComments?.pageId === event.data.pageId) {
            refreshQuietly(() => fetchComments(event.data.pageId));
          }
          break;
        case "NotificationRaised": {
          const row = event.data;
          get().showToast(notificationText(row, personName(get().people[row.spaceId], row.actor, row.actorName)), "info", {
            label: t("comments.notify.open"),
            run: () => {
              get().openNotification(row);
            },
          });
          refreshQuietly(fetchInbox);
          break;
        }
        case "NotificationsChanged":
          refreshQuietly(fetchInbox);
          break;
        case "PeopleChanged": {
          for (const spaceId of Object.keys(get().people)) {
            refreshQuietly(() => loadPeople(spaceId));
          }
          // Who is "a former member" changed with the members: the authors of the open page are read again.
          const open = get().pageComments?.pageId;
          if (open !== undefined) {
            refreshQuietly(() => fetchComments(open));
          }
          break;
        }
        case "SpaceMembersChanged": {
          if (get().people[event.data.spaceId] !== undefined) {
            refreshQuietly(() => loadPeople(event.data.spaceId));
          }
          const open = get().pageComments?.pageId;
          if (open !== undefined) {
            refreshQuietly(() => fetchComments(open));
          }
          break;
        }
        // The other events are not about comments.
      }
    },
  };
}
