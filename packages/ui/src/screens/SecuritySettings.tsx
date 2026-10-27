// Settings → Security (Milestone 4): the recovery kit's status (create, re-issue), the app
// lock (turn on, change, turn off, lock now, auto-lock delay), the devices of the account
// (Milestone 7: add one by pairing, remove one) and backups (export, and the scheduled ones of
// `ScheduledBackups.tsx`, ADR-0036). Passphrases and kits
// stay in the state of the form or dialog that asks for or shows them, and are gone when it
// closes.

import { PASSPHRASE_MAX_BYTES, toCoreError } from "@atomik-kb/core-client";
import type { BackupResult, DeviceEntry, RecoveryKit, SecurityInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { AddDeviceDialog } from "../components/AddDeviceDialog";
import { Modal } from "../components/Modal";
import { RecoveryKitDialog, RecoveryKitFlow } from "../components/RecoveryKit";
import { RemoveDeviceDialog } from "../components/RemoveDeviceDialog";
import { detailOf, isFieldError } from "../errors";
import { formatBytes, formatTimestamp } from "../format";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";
import { AUTO_LOCK_CHOICES, AUTO_LOCK_KEY, parseAutoLockMinutes } from "../store-security";
import { ScheduledBackupSettings } from "./ScheduledBackups";

/**
 * The shortest passphrase the forms accept, in characters (the core counts 8 to 256 UTF-8
 * bytes; 8 characters are at least 8 bytes).
 */
export const MIN_PASSPHRASE_CHARS = 8;

const utf8 = new TextEncoder();

const AUTO_LOCK_LABELS: Readonly<Record<string, MessageKey>> = {
  "0": "security.lock.auto.0",
  "1": "security.lock.auto.1",
  "5": "security.lock.auto.5",
  "15": "security.lock.auto.15",
  "60": "security.lock.auto.60",
};

function Timestamp({ value }: { value: number }) {
  const formatted = formatTimestamp(value);
  return formatted === null ? null : <time dateTime={formatted.iso}>{formatted.text}</time>;
}

// ----- recovery kit -------------------------------------------------------------------

/**
 * Re-issuing: first the warning (and the app-lock passphrase while the lock is on), then
 * the new kit, shown and checked like the first one. Once the new kit is up the dialog
 * only closes through "Done": it is the only time that kit is shown.
 */
export function ReissueKitDialog({ lockEnabled, onClose }: { lockEnabled: boolean; onClose: () => void }) {
  const reissueRecoveryKit = useAppStore((state) => state.reissueRecoveryKit);
  const [kit, setKit] = useState<RecoveryKit | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const titleId = useId();
  const bodyId = useId();
  const fieldId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (lockEnabled && passphrase === "") {
      setError(t("security.reissue.error.empty"));
      return;
    }
    const typed = passphrase;
    setPassphrase("");
    setError(null);
    setBusy(true);
    try {
      setKit(await reissueRecoveryKit(lockEnabled ? typed : undefined));
    } catch (failure) {
      const reason = toCoreError(failure);
      setError(
        isFieldError(reason, "passphrase")
          ? t("security.reissue.error.wrong")
          : t("security.reissue.error.generic", { message: reason.message }),
      );
      setBusy(false);
    }
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={kit === null ? bodyId : undefined}
      onDismiss={kit !== null || busy ? undefined : onClose}
      className={kit === null ? undefined : "kit-dialog"}
    >
      <h2 className="subtitle" id={titleId}>
        {kit === null ? t("security.reissue.title") : t("security.reissue.newTitle")}
      </h2>
      {kit === null ? (
        <form
          className="form reissue-form"
          noValidate
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <p id={bodyId}>{t("security.reissue.body")}</p>
          {lockEnabled && (
            <>
              <label htmlFor={fieldId}>{t("security.reissue.passphraseLabel")}</label>
              <input
                id={fieldId}
                className="input"
                type="password"
                value={passphrase}
                autoComplete="current-password"
                readOnly={busy}
                aria-invalid={error !== null}
                aria-describedby={error !== null ? errorId : undefined}
                onChange={(event) => {
                  setPassphrase(event.target.value);
                  setError(null);
                }}
              />
            </>
          )}
          {error !== null && (
            <p className="error-text" id={errorId} role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={busy} onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-danger" disabled={busy}>
              {busy ? t("security.reissue.working") : t("security.reissue.confirm")}
            </button>
          </div>
        </form>
      ) : (
        <RecoveryKitFlow initialKit={kit} onDone={onClose} />
      )}
    </Modal>
  );
}

function RecoveryKitSettings({ security }: { security: SecurityInfo }) {
  const [dialog, setDialog] = useState<"create" | "reissue" | null>(null);
  const titleId = useId();
  const kit = security.recoveryKit;
  const close = (): void => {
    setDialog(null);
  };

  return (
    <section className="subpanel" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("security.kit.title")}
      </h3>
      {kit === undefined ? (
        <>
          <p>{t("security.kit.missing")}</p>
          <div className="row-actions">
            <button
              type="button"
              className="button button-primary"
              onClick={() => {
                setDialog("create");
              }}
            >
              {t("security.kit.create")}
            </button>
          </div>
        </>
      ) : (
        <>
          <dl className="facts">
            <dt>{t("security.kit.created")}</dt>
            <dd>
              <Timestamp value={kit.createdAt} />
            </dd>
            <dt>{t("security.kit.verified")}</dt>
            <dd>{kit.verifiedAt === undefined ? t("security.kit.notVerified") : <Timestamp value={kit.verifiedAt} />}</dd>
            {kit.reissuedAt !== undefined && (
              <>
                <dt>{t("security.kit.reissued")}</dt>
                <dd>
                  <Timestamp value={kit.reissuedAt} />
                </dd>
              </>
            )}
          </dl>
          <p className="muted">{t("security.kit.reissueNote")}</p>
          <div className="row-actions">
            <button
              type="button"
              className="button"
              onClick={() => {
                setDialog("reissue");
              }}
            >
              {t("security.kit.reissue")}
            </button>
          </div>
        </>
      )}
      {dialog === "create" && <RecoveryKitDialog onClose={close} />}
      {dialog === "reissue" && <ReissueKitDialog lockEnabled={security.appLock.enabled} onClose={close} />}
    </section>
  );
}

// ----- app lock -----------------------------------------------------------------------

type LockFormMode = "enable" | "change" | "disable";

/** A problem with what was typed; `field` says which input it belongs to. */
interface FormError {
  field: "current" | "next" | "repeat" | null;
  text: string;
}

function checkNewPassphrase(next: string, repeat: string): FormError | null {
  if (Array.from(next).length < MIN_PASSPHRASE_CHARS) {
    return { field: "next", text: t("security.lock.error.short") };
  }
  if (utf8.encode(next).length > PASSPHRASE_MAX_BYTES) {
    return { field: "next", text: t("security.lock.error.long", { max: PASSPHRASE_MAX_BYTES }) };
  }
  if (next !== repeat) {
    return { field: "repeat", text: t("security.lock.error.mismatch") };
  }
  return null;
}

/**
 * What the core's refusal means for the form: `newField` is the request field that holds
 * the new passphrase (`passphrase` when turning the lock on, `new` when changing it); a
 * refused current passphrase is a wrong one.
 */
function lockFailure(error: unknown, newField: string | null): FormError {
  const failure = toCoreError(error);
  const field = failure.code === "InvalidInput" ? detailOf(failure, "field") : null;
  if (field !== null && field === newField) {
    return { field: "next", text: t("security.lock.error.field", { message: failure.message }) };
  }
  if (field === "passphrase" || field === "current") {
    return { field: "current", text: t("security.lock.error.wrong") };
  }
  return { field: null, text: t("security.lock.error.generic", { message: failure.message }) };
}

interface PassphraseFieldProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  autoComplete: "current-password" | "new-password";
  autoFocus?: boolean;
  readOnly: boolean;
  invalid: boolean;
  describedBy?: string;
}

