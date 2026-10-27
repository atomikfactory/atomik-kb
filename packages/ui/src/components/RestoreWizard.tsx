// The restore wizard (ADR-0059, `docs/architecture/drive-backup.md` section 6): "Restore from a backup" on the first onboarding screen
// opens a dialog with four steps.
//
// 1. **Source.** "A backup file…" (the shell's file dialog, as before ADR-0059), "A folder of backups…" (when the transport lists
//    folders) and "Google Drive" (when the transport has it and a Google client exists: Connect, with a waiting state and Cancel,
//    or "Connected as <address>").
// 2. **List.** The backups of the folder or of Drive: name, local date and time, size, newest first, the newest selected;
//    "Choose another folder…" or "Disconnect" as fits. An empty list says so.
// 3. **Fetch.** A folder's backup is used in place (its `id` is its absolute path); a Drive backup is downloaded into the shell's
//    staging file, with a progress bar and Cancel.
// 4. **Restore.** `importBackup(undefined, path)`; when the backup needs its recovery kit (`InvalidInput` for the field `kit`) the
//    existing kit dialog opens with the path already known, so the shell asks for no second file.
//
// The Drive connection is the store's (`store-drive.ts`, which Settings uses too); the list and the download go straight to
// `client.drive()`. Keyboard and screen readers: focus moves to the heading of each step, the backups are a radio group, a
// failure is announced where it happened, and Escape closes the wizard whenever nothing runs.

import { driveReasonOf, toCoreError } from "@atomik-kb/core-client";
import type { BackupFileInfo, DriveErrorReason, DriveProgress, DriveStatus } from "@atomik-kb/core-client";
import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import type { ReactNode, RefObject } from "react";

import { detailOf, isFieldError } from "../errors";
import { formatBytes, formatTimestamp } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore, useStoreApi } from "../store-context";
import type { DriveNotice } from "../store-drive";
import { Modal } from "./Modal";
import { NoticeText, RestoreKitDialog, describeRestoreError } from "./RestoreKit";
import type { Notice } from "./RestoreKit";

type Stage = "source" | "list" | "download" | "restore";

/** What a command that runs in the source step or the list is doing (it disables the other buttons). */
type Pending = "file" | "folder" | "list";

/** What a Drive failure interrupted: the sign-in, the list of backups or the download. */
type DriveStep = "connect" | "list" | "download";

/** The backups the list step shows. */
interface Listing {
  source: "folder" | "drive";
  /** The folder's absolute path (a folder listing). */
  folder?: string;
  /** Newest first. */
  backups: readonly BackupFileInfo[];
}

/** What a failure of a Drive command says, by the reason the shell gave it. */
const DRIVE_REASON_TEXT: Readonly<Record<DriveErrorReason, MessageKey>> = {
  consentDeclined: "restore.wizard.error.consentDeclined",
  cancelled: "restore.wizard.error.signInCancelled",
  noClient: "restore.wizard.error.noClient",
  browserTimeout: "restore.wizard.error.browserTimeout",
  denied: "restore.wizard.error.denied",
  reconnect: "restore.wizard.error.reconnect",
  quota: "restore.wizard.error.quota",
  notConnected: "restore.wizard.error.notConnected",
  notFound: "restore.wizard.error.notFound",
  network: "restore.wizard.error.network",
  server: "restore.wizard.error.server",
};

/** The notice of a failed Drive command: giving up (a cancel, a declined consent) is information, anything else an error. */
function driveNoticeOf(reason: DriveErrorReason | null, message: string, step: DriveStep): Notice {
  if (reason === null) {
    return { tone: "error", text: t("restore.wizard.error.generic", { message }) };
  }
  if (reason === "cancelled") {
    return {
      tone: "info",
      text: t(step === "download" ? "restore.wizard.error.downloadCancelled" : "restore.wizard.error.signInCancelled"),
    };
  }
  return { tone: reason === "consentDeclined" ? "info" : "error", text: t(DRIVE_REASON_TEXT[reason]) };
}

/** The notice of a failure the store kept (`driveNotice`). */
function storedDriveNotice(stored: DriveNotice, step: DriveStep): Notice {
  return driveNoticeOf(stored.reason, stored.message, step);
}

/**
 * A cancelled folder dialog is `InvalidInput` for `path` ("no file was chosen" or "no folder was chosen"), as for the other
 * dialogs; any other `path` failure is a folder that cannot be read.
 */
