// The highlights of anchored comment threads (ADR-0048): decorations over the text each thread's anchor points at, not marks, so
// a Commenter, who cannot write the page, sees them as everyone does. The threads are pushed in by the page (`setCommentThreads`);
// the plugin resolves each anchor to a range with the binding's cursor conversions (`comment-anchors.ts`), keeps the ranges while
// the document changes (a local edit maps them, a remote change or an undo resolves them again once the binding is up to date), and
// tells which anchors cannot be resolved any more: the panel shows those as page-level threads with their quote.
//
// A resolved thread is highlighted only while "show resolved" is on. The thread the panel has in focus is drawn stronger
// (`setActiveCommentThread`). A click on a highlight asks the page to open that thread in the panel.

import { Extension } from "@tiptap/core";
import type { Editor } from "@tiptap/core";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import { Decoration, DecorationSet } from "@tiptap/pm/view";
import type { CommentAnchor, ThreadId } from "@atomik-kb/core-client";

import { anchorRange } from "./comment-anchors";
import type { DocRange } from "./comment-anchors";
import { isBindingTransaction } from "./schema";

/** A thread the editor highlights: its anchor, and whether it is resolved. */
export interface HighlightThread {
  threadId: ThreadId;
  resolved: boolean;
  anchor: CommentAnchor;
}

/** Where a thread's anchor points now. */
export interface HighlightRange extends DocRange {
  threadId: ThreadId;
}

/** What the plugin knows: the ranges, and the anchors that could not be resolved. */
export interface CommentAnchorState {
  ranges: readonly HighlightRange[];
  orphans: readonly ThreadId[];
}

interface PluginState extends CommentAnchorState {
  threads: readonly HighlightThread[];
  showResolved: boolean;
  active: ThreadId | null;
  /** The threads changed and the ranges were not worked out for them yet. */
  dirty: boolean;
  decorations: DecorationSet;
}

type Message =
  | { type: "threads"; threads: readonly HighlightThread[]; showResolved: boolean }
  | { type: "active"; threadId: ThreadId | null }
  | { type: "ranges"; ranges: readonly HighlightRange[]; orphans: readonly ThreadId[] };

export const commentHighlightsKey = new PluginKey<PluginState>("commentHighlights");

export interface CommentHighlightsOptions {
  /** A click on a highlight: open this thread (in the panel). */
  onOpenThread: ((threadId: ThreadId) => void) | null;
}

const EMPTY: PluginState = {
  threads: [],
  showResolved: false,
  active: null,
  ranges: [],
  orphans: [],
  dirty: false,
  decorations: DecorationSet.empty,
};

function decorate(state: Pick<PluginState, "threads" | "showResolved" | "active" | "ranges">, doc: EditorState["doc"]): DecorationSet {
  const decorations: Decoration[] = [];
  for (const range of state.ranges) {
    const thread = state.threads.find((candidate) => candidate.threadId === range.threadId);
    if (thread === undefined || (thread.resolved && !state.showResolved && state.active !== thread.threadId)) {
      continue;
    }
    const classes = ["comment-highlight", thread.resolved ? "is-resolved" : "", state.active === thread.threadId ? "is-active" : ""];
    decorations.push(
      Decoration.inline(
        range.from,
        range.to,
        { class: classes.filter((name) => name !== "").join(" "), "data-comment-thread": thread.threadId },
        { inclusiveStart: false, inclusiveEnd: false },
      ),
    );
  }
  return DecorationSet.create(doc, decorations);
}

/** The ranges of every anchored thread now, and the ones that cannot be resolved; `null` while the binding is not linked yet. */
function resolveAll(threads: readonly HighlightThread[], state: EditorState): CommentAnchorState | null {
  const ranges: HighlightRange[] = [];
  const orphans: ThreadId[] = [];
  for (const thread of threads) {
    const range = anchorRange(state, thread.anchor);
    if (range === "pending") {
      return null;
    }
    if (range === null) {
      orphans.push(thread.threadId);
    } else {
      ranges.push({ threadId: thread.threadId, ...range });
    }
  }
  return { ranges, orphans };
}

function sameState(a: CommentAnchorState, b: CommentAnchorState): boolean {
  return (
    a.orphans.length === b.orphans.length &&
    a.orphans.every((id, at) => id === b.orphans[at]) &&
    a.ranges.length === b.ranges.length &&
    a.ranges.every((range, at) => {
      const other = b.ranges[at];
      return other !== undefined && range.threadId === other.threadId && range.from === other.from && range.to === other.to;
    })
  );
}

