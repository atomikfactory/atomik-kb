// Settings -> Security -> Backup -> Scheduled backups -> "Google Drive" (ADR-0059, `docs/architecture/drive-backup.md` section 5):
// the second target of the scheduled backups. Not connected: "Connect Google Drive" (the shell asks for consent, then signs in in
// the system browser: the page shows "Finish signing in in your browser…" with Cancel while it waits). Connected: the account's
// address, "Open the folder in Drive", Disconnect (a question first: the files stay in Drive) and the last upload; a banner asks
// to reconnect when Google refused the stored sign-in. Without a Google client (a build that has none and no client of one's
// own) it says so and opens "Advanced", where one's own client id and secret are entered and the built-in client is chosen again.
//
// The slice (`store-drive.ts`) does the work and keeps the status; this part words it. Shown only where the shell has Drive.

import type { BackupRunStatus, DriveErrorReason, DriveStatus } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";

import { ConfirmDialog } from "../components/ConfirmDialog";
import { formatTimestamp } from "../format";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import type { DriveNotice } from "../store-drive";

/** The sentence for a failure's reason. */
export function driveReasonText(reason: DriveErrorReason): string {
  return t(`security.scheduled.drive.error.${reason}`);
}

/** The sentence for a notice: its reason's own text, or the generic one with the core's message. */
export function driveNoticeText(notice: DriveNotice): string {
  return notice.reason === null
    ? t("security.scheduled.drive.error.other", { message: notice.message })
    : driveReasonText(notice.reason);
}

/** The last upload to Drive: when and which file, or when and why it failed (the shell's own text, generic by contract). */
function LastUpload({ last }: { last: BackupRunStatus | undefined }) {
  if (last === undefined) {
    return <p className="muted">{t("security.scheduled.drive.last.none")}</p>;
  }
  const when = formatTimestamp(last.at)?.text ?? "";
  if (last.ok) {
    return <p>{t("security.scheduled.drive.last.ok", { date: when, file: last.file ?? "" })}</p>;
  }
  return <p className="error-text">{t("security.scheduled.drive.last.failed", { date: when, error: last.error ?? "" })}</p>;
}

interface AdvancedProps {
  status: DriveStatus;
  /** Whether the person opened or closed it; `null` before they did (open when there is no client at all). */
  open: boolean | null;
  onToggle: (open: boolean) => void;
  /** A sign-in or another change runs: the form waits. */
  disabled: boolean;
}

/** "Use your own Google client": the id and the secret, Save, and "Use the built-in client". */
function DriveAdvanced({ status, open, onToggle, disabled }: AdvancedProps) {
  const setDriveClient = useAppStore((state) => state.setDriveClient);
  // What was typed in the id field; `null` shows the id that is stored.
  const [idDraft, setIdDraft] = useState<string | null>(null);
  // Held only while the form is open; it goes to the shell in one call and is never kept anywhere else.
  const [secret, setSecret] = useState("");
  const idId = useId();
  const secretId = useId();
  const clientId = idDraft ?? status.ownClientId ?? "";

  async function save(): Promise<void> {
    const id = clientId.trim();
    const typed = secret.trim();
    if (await setDriveClient({ clientId: id, ...(typed === "" ? {} : { clientSecret: typed }) })) {
      setSecret("");
      setIdDraft(null);
    }
  }

  async function chooseBuiltIn(): Promise<void> {
    if (await setDriveClient(null)) {
      setSecret("");
      setIdDraft(null);
    }
  }

  return (
    <details
      className="scheduled-backups-advanced"
      open={open ?? status.client === "none"}
      onToggle={(event) => {
        onToggle(event.currentTarget.open);
      }}
    >
      <summary>{t("security.scheduled.drive.advanced")}</summary>
      <div className="scheduled-backups-advanced-body">
        <p className="scheduled-backups-label">{t("security.scheduled.drive.ownTitle")}</p>
        <p className="muted">{t("security.scheduled.drive.ownHelp")}</p>
        {status.client === "own" && <p>{t("security.scheduled.drive.ownInUse")}</p>}
        <div className="field">
          <label htmlFor={idId}>{t("security.scheduled.drive.clientId")}</label>
          <input
            id={idId}
            className="input"
            type="text"
            autoComplete="off"
            spellCheck={false}
            value={clientId}
            disabled={disabled}
            onChange={(event) => {
              setIdDraft(event.target.value);
            }}
          />
        </div>
        <div className="field">
          <label htmlFor={secretId}>{t("security.scheduled.drive.clientSecret")}</label>
          <input
            id={secretId}
            className="input"
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={secret}
            disabled={disabled}
            onChange={(event) => {
              setSecret(event.target.value);
            }}
          />
        </div>
        <div className="row-actions">
          <button
            type="button"
            className="button"
            disabled={disabled || clientId.trim() === ""}
            onClick={() => {
              void save();
            }}
          >
            {t("security.scheduled.drive.saveClient")}
          </button>
          {status.client === "own" && (
            <button
              type="button"
              className="button"
              disabled={disabled}
              onClick={() => {
                void chooseBuiltIn();
              }}
            >
              {t("security.scheduled.drive.useBuiltIn")}
            </button>
          )}
        </div>
      </div>
    </details>
  );
}

