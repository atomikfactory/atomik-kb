// The host of the peek of a database row (ADR-0054, `docs/architecture/database-views.md` section 8): the side panel
// (resizable, at the right of the main area, like the chat and comments panels) or the large dialog (`Modal` over the page)
// that shows the page of `state.peek`: its icon, title and properties panel, and its editor, with the buttons "Open as page",
// "Open in new tab" and Close. It shows no comments and no backlinks ("Open as page" gives them).
//
// The peek is the second page on screen and never the page in front: it reads its page from the tree (`PageInfo` follows the
// title, the icon and the lock live), mounts a `PageEditor` with `peek` (which keeps the single slots of the page in front
// to the main editor: presence, the comments panel's and the chat's editor, dictation, Ctrl+F), has its own title id
// (`peek-title`) and never calls `loadBacklinks` or `loadComments`. It changes neither `selectedPageId`, `activeTabId` nor
// a tab's history; `store-peek.ts` closes it when any of them changes.
//
// Esc closes it, after anything inside that took the key (a menu, a picker, the slash menu, the find bar, a field being
// edited: they cancel the key's default or stop it). The editor's own Escape, which selects the block the cursor is in and
// clears a range of blocks, is not a popup and does not keep the peek open. Esc also closes it when the focus is on nothing (the
// `<body>`: the rename field of the title went away with Enter, a button inside was removed), unless a popup, a menu or a dialog
// outside the peek is open or something took the key; the focus in the page in front's editor or fields is theirs. Enter in the
// title's field moves the caret to the start of the peek's text, so the focus does not land on the body. Closing with the Close
// button, Esc or a click on the dialog's backdrop returns the focus to the element that had it when the peek opened (the row).
// Both frames carry `data-peek`, which the find bar and the print styles read.
//
// A template (ADR-0056) opened here ("New template" and "Edit" of a database's templates) shows the template banner, compact: its
// "Use template" makes the page under the template's parent and shows it in the peek as a click on a row would.

import { toCoreError } from "@atomik-kb/core-client";
import type { PageInfo } from "@atomik-kb/core-client";
import { Selection } from "@tiptap/pm/state";
import type { Editor } from "@tiptap/core";
import { useCallback, useEffect, useEffectEvent, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent, RefObject } from "react";

import { openRow } from "../editor/open-row";
import { PageEditor } from "../editor/PageEditor";
import { t } from "../i18n";
import { useSpaceAccess } from "../sharing-hooks";
import { useAppStore, useStoreApi } from "../store-context";
import type { PeekTarget } from "../store-peek";
import { selectCurrentCanEdit } from "../store-sharing";
import { selectPageLocked } from "../store-structure";
import { findPage } from "../tree";
import { CloseIcon, HistoryIcon } from "./icons";
import { InlineEdit } from "./InlineEdit";
import { Modal } from "./Modal";
import { PageHistoryDialog } from "./PageHistoryDialog";
import { PageIcon } from "./PageIcon";
import { pageLookClasses } from "./PageLook";
import { PeekResizeHandle } from "./PeekResizeHandle";
import { PropertiesPanel } from "./PropertiesPanel";
import { TemplateBanner } from "./TemplateBanner";

/** The id of the peek's title: the page's own is `page-title`, and a document has one of each. */
const PEEK_TITLE_ID = "peek-title";

/** What the editor draws on the blocks its Escape selected (`editor/block-selection.ts`; print hides it too). */
const BLOCK_SELECTED = ".is-block-selected";

/** What an Esc with the focus on nothing could be meant for when it is open outside the peek: it keeps the peek open. */
const POPUPS = '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [aria-modal="true"]';

/** Whether a popup, a menu or a dialog is open that is neither part of the peek nor holds it (the center peek's dialog does). */
function popupOutsidePeek(): boolean {
  return Array.from(document.querySelectorAll(POPUPS)).some(
    (popup) => popup.closest("[data-peek]") === null && popup.querySelector("[data-peek]") === null,
  );
}

/** Puts the caret at the start of the editor's text and the focus in it (at once: Tiptap's own `focus` command takes a frame). */
function focusEditorStart(editor: Editor): void {
  if (editor.isDestroyed) {
    return;
  }
  editor.view.dispatch(editor.state.tr.setSelection(Selection.atStart(editor.state.doc)));
  editor.view.focus();
}

/**
 * The element that had the focus last outside the peek, kept for the focus to go back to: the row (or card, or chip) whose
 * click opened it. A focus change inside the peek (a click on a row of a database in the peeked page) is not remembered.
 */
