// The page editor: opens the page's document through its session (page-session.ts),
// mounts a Tiptap editor on it once the core's snapshot is imported, and adds the block
// UX around it (the `/` menu, the `[[` picker, the block handle, the bubble menu and the
// table and column toolbars). It also draws the comment threads' highlights (ADR-0048) and starts a comment from a selection.

import { toCoreError } from "@atomik-kb/core-client";
import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import type { Schema } from "@tiptap/pm/model";
import { EditorContent, useEditorState } from "@tiptap/react";
import type { LoroDoc } from "loro-crdt";
import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";

import { PageIcon } from "../components/PageIcon";
import { t } from "../i18n";
import { useMembers, useSpaceAccess } from "../sharing-hooks";
import type { AppStore } from "../store";
import { useAppStore, useStoreApi } from "../store-context";
import type { PageChoice } from "../store-content";
import { createFromChoice } from "../template-choices";
import { templateRootOf } from "../template-tree";
import { locate } from "../tree";
import { AiEditorProvider } from "./ai/EditorAi";
import type { AttachmentService } from "./attachments";
import { BlockHandle } from "./BlockHandle";
import { ColumnsMenu } from "./ColumnsMenu";
import { DictationEditorProvider } from "./dictation/EditorDictation";
import type { EmojiChoice } from "./emoji-data";
import { EmojiMenu } from "./emoji-menu";
import { setActiveCommentThread, setCommentThreads } from "./comment-highlights";
import type { HighlightThread } from "./comment-highlights";
import { registerCommentsHost, unregisterCommentsHost } from "./comments-host";
import { blockHighlightKey, createEditorExtensions } from "./extensions";
import { FindBar } from "./FindBar";
import { FormatMenu } from "./FormatMenu";
import { ImageUrlDialog, ImageUrlDialogStore } from "./ImageUrlDialog";
import { EditorPageContext } from "./template-context";
import { TemplatePickerHost, TemplatePickerStore } from "./TemplatePickerHost";
import { createNodeFromLoroObj, findUnknownNames, loroSyncPluginKey, NODE_NAME_KEY, ROOT_DOC_KEY } from "./loro-binding";
import type { LoroNode, LoroSyncReadOnly } from "./loro-binding";
import type { MentionChoice } from "./mention-menu";
import { MentionMenuView } from "./MentionMenu";
import { referencedTitle } from "./node-views";
import { attachPageAreaDrop } from "./page-drop";
import { PagePresence, followWindowActivity } from "./page-presence";
import { setPageLock } from "./page-lock";
import { FAILED_FLUSHES_NOTICE } from "./page-session";
import type { PageDocSession, PageDocSnapshot } from "./page-session";
import { createEditorSchema } from "./schema";
import type { SlashCommand } from "./slash-commands";
import { SlashMenu } from "./slash-menu";
import { SuggestionMenu, SuggestionMenuStore } from "./suggestion-menu";
import { TableMenu } from "./TableMenu";

const OPENING: PageDocSnapshot = { status: "opening", doc: null, error: null, failedFlushes: 0 };
const noSubscription = (): (() => void) => () => undefined;
const opening = (): PageDocSnapshot => OPENING;

let schema: Schema | null = null;

/** The schema, built once: what the editor will build from the same extensions. */
function editorSchema(): Schema {
  schema ??= createEditorSchema();
  return schema;
}

interface Start {
  /** The document as the editor's initial content (what the binding then shows). */
  content: JSONContent;
  /** Set when the document holds content the editor cannot represent. */
  readOnly: LoroSyncReadOnly | null;
}

/**
 * What the editor starts with, read from the imported snapshot. `null` for a document
 * without a root node: the core creates every page document with one, so the editor never
 * mounts on an empty document (and the binding would refuse to write into one).
 */
function startOf(doc: LoroDoc): Start | null {
  const root = doc.getMap(ROOT_DOC_KEY);
  if (root.size === 0 || root.get(NODE_NAME_KEY) === undefined) {
    return null;
  }
  const unknown = findUnknownNames(editorSchema(), root as unknown as LoroNode);
  const node = createNodeFromLoroObj(editorSchema(), root as unknown as LoroNode, new Map());
  const readOnly =
    unknown.nodes.length > 0 || unknown.marks.length > 0 ? { reason: "unknown-content" as const, ...unknown } : null;
  return { content: node.toJSON() as JSONContent, readOnly };
}

/** Holds the editor instance for `useSyncExternalStore` (created and destroyed by an effect). */
class EditorHolder {
  #editor: Editor | null = null;
  readonly #listeners = new Set<() => void>();

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly get = (): Editor | null => this.#editor;