/** The Google Drive target of the scheduled backups, in Settings. */
export function DriveBackupTarget() {
  const status = useAppStore((state) => state.driveStatus);
  const connecting = useAppStore((state) => state.driveConnecting);
  const busy = useAppStore((state) => state.driveBusy);
  const notice = useAppStore((state) => state.driveNotice);
  const connectDrive = useAppStore((state) => state.connectDrive);
  const clearDriveNotice = useAppStore((state) => state.clearDriveNotice);
  const cancelDriveConnect = useAppStore((state) => state.cancelDriveConnect);
  const disconnectDrive = useAppStore((state) => state.disconnectDrive);
  const openExternal = useAppStore((state) => state.openExternal);
  const [disconnecting, setDisconnecting] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState<boolean | null>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const labelId = useId();
  const noClientId = useId();

  // A notice belongs to what was just done here: one left by the restore wizard, or from an earlier visit, is not shown.
  useEffect(() => {
    clearDriveNotice();
    return () => {
      clearDriveNotice();
    };
  }, [clearDriveNotice]);

  // The Connect button is replaced by Cancel while the browser is awaited: focus follows it.
  useEffect(() => {
    if (connecting) {
      cancelButton.current?.focus();
    }
  }, [connecting]);

  const waiting = (
    <div className="row-actions scheduled-backups-waiting">
      <span role="status">{t("security.scheduled.drive.connecting")}</span>
      <button
        ref={cancelButton}
        type="button"
        className="button"
        onClick={() => {
          void cancelDriveConnect();
        }}
      >
        {t("security.scheduled.drive.cancel")}
      </button>
    </div>
  );

  function connectButton(label: string, describedBy?: string, disabled = false) {
    return (
      <button
        type="button"
        className="button"
        disabled={disabled || busy}
        aria-describedby={describedBy}
        onClick={() => {
          void connectDrive();
        }}
      >
        {label}
      </button>
    );
  }

  function body(current: DriveStatus) {
    if (!current.connected) {
      const noClient = current.client === "none";
      return (
        <>
          <p className="muted" id={noClient ? noClientId : undefined}>
            {noClient ? t("security.scheduled.drive.noClient") : t("security.scheduled.drive.about")}
          </p>
          {connecting ? (
            waiting
          ) : (
            <div className="row-actions">
              {connectButton(t("security.scheduled.drive.connect"), noClient ? noClientId : undefined, noClient)}
            </div>
          )}
        </>
      );
    }
    const { folderUrl } = current;
    return (
      <>
        <p>
          {current.account === undefined
            ? t("security.scheduled.drive.connected")
            : t("security.scheduled.drive.connectedAs", { account: current.account })}
        </p>
        {current.needsReconnect && (
          <div className="banner banner-warning" role="group" aria-label={t("security.scheduled.drive.reconnectLabel")}>
            <p>{t("security.scheduled.drive.reconnectText")}</p>
            {connecting ? waiting : <div className="row-actions">{connectButton(t("security.scheduled.drive.reconnect"))}</div>}
          </div>
        )}
        <div className="row-actions">
          {folderUrl !== undefined && (
            <button
              type="button"
              className="button"
              onClick={() => {
                void openExternal(folderUrl);
              }}
            >
              {t("security.scheduled.drive.openFolder")}
            </button>
          )}
          <button
            type="button"
            className="button"
            disabled={busy || connecting}
            onClick={() => {
              setDisconnecting(true);
            }}
          >
            {t("security.scheduled.drive.disconnect")}
          </button>
        </div>
        <LastUpload last={current.last} />
      </>
    );
  }

  return (
    <div className="field scheduled-backups-drive" role="group" aria-labelledby={labelId}>
      <span className="scheduled-backups-label" id={labelId}>
        {t("security.scheduled.drive.label")}
      </span>
      {status === null ? (
        notice === null && <p className="muted">{t("security.scheduled.drive.checking")}</p>
      ) : (
        body(status)
      )}
      {notice !== null && (
        <p className={notice.kind === "error" ? "error-text" : "muted"} role={notice.kind === "error" ? "alert" : "status"}>
          {driveNoticeText(notice)}
        </p>
      )}
      {status !== null && (
        <DriveAdvanced
          status={status}
          open={advancedOpen}
          onToggle={setAdvancedOpen}
          disabled={busy || connecting}
        />
      )}
      {disconnecting && (
        <ConfirmDialog
          title={t("security.scheduled.drive.disconnectTitle")}
          body={t("security.scheduled.drive.disconnectBody")}
          confirmLabel={t("security.scheduled.drive.disconnect")}
          onConfirm={async () => {
            await disconnectDrive();
          }}
          onClose={() => {
            setDisconnecting(false);
          }}
        />
      )}
    </div>
  );
}
