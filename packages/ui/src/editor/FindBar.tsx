// The find bar of the page editor: Ctrl+F (Cmd+F on macOS) opens it from the page, "Find in page" in
// the page menu opens it too. It sits fixed at the top right of the page area, with the search
// field (focused, prefilled with a short selection), "n of m", previous and next, the match case
// and whole word toggles, a Replace disclosure (not in a read-only page) and a close button.
// The search itself, its matches and the replacing are `find-replace.ts`; this is the face of it.
//
// Ctrl+F is taken on the document (the editor's keys, in the page or out of it, bubble there) and
// the default is prevented, which is what keeps WebView2's and the browsers' own find bar away.
// The bar belongs to the editor of one page: opening another page mounts another editor and its
// bar is closed. While a peek of a database row is open (ADR-0054) two editors are on screen, each with its bar: Ctrl+F and
// "Find in page" open the bar of the editor that holds the focus (the peek's when the focus is inside it, else the page's).

import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { useEditorState } from "@tiptap/react";
import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";

import { t } from "../i18n";
import { findStateOf } from "./find-replace";
import { FIND_IN_PAGE_EVENT } from "./find-request";
import { loroSyncPluginKey } from "./loro-binding";

export { requestFindInPage } from "./find-request";

/** A selection longer than this is not a word to look for. */
const MAX_SEED_LENGTH = 100;

/** The selected text when it is short and on one line (what the bar starts with), else `null`. */
export function selectedSearchText(editor: Editor): string | null {
  const { selection, doc } = editor.state;
  if (!(selection instanceof TextSelection) || selection.empty || !selection.$from.sameParent(selection.$to)) {
    return null;
  }
  const text = doc.textBetween(selection.from, selection.to, "\n", "\n");
  return text.trim() === "" || text.includes("\n") || text.length > MAX_SEED_LENGTH ? null : text;
}

/** True while a modal dialog is open: the keys are the dialog's then (the center peek is not one: it holds an editor). */
function modalIsOpen(): boolean {
  return document.querySelector('[aria-modal="true"]:not(.peek-dialog)') !== null;
}

/** Whether a node is inside a peek of a database row (the side panel or the center dialog; `PeekHost` marks both). */
function inPeek(node: unknown): boolean {
  return node instanceof Element && node.closest("[data-peek]") !== null;
}

/**
 * The find bar of an editor, closed until Ctrl+F (or `requestFindInPage`) opens it. Mount it
 * with the editor and unmount it with it.
 */
export function FindBar({ editor }: { editor: Editor }) {
  // `null`: closed. Otherwise how often it was asked to open (asking again moves focus to the field).
  const [requests, setRequests] = useState<number | null>(null);

  function open(fromEditor: boolean): void {
    if (editor.isDestroyed) {
      return;
    }
    // The selection is only a search term while the keys come from the editor (it has the focus).
    const seed = fromEditor ? selectedSearchText(editor) : null;
    if (seed !== null) {
      editor.commands.setFind({ query: seed });
    }
    setRequests((current) => (current ?? 0) + 1);
  }

  /** The bar of the editor that holds the focus answers: the peek's editor's when the focus is in the peek, else the page's. */
  const holdsFocus = (focused: unknown): boolean => inPeek(editor.view.dom) === inPeek(focused);

  const onKeyDown = useEffectEvent((event: globalThis.KeyboardEvent) => {
    if (
      event.defaultPrevented ||
      event.isComposing ||
      !(event.ctrlKey || event.metaKey) ||
      event.altKey ||
      event.shiftKey ||
      event.key.toLowerCase() !== "f" ||
      modalIsOpen() ||
      !holdsFocus(event.target)
    ) {
      return;
    }
    event.preventDefault(); // not the webview's own find bar
    open(event.target instanceof Node && editor.view.dom.contains(event.target));
  });

  const onRequest = useEffectEvent(() => {
    if (holdsFocus(document.activeElement)) {
      open(false);
    }
  });

  useEffect(() => {
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener(FIND_IN_PAGE_EVENT, onRequest);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener(FIND_IN_PAGE_EVENT, onRequest);
    };
  }, []);

  if (requests === null) {
    return null;
  }
  return (
    <FindPanel
      editor={editor}
      requests={requests}
      onClose={() => {
        setRequests(null);
      }}
    />
  );
}

interface FindView {
  query: string;
  matchCase: boolean;
  wholeWord: boolean;
  count: number;
  /** The index of the active match, `-1` for none. */
  active: number;
  /** Replacing is possible (an editable page the binding writes). */
  writable: boolean;
}

