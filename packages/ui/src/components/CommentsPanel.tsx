// The comments panel (ADR-0048): a column beside the open page with its comment threads. Each thread shows the words it is about
// (the quote of its anchor; one whose text is gone says so and reads as a page-level thread), its comments with their authors
// ("Former member" once they left), edit and delete for those who may, a reply field, and resolve or reopen. "Show resolved"
// lists the resolved threads too. A new comment starts from "Comment" in the selection toolbar (anchored), or from "New comment on
// the page" here and the page menu. Every writing control follows the role: a Viewer reads, a Commenter writes comments but not the
// page (a thread is resolved by its author or an Editor and up, a comment deleted by its author or an Owner).

import type { AccountId, CommentInfo, SpaceId, ThreadInfo } from "@atomik-kb/core-client";
import { useEditorState } from "@tiptap/react";
import { useEffect, useRef, useState } from "react";

import { authorName, canDeleteComment, canEditComment, canResolveThread, initialOf, refusalText, textFromBody } from "../comments";
import { usePeople, usePersonNames } from "../comments-hooks";
import { commentAnchorState, revealCommentThread } from "../editor/comment-highlights";
import { commentsHost } from "../editor/comments-host";
import { useReferencedPage } from "../editor/node-views";
import { useObservable } from "../editor/ai/observable";
import { formatAgo } from "../format";
import { t } from "../i18n";
import { openGestures } from "../page-open";
import { useSpaceAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { CommentComposer } from "./CommentComposer";
import { relatedPageTitle } from "./RelationEditor";
import { CloseIcon } from "./icons";

/** The accounts a comment mentions. */
function mentionedIn(comment: CommentInfo): AccountId[] {
  return comment.body.flatMap((item) => (item.type === "mention" ? [item.accountId] : []));
}

/** A comment's words: text, `@name` for a mention (the name now, "Former member" for someone who left), a page link. */
function CommentBody({ comment, spaceId }: { comment: CommentInfo; spaceId: SpaceId }) {
  const people = usePeople(spaceId);
  const nameOf = usePersonNames(spaceId, mentionedIn(comment));
  if (comment.deleted) {
    return <p className="comment-body is-deleted muted">{t("comments.deleted")}</p>;
  }
  return (
    <p className="comment-body">
      {comment.body.map((item, index) => {
        switch (item.type) {
          case "text":
            return <span key={index}>{item.text}</span>;
          case "mention": {
            const former = people !== undefined && !people.some((person) => person.accountId === item.accountId);
            const mine = people?.find((person) => person.accountId === item.accountId)?.isMe === true;
            return (
              <span key={index} className={`person-mention${former ? " is-former" : ""}${mine ? " is-me" : ""}`}>
                @{nameOf(item.accountId)}
              </span>
            );
          }
          case "page":
            return <PageLink key={index} pageId={item.id} />;
        }
      })}
    </p>
  );
}

/** A link to a page in a comment: its title, opened like any page link. */
function PageLink({ pageId }: { pageId: string }) {
  const page = useReferencedPage(pageId);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const found = page.status === "found" ? page.page : null;
  return (
    <button
      type="button"
      className="link-button comment-page-link"
      disabled={found === null}
      {...openGestures<HTMLButtonElement>((disposition) => {
        if (found !== null) {
          navigateTo({ pageId: found.id, spaceId: found.spaceId, disposition });
        }
      })}
    >
      {relatedPageTitle(page)}
    </button>
  );
}

interface CommentRowProps {
  comment: CommentInfo;
  thread: ThreadInfo;
  spaceId: SpaceId;
}

/** One comment of a thread, with its author, its age and what this account may do to it. */
function CommentRow({ comment, thread, spaceId }: CommentRowProps) {
  const access = useSpaceAccess(spaceId);
  const nameOf = usePersonNames(spaceId, mentionedIn(comment));
  const editComment = useAppStore((state) => state.editComment);
  const deleteComment = useAppStore((state) => state.deleteComment);
  const [asked, setMode] = useState<"read" | "edit" | "confirm">("read");
  // A deleted comment has nothing left to edit, delete or confirm.
  const mode = comment.deleted ? "read" : asked;
  const [failure, setFailure] = useState<string | null>(null);
  const name = authorName(comment);
  const when = formatAgo(comment.createdAt);
  const editable = canEditComment(access, comment);
  const deletable = canDeleteComment(access, comment);

  async function remove(): Promise<void> {
    setFailure(null);
    try {
      await deleteComment(thread.pageId, comment.commentId);
    } catch (error) {
      setFailure(t("comments.error.action", { message: refusalText(error, "delete") }));
      setMode("read");
    }
  }

  const editing = mode === "edit" ? textFromBody(comment.body, (accountId) => nameOf(accountId)) : null;
  return (
    <li className={`comment${comment.deleted ? " is-deleted" : ""}`} data-comment-id={comment.commentId}>
      <div className="comment-head">
        <span className={`person-mark${comment.authorFormer ? " is-former" : ""}`} aria-hidden="true">
          {initialOf(name)}
        </span>
        <span className="comment-author">{name}</span>
        {when !== null && (
          <time className="comment-time muted" dateTime={new Date(comment.createdAt).toISOString()}>
            {when}
          </time>
        )}
        {comment.editedAt !== undefined && !comment.deleted && <span className="muted comment-edited">{t("comments.edited")}</span>}
      </div>
      {editing !== null ? (
        <CommentComposer
          spaceId={spaceId}
          label={t("comments.edit.label")}
          initialText={editing.text}
          initialMentions={editing.mentions}
          submitLabel={t("comments.save")}
          action="edit"
          autoFocus
          onSubmit={async (body) => {
            await editComment(thread.pageId, comment.commentId, body);
            setMode("read");
          }}
          onCancel={() => {
            setMode("read");
          }}
        />
      ) : (
        <CommentBody comment={comment} spaceId={spaceId} />
      )}
      {mode === "confirm" && (
        <div className="comment-confirm" role="group" aria-label={t("comments.delete.confirmLabel")}>
          <p>{t("comments.delete.confirm")}</p>
          <div className="comment-actions">
            <button
              type="button"
              className="button button-danger"
              onClick={() => {
                void remove();
              }}
            >
              {t("comments.delete.yes")}
            </button>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setMode("read");
              }}
            >
              {t("comments.cancel")}
            </button>
          </div>
        </div>
      )}
      {mode === "read" && (editable || deletable) && (
        <div className="comment-actions">
          {editable && (
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setFailure(null);
                setMode("edit");
              }}
            >
              {t("comments.edit")}
            </button>
          )}
          {deletable && (
            <button
              type="button"
              className="link-button"
              onClick={() => {
                setFailure(null);
                setMode("confirm");
              }}
            >
              {t("comments.delete")}
            </button>
          )}
        </div>
      )}
      {failure !== null && (
        <p className="error-text" role="alert">
          {failure}
        </p>
      )}
    </li>
  );
}