function folderNoticeOf(failure: unknown): Notice {
  const error = toCoreError(failure);
  if (isFieldError(error, "path") && /^no (file|folder) was chosen/i.test(error.message)) {
    return { tone: "info", text: t("restore.wizard.folderCancelled") };
  }
  if (isFieldError(error, "path")) {
    return { tone: "error", text: t("restore.wizard.error.folder", { message: error.message }) };
  }
  return { tone: "error", text: t("restore.wizard.error.generic", { message: error.message }) };
}

/** What a failed restore of a listed backup says: a `path` failure is a file that is gone, not a cancelled dialog. */
function restoreNoticeOf(failure: unknown): Notice {
  const error = toCoreError(failure);
  if (isFieldError(error, "path")) {
    return { tone: "error", text: t("restore.wizard.error.missing") };
  }
  return describeRestoreError(error);
}

/** The listing's backups, newest first whatever order the shell sent them in. */
function newestFirst(backups: readonly BackupFileInfo[]): readonly BackupFileInfo[] {
  return [...backups].sort((a, b) => b.modifiedAt - a.modifiedAt);
}

/** "Connected as <address>", or "Connected" when Google gave no address. */
function connectedText(status: DriveStatus | null): string {
  return status?.account === undefined
    ? t("restore.wizard.drive.connected")
    : t("restore.wizard.drive.connectedAs", { account: status.account });
}

function StepHeading({
  id,
  headingRef,
  children,
}: {
  id: string;
  headingRef: RefObject<HTMLHeadingElement | null>;
  children: ReactNode;
}) {
  return (
    <h3 className="subheading restore-step-heading" id={id} ref={headingRef} tabIndex={-1}>
      {children}
    </h3>
  );
}

interface StepProps {
  headingRef: RefObject<HTMLHeadingElement | null>;
}

interface SourceStepProps extends StepProps {
  notice: Notice | null;
  /** Something runs: the buttons wait. */
  busy: boolean;
  pending: Pending | null;
  offerFolder: boolean;
  /** Google Drive is offered: the transport has it and a Google client exists. */
  offerDrive: boolean;
  driveStatus: DriveStatus | null;
  /** A sign-in waits for the browser. */
  connecting: boolean;
  onFile: () => void;
  onFolder: () => void;
  onConnect: () => void;
  onCancelConnect: () => void;
  onShowDrive: () => void;
  onCancel: () => void;
}

function SourceStep(props: SourceStepProps) {
  const { notice, busy, pending, driveStatus, connecting } = props;
  const headingId = useId();
  const fileHintId = useId();
  const folderHintId = useId();
  const driveTitleId = useId();
  const driveHintId = useId();
  const ready = driveStatus?.connected === true && !driveStatus.needsReconnect;
  return (
    <section aria-labelledby={headingId}>
      <StepHeading id={headingId} headingRef={props.headingRef}>
        {t("restore.wizard.source.title")}
      </StepHeading>
      <p className="muted">{t("restore.wizard.source.intro")}</p>
      {notice !== null && <NoticeText notice={notice} />}
      <ul className="restore-sources">
        <li className="restore-source">
          <button
            type="button"
            className="button"
            disabled={busy}
            aria-describedby={fileHintId}
            onClick={props.onFile}
          >
            {pending === "file" ? t("onboarding.restore.working") : t("restore.wizard.source.file")}
          </button>
          <p className="muted" id={fileHintId}>
            {t("restore.wizard.source.file.hint")}
          </p>
        </li>
        {props.offerFolder && (
          <li className="restore-source">
            <button
              type="button"
              className="button"
              disabled={busy}
              aria-describedby={folderHintId}
              onClick={props.onFolder}
            >
              {t("restore.wizard.source.folder")}
            </button>
            <p className="muted" id={folderHintId}>
              {t("restore.wizard.source.folder.hint")}
            </p>
          </li>
        )}
        {props.offerDrive && (
          <li className="restore-source" aria-labelledby={driveTitleId}>
            <span className="restore-source-title" id={driveTitleId}>
              {t("restore.wizard.source.drive")}
            </span>
            {connecting ? (
              <>
                <p className="muted" role="status">
                  {t("restore.wizard.drive.waiting")}
                </p>
                <button type="button" className="button" onClick={props.onCancelConnect}>
                  {t("restore.wizard.drive.cancel")}
                </button>
              </>
            ) : ready ? (
              <>
                <p className="muted" id={driveHintId}>
                  {connectedText(driveStatus)}
                </p>
                <button
                  type="button"
                  className="button"
                  disabled={busy}
                  aria-describedby={driveHintId}
                  onClick={props.onShowDrive}
                >
                  {pending === "list" ? t("restore.wizard.drive.loading") : t("restore.wizard.drive.show")}
                </button>
              </>
            ) : (
              <>
                <p className="muted" id={driveHintId}>
                  {driveStatus?.needsReconnect === true
                    ? t("restore.wizard.drive.reconnect")
                    : t("restore.wizard.drive.hint")}
                </p>
                <button
                  type="button"
                  className="button"
                  disabled={busy}
                  aria-describedby={driveHintId}
                  onClick={props.onConnect}
                >
                  {t("restore.wizard.drive.connect")}
                </button>
              </>
            )}
          </li>
        )}
      </ul>
      <div className="dialog-actions">
        <button type="button" className="button" disabled={busy} onClick={props.onCancel}>
          {t("dialog.cancel")}
        </button>
      </div>
    </section>
  );
}