function readFind(editor: Editor): FindView {
  const find = findStateOf(editor.state);
  return {
    query: find?.query ?? "",
    matchCase: find?.matchCase ?? false,
    wholeWord: find?.wholeWord ?? false,
    count: find?.matches.length ?? 0,
    active: find?.active ?? -1,
    writable: editor.isEditable && (loroSyncPluginKey.getState(editor.state)?.readOnlyReason ?? null) === null,
  };
}

/** A small icon that follows the text colour (the control that holds it has the accessible name). */
function Glyph({ children }: { children: ReactNode }) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

interface FindPanelProps {
  editor: Editor;
  /** Changes when the bar is asked to open while it is open: the field takes focus again. */
  requests: number;
  onClose: () => void;
}

function FindPanel({ editor, requests, onClose }: FindPanelProps) {
  const find = useEditorState({ editor, selector: ({ editor: current }) => readFind(current) });
  const [replacement, setReplacement] = useState("");
  const [showReplace, setShowReplace] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const replaceId = useId();
  const replaceVisible = showReplace && find.writable;

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, [requests]);

  /** Closes the bar and puts the editor where the active match was, with the match selected. */
  function close(): void {
    if (!editor.isDestroyed) {
      editor.commands.closeFind({ select: true });
      editor.view.focus();
    }
    onClose();
  }

  function onBarKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    }
  }

  let counter = "";
  if (find.query !== "") {
    counter = find.count === 0 ? t("find.noMatches") : t("find.count", { current: find.active + 1, total: find.count });
  }

  return (
    <div className="find-bar" role="search" aria-label={t("find.label")} onKeyDown={onBarKeyDown}>
      <div className="find-row">
        {find.writable && (
          <button
            type="button"
            className="icon-button find-toggle"
            aria-label={t("find.toggleReplace")}
            title={t("find.toggleReplace")}
            aria-expanded={showReplace}
            aria-controls={replaceId}
            onClick={() => {
              setShowReplace((current) => !current);
            }}
          >
            <Glyph>
              <path d="M6 3.5 10.5 8 6 12.5" />
            </Glyph>
          </button>
        )}
        <input
          ref={inputRef}
          className="input find-input"
          type="text"
          value={find.query}
          aria-label={t("find.field")}
          placeholder={t("find.placeholder")}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => {
            editor.commands.setFind({ query: event.target.value });
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              if (event.shiftKey) {
                editor.commands.findPrevious();
              } else {
                editor.commands.findNext();
              }
            }
          }}
        />
        <span className="find-count" role="status">
          {counter}
        </span>
        <button
          type="button"
          className="icon-button"
          aria-label={t("find.previous")}
          title={t("find.previous")}
          disabled={find.count === 0}
          onClick={() => {
            editor.commands.findPrevious();
          }}
        >
          <Glyph>
            <path d="m3.5 10 4.5-4.5 4.5 4.5" />
          </Glyph>
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={t("find.next")}
          title={t("find.next")}
          disabled={find.count === 0}
          onClick={() => {
            editor.commands.findNext();
          }}
        >
          <Glyph>
            <path d="m3.5 6 4.5 4.5L12.5 6" />
          </Glyph>
        </button>
        <button
          type="button"
          className="icon-button find-option"
          aria-label={t("find.matchCase")}
          title={t("find.matchCase")}
          aria-pressed={find.matchCase}
          onClick={() => {
            editor.commands.setFind({ matchCase: !find.matchCase });
          }}
        >
          Aa
        </button>
        <button
          type="button"
          className="icon-button find-option find-whole-word"
          aria-label={t("find.wholeWord")}
          title={t("find.wholeWord")}
          aria-pressed={find.wholeWord}
          onClick={() => {
            editor.commands.setFind({ wholeWord: !find.wholeWord });
          }}
        >
          ab
        </button>
        <button type="button" className="icon-button" aria-label={t("find.close")} title={t("find.close")} onClick={close}>
          <Glyph>
            <path d="m4 4 8 8M12 4l-8 8" />
          </Glyph>
        </button>
      </div>
      {replaceVisible && (
        <div className="find-row" id={replaceId}>
          <input
            className="input find-input"
            type="text"
            value={replacement}
            aria-label={t("find.replaceField")}
            placeholder={t("find.replacePlaceholder")}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => {
              setReplacement(event.target.value);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                event.preventDefault();
                editor.commands.replaceActive(replacement);
              }
            }}
          />
          <button
            type="button"
            className="button find-replace-button"
            disabled={find.active < 0}
            onClick={() => {
              editor.commands.replaceActive(replacement);
            }}
          >
            {t("find.replace")}
          </button>
          <button
            type="button"
            className="button find-replace-button"
            disabled={find.count === 0}
            onClick={() => {
              editor.commands.replaceAll(replacement);
            }}
          >
            {t("find.replaceAll")}
          </button>
        </div>
      )}
    </div>
  );
}