function PassphraseField({ label, value, onChange, autoComplete, autoFocus, readOnly, invalid, describedBy }: PassphraseFieldProps) {
  const id = useId();
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        id={id}
        className="input"
        type="password"
        value={value}
        autoComplete={autoComplete}
        autoFocus={autoFocus}
        readOnly={readOnly}
        aria-invalid={invalid}
        aria-describedby={describedBy}
        onChange={(event) => {
          onChange(event.target.value);
        }}
      />
    </div>
  );
}

const LOCK_FORM_LABELS: Readonly<Record<LockFormMode, { submit: MessageKey; busy: MessageKey; done: MessageKey }>> = {
  enable: {
    submit: "security.lock.enableSubmit",
    busy: "security.lock.enabling",
    done: "security.lock.done.enabled",
  },
  change: {
    submit: "security.lock.changeSubmit",
    busy: "security.lock.changing",
    done: "security.lock.done.changed",
  },
  disable: {
    submit: "security.lock.disableSubmit",
    busy: "security.lock.disabling",
    done: "security.lock.done.disabled",
  },
};

/** Turning the lock on (new passphrase twice), changing the passphrase, or turning it off. */
function LockForm({
  mode,
  onDone,
  onCancel,
}: {
  mode: LockFormMode;
  onDone: (notice: string) => void;
  onCancel: () => void;
}) {
  const enableAppLock = useAppStore((state) => state.enableAppLock);
  const changeAppLockPassphrase = useAppStore((state) => state.changeAppLockPassphrase);
  const disableAppLock = useAppStore((state) => state.disableAppLock);
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [error, setError] = useState<FormError | null>(null);
  const [busy, setBusy] = useState(false);
  const errorId = useId();
  const noteId = useId();
  const asksCurrent = mode !== "enable";
  const asksNext = mode !== "disable";
  const labels = LOCK_FORM_LABELS[mode];

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (asksCurrent && current === "") {
      setError({ field: "current", text: t("security.lock.error.empty") });
      return;
    }
    const problem = asksNext ? checkNewPassphrase(next, repeat) : null;
    if (problem !== null) {
      setError(problem);
      return;
    }
    setError(null);
    setBusy(true);
    try {
      if (mode === "enable") {
        await enableAppLock(next);
      } else if (mode === "change") {
        await changeAppLockPassphrase(current, next);
      } else {
        await disableAppLock(current);
      }
      onDone(t(labels.done));
    } catch (failure) {
      const reason = lockFailure(failure, mode === "enable" ? "passphrase" : mode === "change" ? "new" : null);
      if (reason.field === "current") {
        setCurrent("");
      }
      setError(reason);
      setBusy(false);
    }
  }

  const errorFor = (field: FormError["field"]): string | undefined =>
    error !== null && error.field === field ? errorId : undefined;
  const clearError = (): void => {
    setError(null);
  };

  return (
    <form
      className="form lock-form"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      {asksCurrent && (
        <PassphraseField
          label={t("security.lock.currentLabel")}
          value={current}
          onChange={(value) => {
            setCurrent(value);
            clearError();
          }}
          autoComplete="current-password"
          autoFocus
          readOnly={busy}
          invalid={error?.field === "current"}
          describedBy={errorFor("current")}
        />
      )}
      {asksNext && (
        <>
          <PassphraseField
            label={mode === "enable" ? t("security.lock.passphraseLabel") : t("security.lock.nextLabel")}
            value={next}
            onChange={(value) => {
              setNext(value);
              clearError();
            }}
            autoComplete="new-password"
            autoFocus={!asksCurrent}
            readOnly={busy}
            invalid={error?.field === "next"}
            describedBy={[noteId, errorFor("next")].filter((id) => id !== undefined).join(" ")}
          />
          <PassphraseField
            label={mode === "enable" ? t("security.lock.repeatLabel") : t("security.lock.repeatNextLabel")}
            value={repeat}
            onChange={(value) => {
              setRepeat(value);
              clearError();
            }}
            autoComplete="new-password"
            readOnly={busy}
            invalid={error?.field === "repeat"}
            describedBy={errorFor("repeat")}
          />
          <p className="muted" id={noteId}>
            {t("security.lock.forgetNote")}
          </p>
        </>
      )}
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error.text}
        </p>
      )}
      <div className="row-actions">
        <button
          type="submit"
          className={mode === "disable" ? "button button-danger" : "button button-primary"}
          disabled={busy}
        >
          {busy ? t(labels.busy) : t(labels.submit)}
        </button>
        <button type="button" className="button" disabled={busy} onClick={onCancel}>
          {t("dialog.cancel")}
        </button>
      </div>
    </form>
  );
}