function useFocusOutsidePeek(): RefObject<HTMLElement | null> {
  const last = useRef<HTMLElement | null>(null);
  useEffect(() => {
    function onFocusIn(event: FocusEvent): void {
      const target = event.target;
      if (target instanceof HTMLElement && target.closest("[data-peek]") === null) {
        last.current = target;
      }
    }
    document.addEventListener("focusin", onFocusIn);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
    };
  }, []);
  return last;
}

/** The peek of a database row, drawn beside the page (`MainLayout` puts it in the main area's grid) or over it. */
export function PeekHost() {
  const peek = useAppStore((state) => state.peek);
  const outside = useFocusOutsidePeek();
  if (peek === null) {
    return null;
  }
  return peek.mode === "center" ? <CenterPeek peek={peek} outside={outside} /> : <SidePeek peek={peek} outside={outside} />;
}

interface FrameProps {
  peek: PeekTarget;
  /** The element that had the focus outside the peek. */
  outside: RefObject<HTMLElement | null>;
}

/**
 * What a frame needs: the element the focus goes back to (the one that held it when the peek opened or swapped its page), and
 * `close` (the peek's own ways of closing: it hands the focus back first, so that it is not lost with the peek) and `onKeyDown`
 * for the frame (Esc).
 */
function useFrame({ peek, outside }: FrameProps) {
  const closePeek = useAppStore((state) => state.closePeek);
  const opener = useRef<HTMLElement | null>(null);
  // Whether blocks were selected when the key came (the editor's Escape clears them: it consumes the key too).
  const blocksSelected = useRef(false);

  // A new page in the peek (it opened, or a row swapped it): the element that opened it is the one that held the focus.
  useLayoutEffect(() => {
    opener.current = outside.current;
  }, [peek.pageId, outside]);

  function close(): void {
    const row = opener.current;
    if (row?.isConnected === true) {
      row.focus({ preventScroll: true });
    }
    closePeek();
  }

  function onKeyDownCapture(event: KeyboardEvent<HTMLElement>): void {
    if (event.key === "Escape") {
      blocksSelected.current = event.currentTarget.querySelector(BLOCK_SELECTED) !== null;
    }
  }

  // Esc with the focus on nothing (the `<body>`, which is where it lands when what held it went away): the key is the peek's
  // unless something took it (a handler that cancels the default or stops the key never reaches here: this listens on the
  // document, last) or a popup, a menu or a dialog outside the peek is open. A key from a field, a button or the editor of the
  // page in front has its own target and is theirs; the frame's `onKeyDown` takes the keys of what is inside the peek.
  const closeOnBody = useEffectEvent((event: globalThis.KeyboardEvent): void => {
    const target = event.target;
    const onNothing = target === document.body || target === document.documentElement || target === document;
    if (event.key === "Escape" && onNothing && !event.defaultPrevented && !event.isComposing && !popupOutsidePeek()) {
      close();
    }
  });
  useEffect(() => {
    function onDocumentKeyDown(event: globalThis.KeyboardEvent): void {
      closeOnBody(event);
    }
    document.addEventListener("keydown", onDocumentKeyDown);
    return () => {
      document.removeEventListener("keydown", onDocumentKeyDown);
    };
  }, []);

  function onKeyDown(event: KeyboardEvent<HTMLElement>): void {
    // A key from a dialog opened over the peek (another portal) bubbles here through React: the dialog's own.
    if (event.key !== "Escape" || !(event.target instanceof Node) || !event.currentTarget.contains(event.target)) {
      return;
    }
    // Taken here, so that the dialog's own Escape (it does not look at `defaultPrevented`) never sees a key that something
    // inside the peek consumed.
    event.stopPropagation();
    // The editor takes every Escape (it selects the block, or clears the blocks selected): when that is all it did with it, no
    // popup took the key and it closes the peek. A menu, a picker or the slash menu that took it leaves blocks as they were.
    const editorOnly = event.currentTarget.querySelector(BLOCK_SELECTED) !== null || blocksSelected.current;
    if ((!event.defaultPrevented || editorOnly) && !event.nativeEvent.isComposing) {
      close();
    }
  }

  return { opener, close, onKeyDown, onKeyDownCapture };
}

