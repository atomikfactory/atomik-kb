// A version of a page shown read-only with its differences from the current page (ADR-0057, `docs/architecture/
// page-history.md` section 8). The history dialog (agent C) mounts it.
//
// **How it draws.** The merged document of `diffPages` (or the version alone) is given to a Tiptap editor that is not editable,
// built from the page's own schema (`createSchemaExtensions`) with the views of `preview-views.tsx`: no Loro document and no
// session (the portable JSON of the version is all there is), so no editing, no comments, no presence, no menus, nothing that
// writes. The markup and the classes are the page editor's (`page-editor-content`), so `app.css` draws the blocks as it does a
// page; `styles/page-history-preview.css` adds the differences. A new document (another version, "Show changes" toggled, the
// current page arriving) makes a new editor: the preview is a view, not a place to keep state. Keep the component mounted when
// the version changes (do not key it by version) and the toggle stays where the person put it.

import type { PageId, SpaceId } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { JSONContent } from "@tiptap/core";
import { EditorContent } from "@tiptap/react";
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";

import { t, tp } from "../i18n";
import { diffPages } from "./page-diff";
import type { PageDiff, PortableNode } from "./page-diff";
import { createPreviewExtensions, isShowable, previewContent } from "./preview-extensions";

/** The props of the preview. */
export interface VersionPreviewProps {
  readonly spaceId: SpaceId;
  readonly pageId: PageId;
  /** The version's content (`PageHistoryGet`). */
  readonly version: PortableNode;
  /** The page's content now (`PageContentGet`); `null` while it loads (the preview shows the version alone). */
  readonly current: PortableNode | null;
  /** The version's title, when known. */
  readonly title?: string;
}

/** "3 blocks added, 1 removed, 2 changed" (only what is not zero), or that nothing differs. */
function summaryText(summary: PageDiff["summary"]): string {
  const parts: string[] = [];
  if (summary.added > 0) {
    parts.push(tp("history.preview.summary.added", summary.added));
  }
  if (summary.removed > 0) {
    parts.push(tp("history.preview.summary.removed", summary.removed));
  }
  if (summary.changed > 0) {
    parts.push(tp("history.preview.summary.changed", summary.changed));
  }
  return parts.length === 0 ? t("history.preview.noChanges") : parts.join(", ");
}

/** Holds the editor instance for `useSyncExternalStore` (an effect makes and destroys it). */
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

/** The editor that shows `doc`, made anew for every document. */
function PreviewDocument({ doc }: { doc: PortableNode }) {
  const showable = useMemo(() => isShowable(doc), [doc]);
  const [holder] = useState(() => new EditorHolder());
  const editor = useSyncExternalStore(holder.subscribe, holder.get);

  useEffect(() => {
    if (!showable) {
      return;
    }
    const created = new Editor({
      extensions: createPreviewExtensions(),
      content: doc as JSONContent,
      editable: false,
      editorProps: {
        attributes: {
          class: "page-editor-content version-preview-content",
          role: "document",
          "aria-readonly": "true",
          "aria-label": t("history.preview.label"),
        },
      },
    });
    holder.set(created);
    return () => {
      holder.set(null);
      created.destroy();
    };
  }, [doc, showable, holder]);

  if (!showable) {
    return (
      <p className="banner banner-warning version-preview-error" role="alert">
        {t("history.preview.error")}
      </p>
    );
  }
  return editor === null ? null : <EditorContent editor={editor} className="page-editor-body version-preview-body" />;
}

/** Draws a version read-only, with the differences highlighted when "Show changes" is on. */
export function VersionPreview({ spaceId, pageId, version, current, title }: VersionPreviewProps) {
  const [showChanges, setShowChanges] = useState(true);
  const diff = useMemo(() => (current === null ? null : diffPages(version, current)), [version, current]);
  const comparing = showChanges && diff !== null;
  const doc = useMemo(() => (diff !== null && showChanges ? previewContent(diff.doc, true) : previewContent(version, false)), [diff, showChanges, version]);

  return (
    <div className="version-preview" data-space={spaceId} data-page={pageId}>
      <div className="version-preview-header">
        <label className="version-preview-toggle">
          <input
            type="checkbox"
            checked={showChanges}
            disabled={current === null}
            onChange={(event) => {
              setShowChanges(event.target.checked);
            }}
          />
          <span>{t("history.preview.showChanges")}</span>
        </label>
        {comparing && (
          <ul className="version-preview-legend" aria-label={t("history.preview.legend")}>
            <li className="is-added">
              <span className="version-preview-swatch" aria-hidden="true" />
              {t("history.preview.legend.added")}
            </li>
            <li className="is-removed">
              <span className="version-preview-swatch" aria-hidden="true" />
              {t("history.preview.legend.removed")}
            </li>
            <li className="is-changed">
              <span className="version-preview-swatch" aria-hidden="true" />
              {t("history.preview.legend.changed")}
            </li>
          </ul>
        )}
        {comparing && (
          <p className="version-preview-summary" role="status">
            {summaryText(diff.summary)}
          </p>
        )}
        {current === null && (
          <p className="version-preview-summary muted" role="status">
            {t("history.preview.comparing")}
          </p>
        )}
      </div>
      <div className="version-preview-scroll" tabIndex={0}>
        {title !== undefined && (
          <h2 className={title === "" ? "version-preview-title is-untitled" : "version-preview-title"}>
            {title === "" ? t("page.untitled") : title}
          </h2>
        )}
        <PreviewDocument doc={doc} />
      </div>
    </div>
  );
}