/** The auto-lock delay (`app.lock.auto_minutes`, see `AUTO_LOCK_KEY`), a UI setting (ADR-0018). */
function AutoLockChoice() {
  const minutes = useAppStore((state) => parseAutoLockMinutes(state.settings[AUTO_LOCK_KEY]));
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);
  const id = useId();

  return (
    <div className="field auto-lock">
      <label htmlFor={id}>{t("security.lock.auto")}</label>
      <select
        id={id}
        className="input select"
        value={String(minutes)}
        onChange={(event) => {
          setError(null);
          setSetting(AUTO_LOCK_KEY, Number(event.target.value)).catch((failure: unknown) => {
            setError(t("security.lock.error.auto", { message: toCoreError(failure).message }));
          });
        }}
      >
        {AUTO_LOCK_CHOICES.map((choice) => (
          <option key={choice} value={String(choice)}>
            {t(AUTO_LOCK_LABELS[String(choice)] ?? "security.lock.auto.0")}
          </option>
        ))}
      </select>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function AppLockSettings({ security }: { security: SecurityInfo }) {
  const available = useAppStore((state) => state.capabilities.appLock);
  const lock = useAppStore((state) => state.lock);
  const [form, setForm] = useState<LockFormMode | null>(null);
  const [notice, setNotice] = useState("");
  const [lockError, setLockError] = useState<string | null>(null);
  const titleId = useId();
  const enabled = security.appLock.enabled;

  function open(mode: LockFormMode): void {
    setForm(mode);
    setNotice("");
    setLockError(null);
  }

  return (
    <section className="subpanel" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("security.lock.title")}
      </h3>
      <p className="muted">{t("security.lock.intro")}</p>
      {available ? (
        <>
          <p className="lock-state">{enabled ? t("security.lock.on") : t("security.lock.off")}</p>
          {form === null ? (
            <div className="row-actions">
              {enabled ? (
                <>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      setLockError(null);
                      lock().catch((failure: unknown) => {
                        setLockError(t("security.lock.error.lock", { message: toCoreError(failure).message }));
                      });
                    }}
                  >
                    {t("security.lock.lockNow")}
                  </button>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      open("change");
                    }}
                  >
                    {t("security.lock.change")}
                  </button>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      open("disable");
                    }}
                  >
                    {t("security.lock.disable")}
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="button button-primary"
                  onClick={() => {
                    open("enable");
                  }}
                >
                  {t("security.lock.enable")}
                </button>
              )}
            </div>
          ) : (
            <LockForm
              key={form}
              mode={form}
              onDone={(text) => {
                setForm(null);
                setNotice(text);
              }}
              onCancel={() => {
                setForm(null);
              }}
            />
          )}
          {enabled && form === null && <AutoLockChoice />}
          <p className="muted" role="status">
            {notice}
          </p>
          {lockError !== null && (
            <p className="error-text" role="alert">
              {lockError}
            </p>
          )}
        </>
      ) : (
        <p>{t("security.lock.unavailable")}</p>
      )}
    </section>
  );
}

