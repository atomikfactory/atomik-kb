// "Add a device..." in Settings -> Security -> Devices (ADR-0022, the sponsor): type the code
// the new device shows, compare the six digits, see "You are adding <name>" once the new
// device confirmed, confirm here, and the core adds it to the account and sends it the keys.

import { toCoreError } from "@atomik-kb/core-client";
import type { PairingInfo, PairingState } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { PAIRING_CODE_CHARS, normalizePairingCode } from "../pairing";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { PairingDigits, PairingProgress, useFocusOnMount } from "./Pairing";

/** Steps of a session this dialog picks up again when it opens (after a restart, say). */
const RESUMABLE: readonly PairingState[] = ["Starting", "Waiting", "Confirming", "PeerConfirming", "Transferring"];

function describeStartError(error: unknown): string {
  const failure = toCoreError(error);
  if (isFieldError(failure, "code")) {
    return t("addDevice.error.code");
  }
  if (isFieldError(failure, "serverUrl")) {
    return t("addDevice.error.noServer");
  }
  if (failure.code === "NotFound") {
    return t("addDevice.error.notFound");
  }
  return t("addDevice.error.generic", { message: failure.message });
}

function CodeForm({
  failure,
  onStarted,
  onClose,
}: {
  failure: string | null;
  onStarted: () => void;
  onClose: () => void;
}) {
  const startPairing = useAppStore((state) => state.startPairing);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fieldId = useId();
  const introId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const code = normalizePairingCode(text);
    if (code === "") {
      setError(t("addDevice.error.empty"));
      return;
    }
    if (code.length !== PAIRING_CODE_CHARS) {
      setError(t("addDevice.error.length", { count: code.length }));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      onStarted();
      await startPairing("Sponsor", { code });
    } catch (reason) {
      setError(describeStartError(reason));
    }
    setBusy(false);
  }

  return (
    <form
      className="form add-device-form"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      <p className="muted" id={introId}>
        {t("addDevice.intro")}
      </p>
      {failure !== null && (
        <p className="error-text" role="alert">
          {failure}
        </p>
      )}
      <label htmlFor={fieldId}>{t("addDevice.codeLabel")}</label>
      <input
        id={fieldId}
        className="input pair-code-input"
        type="text"
        value={text}
        placeholder={t("addDevice.codePlaceholder")}
        autoComplete="off"
        autoCapitalize="characters"
        spellCheck={false}
        autoFocus
        readOnly={busy}
        aria-invalid={error !== null}
        aria-describedby={[introId, error !== null ? errorId : undefined].filter((id) => id !== undefined).join(" ")}
        onChange={(event) => {
          setText(event.target.value);
          setError(null);
        }}
      />
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div className="dialog-actions">
        <button type="button" className="button" disabled={busy} onClick={onClose}>
          {t("addDevice.close")}
        </button>
        <button type="submit" className="button button-primary" disabled={busy}>
          {busy ? t("addDevice.submitting") : t("addDevice.submit")}
        </button>
      </div>
    </form>
  );
}

function SponsorSession({ pairing }: { pairing: PairingInfo }) {
  const confirmPairing = useAppStore((state) => state.confirmPairing);
  const cancelPairing = useAppStore((state) => state.cancelPairing);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { state, peerName, sas } = pairing;
  const name = peerName ?? t("addDevice.unnamed");
  const container = useFocusOnMount<HTMLDivElement>();

  async function act(action: () => Promise<unknown>, errorKey: "pair.error.confirm" | "pair.error.cancel"): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (failure) {
      setError(t(errorKey, { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  const canConfirm = state === "Confirming" && peerName !== undefined && sas !== undefined;
  return (
    <div className="pair-session" ref={container} tabIndex={-1}>
      {(state === "Starting" || state === "Waiting") && <PairingProgress text={t("addDevice.starting")} />}
      {(state === "Confirming" || state === "PeerConfirming") && sas !== undefined && (
        <>
          {canConfirm && <p className="pair-peer">{t("addDevice.adding", { name })}</p>}
          <p>{canConfirm ? t("addDevice.digitsCheck") : t("addDevice.digitsIntro")}</p>
          <PairingDigits sas={sas} warning={canConfirm} />
          {!canConfirm && <PairingProgress text={t("addDevice.waitingPeer")} />}
        </>
      )}
      {state === "Transferring" && <PairingProgress text={t("addDevice.transferring", { name })} />}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {state !== "Transferring" && (
        <div className="dialog-actions">
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              void act(cancelPairing, "pair.error.cancel");
            }}
          >
            {t("pair.cancel")}
          </button>
          {canConfirm && (
            <button
              type="button"
              className="button button-primary"
              disabled={busy}
              onClick={() => {
                void act(confirmPairing, "pair.error.confirm");
              }}
            >
              {busy ? t("pair.confirming") : t("pair.confirm")}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The sponsor flow in a modal. A session already under way when it opens is picked up;
 * one that ended before (done, failed) is not shown again. Closing it mid-way cancels the
 * session; it cannot be closed while the keys are being sent.
 */
export function AddDeviceDialog({ onClose }: { onClose: () => void }) {
  const pairing = useAppStore((state) => state.pairing);
  const refreshPairing = useAppStore((state) => state.refreshPairing);
  const cancelPairing = useAppStore((state) => state.cancelPairing);
  // A session ending (done, failed) is shown only when it started in this dialog.
  const [startedHere, setStartedHere] = useState(false);
  const titleId = useId();

  useEffect(() => {
    refreshPairing().catch((error: unknown) => {
      console.error("could not read the pairing status", toCoreError(error).code);
    });
  }, [refreshPairing]);

  const session = pairing?.role === "Sponsor" ? pairing : null;
  const resumable = session !== null && RESUMABLE.includes(session.state);
  const done = startedHere && session?.state === "Done";
  const failure =
    startedHere && session?.state === "Failed" ? t("addDevice.failed", { message: session.error ?? "" }) : null;

  function close(): void {
    if (resumable) {
      cancelPairing().catch((error: unknown) => {
        console.error("could not cancel the pairing", toCoreError(error).code);
      });
    }
    onClose();
  }

  return (
    <Modal
      labelledBy={titleId}
      onDismiss={session?.state === "Transferring" ? undefined : close}
      className="add-device-dialog"
    >
      <h2 className="subtitle" id={titleId}>
        {t("addDevice.title")}
      </h2>
      {resumable && session !== null ? (
        <SponsorSession pairing={session} />
      ) : done ? (
        <>
          <p role="status">{t("addDevice.done", { name: session?.peerName ?? t("addDevice.unnamed") })}</p>
          <div className="dialog-actions">
            <button type="button" className="button button-primary" onClick={onClose}>
              {t("kit.done")}
            </button>
          </div>
        </>
      ) : (
        <CodeForm
          failure={failure}
          onStarted={() => {
            setStartedHere(true);
          }}
          onClose={onClose}
        />
      )}
    </Modal>
  );
}
