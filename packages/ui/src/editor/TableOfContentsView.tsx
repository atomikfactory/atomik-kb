// The view of the table of contents block: the page's headings (levels 1 to 3, in reading
// order, indented by level), each a button that scrolls its heading into view and puts the
// caret in it. The list is read from the editor's document and redrawn on every change of it;
// the view writes nothing (a click is a selection, which is no document change), so a
// read-only editor shows and uses the same list.

import type { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { TextSelection } from "@tiptap/pm/state";
import { NodeViewWrapper } from "@tiptap/react";
import type { ReactNodeViewProps } from "@tiptap/react";
import { useMemo, useSyncExternalStore } from "react";

import { t } from "../i18n";
import { collectHeadings, findHeading, sameEntries } from "./table-of-contents";
import type { TocEntry } from "./table-of-contents";

/**
 * The headings of an editor's document for `useSyncExternalStore`: re-read when the document
 * changes (the editor's `update` event, which the binding's own transactions cause too), and the
 * same list is handed back while the lines are the same, so typing in a paragraph redraws nothing.
 */
class HeadingsStore {
  readonly #editor: Editor;
  #doc: PmNode | null = null;
  #entries: readonly TocEntry[] = [];

  constructor(editor: Editor) {
    this.#editor = editor;
  }

  readonly subscribe = (onChange: () => void): (() => void) => {
    this.#editor.on("update", onChange);
    return () => {
      this.#editor.off("update", onChange);
    };
  };

  readonly getSnapshot = (): readonly TocEntry[] => {
    const { doc } = this.#editor.state;
    if (doc !== this.#doc) {
      const next = collectHeadings(doc);
      this.#doc = doc;
      if (!sameEntries(this.#entries, next)) {
        this.#entries = next;
      }
    }
    return this.#entries;
  };
}

/**
 * Brings the heading of `entry` into view and puts the caret at its start. Only the selection
 * changes, in a read-only editor too, and the editor takes the focus when it is editable.
 */
function goToHeading(editor: Editor, entry: TocEntry, ordinal: number): void {
  if (editor.isDestroyed) {
    return;
  }
  const { state, view } = editor;
  const pos = findHeading(state.doc, entry, ordinal);
  if (pos === null) {
    return;
  }
  view.dispatch(state.tr.setSelection(TextSelection.create(state.doc, pos + 1)));
  const element = view.nodeDOM(pos);
  if (element instanceof HTMLElement) {
    element.scrollIntoView({ block: "center" });
  }
  if (editor.isEditable) {
    view.focus();
  }
}

/** The table of contents block of a page: the headings of the page it is in, as a list of buttons. */
export function TableOfContentsView({ editor }: ReactNodeViewProps) {
  const store = useMemo(() => new HeadingsStore(editor), [editor]);
  const headings = useSyncExternalStore(store.subscribe, store.getSnapshot);

  return (
    <NodeViewWrapper className="toc">
      <nav aria-label={t("toc.label")}>
        <p className="toc-title">{t("toc.title")}</p>
        {headings.length === 0 ? (
          <p className="toc-empty">{t("toc.empty")}</p>
        ) : (
          <ul className="toc-list">
            {headings.map((entry, index) => (
              <li key={`${entry.id ?? "heading"}:${index}`} className="toc-item" data-level={entry.level}>
                <button
                  type="button"
                  className="toc-link"
                  onClick={() => {
                    goToHeading(editor, entry, index);
                  }}
                >
                  {entry.text === "" ? t("toc.untitled") : entry.text}
                </button>
              </li>
            ))}
          </ul>
        )}
      </nav>
    </NodeViewWrapper>
  );
}