// ----- devices ------------------------------------------------------------------------

/**
 * The devices of the account: each with its name, when and how it joined and who added it,
 * "Remove..." for the other active ones; "Add a device..." once the account exists.
 */
function DeviceSettings({ security }: { security: SecurityInfo }) {
  const devices = useAppStore((state) => state.devices);
  const [adding, setAdding] = useState(false);
  const [removing, setRemoving] = useState<DeviceEntry | null>(null);
  const titleId = useId();
  const noteId = useId();
  const names = new Map(devices.map((device) => [device.deviceId, device.name]));

  return (
    <section className="subpanel" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("security.devices.title")}
      </h3>
      {devices.length === 0 ? (
        <p className="muted">{t("security.devices.empty")}</p>
      ) : (
        <>
          <ul className="device-list" aria-labelledby={titleId}>
            {devices.map((device) => {
              const added = formatTimestamp(device.addedAt);
              const revoked = device.revokedAt === undefined ? null : formatTimestamp(device.revokedAt);
              const addedBy = device.addedBy === undefined ? undefined : names.get(device.addedBy);
              const removable = !device.isThisDevice && device.revokedAt === undefined;
              return (
                <li key={device.deviceId} className="device-item">
                  <div className="device-head">
                    <span className="device-name">{device.name}</span>
                    {device.isThisDevice && <span className="tag tag-accent">{t("security.devices.thisDevice")}</span>}
                    {revoked !== null && (
                      <span className="tag tag-danger">{t("security.devices.revoked", { date: revoked.text })}</span>
                    )}
                    {removable && (
                      <button
                        type="button"
                        className="link-button link-danger device-remove"
                        aria-label={t("security.devices.removeLabel", { name: device.name })}
                        onClick={() => {
                          setRemoving(device);
                        }}
                      >
                        {t("security.devices.remove")}
                      </button>
                    )}
                  </div>
                  <p className="muted device-meta">
                    {added !== null && t("security.devices.added", { date: added.text })}
                    {device.addedViaKit && <span className="device-via-kit">{t("security.devices.viaKit")}</span>}
                    {addedBy !== undefined && <span>{t("security.devices.addedBy", { name: addedBy })}</span>}
                  </p>
                </li>
              );
            })}
          </ul>
          <p className="muted device-add-note" id={noteId}>
            {t("security.devices.addNote")}
          </p>
          <div className="row-actions">
            <button
              type="button"
              className="button"
              aria-describedby={noteId}
              onClick={() => {
                setAdding(true);
              }}
            >
              {t("security.devices.add")}
            </button>
          </div>
        </>
      )}
      {adding && (
        <AddDeviceDialog
          onClose={() => {
            setAdding(false);
          }}
        />
      )}
      {removing !== null && (
        <RemoveDeviceDialog
          device={removing}
          lockEnabled={security.appLock.enabled}
          onClose={() => {
            setRemoving(null);
          }}
        />
      )}
    </section>
  );
}