interface ThreadCardProps {
  thread: ThreadInfo;
  spaceId: SpaceId;
  focused: boolean;
  orphan: boolean;
}

/** One thread: its quote, its comments, a reply field and resolve or reopen. */
function ThreadCard({ thread, spaceId, focused, orphan }: ThreadCardProps) {
  const access = useSpaceAccess(spaceId);
  const people = usePeople(spaceId);
  const postComment = useAppStore((state) => state.postComment);
  const resolveThread = useAppStore((state) => state.resolveThread);
  const focusThread = useAppStore((state) => state.focusThread);
  const [replying, setReplying] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const root = thread.comments[0];
  const quote = thread.anchor?.quote;
  const resolver = thread.resolvedBy === undefined ? undefined : people?.find((person) => person.accountId === thread.resolvedBy);

  async function resolve(resolved: boolean): Promise<void> {
    setFailure(null);
    try {
      await resolveThread(thread.pageId, thread.threadId, resolved);
    } catch (error) {
      setFailure(t("comments.error.action", { message: refusalText(error, "resolve") }));
    }
  }

  return (
    <li
      className={`comment-thread${thread.resolved ? " is-resolved" : ""}${focused ? " is-focused" : ""}${orphan ? " is-orphan" : ""}`}
      data-thread-id={thread.threadId}
    >
      <article aria-label={t("comments.thread.label", { name: root === undefined ? "" : authorName(root) })}>
        {quote !== undefined && quote !== "" && (
          <>
            {orphan ? (
              <blockquote className="comment-quote is-orphan">{quote}</blockquote>
            ) : (
              <button
                type="button"
                className="comment-quote"
                title={t("comments.quote.show")}
                onClick={() => {
                  focusThread(thread.threadId);
                }}
              >
                {quote}
              </button>
            )}
            {orphan && <p className="comment-orphan muted">{t("comments.orphan")}</p>}
          </>
        )}
        <ol className="comment-list" aria-label={t("comments.thread.comments")}>
          {thread.comments.map((comment) => (
            <CommentRow key={comment.commentId} comment={comment} thread={thread} spaceId={spaceId} />
          ))}
        </ol>
        {thread.resolved && (
          <p className="comment-resolved muted">
            {resolver === undefined ? t("comments.resolved") : t("comments.resolvedBy", { name: resolver.name })}
          </p>
        )}
        {access.canComment && (
          <div className="comment-thread-actions">
            {replying ? (
              <CommentComposer
                spaceId={spaceId}
                label={t("comments.reply.label")}
                placeholder={t("comments.reply.placeholder")}
                submitLabel={t("comments.reply.send")}
                autoFocus
                onSubmit={async (body) => {
                  await postComment({ pageId: thread.pageId, threadId: thread.threadId, body });
                  setReplying(false);
                }}
                onCancel={() => {
                  setReplying(false);
                }}
              />
            ) : (
              <button
                type="button"
                className="link-button"
                onClick={() => {
                  setReplying(true);
                }}
              >
                {t("comments.reply")}
              </button>
            )}
            {canResolveThread(access, thread) && (
              <button
                type="button"
                className="link-button"
                onClick={() => {
                  void resolve(!thread.resolved);
                }}
              >
                {thread.resolved ? t("comments.reopen") : t("comments.resolve")}
              </button>
            )}
          </div>
        )}
        {failure !== null && (
          <p className="error-text" role="alert">
            {failure}
          </p>
        )}
      </article>
    </li>
  );
}

