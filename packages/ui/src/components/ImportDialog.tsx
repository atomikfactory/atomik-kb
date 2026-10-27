// "Import..." into a page (page menu) or into the root of a space (space menu), ADR-0036. Three steps in
// one dialog: the source (files, a folder or a .zip such as a Notion or atomik-kb export: the shell shows
// its own dialog for the choice), a progress view (indeterminate while the files are read and uploaded, then
// the pages written out of the pages to write, from `ImportProgress`), and the report: what was created, what
// was left out or changed grouped by kind with the files concerned, and "Open" for the first page created.
// A closed source dialog says nothing and leaves the dialog at the first step.
//
// With `appflowy` it is the "Import AppFlowy workspace…" of the space menu (ADR-0039): the first step says where
// AppFlowy makes its backup and offers the one choice, the backup .zip; the pages go into new spaces, one for each
// space of AppFlowy, and the report lists them (name, pages) with "Open" for each. A report that has new spaces
// lists them whichever way the import was started.

import { toCoreError } from "@atomik-kb/core-client";
import type {
  ImportReport,
  ImportSourceKind,
  ImportWarning,
  ImportWarningCode,
  PageId,
  SpaceId,
} from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";

import { isFieldError } from "../errors";
import { formatPageCount } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { PageIcon } from "./PageIcon";

export interface ImportDialogProps {
  spaceId: SpaceId;
  /** The page the imported pages go under; leave it out for the root of the space. */
  parentId?: PageId;
  /** The name of the page, or of the space when there is no `parentId`. */
  destination: string;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  /** An AppFlowy backup into new spaces (ADR-0039) instead of files into `spaceId`. */
  appflowy?: boolean;
  onClose: () => void;
}

/** A group of warnings with this many entries or fewer starts open; a longer one is collapsed. */
export const WARNINGS_OPEN_UP_TO = 10;

const SOURCES: readonly { kind: ImportSourceKind; label: MessageKey; hint: MessageKey }[] = [
  { kind: "files", label: "import.files", hint: "import.files.hint" },
  { kind: "folder", label: "import.folder", hint: "import.folder.hint" },
  { kind: "zip", label: "import.zip", hint: "import.zip.hint" },
];

const WARNING_LABELS: Readonly<Record<ImportWarningCode, MessageKey>> = {
  unsupportedFile: "import.warning.unsupportedFile",
  notUtf8: "import.warning.notUtf8",
  tooLarge: "import.warning.tooLarge",
  unresolvedLink: "import.warning.unresolvedLink",
  missingFile: "import.warning.missingFile",
  remoteImage: "import.warning.remoteImage",
  embeddedImage: "import.warning.embeddedImage",
  propertyLimit: "import.warning.propertyLimit",
  invalidValue: "import.warning.invalidValue",
  truncated: "import.warning.truncated",
  unsupportedContent: "import.warning.unsupportedContent",
  attachmentFailed: "import.warning.attachmentFailed",
};

/** The warnings grouped by kind, in the order each kind first appears. */
export function groupWarnings(warnings: readonly ImportWarning[]): { code: ImportWarningCode; entries: ImportWarning[] }[] {
  const groups = new Map<ImportWarningCode, ImportWarning[]>();
  for (const warning of warnings) {
    const entries = groups.get(warning.code);
    if (entries === undefined) {
      groups.set(warning.code, [warning]);
    } else {
      entries.push(warning);
    }
  }
  return Array.from(groups, ([code, entries]) => ({ code, entries }));
}