// ----- backup -------------------------------------------------------------------------

type ExportState =
  | { kind: "idle" }
  | { kind: "busy" }
  | { kind: "cancelled" }
  | { kind: "error"; message: string }
  | { kind: "done"; result: BackupResult };

function BackupDone({ result }: { result: BackupResult }) {
  const { summary, path } = result;
  return (
    <div className="backup-result">
      <p className="backup-result-title" role="status">
        {t("security.backup.done")}
      </p>
      <dl className="facts">
        <dt>{t("security.backup.path")}</dt>
        <dd className="mono">{path}</dd>
        <dt>{t("security.backup.documents")}</dt>
        <dd>{summary.documents}</dd>
        <dt>{t("security.backup.settings")}</dt>
        <dd>{summary.settings}</dd>
        <dt>{t("security.backup.size")}</dt>
        <dd>{formatBytes(summary.bytes)}</dd>
        <dt>{t("security.backup.created")}</dt>
        <dd>
          <Timestamp value={summary.createdAt} />
        </dd>
      </dl>
      <p className="muted">{t("security.backup.restoreNote")}</p>
    </div>
  );
}

function BackupSettings({ security }: { security: SecurityInfo }) {
  const available = useAppStore((state) => state.capabilities.backups);
  const exportBackup = useAppStore((state) => state.exportBackup);
  const [state, setState] = useState<ExportState>({ kind: "idle" });
  const titleId = useId();
  const noteId = useId();

  let unavailable: string | null = null;
  if (!available) {
    unavailable = t("security.backup.unavailable");
  } else if (security.recoveryKit === undefined) {
    unavailable = t("security.backup.needsKit");
  }

  async function run(): Promise<void> {
    setState({ kind: "busy" });
    try {
      setState({ kind: "done", result: await exportBackup() });
    } catch (failure) {
      const reason = toCoreError(failure);
      if (isFieldError(reason, "path")) {
        setState({ kind: "cancelled" });
      } else if (reason.code === "InvalidInput") {
        // The core refuses to export a workspace without a kit: nothing could restore it.
        setState({ kind: "error", message: t("security.backup.needsKit") });
      } else {
        setState({ kind: "error", message: t("security.backup.error", { message: reason.message }) });
      }
    }
  }

  return (
    <section className="subpanel" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("security.backup.title")}
      </h3>
      <p className="muted">{t("security.backup.intro")}</p>
      {unavailable !== null && (
        <p className="note" id={noteId}>
          {unavailable}
        </p>
      )}
      <div className="row-actions">
        <button
          type="button"
          className="button"
          disabled={unavailable !== null || state.kind === "busy"}
          aria-describedby={unavailable !== null ? noteId : undefined}
          onClick={() => {
            void run();
          }}
        >
          {state.kind === "busy" ? t("security.backup.exporting") : t("security.backup.export")}
        </button>
      </div>
      {state.kind === "cancelled" && (
        <p className="muted" role="status">
          {t("security.backup.cancelled")}
        </p>
      )}
      {state.kind === "error" && (
        <p className="error-text" role="alert">
          {state.message}
        </p>
      )}
      {state.kind === "done" && <BackupDone result={state.result} />}
    </section>
  );
}

// ----- the section --------------------------------------------------------------------

/** Settings → Security. */
export function SecuritySettings() {
  const security = useAppStore((state) => state.security);
  const refreshSecurity = useAppStore((state) => state.refreshSecurity);

  if (security === null) {
    return (
      <div className="subpanel">
        <p className="muted">{t("security.unavailable")}</p>
        <div className="row-actions">
          <button
            type="button"
            className="button"
            onClick={() => {
              refreshSecurity().catch((error: unknown) => {
                console.error("could not read the security status", toCoreError(error).code);
              });
            }}
          >
            {t("security.retry")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <>
      <RecoveryKitSettings security={security} />
      <AppLockSettings security={security} />
      <DeviceSettings security={security} />
      <BackupSettings security={security} />
      <ScheduledBackupSettings hasKit={security.recoveryKit !== undefined} />
    </>
  );
}
