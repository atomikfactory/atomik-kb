// "Join a shared space" (the space list, Milestone 13, ADR-0046): the invitee's side of the code flow, modelled on adding this device
// to an account (`JoinAccount`). The person types the server address and the code the inviter gave, both compare six digits (the
// invitee confirms first), then "You are joining <space> as <role>" while the space and its history arrive. A person who has no
// recovery kit yet is told to create one: joining needs an account.

import { toCoreError } from "@atomik-kb/core-client";
import type { InviteSessionInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { PAIRING_CODE_CHARS, normalizePairingCode } from "../pairing";
import { inviteErrorText, inviteUnderWay, roleLabel } from "../sharing";
import { useOpenJoinedSpace } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { selectVisibleInvite } from "../store-sharing";
import { Modal } from "./Modal";
import { PairingDigits, PairingProgress, useFocusOnMount } from "./Pairing";

function describeStartError(error: unknown): string {
  const failure = toCoreError(error);
  if (isFieldError(failure, "serverUrl")) {
    return t("join.error.url", { message: failure.message });
  }
  if (isFieldError(failure, "code")) {
    return t("sharing.join.error.code");
  }
  if (isFieldError(failure, "account")) {
    return t("sharing.join.error.account");
  }
  return t("sharing.join.error.generic", { message: failure.message });
}

function Session({ session }: { session: InviteSessionInfo }) {
  const confirmInvite = useAppStore((state) => state.confirmInvite);
  const cancelInvite = useAppStore((state) => state.cancelInvite);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const container = useFocusOnMount<HTMLDivElement>();
  const { state, spaceName, role } = session;
  const receiving = state === "Transferring" || state === "Joining";

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
      {state === "Starting" && <PairingProgress text={t("sharing.join.starting")} />}
      {(state === "Confirming" || state === "PeerConfirming") && session.sas !== undefined && (
        <>
          <p>{t("sharing.join.digitsIntro")}</p>
          <PairingDigits sas={session.sas} warning={state === "Confirming"} warningText={t("sharing.pair.warning")} />
          {state === "PeerConfirming" && <PairingProgress text={t("sharing.join.peerConfirming")} />}
        </>
      )}
      {receiving && (
        <>
          {spaceName !== undefined && role !== undefined && (
            <p className="pair-peer">{t("sharing.join.joiningSpace", { space: spaceName, role: roleLabel(role) })}</p>
          )}
          <PairingProgress text={t("sharing.join.receiving")} />
        </>
      )}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {!receiving && (
        <div className="dialog-actions">
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              void act(cancelInvite, "pair.error.cancel");
            }}
          >
            {t("pair.cancel")}
          </button>
          {state === "Confirming" && (
            <button
              type="button"
              className="button button-primary"
              disabled={busy}
              onClick={() => {
                void act(confirmInvite, "pair.error.confirm");
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
 * The invitee's side of the code flow in a modal. Closing it before the end cancels the session; it cannot be closed while the
 * space is arriving.
 */
export function JoinSpaceDialog({ onClose }: { onClose: () => void }) {
  const invite = useAppStore(selectVisibleInvite);
  const joinWithCode = useAppStore((state) => state.joinWithCode);
  const cancelInvite = useAppStore((state) => state.cancelInvite);
  const dismissInvite = useAppStore((state) => state.dismissInvite);
  const [url, setUrl] = useState("");
  const [code, setCode] = useState("");
  // A session already under way when the dialog mounts (the page was reloaded) is picked up, not asked for again.
  const [startedHere, setStartedHere] = useState(() => invite?.flow === "Code" && invite.side === "Invitee");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const urlId = useId();
  const codeId = useId();
  const errorId = useId();

  const mine = invite?.flow === "Code" && invite.side === "Invitee" ? invite : null;
  const session = inviteUnderWay(mine, "Code", "Invitee");
  const shown = startedHere ? mine : null;
  const done = shown?.state === "Done" ? shown : null;
  const failure =
    shown?.state === "Failed"
      ? t("sharing.join.failed", { message: inviteErrorText(shown.error) })
      : shown?.state === "Cancelled"
        ? t("sharing.join.cancelled")
        : null;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const serverUrl = url.trim();
    if (serverUrl === "") {
      setError(t("sharing.join.error.emptyServer"));
      return;
    }
    const normalized = normalizePairingCode(code);
    if (normalized === "") {
      setError(t("sharing.join.error.emptyCode"));
      return;
    }
    if (normalized.length !== PAIRING_CODE_CHARS) {
      setError(t("sharing.join.error.code"));
      return;
    }
    setError(null);
    setBusy(true);
    setStartedHere(true);
    try {
      await joinWithCode(serverUrl, normalized);
    } catch (reason) {
      setStartedHere(false);
      setError(describeStartError(reason));
    }
    setBusy(false);
  }

  function close(): void {
    if (session !== null) {
      cancelInvite().catch((reason: unknown) => {
        console.error("could not cancel joining", toCoreError(reason).code);
      });
    }
    dismissInvite();
    onClose();
  }

  /** The space the person just joined is the newest one of that name that is shared; it may not have arrived yet. */
  const { open, opening, late } = useOpenJoinedSpace(done?.spaceName, close);

  return (
    <Modal
      labelledBy={titleId}
      onDismiss={session?.state === "Transferring" || session?.state === "Joining" ? undefined : close}
      className="invite-dialog"
    >
      <h2 className="subtitle" id={titleId}>
        {t("sharing.join.title")}
      </h2>
      {session !== null && startedHere ? (
        <Session session={session} />
      ) : done !== null ? (
        <>
          <p role="status">{t("sharing.join.done", { space: done.spaceName ?? "" })}</p>
          {late && (
            <p className="note" role="status">
              {t("sharing.join.openLate")}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" onClick={close}>
              {t("dialog.close")}
            </button>
            <button type="button" className="button button-primary" disabled={opening} onClick={open}>
              {opening ? t("sharing.join.opening") : t("sharing.join.open")}
            </button>
          </div>
        </>
      ) : (
        <form
          className="form invite-form"
          noValidate
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <p className="muted">{t("sharing.join.intro")}</p>
          {failure !== null && (
            <p className="error-text" role="alert">
              {failure}
            </p>
          )}
          <label htmlFor={urlId}>{t("sharing.join.serverLabel")}</label>
          <input
            id={urlId}
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
          <label htmlFor={codeId}>{t("sharing.join.codeLabel")}</label>
          <input
            id={codeId}
            className="input pair-code-input"
            type="text"
            value={code}
            placeholder={t("addDevice.codePlaceholder")}
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            readOnly={busy}
            aria-invalid={error !== null}
            aria-describedby={error !== null ? errorId : undefined}
            onChange={(event) => {
              setCode(event.target.value);
              setError(null);
            }}
          />
          {error !== null && (
            <p className="error-text" id={errorId} role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={busy} onClick={close}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-primary" disabled={busy}>
              {busy ? t("sharing.join.submitting") : t("sharing.join.submit")}
            </button>
          </div>
        </form>
      )}
    </Modal>
  );
}