interface BackupOptionProps {
  file: BackupFileInfo;
  group: string;
  checked: boolean;
  onSelect: (id: string) => void;
}

/** One backup of the list: a radio named by the file's name and described by its date and size. */
function BackupOption({ file, group, checked, onSelect }: BackupOptionProps) {
  const nameId = useId();
  const factsId = useId();
  const when = formatTimestamp(file.modifiedAt);
  return (
    <label className={checked ? "restore-backup is-checked" : "restore-backup"}>
      <input
        type="radio"
        name={group}
        checked={checked}
        aria-labelledby={nameId}
        aria-describedby={factsId}
        onChange={() => {
          onSelect(file.id);
        }}
      />
      <span className="restore-backup-body">
        <span className="restore-backup-name" id={nameId}>
          {file.name}
        </span>
        <span className="muted restore-backup-facts" id={factsId}>
          {when !== null && <time dateTime={when.iso}>{when.text}</time>}{" "}
          <span>{formatBytes(file.size)}</span>
        </span>
      </span>
    </label>
  );
}

interface ListStepProps extends StepProps {
  listing: Listing;
  selected: string | null;
  notice: Notice | null;
  busy: boolean;
  driveStatus: DriveStatus | null;
  onSelect: (id: string) => void;
  onChooseAnother: () => void;
  onDisconnect: () => void;
  onBack: () => void;
  onCancel: () => void;
  onRestore: () => void;
}

function ListStep(props: ListStepProps) {
  const { listing, selected, notice, busy } = props;
  const headingId = useId();
  const group = useId();
  return (
    <section aria-labelledby={headingId}>
      <StepHeading id={headingId} headingRef={props.headingRef}>
        {t("restore.wizard.list.title")}
      </StepHeading>
      <p className="muted restore-origin">
        {listing.source === "folder"
          ? t("restore.wizard.list.folder", { folder: listing.folder ?? "" })
          : `${t("restore.wizard.source.drive")}: ${connectedText(props.driveStatus)}`}
      </p>
      {notice !== null && <NoticeText notice={notice} />}
      {listing.backups.length === 0 ? (
        <p className="muted" role="status">
          {t(listing.source === "folder" ? "restore.wizard.list.empty.folder" : "restore.wizard.list.empty.drive")}
        </p>
      ) : (
        <div className="restore-backups" role="radiogroup" aria-labelledby={headingId}>
          {listing.backups.map((file) => (
            <BackupOption
              key={file.id}
              file={file}
              group={group}
              checked={selected === file.id}
              onSelect={props.onSelect}
            />
          ))}
        </div>
      )}
      <div className="restore-list-actions">
        {listing.source === "folder" ? (
          <button type="button" className="button" disabled={busy} onClick={props.onChooseAnother}>
            {t("restore.wizard.list.chooseAnother")}
          </button>
        ) : (
          <button type="button" className="button" disabled={busy} onClick={props.onDisconnect}>
            {t("restore.wizard.drive.disconnect")}
          </button>
        )}
      </div>
      <div className="dialog-actions">
        <button type="button" className="button" disabled={busy} onClick={props.onCancel}>
          {t("dialog.cancel")}
        </button>
        <button type="button" className="button" disabled={busy} onClick={props.onBack}>
          {t("onboarding.back")}
        </button>
        <button
          type="button"
          className="button button-primary"
          disabled={busy || selected === null}
          onClick={props.onRestore}
        >
          {t("restore.wizard.list.restore")}
        </button>
      </div>
    </section>
  );
}

interface DownloadStepProps extends StepProps {
  progress: DriveProgress | null;
  onCancel: () => void;
}