/** What the import left out or changed, by kind, with the files concerned (a group of more than ten is collapsed). */
function Warnings({ report }: { report: ImportReport }) {
  const warnings = report.warnings ?? [];
  const groups = groupWarnings(warnings);
  if (groups.length === 0 && report.moreWarnings === 0) {
    return <p className="muted">{t("import.report.noWarnings")}</p>;
  }
  return (
    <section className="import-warnings" aria-label={t("import.report.warnings")}>
      <h3 className="subheading">{t("import.report.warnings")}</h3>
      {groups.map(({ code, entries }) => (
        <details key={code} className="import-warning-group" open={entries.length <= WARNINGS_OPEN_UP_TO}>
          <summary>{t("import.report.group", { label: t(WARNING_LABELS[code]), count: entries.length })}</summary>
          <ul className="import-warning-list">
            {entries.map((entry, index) => (
              // The same file can have the same warning twice with different details: the position tells them apart.
              <li key={`${entry.path}\n${String(index)}`}>
                <span className="mono">{entry.path}</span>
                {entry.detail !== undefined && entry.detail !== "" && <span className="muted import-warning-detail">{entry.detail}</span>}
              </li>
            ))}
          </ul>
        </details>
      ))}
      {report.moreWarnings > 0 && <p className="muted">{t("import.report.more", { count: report.moreWarnings })}</p>}
    </section>
  );
}

