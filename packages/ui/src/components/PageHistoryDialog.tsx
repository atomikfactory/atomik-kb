// "Page history" (ADR-0057, `docs/architecture/page-history.md` section 7): a large dialog with the page's versions listed at the
// right (by day, newest first: the time or the range of the session, who made it, "Restored" and "Current" marks, the title when it
// differs from the page's now; then the document's older, undated history by position, "Load older versions" while there is more) and
// the selected version at the left, read-only, with its differences from the page now highlighted (`history/VersionPreview.tsx`
// draws it; this file never draws a difference). The newest version that is not the page as it is now is selected when the dialog
// opens.
//
// "Restore this version" (an Editor of an unlocked page that is not in the trash, and not for the current version) makes the page what
// the version was as a new change (`store-history.ts`), closes the dialog and leaves a toast "Page restored" with "Undo", which
// restores the version that was current before. A Viewer, a Commenter and a read-only copy only look: the list and the preview, with
// the reason in the place of the button. Keys: Up, Down, Home and End move through the list, Enter asks to restore the selected
// version (the question is in the dialog, answered with Enter again or Esc), Esc closes the dialog (or the question).

import { toCoreError } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, PageVersion, PageVersionContent, VersionAuthor } from "@atomik-kb/core-client";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";

import { usePeople, usePersonNames } from "../comments-hooks";
import type { PortableNode } from "../history/page-diff";
import { VersionPreview } from "../history/VersionPreview";
import { t } from "../i18n";
import { groupVersions, initialSelection, instantLabel, stepSelection, timeLabel } from "../page-history";
import { deviceLabel } from "../property-format";
import { useSpaceAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { CloseIcon } from "./icons";
import { Modal } from "./Modal";

export interface PageHistoryDialogProps {
  /** The page, as the tree has it (its title, lock and place in the trash are followed live). */
  page: PageInfo;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  onClose: () => void;
}

/** Something read from the core: on its way, there, or refused. */
type Read<T> = { status: "loading" } | { status: "ready"; value: T } | { status: "error"; message: string };

/** Why the selected version cannot be restored from here; `null` when it can. */
type NoRestore = "viewOnly" | "trashed" | "locked" | "current" | null;

const NO_VERSIONS: readonly PageVersion[] = [];

/** A portable document (`{ type: "doc", ... }`) from the JSON the core answers; `null` for anything else. */
function asDocument(content: JsonValue): PortableNode | null {
  if (typeof content !== "object" || content === null || Array.isArray(content) || content["type"] !== "doc") {
    return null;
  }
  return content as unknown as PortableNode;
}

/** The label a version goes by: its date and time, or "Earlier version N" by position among the undated ones. */
function versionName(version: PageVersion, undated: readonly PageVersion[]): string {
  if (version.endedAt !== undefined) {
    return instantLabel(version);
  }
  return t("history.list.earlierVersion", { n: undated.findIndex((candidate) => candidate.id === version.id) + 1 });
}

export function PageHistoryDialog({ page, returnFocus, onClose }: PageHistoryDialogProps) {
  const titleId = useId();
  const confirmId = useId();
  const optionBase = useId();
  const listPageVersions = useAppStore((state) => state.listPageVersions);
  const getPageVersion = useAppStore((state) => state.getPageVersion);
  const getPageContent = useAppStore((state) => state.getPageContent);
  const restorePageVersion = useAppStore((state) => state.restorePageVersion);
  const devices = useAppStore((state) => state.devices);
  const device = useAppStore((state) => state.device);
  const access = useSpaceAccess(page.spaceId);

  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const confirmButton = useRef<HTMLButtonElement>(null);
  const alive = useRef(true);

  // "Today" and "Yesterday" are as of the moment the dialog opened.
  const [openedAt] = useState(() => Date.now());
  const [list, setList] = useState<Read<{ versions: readonly PageVersion[]; more: boolean }>>({ status: "loading" });
  const [listAttempt, setListAttempt] = useState(0);
  const [older, setOlder] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // What was read of each version: versions are immutable, so one read serves every later selection of it.
  const [reads, setReads] = useState<ReadonlyMap<string, Read<PageVersionContent>>>(() => new Map());
  const pending = useRef(new Set<string>());
  // The version the preview draws: the selected one once it is read, the one before while it is being read (the preview stays
  // mounted, so that its "Show changes" stays where the person put it).
  const [shownId, setShownId] = useState<string | null>(null);
  const [now, setNow] = useState<PortableNode | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [restoreError, setRestoreError] = useState<string | null>(null);

  const versions = list.status === "ready" ? list.value.versions : NO_VERSIONS;
  const groups = useMemo(() => groupVersions(versions, openedAt), [versions, openedAt]);
  const selectedVersion = selectedId ?? initialSelection(versions);
  const selected = versions.find((version) => version.id === selectedVersion);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // The versions, newest first.
  useEffect(() => {
    let live = true;
    listPageVersions(page.id).then(
      (history) => {
        if (live) {
          setList({ status: "ready", value: { versions: history.versions, more: history.more } });
        }
      },
      (failure: unknown) => {
        if (live) {
          setList({ status: "error", message: toCoreError(failure).message });
        }
      },
    );
    return () => {
      live = false;
    };
  }, [listPageVersions, page.id, listAttempt]);

  // The page as it is now, which the preview compares the version with (it shows the version alone until it is there).
  useEffect(() => {
    let live = true;
    getPageContent(page.id).then(
      (content) => {
        if (live) {
          setNow(asDocument(content.content));
        }
      },
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [getPageContent, page.id]);

  // The content of the selected version, unless it was read (or is being read): the answer is kept whatever is selected by then.
  useEffect(() => {
    if (selectedVersion === null || reads.has(selectedVersion) || pending.current.has(selectedVersion)) {
      return;
    }
    const id = selectedVersion;
    pending.current.add(id);
    getPageVersion(page.id, id).then(
      (content) => {
        pending.current.delete(id);
        setReads((before) => new Map(before).set(id, { status: "ready", value: content }));
      },
      (failure: unknown) => {
        pending.current.delete(id);
        setReads((before) => new Map(before).set(id, { status: "error", message: toCoreError(failure).message }));
      },
    );
  }, [getPageVersion, page.id, reads, selectedVersion]);

  // The list takes the focus once it is there (the dialog's body has it until then), so that the keys work at once.
  const hasList = versions.length > 0;
  useEffect(() => {
    if (hasList && document.activeElement === bodyRef.current) {
      listRef.current?.focus();
    }
  }, [hasList]);

  // The selected row stays in view while the keys move it.
  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [selectedVersion]);

  // The question's own button has the focus (Enter again restores).
  useEffect(() => {
    if (confirming) {
      confirmButton.current?.focus();
    }
  }, [confirming]);

  // ----- who made it -----

  const own = device === null ? null : { deviceId: device.deviceId, name: device.name };
  const people = usePeople(page.spaceId);
  const myAccount = people?.find((person) => person.isMe)?.accountId;
  const accounts = useMemo(
    () => versions.flatMap((version) => (version.author?.accountId === undefined ? [] : [version.author.accountId])),
    [versions],
  );
  const personName = usePersonNames(page.spaceId, accounts);

  /** This device by its name, another member by the name the space gives them, a device of this account by its label, else nothing. */
  function authorLabel(author: VersionAuthor | undefined): string {
    if (author === undefined) {
      return "";
    }
    if (author.thisDevice === true) {
      return own?.name ?? t("history.list.thisDevice");
    }
    if (author.accountId !== undefined && author.accountId !== myAccount) {
      const name = personName(author.accountId);
      if (name !== "") {
        return name;
      }
    }
    return author.deviceId === undefined ? "" : deviceLabel(author.deviceId, devices, own);
  }

  // ----- restoring -----

  const noRestore: NoRestore =
    selected === undefined
      ? null
      : !access.canEdit
        ? "viewOnly"
        : page.trashedAt !== undefined
          ? "trashed"
          : page.locked === true
            ? "locked"
            : selected.current === true
              ? "current"
              : null;
  const restorable = selected !== undefined && noRestore === null;

  async function restore(): Promise<void> {
    if (selected === undefined || !restorable || restoring) {
      return;
    }
    setConfirming(false);
    setRestoring(true);
    setRestoreError(null);
    try {
      await restorePageVersion(page.id, selected.id, versions.find((version) => version.current === true)?.id ?? null);
    } catch (failure) {
      if (alive.current) {
        setRestoring(false);
        setRestoreError(t("history.action.error", { message: toCoreError(failure).message }));
      }
      return;
    }
    onClose();
  }

  /** Esc: leaves the question first, then closes; not while a restore is under way. */
  function dismiss(): void {
    if (restoring) {
      return;
    }
    if (confirming) {
      setConfirming(false);
      listRef.current?.focus();
      return;
    }
    onClose();
  }

  // ----- the list -----

  function select(id: string | null): void {
    if (id !== null) {
      setSelectedId(id);
      setRestoreError(null);
      setConfirming(false);
    }
  }

  function onListKeyDown(event: KeyboardEvent<HTMLDivElement>): void {
    if (event.nativeEvent.isComposing) {
      return;
    }
    const step = { ArrowDown: 1, ArrowUp: -1, Home: "first", End: "last" } as const;
    if (event.key in step) {
      event.preventDefault();
      select(stepSelection(groups.ordered, selectedVersion, step[event.key as keyof typeof step]));
    } else if (event.key === "Enter" && restorable && !event.repeat && !restoring) {
      event.preventDefault();
      setConfirming(true);
    }
  }

  async function loadOlder(): Promise<void> {
    const last = versions[versions.length - 1];
    if (list.status !== "ready" || last === undefined || older.busy) {
      return;
    }
    setOlder({ busy: true, error: null });
    try {
      const history = await listPageVersions(page.id, last.id);
      if (!alive.current) {
        return;
      }
      setList((read) => {
        if (read.status !== "ready") {
          return read;
        }
        const known = new Set(read.value.versions.map((version) => version.id));
        return {
          status: "ready",
          value: {
            versions: [...read.value.versions, ...history.versions.filter((version) => !known.has(version.id))],
            more: history.more,
          },
        };
      });
      setOlder({ busy: false, error: null });
    } catch (failure) {
      if (alive.current) {
        setOlder({ busy: false, error: toCoreError(failure).message });
      }
    }
  }

  // ----- drawing -----

  const pageTitle = page.title === "" ? t("page.untitled") : page.title;
  const indexOf = (version: PageVersion): number => groups.ordered.findIndex((candidate) => candidate.id === version.id);

  /** A row of the list (a function, not a component: a component made here would be a new one on every draw). */
  function renderRow(version: PageVersion) {
    const dated = version.endedAt !== undefined;
    const author = authorLabel(version.author);
    const titled = version.title !== undefined && version.title !== page.title;
    return (
      <div
        key={version.id}
        role="option"
        id={`${optionBase}-${indexOf(version)}`}
        aria-selected={version.id === selectedVersion}
        className="history-row"
        data-version={version.id}
        onClick={() => {
          select(version.id);
          listRef.current?.focus();
        }}
      >
        <span className="history-row-main">
          <span className="history-row-time">{dated ? timeLabel(version) : versionName(version, groups.undated)}</span>
          {version.current === true && <span className="history-badge history-badge-current">{t("history.list.current")}</span>}
          {version.kind === "restore" && <span className="history-badge">{t("history.list.restored")}</span>}
        </span>
        {author !== "" && <span className="history-row-author muted">{author}</span>}
        {titled && (
          <span className="history-row-title muted">
            {t("history.list.title", { title: version.title === "" ? t("page.untitled") : (version.title ?? "") })}
          </span>
        )}
      </div>
    );
  }

  const preview: Read<PageVersionContent> = (selectedVersion === null ? undefined : reads.get(selectedVersion)) ?? { status: "loading" };
  if (selectedVersion !== null && preview.status === "ready" && shownId !== selectedVersion) {
    setShownId(selectedVersion);
  }
  const shown = shownId === null ? undefined : reads.get(shownId);
  const shownDocument = shown?.status === "ready" ? asDocument(shown.value.content) : null;
  const name = selected === undefined ? "" : versionName(selected, groups.undated);
  const dayGroups = groups.days.map((day) => ({ ...day, id: `${optionBase}-day-${day.key}` }));

  return (
    <Modal labelledBy={titleId} onDismiss={dismiss} className="history-dialog" initialFocus={bodyRef} returnFocus={returnRef}>
      <div className="history-body" ref={bodyRef} tabIndex={-1}>
        <header className="history-header">
          <h2 className="subtitle history-title" id={titleId}>
            {t("history.dialog.title")}
          </h2>
          <span className="history-page muted">{pageTitle}</span>
          <button
            type="button"
            className="icon-button history-close"
            aria-label={t("history.dialog.close")}
            title={t("history.dialog.close")}
            disabled={restoring}
            onClick={onClose}
          >
            <CloseIcon />
          </button>
        </header>
        <div className="history-main">
          <section className="history-preview" aria-label={t("history.dialog.previewLabel")} aria-busy={preview.status === "loading"}>
            {list.status === "ready" && versions.length === 0 && <p className="muted history-empty">{t("history.dialog.empty")}</p>}
            {selected !== undefined && (
              <>
                <h3 className="history-version-heading">{name}</h3>
                {preview.status === "loading" && (
                  <p className="muted" role="status">
                    {t("history.dialog.previewLoading")}
                  </p>
                )}
                {preview.status === "error" && (
                  <div className="history-error" role="alert">
                    <p className="error-text">{t("history.dialog.previewError", { message: preview.message })}</p>
                    <button
                      type="button"
                      className="button button-small"
                      onClick={() => {
                        // Forgotten, so that the effect asks again.
                        setReads((before) => {
                          const next = new Map(before);
                          next.delete(selected.id);
                          return next;
                        });
                      }}
                    >
                      {t("history.dialog.retry")}
                    </button>
                  </div>
                )}
                {preview.status !== "error" && shown?.status === "ready" && shownDocument !== null && (
                  <div className={preview.status === "loading" ? "history-preview-body is-loading" : "history-preview-body"}>
                    <VersionPreview
                      spaceId={page.spaceId}
                      pageId={page.id}
                      version={shownDocument}
                      current={now}
                      {...(shown.value.title === undefined ? {} : { title: shown.value.title })}
                    />
                  </div>
                )}
              </>
            )}
          </section>
          <div className="history-side">
            {list.status === "loading" && (
              <p className="muted" role="status">
                {t("history.dialog.loading")}
              </p>
            )}
            {list.status === "error" && (
              <div className="history-error" role="alert">
                <p className="error-text">{t("history.dialog.error", { message: list.message })}</p>
                <button
                  type="button"
                  className="button button-small"
                  onClick={() => {
                    setList({ status: "loading" });
                    setListAttempt((count) => count + 1);
                  }}
                >
                  {t("history.dialog.retry")}
                </button>
              </div>
            )}
            {hasList && (
              <div className="history-list-scroll">
                <div
                  ref={listRef}
                  role="listbox"
                  tabIndex={0}
                  className="history-list"
                  aria-label={t("history.list.label")}
                  aria-activedescendant={selected === undefined ? undefined : `${optionBase}-${indexOf(selected)}`}
                  onKeyDown={onListKeyDown}
                >
                  {dayGroups.map((day) => (
                    <div key={day.key} role="group" aria-labelledby={day.id} className="history-group">
                      <div className="history-group-title" id={day.id}>
                        {day.label}
                      </div>
                      {day.versions.map(renderRow)}
                    </div>
                  ))}
                  {groups.undated.length > 0 && (
                    <div role="group" aria-labelledby={`${optionBase}-undated`} className="history-group">
                      <div className="history-group-title" id={`${optionBase}-undated`}>
                        {t("history.list.undated")}
                      </div>
                      {groups.undated.map(renderRow)}
                    </div>
                  )}
                </div>
                {list.status === "ready" && list.value.more && (
                  <div className="history-more">
                    <button type="button" className="button button-small" disabled={older.busy} onClick={() => void loadOlder()}>
                      {older.busy ? t("history.list.loadingOlder") : t("history.list.older")}
                    </button>
                    {older.error !== null && (
                      <p className="error-text" role="alert">
                        {t("history.list.olderError", { message: older.error })}
                      </p>
                    )}
                  </div>
                )}
              </div>
            )}
            <footer className="history-actions">
              {restoreError !== null && (
                <p className="error-text history-restore-error" role="alert">
                  {restoreError}
                </p>
              )}
              {confirming && selected !== undefined ? (
                <div className="history-confirm" role="group" aria-labelledby={confirmId}>
                  <p id={confirmId}>{t("history.action.confirm", { version: name })}</p>
                  <div className="history-confirm-buttons">
                    <button
                      type="button"
                      className="button"
                      onClick={() => {
                        setConfirming(false);
                        listRef.current?.focus();
                      }}
                    >
                      {t("dialog.cancel")}
                    </button>
                    <button
                      ref={confirmButton}
                      type="button"
                      className="button button-primary"
                      onClick={() => {
                        void restore();
                      }}
                    >
                      {t("history.action.confirmYes")}
                    </button>
                  </div>
                </div>
              ) : restorable || restoring ? (
                <button
                  type="button"
                  className="button button-primary"
                  disabled={restoring}
                  onClick={() => {
                    void restore();
                  }}
                >
                  {restoring ? t("history.action.restoring") : t("history.action.restore")}
                </button>
              ) : noRestore !== null ? (
                <p className="muted history-note">{t(`history.action.${noRestore}`)}</p>
              ) : null}
            </footer>
          </div>
        </div>
      </div>
    </Modal>
  );
}