/** The panel at the right of the page: a column of the main area, a copy of the chat's and comments' columns. */
function SidePeek({ peek, outside }: FrameProps) {
  const frame = useRef<HTMLElement>(null);
  const { close, onKeyDown, onKeyDownCapture } = useFrame({ peek, outside });

  // The focus moves into the peek when it opens and when a row swaps its page (a click on a row of the page in front left it
  // there, or on nothing at all when the row that held it went away with the old page).
  useLayoutEffect(() => {
    const panel = frame.current;
    if (panel !== null && !panel.contains(document.activeElement)) {
      panel.focus({ preventScroll: true });
    }
  }, [peek.pageId]);

  return (
    <aside
      ref={frame}
      className="peek"
      data-peek=""
      data-mode="side"
      tabIndex={-1}
      aria-labelledby={PEEK_TITLE_ID}
      onKeyDownCapture={onKeyDownCapture}
      onKeyDown={onKeyDown}
    >
      <PeekResizeHandle />
      <PeekPage key={peek.pageId} peek={peek} onClose={close} />
    </aside>
  );
}

/** The large dialog over the page. */
function CenterPeek({ peek, outside }: FrameProps) {
  const frame = useRef<HTMLDivElement>(null);
  const { opener, close, onKeyDown, onKeyDownCapture } = useFrame({ peek, outside });
  return (
    <Modal labelledBy={PEEK_TITLE_ID} onDismiss={close} className="peek-dialog" initialFocus={frame} returnFocus={opener}>
      <div
        ref={frame}
        className="peek"
        data-peek=""
        data-mode="center"
        tabIndex={-1}
        onKeyDownCapture={onKeyDownCapture}
        onKeyDown={onKeyDown}
      >
        <PeekPage key={peek.pageId} peek={peek} onClose={close} />
      </div>
    </Modal>
  );
}

/** The page of the peek: the tree's (live), else the lookup's while the tree has not caught up with a page made a moment ago. */
function usePeekPage(peek: PeekTarget): { page: PageInfo | null; gone: boolean } {
  const tree = useAppStore((state) => state.tree);
  const lookup = useAppStore((state) => state.pageLookup[peek.pageId]);
  const lookupPage = useAppStore((state) => state.lookupPage);
  const fromTree = useMemo(() => (tree === null ? null : findPage(tree.roots, peek.pageId)), [tree, peek.pageId]);
  useEffect(() => {
    if (fromTree === null) {
      lookupPage(peek.pageId);
    }
  }, [fromTree, lookupPage, peek.pageId]);
  const gone = lookup?.status === "missing" || (lookup?.status === "found" && lookup.page.trashedAt !== undefined);
  if (fromTree !== null) {
    return { page: fromTree, gone: false };
  }
  return { page: lookup?.status === "found" && !gone ? lookup.page : null, gone };
}

interface PeekPageProps {
  peek: PeekTarget;
  onClose: () => void;
}