  set(editor: Editor | null): void {
    this.#editor = editor;
    for (const listener of [...this.#listeners]) {
      listener();
    }
  }
}

/** The session of a page, held while the component shows it. */
function usePageSession(pageId: PageId): PageDocSession | null {
  const sessions = useAppStore((state) => state.pageDocs);
  const session = useSyncExternalStore(sessions.subscribe, () => sessions.peek(pageId));
  useEffect(() => {
    const held = sessions.acquire(pageId);
    return () => {
      sessions.release(held);
    };
  }, [sessions, pageId]);
  return session;
}

/**
 * Uploads into one space through the store (attachment blocks of the page land there). The link
 * of "Image from URL" is asked for in `imageUrlDialog` and goes to the shell through the store.
 */
function attachmentService(store: AppStore, spaceId: SpaceId, imageUrlDialog: ImageUrlDialogStore): AttachmentService {
  const { attachmentCapabilities } = store.getState();
  return {
    canUpload: attachmentCapabilities.upload,
    canImport: attachmentCapabilities.importPath,
    upload: (file, onProgress) => store.getState().uploadAttachment(spaceId, file, onProgress),
    importFile: () => store.getState().importAttachment(spaceId),
    canImportUrl: attachmentCapabilities.importUrl,
    importUrl: (url) => store.getState().importAttachmentFromUrl(spaceId, url),
    askUrl: imageUrlDialog.ask,
  };
}

/** The file name of an attachment the store knows, for copies of file blocks. */
function knownAttachmentName(store: AppStore, attachmentId: string): string | null {
  const lookup = store.getState().attachmentLookup[attachmentId];
  return lookup?.status === "found" ? lookup.info.name : null;
}

interface PageEditorProps {
  pageId: PageId;
  spaceId: SpaceId;
  /**
   * Told about the editor once it is mounted (so the page view can read the live document, as "Page info"
   * does), and `null` when it goes. Keep it stable: a new function is a new call.
   */
  onEditor?: (editor: Editor | null) => void;
  /**
   * The page is locked (ADR-0040), or this account's role cannot edit its space (Milestone 13): the editor is read-only
   * and writes nothing, until it is not. It follows the prop while it is open (`page-lock.ts`); the banner about content
   * this version cannot show is not drawn for it.
   */
  locked?: boolean;
  /**
   * The editor of the peek of a database row (ADR-0054, `docs/architecture/database-views.md` section 8), the second editor
   * on screen: the page in front keeps every single slot. A peek's editor is not announced as this device's page (presence),
   * is not the comments panel's or the AI chat's editor, has no dictation (it targets the main editor) and no comment
   * button (the peek shows no comments), and its find bar answers Ctrl+F only while the focus is in the peek.
   */
  peek?: boolean;
}

/** The editor of one page (of the space `spaceId`), below the page's title. */
export function PageEditor({ pageId, spaceId, onEditor, locked = false, peek = false }: PageEditorProps) {
  const session = usePageSession(pageId);
  const snapshot = useSyncExternalStore(session?.subscribe ?? noSubscription, session?.getSnapshot ?? opening);

  if (session === null || snapshot.status === "opening") {
    return (
      <p className="editor-status muted" role="status">
        {t("editor.loading")}
      </p>
    );
  }
  if (snapshot.status === "failed" || snapshot.doc === null) {
    return (
      <div className="banner banner-warning editor-error" role="alert">
        <p>{t("editor.openError", { message: snapshot.error?.message ?? "" })}</p>
        <button
          type="button"
          className="button"
          onClick={() => {
            session.retry();
          }}
        >
          {t("editor.retry")}
        </button>
      </div>
    );
  }
  return (
    // The node views of the page ask it whether they are in a template (ADR-0056).
    <EditorPageContext value={pageId}>
      <DocEditor
        key={pageId}
        pageId={pageId}
        spaceId={spaceId}
        session={session}
        doc={snapshot.doc}
        onEditor={onEditor}
        locked={locked}
        peek={peek}
      />
    </EditorPageContext>
  );
}

interface DocEditorProps {
  pageId: PageId;
  spaceId: SpaceId;
  session: PageDocSession;
  doc: LoroDoc;
  onEditor?: (editor: Editor | null) => void;
  locked: boolean;
  peek: boolean;
}

function DocEditor({ pageId, spaceId, session, doc, onEditor, locked, peek }: DocEditorProps) {
  const store = useStoreApi();
  const [start] = useState(() => startOf(doc));
  const [holder] = useState(() => new EditorHolder());
  const [slashMenu] = useState(() => new SuggestionMenuStore<SlashCommand>());
  const [pagePicker] = useState(() => new SuggestionMenuStore<PageChoice>());
  const [emojiPicker] = useState(() => new SuggestionMenuStore<EmojiChoice>());
  const [mentionMenu] = useState(() => new SuggestionMenuStore<MentionChoice>());
  const [imageUrlDialog] = useState(() => new ImageUrlDialogStore());
  const [templatePicker] = useState(() => new TemplatePickerStore());
  const [subPageError, setSubPageError] = useState<string | null>(null);
  // The page's presence in a shared space (Milestone 13, slice 3), made with the editor and followed by the effect below.
  const presenceRef = useRef<PagePresence | null>(null);

  // A dialog still open when the page goes away gives up its ask: the editor is not waiting any more.
  useEffect(
    () => () => {
      imageUrlDialog.cancel();
      templatePicker.finish(null);
    },
    [imageUrlDialog, templatePicker],
  );

  useEffect(() => {
    if (start === null) {
      return;
    }
    const readOnly = start.readOnly !== null;
    // Presence (ADR-0050): the other members' cursors and this editor's own, when the transport carries it. Whether anything is
    // sent or drawn follows the space's access (`configure` below): a personal space's page sends nothing. A device is on one
    // page at a time, the one in front: the peek's page is not announced (and shows nobody's cursor).
    const { presenceAvailable, sendPresence, subscribePresence } = store.getState();
    const presence =
      presenceAvailable && !peek
        ? new PagePresence({ spaceId, pageId, peerKey: doc.peerIdStr, send: sendPresence, subscribe: subscribePresence })
        : null;
    const editor = new Editor({
      extensions: createEditorExtensions({
        doc,
        pageId,
        readOnly,
        slashMenu,
        pagePicker,
        emojiPicker,
        mentionMenu,
        findPages: (query) => store.getState().findPages(query),
        spaceId,
        presence,
        findPeople: () => store.getState().ensurePeople(spaceId),
        inTemplate: () => templateRootOf(store.getState().tree, pageId) !== null,
        onOpenThread: (threadId) => {
          store.getState().openComments({ threadId });
        },
        openLink: (href) => {
          void store.getState().openExternal(href);
        },
        pageTitle: (id) => referencedTitle(store.getState(), id),
        attachments: attachmentService(store, spaceId, imageUrlDialog),
        attachmentName: (id) => knownAttachmentName(store, id),
        subPages: {
          // The page is made first and opened after the card to it is in the document and saved.
          create: async () => {
            setSubPageError(null);
            try {
              return (await store.getState().createPage({ parentId: pageId, select: false })).id;
            } catch (error) {
              setSubPageError(t("subpage.error", { message: toCoreError(error).message }));
              throw error;
            }
          },
          open: async (childId) => {
            await session.flush();
            store.getState().focusPageTitle(childId);
            store.getState().selectPage(childId);
          },
          // ADR-0052: the sidebar's sort of the open page's sub-pages, from the `/` menu.
          count: () => {
            const { tree, currentSpaceId } = store.getState();
            return tree === null || currentSpaceId !== spaceId ? 0 : (locate(tree.roots, pageId)?.node.children.length ?? 0);
          },
          sort: async (by) => {
            setSubPageError(null);
            try {
              await store.getState().sortChildren(spaceId, pageId, by);
            } catch (error) {
              setSubPageError(t("tree.error.sort", { message: toCoreError(error).message }));
            }
          },
          // ADR-0056: `/template` makes a sub-page from a template or a starter and puts its card in the page.
          templates: {
            choose: templatePicker.ask,
            create: async (choice) => {
              setSubPageError(null);
              try {
                return (await createFromChoice(store.getState(), choice, { parentId: pageId, open: false })).id;
              } catch (error) {
                setSubPageError(t("template.error.use", { message: toCoreError(error).message }));
                throw error;
              }
            },
            open: async (childId) => {
              await session.flush();
              store.getState().selectPage(childId);
            },
          },
        },
      }),
      content: start.content,
      editable: !readOnly,
      editorProps: {
        attributes: {
          class: "page-editor-content",
          role: "textbox",
          "aria-multiline": "true",
          "aria-label": t("editor.label"),
        },
      },
      onBlur: () => {
        void session.flush();
      },
    });
    holder.set(editor);
    presenceRef.current = presence;
    const stopFollowing = presence === null ? undefined : followWindowActivity(presence);
    return () => {
      stopFollowing?.();
      holder.set(null);
      editor.destroy();
      // After the editor: its cursor plugin clears this device's cursor on its way out, and the last frame is the Idle one.
      presence?.destroy();
      presenceRef.current = null;
    };
  }, [doc, session, start, store, holder, slashMenu, pagePicker, emojiPicker, mentionMenu, imageUrlDialog, templatePicker, spaceId, pageId, peek]);

  const editor = useSyncExternalStore(holder.subscribe, holder.get);
  // A page dragged from the sidebar is accepted over the whole page area, not only over the text (the peek has its own area).
  // After the mount: the editor's element is in the page once the editor is rendered.
  useEffect(() => {
    const area = peek || editor === null ? null : editor.view.dom.closest<HTMLElement>(".content-body");
    return area === null || editor === null ? undefined : attachPageAreaDrop(area, editor.view, { spaceId, pageId });
  }, [editor, peek, spaceId, pageId]);
  // The lock flips with the page (this device's menu or another device's change), the editor stays. A layout
  // effect, so that a locked page is never drawn editable: the view is made when the editor is mounted (a child
  // of this component), which runs before it.
  useLayoutEffect(() => {
    if (editor !== null && start?.readOnly === null) {
      setPageLock(editor, locked);
    }
  }, [editor, locked, start]);
  useEffect(() => {
    onEditor?.(editor);
    return () => {
      onEditor?.(null);
    };
  }, [editor, onEditor]);
  const bindingReadOnly = useEditorState({
    editor,
    selector: ({ editor: current }) =>
      current === null ? null : (loroSyncPluginKey.getState(current.state)?.readOnlyReason ?? null),
  });
  const failedFlushes = useSyncExternalStore(session.subscribe, () => session.getSnapshot().failedFlushes);
  useScrollToBlock(editor, pageId);
  useCommentHighlights(editor, pageId, !peek);
  const access = useSpaceAccess(spaceId);
  // The peek shows no comments (the panel is the page in front's), so its selection toolbar offers none either.
  const canComment = access.canComment && !peek;
  // Presence goes in a live shared space only, and a device that may not write the page views it (a Viewer's and a locked page's).
  const presenceEnabled = access.shared && !access.archived && !access.pending;
  const presenceCanEdit = access.canEdit && !locked;
  const selfAccount = useMembers(spaceId)?.members.find((member) => member.isMe)?.accountId;
  useEffect(() => {
    presenceRef.current?.configure({
      enabled: presenceEnabled,
      canEdit: presenceCanEdit,
      ...(selfAccount === undefined ? {} : { self: selfAccount }),
    });
  }, [editor, presenceEnabled, presenceCanEdit, selfAccount]);

  if (start === null) {
    return (
      <div className="banner banner-warning editor-error" role="alert">
        <p>{t("editor.emptyDocument")}</p>
      </div>
    );
  }
  // A locked page says so in its header; the warning is for content the editor cannot show.
  const readOnly = start.readOnly ?? (bindingReadOnly?.reason === "requested" ? null : bindingReadOnly);

  return (
    <AiEditorProvider editor={editor} pageId={pageId} spaceId={spaceId} front={!peek}>
      <Dictation editor={editor} enabled={!peek}>
        <div className="page-editor">
          {readOnly !== null && (
            <p className="banner banner-warning editor-read-only" role="status">
              {readOnly.reason === "empty-document" ? t("editor.emptyDocument") : t("editor.readOnly")}
            </p>
          )}
          {subPageError !== null && (
            <p className="banner banner-warning editor-sub-page-error" role="alert">
              {subPageError}
            </p>
          )}
          {failedFlushes >= FAILED_FLUSHES_NOTICE && (
            <p className="editor-unsaved" role="status">
              {t("editor.unsaved")}
            </p>
          )}
          {editor !== null && (
            <>
              <FindBar editor={editor} />
              <EditorContent editor={editor} className="page-editor-body" />
              <BlockHandle editor={editor} pageId={pageId} />
              <FormatMenu
                editor={editor}
                canComment={canComment}
                onComment={(anchor) => {
                  store.getState().beginComment({ pageId, anchor });
                }}
              />
              <TableMenu editor={editor} />
              <ColumnsMenu editor={editor} />
              <div
                className="page-editor-tail"
                aria-hidden="true"
                onMouseDown={(event) => {
                  event.preventDefault();
                  focusEnd(editor);
                }}
              />
            </>
          )}
          <SlashMenu store={slashMenu} editor={editor} />
          <ImageUrlDialog store={imageUrlDialog} />
          <TemplatePickerHost store={templatePicker} />
          <EmojiMenu store={emojiPicker} />
          <MentionMenuView store={mentionMenu} />
          <SuggestionMenu
            store={pagePicker}
            label={t("picker.label")}
            emptyText={t("picker.empty")}
            itemKey={(page) => page.pageId}
            renderItem={(page) => (
              <>
                <PageIcon icon={page.icon} className="suggestion-icon" />
                <span className="suggestion-title">{page.title === "" ? t("page.untitled") : page.title}</span>
              </>
            )}
          />
        </div>
      </Dictation>
    </AiEditorProvider>
  );
}

/** The dictation of the editor (ADR-0053); the peek's editor has none: dictation targets the editor of the page in front. */
function Dictation({ editor, enabled, children }: { editor: Editor | null; enabled: boolean; children: ReactNode }) {
  return enabled ? <DictationEditorProvider editor={editor}>{children}</DictationEditorProvider> : children;
}

/** A click below the last block puts the cursor at the end, in a new paragraph if needed. */
function focusEnd(editor: Editor): void {
  if (!editor.isEditable) {
    return;
  }
  const { doc } = editor.state;
  const last = doc.lastChild;
  if (last?.type.name === "paragraph" && last.content.size === 0) {
    editor.commands.focus("end");
    return;
  }
  editor.chain().insertContentAt(doc.content.size, { type: "paragraph" }).focus("end").run();
}

/** Where the node with block id `blockId` starts, if the document has one. */
function findBlock(editor: Editor, blockId: string): number | null {
  let found: number | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (found !== null) {
      return false;
    }
    if (node.attrs.id === blockId) {
      found = pos;
      return false;
    }
    return true;
  });
  return found;
}