function DownloadStep({ headingRef, progress, onCancel }: DownloadStepProps) {
  const headingId = useId();
  const total = progress?.total !== undefined && progress.total > 0 ? progress.total : null;
  let text = t("restore.wizard.download.starting");
  if (progress !== null) {
    text =
      total === null
        ? t("restore.wizard.download.received", { received: formatBytes(progress.received) })
        : t("restore.wizard.download.progress", {
            received: formatBytes(Math.min(progress.received, total)),
            total: formatBytes(total),
          });
  }
  return (
    <section aria-labelledby={headingId}>
      <StepHeading id={headingId} headingRef={headingRef}>
        {t("restore.wizard.download.title")}
      </StepHeading>
      <div className="restore-download">
        {progress !== null && total !== null ? (
          <progress aria-label={t("restore.wizard.download.label")} max={total} value={Math.min(progress.received, total)} />
        ) : (
          <progress aria-label={t("restore.wizard.download.label")} />
        )}
        <p className="muted">{text}</p>
      </div>
      <div className="dialog-actions">
        <button type="button" className="button" onClick={onCancel}>
          {t("restore.wizard.download.cancel")}
        </button>
      </div>
    </section>
  );
}

function RestoringStep({ headingRef }: StepProps) {
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} aria-busy="true">
      <StepHeading id={headingId} headingRef={headingRef}>
        {t("restore.wizard.restoring.title")}
      </StepHeading>
      <p className="muted" role="status">
        {t("restore.wizard.restoring.body")}
      </p>
    </section>
  );
}

export interface RestoreWizardProps {
  /** Where focus goes when the wizard closes (the "Restore from a backup" button). */
  returnFocus?: RefObject<HTMLElement | null>;
  onClose: () => void;
}

/**
 * The restore wizard: pick a source (file, folder, Google Drive), pick a backup of the list, fetch it and restore it, asking for
 * the recovery kit when the backup needs it. A successful restore starts the app again with the workspace and takes the
 * onboarding, and this dialog with it.
 */