const NO_THREADS: readonly ThreadInfo[] = [];

/** The panel beside the page. Shown by the main layout while it is open and a page is. */
export function CommentsPanel() {
  const pageComments = useAppStore((state) => state.pageComments);
  const showResolved = useAppStore((state) => state.showResolved);
  const draft = useAppStore((state) => state.commentDraft);
  const focus = useAppStore((state) => state.commentFocus);
  const spaceId = useAppStore((state) => state.currentSpaceId);
  const setShowResolved = useAppStore((state) => state.setShowResolved);
  const closeComments = useAppStore((state) => state.closeComments);
  const beginComment = useAppStore((state) => state.beginComment);
  const cancelComment = useAppStore((state) => state.cancelComment);
  const postComment = useAppStore((state) => state.postComment);
  const focusThread = useAppStore((state) => state.focusThread);
  const loadComments = useAppStore((state) => state.loadComments);
  const access = useSpaceAccess(spaceId);
  const host = useObservable(commentsHost);
  const list = useRef<HTMLUListElement>(null);
  const handled = useRef(0);
  const pageId = pageComments?.pageId ?? null;
  const editor = host !== null && host.pageId === pageId ? host.editor : null;
  // The threads whose text the editor cannot find any more (null while no editor shows the page).
  const orphans = useEditorState({
    editor,
    selector: ({ editor: current }) => (current === null ? null : commentAnchorState(current.state).orphans.join(",")),
  });
  const orphaned = new Set(orphans === null || orphans === "" ? [] : orphans.split(","));
  const threads = pageComments?.threads ?? NO_THREADS;
  const focusedThread = focus?.threadId;
  const visible = threads.filter((thread) => showResolved || !thread.resolved || thread.threadId === focusedThread);
  const hiddenResolved = threads.filter((thread) => thread.resolved).length;

  // A comment named by its id (a notification, a search hit) is found in its thread, which then takes the focus.
  useEffect(() => {
    if (focus === null || focus.threadId !== undefined || focus.commentId === undefined || pageComments?.status !== "ready") {
      return;
    }
    const found = threads.find((thread) => thread.comments.some((comment) => comment.commentId === focus.commentId));
    if (found !== undefined) {
      focusThread(found.threadId);
    }
  }, [focus, pageComments?.status, threads, focusThread]);

  // A thread that takes the focus is scrolled to here, and its text in the page.
  useEffect(() => {
    if (focus === null || focus.threadId === undefined || focus.seq === handled.current) {
      return;
    }
    const card = Array.from(list.current?.children ?? []).find((entry) => entry.getAttribute("data-thread-id") === focus.threadId);
    if (card === undefined) {
      return;
    }
    handled.current = focus.seq;
    if (typeof card.scrollIntoView === "function") {
      card.scrollIntoView({ block: "nearest" });
    }
    if (editor !== null) {
      revealCommentThread(editor, focus.threadId);
    }
  }, [focus, editor, visible.length]);

  if (spaceId === null) {
    return null;
  }
  return (
    <aside className="comments-panel" aria-label={t("comments.panel")}>
      <header className="comments-header">
        <h2 className="comments-title">{t("comments.title")}</h2>
        <label className="comments-show-resolved">
          <input
            type="checkbox"
            checked={showResolved}
            onChange={(event) => {
              setShowResolved(event.target.checked);
            }}
          />
          <span>{t("comments.showResolved")}</span>
        </label>
        <button type="button" className="icon-button" aria-label={t("comments.close")} title={t("comments.close")} onClick={closeComments}>
          <CloseIcon />
        </button>
      </header>
      <div className="comments-body">
        {pageComments?.status === "failed" && pageId !== null && (
          <div className="banner banner-warning" role="alert">
            <p>{t("comments.error.load", { message: pageComments.error ?? "" })}</p>
            <button
              type="button"
              className="button"
              onClick={() => {
                loadComments(pageId);
              }}
            >
              {t("comments.retry")}
            </button>
          </div>
        )}
        {pageComments?.status === "loading" && (
          <p className="muted" role="status">
            {t("comments.loading")}
          </p>
        )}
        {pageId !== null && access.canComment && draft?.pageId === pageId && (
          <section className="comment-draft" aria-label={t("comments.new.label")}>
            {draft.anchor !== undefined && draft.anchor.quote !== "" && <blockquote className="comment-quote">{draft.anchor.quote}</blockquote>}
            <CommentComposer
              spaceId={spaceId}
              label={t("comments.new.field")}
              placeholder={t("comments.new.placeholder")}
              submitLabel={t("comments.new.send")}
              autoFocus
              onSubmit={async (body) => {
                await postComment({ pageId, body, ...(draft.anchor === undefined ? {} : { anchor: draft.anchor }) });
              }}
              onCancel={cancelComment}
            />
          </section>
        )}
        {pageId !== null && access.canComment && draft?.pageId !== pageId && (
          <button
            type="button"
            className="button comments-new"
            onClick={() => {
              beginComment({ pageId });
            }}
          >
            {t("comments.newOnPage")}
          </button>
        )}
        {!access.canComment && (
          <p className="muted comments-readonly" role="note">
            {access.archived ? t("comments.readOnly.archive") : t("comments.readOnly.viewer")}
          </p>
        )}
        <ul ref={list} className="comment-threads" aria-label={t("comments.threads")}>
          {visible.map((thread) => (
            <ThreadCard
              key={thread.threadId}
              thread={thread}
              spaceId={spaceId}
              focused={focusedThread === thread.threadId}
              orphan={thread.anchor !== undefined && orphaned.has(thread.threadId)}
            />
          ))}
        </ul>
        {pageComments?.status === "ready" && visible.length === 0 && (
          <p className="muted comments-empty">{hiddenResolved > 0 ? t("comments.empty.resolved") : t("comments.empty")}</p>
        )}
      </div>
    </aside>
  );
}