/** The spaces an import created: their names and pages as the spaces list has them, and "Open" for each. */
function NewSpaces({ spaceIds, onOpen }: { spaceIds: readonly SpaceId[]; onOpen: (spaceId: SpaceId) => void }) {
  const spaces = useAppStore((state) => state.spaces);
  return (
    <section className="import-spaces" aria-label={t("appflowyImport.report.spaces")}>
      <h3 className="subheading">{t("appflowyImport.report.spaces")}</h3>
      <ul className="import-space-list">
        {spaceIds.map((spaceId) => {
          const space = spaces.find((candidate) => candidate.id === spaceId);
          if (space === undefined) {
            return null;
          }
          return (
            <li key={spaceId} className="import-space">
              <PageIcon icon={space.icon} kind="space" className="tree-icon" />
              <span className="import-space-name" title={space.name}>
                {space.name}
              </span>
              <span className="muted">{formatPageCount(space.pageCount)}</span>
              <button
                type="button"
                className="button"
                aria-label={t("appflowyImport.report.openSpace", { name: space.name })}
                onClick={() => {
                  onOpen(spaceId);
                }}
              >
                {t("appflowyImport.report.open")}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

type Phase = { kind: "choose" } | { kind: "running" } | { kind: "report"; report: ImportReport };

export function ImportDialog({ spaceId, parentId, destination, returnFocus, appflowy = false, onClose }: ImportDialogProps) {
  const runImport = useAppStore((state) => state.runImport);
  const navigateTo = useAppStore((state) => state.navigateTo);
  const selectSpace = useAppStore((state) => state.selectSpace);
  const refreshSpaces = useAppStore((state) => state.refreshSpaces);
  const progress = useAppStore((state) => state.importProgress);
  const [phase, setPhase] = useState<Phase>({ kind: "choose" });
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const introId = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);
  const isPage = parentId !== undefined;
  const shownName = destination === "" ? t("page.untitled") : destination;
  const step = phase.kind;

  // The buttons of a step go with it: focus moves to the new step's heading so that it stays in the dialog.
  const shownStep = useRef(step);
  useEffect(() => {
    if (shownStep.current !== step) {
      shownStep.current = step;
      heading.current?.focus();
    }
  }, [step]);

  async function start(source: ImportSourceKind): Promise<void> {
    setError(null);
    setPhase({ kind: "running" });
    try {
      const report = await runImport({
        spaceId,
        ...(parentId === undefined ? {} : { parentId }),
        source,
        ...(appflowy ? { destination: "newSpaces" as const } : {}),
      });
      if ((report.spaceIds ?? []).length > 0) {
        // The report names the new spaces with the pages they hold: the list must have them by now.
        await refreshSpaces().catch((failure: unknown) => {
          console.error("could not refresh the spaces after an import", failure);
        });
      }
      setPhase({ kind: "report", report });
    } catch (failure) {
      const reason = toCoreError(failure);
      // A closed source dialog is not a failure: back to the choice, with nothing said.
      if (!isFieldError(reason, "path")) {
        setError(t("import.error", { message: reason.message }));
      }
      setPhase({ kind: "choose" });
    }
  }

  function open(report: ImportReport): void {
    const first = report.rootPageIds[0];
    onClose();
    if (first !== undefined) {
      navigateTo({ pageId: first, spaceId });
    }
  }

  function openSpace(newSpaceId: SpaceId): void {
    onClose();
    selectSpace(newSpaceId);
  }

  let title = appflowy ? t("appflowyImport.title") : t("import.title");
  if (phase.kind === "running") {
    title = t("import.running");
  } else if (phase.kind === "report") {
    title = t("import.report.title");
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={phase.kind === "choose" ? introId : undefined}
      onDismiss={phase.kind === "running" ? undefined : onClose}
      className="import-dialog"
      returnFocus={returnRef}
    >
      <h2 className="subtitle" id={titleId} ref={heading} tabIndex={-1}>
        {title}
      </h2>

      {phase.kind === "choose" && appflowy && (
        <>
          <p id={introId}>{t("appflowyImport.intro")}</p>
          <p>{t("appflowyImport.how")}</p>
          <p className="muted">{t("appflowyImport.privacy")}</p>
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button
              type="button"
              className="button button-primary"
              onClick={() => {
                void start("zip");
              }}
            >
              {t("appflowyImport.choose")}
            </button>
          </div>
        </>
      )}

      {phase.kind === "choose" && !appflowy && (
        <>
          <p id={introId}>
            {isPage ? t("import.intro.page", { name: shownName }) : t("import.intro.space", { name: shownName })}
          </p>
          <p className="muted">{t("import.sources")}</p>
          <ul className="import-sources" aria-label={t("import.sourcesLabel")}>
            {SOURCES.map(({ kind, label, hint }) => {
              const hintId = `${titleId}-${kind}`;
              return (
                <li key={kind} className="import-source">
                  <button
                    type="button"
                    className="button"
                    aria-describedby={hintId}
                    onClick={() => {
                      void start(kind);
                    }}
                  >
                    {t(label)}
                  </button>
                  <span className="muted" id={hintId}>
                    {t(hint)}
                  </span>
                </li>
              );
            })}
          </ul>
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" onClick={onClose}>
              {t("dialog.cancel")}
            </button>
          </div>
        </>
      )}

      {phase.kind === "running" && (
        <div className="import-progress">
          {progress !== null && progress.total > 0 ? (
            <>
              <progress aria-label={t("import.progressLabel")} value={progress.done} max={progress.total} />
              <p className="muted" role="status">
                {t("import.writing", { done: progress.done, total: progress.total })}
              </p>
            </>
          ) : (
            <>
              <progress aria-label={t("import.progressLabel")} />
              <p className="muted" role="status">
                {t("import.reading")}
              </p>
            </>
          )}
        </div>
      )}

      {phase.kind === "report" && (
        <>
          {phase.report.pages === 0 && <p>{t("import.report.none")}</p>}
          <dl className="facts import-facts" aria-label={t("import.report.summary")}>
            <dt>{t("import.report.pages")}</dt>
            <dd>{phase.report.pages}</dd>
            <dt>{t("import.report.attachments")}</dt>
            <dd>{phase.report.attachments}</dd>
            <dt>{t("import.report.properties")}</dt>
            <dd>{phase.report.propertiesCreated}</dd>
          </dl>
          {(phase.report.spaceIds ?? []).length > 0 && (
            <NewSpaces spaceIds={phase.report.spaceIds ?? []} onOpen={openSpace} />
          )}
          <Warnings report={phase.report} />
          <div className="dialog-actions">
            <button type="button" className="button" onClick={onClose}>
              {t("dialog.close")}
            </button>
            {phase.report.rootPageIds.length > 0 && (phase.report.spaceIds ?? []).length === 0 && (
              <button
                type="button"
                className="button button-primary"
                onClick={() => {
                  open(phase.report);
                }}
              >
                {t("import.report.open")}
              </button>
            )}
          </div>
        </>
      )}
    </Modal>
  );
}