/** Scrolls a block into view and highlights it for a moment. */
function showBlock(editor: Editor, blockId: string): void {
  if (editor.isDestroyed) {
    return;
  }
  const pos = findBlock(editor, blockId);
  const element = pos === null ? null : editor.view.nodeDOM(pos);
  if (pos === null || !(element instanceof HTMLElement)) {
    return;
  }
  element.scrollIntoView({ block: "center" });
  editor.view.dispatch(editor.state.tr.setMeta(blockHighlightKey, pos));
  setTimeout(() => {
    if (!editor.isDestroyed) {
      editor.view.dispatch(editor.state.tr.setMeta(blockHighlightKey, null));
    }
  }, 1600);
}

/**
 * Registers the editor for the comments panel and keeps the highlights of the page's anchored threads in step with the store:
 * the threads (with their anchors), whether resolved ones show, and the thread in focus (ADR-0048). `enabled` is false for the
 * editor of a peek (ADR-0054): the panel and the highlights are the page in front's, so the peek's editor never registers (and
 * so never replaces the main editor, nor leaves the slot empty when it goes).
 */
function useCommentHighlights(editor: Editor | null, pageId: PageId, enabled: boolean): void {
  const pageComments = useAppStore((state) => state.pageComments);
  const showResolved = useAppStore((state) => state.showResolved);
  const focused = useAppStore((state) => state.commentFocus?.threadId ?? null);
  const threads = enabled && pageComments?.pageId === pageId ? pageComments.threads : null;
  useEffect(() => {
    if (editor === null || !enabled) {
      return;
    }
    registerCommentsHost({ pageId, editor });
    return () => {
      unregisterCommentsHost(editor);
    };
  }, [editor, pageId, enabled]);
  useEffect(() => {
    if (editor === null || threads === null) {
      return;
    }
    const anchored: HighlightThread[] = threads.flatMap((thread) =>
      thread.anchor === undefined ? [] : [{ threadId: thread.threadId, resolved: thread.resolved, anchor: thread.anchor }],
    );
    setCommentThreads(editor, anchored, showResolved);
  }, [editor, threads, showResolved]);
  useEffect(() => {
    if (editor !== null && enabled) {
      setActiveCommentThread(editor, focused);
    }
  }, [editor, focused, enabled]);
}

/** Brings the block a search result or backlink pointed at into view, once. */
function useScrollToBlock(editor: Editor | null, pageId: PageId): void {
  const target = useAppStore((state) => state.scrollTarget);
  const clearScrollTarget = useAppStore((state) => state.clearScrollTarget);
  useEffect(() => {
    if (editor === null || target?.pageId !== pageId) {
      return;
    }
    clearScrollTarget();
    // After the binding has shown the document (it does so in a timeout of its own).
    setTimeout(() => {
      showBlock(editor, target.blockId);
    }, 0);
  }, [editor, target, pageId, clearScrollTarget]);
}
