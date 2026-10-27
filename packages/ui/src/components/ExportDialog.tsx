// "Export..." of a page (page menu) or of a space (space menu), ADR-0036: the format (Markdown or HTML;
// for a page also PDF, which is the print dialog), "Include sub-pages" for a page that has some (not for
// PDF, which is one page at a time), then the shell asks where to save and writes the file or the .zip
// while the dialog shows how far the export got. A cancelled save dialog says nothing and leaves the
// dialog as it was; when the file is written the dialog closes and a toast tells how many pages went to
// which file (`store-interop.ts`).

import { toCoreError } from "@atomik-kb/core-client";
import type { ExportFormat, PageId, SpaceId } from "@atomik-kb/core-client";
import { useId, useRef, useState } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";

export interface ExportDialogProps {
  spaceId: SpaceId;
  /** The page to export; leave it out to export the whole space. */
  pageId?: PageId;
  /** The name of the page or space, as the sidebar shows it. */
  name: string;
  /** The page has sub-pages, so "Include sub-pages" is offered. */
  hasChildren?: boolean;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  onClose: () => void;
}

/** What the dialog offers: the formats of the core, and PDF, which is the print dialog of the webview. */
type Choice = ExportFormat | "pdf";

const FORMAT_LABELS: Readonly<Record<Choice, { label: MessageKey; hint: MessageKey }>> = {
  markdown: { label: "export.format.markdown", hint: "export.format.markdown.hint" },
  html: { label: "export.format.html", hint: "export.format.html.hint" },
  pdf: { label: "export.format.pdf", hint: "export.format.pdf.hint" },
};

export function ExportDialog({ spaceId, pageId, name, hasChildren = false, returnFocus, onClose }: ExportDialogProps) {
  const runExport = useAppStore((state) => state.runExport);
  const startPrint = useAppStore((state) => state.startPrint);
  const progress = useAppStore((state) => state.exportProgress);
  const [format, setFormat] = useState<Choice>("markdown");
  const [includeChildren, setIncludeChildren] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const introId = useId();
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);
  const isPage = pageId !== undefined;
  const choices: readonly Choice[] = isPage ? ["markdown", "html", "pdf"] : ["markdown", "html"];
  const shownName = name === "" ? t("page.untitled") : name;

  async function submit(): Promise<void> {
    if (busy) {
      return;
    }
    if (format === "pdf") {
      // The dialog goes first: print mode shows the page alone, and the print dialog must not find this one on it.
      onClose();
      startPrint();
      return;
    }
    setError(null);
    setBusy(true);
    try {
      await runExport({
        spaceId,
        ...(pageId === undefined ? {} : { pageId }),
        includeChildren: !isPage || includeChildren,
        format,
      });
      onClose();
    } catch (failure) {
      const reason = toCoreError(failure);
      // A closed save dialog is not a failure: the dialog stays, ready for another choice.
      if (!isFieldError(reason, "path")) {
        setError(t("export.error", { message: reason.message }));
      }
      setBusy(false);
    }
  }

  let status: string | null = null;
  if (busy) {
    status =
      progress === null ? t("export.preparing") : t("export.progress", { done: progress.done, total: progress.total });
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={introId}
      onDismiss={busy ? undefined : onClose}
      className="export-dialog"
      returnFocus={returnRef}
    >
      <h2 className="subtitle" id={titleId}>
        {isPage ? t("export.title.page", { name: shownName }) : t("export.title.space", { name: shownName })}
      </h2>
      <p id={introId}>{isPage ? t("export.intro.page") : t("export.intro.space")}</p>
      <form
        className="form export-form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <fieldset className="fieldset" disabled={busy}>
          <legend>{t("export.format")}</legend>
          <div className="export-formats">
            {choices.map((choice) => {
              const hintId = `${titleId}-${choice}`;
              return (
                <div key={choice} className="export-format">
                  <label className="radio">
                    <input
                      type="radio"
                      name="export-format"
                      value={choice}
                      checked={format === choice}
                      aria-describedby={hintId}
                      onChange={() => {
                        setFormat(choice);
                      }}
                    />
                    <span>{t(FORMAT_LABELS[choice].label)}</span>
                  </label>
                  <p className="muted export-format-hint" id={hintId}>
                    {t(FORMAT_LABELS[choice].hint)}
                  </p>
                </div>
              );
            })}
          </div>
        </fieldset>
        {isPage && hasChildren && format !== "pdf" && (
          <label className="checkbox-label export-subpages">
            <input
              type="checkbox"
              checked={includeChildren}
              disabled={busy}
              onChange={(event) => {
                setIncludeChildren(event.target.checked);
              }}
            />
            <span>{t("export.subpages")}</span>
          </label>
        )}
        {status !== null && (
          <p className="muted" role="status">
            {status}
          </p>
        )}
        {error !== null && (
          <p className="error-text" role="alert">
            {error}
          </p>
        )}
        <div className="dialog-actions">
          <button type="button" className="button" disabled={busy} onClick={onClose}>
            {t("dialog.cancel")}
          </button>
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? t("export.working") : t("export.submit")}
          </button>
        </div>
      </form>
    </Modal>
  );
}