/** The toolbar, the title, the properties and the editor of the peeked page. */
function PeekPage({ peek, onClose }: PeekPageProps) {
  const store = useStoreApi();
  const { page, gone } = usePeekPage(peek);
  const renamePage = useAppStore((state) => state.renamePage);
  const closePeek = useAppStore((state) => state.closePeek);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const titleFocus = useAppStore((state) => state.titleFocusPageId);
  const focusPageTitle = useAppStore((state) => state.focusPageTitle);
  const access = useSpaceAccess(peek.spaceId);
  // A page made from a row's "New page" opens with its title ready to be typed, as on the page screen.
  const [renaming, setRenaming] = useState(titleFocus === peek.pageId);
  const [notice, setNotice] = useState<string | null>(null);
  // The page history dialog (ADR-0057), over the peek, with the button that opened it (where focus returns).
  const [history, setHistory] = useState<Element | null>(null);
  // The editor of the page, told once it is mounted: Enter in the title's field moves the caret into it.
  const editorRef = useRef<Editor | null>(null);
  const holdEditor = useCallback((editor: Editor | null) => {
    editorRef.current = editor;
  }, []);

  // The request was taken when the peek mounted (`renaming` above); it is not asked twice.
  useEffect(() => {
    if (titleFocus === peek.pageId) {
      focusPageTitle(null);
    }
  }, [titleFocus, peek.pageId, focusPageTitle]);

  // As the page screen decides it (`PageView.tsx`): a lock is a guard, a role that cannot edit a permission; both make the
  // title, the properties and the text read-only.
  const locked = page?.locked === true;
  const readOnly = locked || !access.canEdit;

  /** "Open as page": the tab in front shows it (the peek closes with the page in front changing). */
  function openAsPage(): void {
    closePeek();
    navigateTo({ pageId: peek.pageId, spaceId: peek.spaceId, disposition: "current" });
  }

  /**
   * "Use template" of the banner made a page: it opens as a click on a row of a database would, in the peek's own mode (the
   * peek swaps its page), or as a page when the page in front is it.
   */
  function showMade(made: PageInfo): void {
    const state = store.getState();
    openRow(state, { pageId: made.id, spaceId: made.spaceId }, { openIn: peek.mode }, "current", state.selectedPageId);
  }

  /** "Open in new tab": a tab in front shows it. */
  function openInNewTab(): void {
    closePeek();
    navigateTo({ pageId: peek.pageId, spaceId: peek.spaceId, disposition: "foreground" });
  }

  if (page === null) {
    return (
      <div className="peek-page">
        <div className="peek-toolbar">
          <span className="peek-toolbar-spacer" />
          <CloseButton onClose={onClose} />
        </div>
        <div className="peek-scroll">
          <h2 className="title page-title peek-page-title" id={PEEK_TITLE_ID}>
            {t("peek.label")}
          </h2>
          <p className="peek-status muted" role="status">
            {gone ? t("peek.gone") : t("peek.loading")}
          </p>
        </div>
      </div>
    );
  }

  const title = page.title === "" ? t("page.untitled") : page.title;
  return (
    <div className={`peek-page ${pageLookClasses(page)}`}>
      <div className="peek-toolbar">
        <button type="button" className="button button-small" onClick={openAsPage}>
          {t("peek.openAsPage")}
        </button>
        <button type="button" className="button button-small" onClick={openInNewTab}>
          {t("page.openInNewTab")}
        </button>
        <span className="peek-toolbar-spacer" />
        <button
          type="button"
          className="icon-button peek-history"
          aria-label={t("history.dialog.menu")}
          title={t("history.dialog.menu")}
          aria-haspopup="dialog"
          onClick={(event) => {
            setHistory(event.currentTarget);
          }}
        >
          <HistoryIcon />
        </button>
        <CloseButton onClose={onClose} />
      </div>
      <div className="peek-scroll">
        <TemplateBanner page={page} compact onUse={showMade} />
        <h2 className="title page-title peek-page-title" id={PEEK_TITLE_ID}>
          {page.icon !== undefined && <PageIcon icon={page.icon} className="page-title-icon" />}
          {renaming && !readOnly ? (
            <InlineEdit
              className="page-title-edit"
              value={page.title}
              label={t("tree.renameLabel")}
              placeholder={t("page.untitled")}
              onCommit={async (next) => {
                // A lock that came while the field was open wins: the tree is read now, not as this render saw it.
                if (selectPageLocked(store.getState(), page.id) || !selectCurrentCanEdit(store.getState())) {
                  return;
                }
                try {
                  await renamePage(page.id, next);
                } catch (error) {
                  setNotice(t("tree.error.rename", { message: toCoreError(error).message }));
                }
              }}
              onDone={() => {
                setRenaming(false);
              }}
              onEnter={() => {
                if (editorRef.current !== null) {
                  focusEditorStart(editorRef.current);
                }
              }}
            />
          ) : readOnly ? (
            <span className={page.title === "" ? "page-title-text is-untitled" : "page-title-text"} title={locked ? t("page.locked.hint") : undefined}>
              {title}
            </span>
          ) : (
            <button
              type="button"
              className={page.title === "" ? "page-title-button is-untitled" : "page-title-button"}
              title={t("page.renameHint")}
              onClick={() => {
                setRenaming(true);
              }}
            >
              {title}
            </button>
          )}
        </h2>
        {notice !== null && (
          <p className="page-notice muted" role="status">
            {notice}
          </p>
        )}
        <PropertiesPanel page={page} readOnly={readOnly ? true : undefined} />
        <PageEditor key={page.id} pageId={page.id} spaceId={page.spaceId} onEditor={holdEditor} locked={readOnly} peek />
      </div>
      {history !== null && (
        <PageHistoryDialog
          key={`history-${page.id}`}
          page={page}
          returnFocus={history}
          onClose={() => {
            setHistory(null);
          }}
        />
      )}
    </div>
  );
}

function CloseButton({ onClose }: { onClose: () => void }) {
  return (
    <button type="button" className="icon-button peek-close" aria-label={t("peek.close")} title={t("peek.close")} onClick={onClose}>
      <CloseIcon />
    </button>
  );
}