export function RestoreWizard({ returnFocus, onClose }: RestoreWizardProps) {
  const client = useAppStore((state) => state.aiClient);
  const importBackup = useAppStore((state) => state.importBackup);
  const driveStatus = useAppStore((state) => state.driveStatus);
  const driveConnecting = useAppStore((state) => state.driveConnecting);
  const driveBusy = useAppStore((state) => state.driveBusy);
  const loadDriveStatus = useAppStore((state) => state.loadDriveStatus);
  const connectDrive = useAppStore((state) => state.connectDrive);
  const cancelDriveConnect = useAppStore((state) => state.cancelDriveConnect);
  const disconnectDrive = useAppStore((state) => state.disconnectDrive);
  const clearDriveNotice = useAppStore((state) => state.clearDriveNotice);
  const store = useStoreApi();

  const hasDrive = client.hasDrive();
  const offerFolder = client.hasBackupFolderList();
  const offerDrive = hasDrive && driveStatus !== null && driveStatus.client !== "none";

  const [stage, setStage] = useState<Stage>("source");
  const [listing, setListing] = useState<Listing | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [progress, setProgress] = useState<DriveProgress | null>(null);
  const [askKit, setAskKit] = useState<{ path: string | undefined } | null>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const titleId = useId();

  const busy = pending !== null || driveConnecting || driveBusy || stage === "download" || stage === "restore";

  // The heading of each step takes focus when the step shows (not under the kit dialog, which has just taken it).
  const focusHeading = useEffectEvent(() => {
    if (askKit === null) {
      headingRef.current?.focus();
    }
  });
  useEffect(() => {
    focusHeading();
  }, [stage]);

  // What Google Drive is here is read each time the wizard opens (no network): the connection may have changed.
  useEffect(() => {
    if (hasDrive) {
      void loadDriveStatus();
    }
  }, [hasDrive, loadDriveStatus]);

  function openList(next: Listing): void {
    setListing(next);
    setSelected(next.backups[0]?.id ?? null);
    setNotice(null);
    setStage("list");
  }

  /** A Drive command failed: say why where it happened; a connection that is gone sends the person back to Connect. */
  function driveFailed(failure: unknown, step: DriveStep): void {
    const error = toCoreError(failure);
    const reason = driveReasonOf(error) ?? null;
    setNotice(driveNoticeOf(reason, error.message, step));
    if (reason === "reconnect" || reason === "notConnected") {
      void loadDriveStatus();
      setStage("source");
    }
  }

  /** Reads the failure the store kept for the last Drive action, if any, and takes it from there. */
  function takeDriveNotice(step: DriveStep): boolean {
    const stored = store.getState().driveNotice;
    if (stored === null) {
      return false;
    }
    setNotice(storedDriveNotice(stored, step));
    clearDriveNotice();
    return true;
  }

  async function restoreFromFile(): Promise<void> {
    setPending("file");
    setNotice(null);
    try {
      await importBackup();
      // Restored: the app starts again with the workspace and the wizard goes with the onboarding.
    } catch (failure) {
      const error = toCoreError(failure);
      setPending(null);
      if (isFieldError(error, "kit")) {
        setAskKit({ path: detailOf(error, "path") ?? undefined });
        return;
      }
      setNotice(describeRestoreError(error));
    }
  }

  async function listFolder(): Promise<void> {
    setPending("folder");
    setNotice(null);
    try {
      const folder = await client.backupFolderList();
      openList({ source: "folder", folder: folder.folder, backups: newestFirst(folder.backups) });
    } catch (failure) {
      setNotice(folderNoticeOf(failure));
    } finally {
      setPending(null);
    }
  }

  async function listDrive(): Promise<void> {
    setPending("list");
    setNotice(null);
    try {
      const backups = await client.drive().listBackups();
      openList({ source: "drive", backups: newestFirst(backups) });
    } catch (failure) {
      driveFailed(failure, "list");
    } finally {
      setPending(null);
    }
  }

  async function connect(): Promise<void> {
    setNotice(null);
    await connectDrive();
    if (takeDriveNotice("connect")) {
      return;
    }
    if (store.getState().driveStatus?.connected === true) {
      await listDrive();
    }
  }

  async function disconnect(): Promise<void> {
    setNotice(null);
    await disconnectDrive();
    if (takeDriveNotice("connect")) {
      return;
    }
    setListing(null);
    setStage("source");
  }

  function cancelDownload(): void {
    client
      .drive()
      .cancel()
      .catch(() => undefined);
  }

  async function restoreSelected(): Promise<void> {
    const file = listing?.backups.find((backup) => backup.id === selected);
    if (listing === null || file === undefined) {
      return;
    }
    setNotice(null);
    let path = file.id;
    if (listing.source === "drive") {
      setProgress(null);
      setStage("download");
      try {
        path = await client.drive().download(file.id, (next) => {
          setProgress(next);
        });
      } catch (failure) {
        setStage("list");
        driveFailed(failure, "download");
        return;
      }
    }
    setStage("restore");
    try {
      await importBackup(undefined, path);
      // Restored: the app starts again with the workspace and the wizard goes with the onboarding.
    } catch (failure) {
      setStage("list");
      const error = toCoreError(failure);
      if (isFieldError(error, "kit")) {
        setAskKit({ path });
        return;
      }
      setNotice(restoreNoticeOf(failure));
    }
  }

  return (
    <>
      <Modal
        labelledBy={titleId}
        onDismiss={busy ? undefined : onClose}
        initialFocus={headingRef}
        returnFocus={returnFocus}
        className="restore-wizard"
      >
        <h2 className="subtitle" id={titleId}>
          {t("restore.wizard.title")}
        </h2>
        {stage === "source" && (
          <SourceStep
            headingRef={headingRef}
            notice={notice}
            busy={busy}
            pending={pending}
            offerFolder={offerFolder}
            offerDrive={offerDrive}
            driveStatus={driveStatus}
            connecting={driveConnecting}
            onFile={() => {
              void restoreFromFile();
            }}
            onFolder={() => {
              void listFolder();
            }}
            onConnect={() => {
              void connect();
            }}
            onCancelConnect={() => {
              void cancelDriveConnect();
            }}
            onShowDrive={() => {
              void listDrive();
            }}
            onCancel={onClose}
          />
        )}
        {stage === "list" && listing !== null && (
          <ListStep
            headingRef={headingRef}
            listing={listing}
            selected={selected}
            notice={notice}
            busy={busy}
            driveStatus={driveStatus}
            onSelect={setSelected}
            onChooseAnother={() => {
              void listFolder();
            }}
            onDisconnect={() => {
              void disconnect();
            }}
            onBack={() => {
              setNotice(null);
              setStage("source");
            }}
            onCancel={onClose}
            onRestore={() => {
              void restoreSelected();
            }}
          />
        )}
        {stage === "download" && <DownloadStep headingRef={headingRef} progress={progress} onCancel={cancelDownload} />}
        {stage === "restore" && <RestoringStep headingRef={headingRef} />}
      </Modal>
      {askKit !== null && (
        <RestoreKitDialog
          path={askKit.path}
          returnFocus={headingRef}
          onClose={() => {
            setAskKit(null);
          }}
        />
      )}
    </>
  );
}