function mapped(value: PluginState, tr: Transaction): PluginState {
  if (!tr.docChanged) {
    return value;
  }
  // The binding's own transactions replace the document: the ranges are resolved again, not mapped.
  if (isBindingTransaction(tr)) {
    return { ...value, ranges: [], decorations: DecorationSet.empty };
  }
  const ranges = value.ranges.map((range) => ({ ...range, from: tr.mapping.map(range.from, 1), to: tr.mapping.map(range.to, -1) }));
  return { ...value, ranges, decorations: value.decorations.map(tr.mapping, tr.doc) };
}

/** The threads' highlights as a Tiptap extension (the editor of a page adds it with `onOpenThread`). */
export const CommentHighlights = Extension.create<CommentHighlightsOptions>({
  name: "commentHighlights",

  addOptions() {
    return { onOpenThread: null };
  },

  addProseMirrorPlugins() {
    const { onOpenThread } = this.options;
    return [
      new Plugin<PluginState>({
        key: commentHighlightsKey,
        state: {
          init: () => EMPTY,
          apply(tr, value, _old, newState) {
            let next = mapped(value, tr);
            const message = tr.getMeta(commentHighlightsKey) as Message | undefined;
            if (message?.type === "threads") {
              next = { ...next, threads: message.threads, showResolved: message.showResolved, dirty: true };
              next.decorations = decorate(next, newState.doc);
            } else if (message?.type === "active") {
              next = { ...next, active: message.threadId };
              next.decorations = decorate(next, newState.doc);
            } else if (message?.type === "ranges") {
              next = { ...next, ranges: message.ranges, orphans: message.orphans, dirty: false };
              next.decorations = decorate(next, newState.doc);
            }
            return next;
          },
        },
        props: {
          decorations: (state) => commentHighlightsKey.getState(state)?.decorations ?? DecorationSet.empty,
          handleDOMEvents: {
            // A click on a highlight opens its thread; the click goes on (the selection and the links are not its business).
            click: (_view, event) => {
              const target = event.target;
              const element = target instanceof Element ? target.closest("[data-comment-thread]") : null;
              const threadId = element?.getAttribute("data-comment-thread");
              if (threadId !== null && threadId !== undefined) {
                onOpenThread?.(threadId);
              }
              return false;
            },
          },
        },
        view: () => ({
          update: (view, prevState) => {
            const value = commentHighlightsKey.getState(view.state);
            if (value === undefined || (!value.dirty && prevState.doc === view.state.doc)) {
              return;
            }
            const next = resolveAll(value.threads, view.state);
            if (next === null) {
              return; // the binding is not linked yet: asked again at its next update
            }
            if (value.dirty || !sameState(next, value)) {
              // After this update, not inside it.
              queueMicrotask(() => {
                if (!view.isDestroyed) {
                  view.dispatch(view.state.tr.setMeta(commentHighlightsKey, { type: "ranges", ...next } satisfies Message));
                }
              });
            }
          },
        }),
      }),
    ];
  },
});

/** Gives the editor the threads to highlight (the page's own, with their anchors) and whether resolved ones show. */
export function setCommentThreads(editor: Editor, threads: readonly HighlightThread[], showResolved: boolean): void {
  if (editor.isDestroyed || commentHighlightsKey.getState(editor.state) === undefined) {
    return;
  }
  editor.view.dispatch(editor.state.tr.setMeta(commentHighlightsKey, { type: "threads", threads, showResolved } satisfies Message));
}

/** Draws one thread stronger (the one in focus in the panel); `null` for none. */
export function setActiveCommentThread(editor: Editor, threadId: ThreadId | null): void {
  if (editor.isDestroyed || commentHighlightsKey.getState(editor.state) === undefined) {
    return;
  }
  if (commentHighlightsKey.getState(editor.state)?.active === threadId) {
    return;
  }
  editor.view.dispatch(editor.state.tr.setMeta(commentHighlightsKey, { type: "active", threadId } satisfies Message));
}

/** The ranges the threads point at now and the threads whose anchor cannot be resolved. */
export function commentAnchorState(state: EditorState): CommentAnchorState {
  const value = commentHighlightsKey.getState(state);
  return value === undefined ? { ranges: [], orphans: [] } : { ranges: value.ranges, orphans: value.orphans };
}

/** Scrolls the text a thread points at into view; `false` when its anchor cannot be resolved. */
export function revealCommentThread(editor: Editor, threadId: ThreadId): boolean {
  if (editor.isDestroyed) {
    return false;
  }
  const range = commentAnchorState(editor.state).ranges.find((candidate) => candidate.threadId === threadId);
  if (range === undefined) {
    return false;
  }
  try {
    const { node } = editor.view.domAtPos(range.from);
    const element = node instanceof Element ? node : node.parentElement;
    if (typeof element?.scrollIntoView === "function") {
      element.scrollIntoView({ block: "center" });
    }
  } catch {
    return false;
  }
  return true;
}
