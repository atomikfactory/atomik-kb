// "Add this device to my account" on the onboarding screen (ADR-0022, the joiner): the sync
// server's address, then the code to type on a device of the account, then the six digits
// to compare and confirm, then "Setting up..." until the core installed the account; its
// `WorkspaceCreated` starts the app with the workspace.

import { toCoreError } from "@atomik-kb/core-client";
import type { PairingInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { PairingCode, PairingDigits, PairingProgress, useFocusOnMount } from "../components/Pairing";
import { isFieldError } from "../errors";
import { activeLocale, t } from "../i18n";
import { useAppStore } from "../store-context";
import { pairingUnderWay } from "../store-devices";

function describeStartError(error: unknown): string {
  const failure = toCoreError(error);
  if (isFieldError(failure, "serverUrl")) {
    return t("join.error.url", { message: failure.message });
  }
  if (failure.code === "AlreadyExists") {
    return t("join.error.alreadyExists");
  }
  return t("join.error.generic", { message: failure.message });
}

/** The time of day the code stops working. */
function expiryText(expiresAt: number | undefined): string | null {
  if (expiresAt === undefined) {
    return null;
  }
  const date = new Date(Number(expiresAt));
  if (Number.isNaN(date.getTime())) {
    return null;
  }
  return t("join.expires", { time: new Intl.DateTimeFormat(activeLocale(), { timeStyle: "short" }).format(date) });
}

/** The address of the sync server, then "Connect"; a failed session's reason above it. */
function JoinStart({ previous, onBack }: { previous: PairingInfo | null; onBack: () => void }) {
  const startPairing = useAppStore((state) => state.startPairing);
  const [url, setUrl] = useState(previous?.serverUrl ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fieldId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const serverUrl = url.trim();
    if (serverUrl === "") {
      setError(t("join.error.emptyUrl"));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      // The session's steps replace this form once the store has the answer.
      await startPairing("Joiner", { serverUrl });
    } catch (failure) {
      setError(describeStartError(failure));
    }
    setBusy(false);
  }

  return (
    <>
      <p className="muted">{t("join.intro")}</p>
      {previous?.state === "Failed" && (
        <p className="error-text" role="alert">
          {t("join.failed", { message: previous.error ?? "" })}
        </p>
      )}
      {previous?.state === "Cancelled" && (
        <p className="muted" role="status">
          {t("join.cancelled")}
        </p>
      )}
      <form
        className="form"
        noValidate
        onSubmit={(event) => {
          void submit(event);
        }}
      >
        <label htmlFor={fieldId}>{t("join.urlLabel")}</label>
        <input
          id={fieldId}
          className="input"
          type="url"
          inputMode="url"
          value={url}
          placeholder={t("join.urlPlaceholder")}
          autoComplete="url"
          spellCheck={false}
          autoFocus
          readOnly={busy}
          aria-invalid={error !== null}
          aria-describedby={error !== null ? errorId : undefined}
          onChange={(event) => {
            setUrl(event.target.value);
            setError(null);
          }}
        />
        {error !== null && (
          <p className="error-text" id={errorId} role="alert">
            {error}
          </p>
        )}
        <div className="row-actions">
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? t("join.connecting") : t("join.connect")}
          </button>
          <button type="button" className="button" disabled={busy} onClick={onBack}>
            {t("onboarding.back")}
          </button>
        </div>
      </form>
    </>
  );
}

/** The steps of a session under way; "Cancel" ends it (the form comes back). */
function JoinSession({ pairing }: { pairing: PairingInfo }) {
  const confirmPairing = useAppStore((state) => state.confirmPairing);
  const cancelPairing = useAppStore((state) => state.cancelPairing);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { state } = pairing;
  const settingUp = state === "Transferring" || state === "Done";
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

  return (
    <div className="pair-session" ref={container} tabIndex={-1}>
      {state === "Starting" && <PairingProgress text={t("join.starting")} />}
      {state === "Waiting" && pairing.code !== undefined && (
        <>
          <p>{t("join.codeIntro")}</p>
          <PairingCode code={pairing.code} />
          {expiryText(pairing.expiresAt) !== null && <p className="muted">{expiryText(pairing.expiresAt)}</p>}
          <PairingProgress text={t("join.waiting")} />
        </>
      )}
      {(state === "Confirming" || state === "PeerConfirming") && pairing.sas !== undefined && (
        <>
          <p>{t("join.digitsIntro")}</p>
          <PairingDigits sas={pairing.sas} warning={state === "Confirming"} />
          {state === "PeerConfirming" && (
            <PairingProgress
              text={
                pairing.peerName === undefined
                  ? t("join.peerConfirmingUnnamed")
                  : t("join.peerConfirming", { name: pairing.peerName })
              }
            />
          )}
        </>
      )}
      {settingUp && <PairingProgress text={t("join.settingUp")} />}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {!settingUp && (
        <div className="row-actions">
          {state === "Confirming" && (
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
        </div>
      )}
    </div>
  );
}

/** The joiner flow, in the onboarding card. `onBack` returns to creating a workspace. */
export function JoinAccount({ onBack }: { onBack: () => void }) {
  const pairing = useAppStore((state) => state.pairing);
  const titleId = useId();
  const session = pairingUnderWay(pairing, "Joiner");

  return (
    <section className="card card-wide join-card" aria-labelledby={titleId}>
      <h1 className="title" id={titleId}>
        {t("join.title")}
      </h1>
      {session !== null ? (
        <JoinSession pairing={session} />
      ) : (
        <JoinStart previous={pairing?.role === "Joiner" ? pairing : null} onBack={onBack} />
      )}
    </section>
  );
}
