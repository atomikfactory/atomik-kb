// "Invite with a code" (the member panel, Milestone 13, ADR-0046): the inviter's side of the code flow, modelled on adding a
// device (`AddDeviceDialog`). The Owner picks the role and the dialog shows the code and the server address to give to the person;
// when they typed them both sides show six digits to compare (the invitee confirms first); then "You are inviting <name>" with
// the person's safety number, and the Owner confirms second. Both lists then show "verified". Closing the dialog before the end
// cancels the session.

import { toCoreError } from "@atomik-kb/core-client";
import type { InviteSessionInfo, MemberRole, SpaceId } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { t } from "../i18n";
import { INVITE_ROLES, inviteErrorText, inviteUnderWay, roleLabel, safetyGroups } from "../sharing";
import { useAppStore } from "../store-context";
import { selectVisibleInvite } from "../store-sharing";
import { Modal } from "./Modal";
import { PairingCode, PairingDigits, PairingProgress, useFocusOnMount } from "./Pairing";

export interface InviteCodeDialogProps {
  spaceId: SpaceId;
  /** The role the form starts on. */
  role?: MemberRole;
  onClose: () => void;
}

function Session({ session }: { session: InviteSessionInfo }) {
  const confirmInvite = useAppStore((state) => state.confirmInvite);
  const cancelInvite = useAppStore((state) => state.cancelInvite);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const container = useFocusOnMount<HTMLDivElement>();
  const { state, peerName, peerSafetyNumber, sas, code, serverUrl } = session;
  const name = peerName ?? t("sharing.inviteCode.unnamed");
  const canConfirm = state === "PeerConfirming" && peerName !== undefined;

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
      {state === "Starting" && <PairingProgress text={t("sharing.inviteCode.starting")} />}
      {state === "Waiting" && code !== undefined && (
        <>
          <p>{t("sharing.inviteCode.intro")}</p>
          <PairingCode code={code} label={t("sharing.codeLabel")} />
          {serverUrl !== undefined && (
            <p className="invite-server">
              <span className="muted">{t("sharing.inviteCode.server")} </span>
              <span className="invite-server-url">{serverUrl}</span>
            </p>
          )}
          <PairingProgress text={t("sharing.inviteCode.waiting")} />
        </>
      )}
      {(state === "Confirming" || state === "PeerConfirming") && sas !== undefined && (
        <>
          {canConfirm && <p className="pair-peer">{t("sharing.inviteCode.inviting", { name })}</p>}
          <p>{canConfirm ? t("sharing.inviteCode.check") : t("sharing.inviteCode.digitsIntro")}</p>
          <PairingDigits sas={sas} warning={canConfirm} warningText={t("sharing.pair.warning")} />
          {canConfirm && peerSafetyNumber !== undefined && (
            <div className="invite-safety">
              <p className="muted">{t("sharing.inviteCode.safety")}</p>
              <p className="member-safety-number" role="group" aria-label={t("sharing.members.safety")}>
                {safetyGroups(peerSafetyNumber).map((group, index) => (
                  <span key={index}>{group}</span>
                ))}
              </p>
            </div>
          )}
          {!canConfirm && <PairingProgress text={t("sharing.inviteCode.waitingPeer")} />}
        </>
      )}
      {state === "Transferring" && <PairingProgress text={t("sharing.inviteCode.transferring", { name })} />}
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
              void act(cancelInvite, "pair.error.cancel");
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
 * The inviter's side of the code flow in a modal. The session of this dialog is the store's invite session while it is a code
 * flow of the inviter's side; one that ended (done, failed, cancelled) is shown when it started in this dialog.
 */
export function InviteCodeDialog({ spaceId, role = "editor", onClose }: InviteCodeDialogProps) {
  const invite = useAppStore(selectVisibleInvite);
  const startCodeInvite = useAppStore((state) => state.startCodeInvite);
  const cancelInvite = useAppStore((state) => state.cancelInvite);
  const dismissInvite = useAppStore((state) => state.dismissInvite);
  const [chosen, setChosen] = useState<MemberRole>(role);
  const [startedHere, setStartedHere] = useState(false);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const titleId = useId();
  const roleId = useId();

  const mine = invite?.flow === "Code" && invite.side === "Inviter" ? invite : null;
  const session = inviteUnderWay(mine, "Code", "Inviter");
  const shown = startedHere ? mine : null;
  const done = shown?.state === "Done";
  const failure =
    shown?.state === "Failed"
      ? t("sharing.inviteCode.failed", { message: inviteErrorText(shown.error) })
      : shown?.state === "Cancelled"
        ? t("sharing.inviteCode.cancelled")
        : null;

  async function start(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (starting) {
      return;
    }
    setStartError(null);
    setStarting(true);
    setStartedHere(true);
    try {
      await startCodeInvite(spaceId, chosen);
    } catch (reason) {
      setStartedHere(false);
      setStartError(t("sharing.inviteCode.error.start", { message: toCoreError(reason).message }));
    }
    setStarting(false);
  }

  function close(): void {
    if (session !== null) {
      cancelInvite().catch((error: unknown) => {
        console.error("could not cancel the invitation", toCoreError(error).code);
      });
    }
    dismissInvite();
    onClose();
  }

  return (
    <Modal
      labelledBy={titleId}
      onDismiss={session?.state === "Transferring" ? undefined : close}
      className="invite-dialog"
    >
      <h2 className="subtitle" id={titleId}>
        {t("sharing.inviteCode.title")}
      </h2>
      {session !== null && startedHere ? (
        <Session session={session} />
      ) : done ? (
        <>
          <p role="status">
            {t("sharing.inviteCode.done", {
              name: shown.peerName ?? t("sharing.inviteCode.unnamed"),
              role: roleLabel(shown.role ?? chosen),
            })}
          </p>
          <div className="dialog-actions">
            <button type="button" className="button button-primary" onClick={close}>
              {t("dialog.close")}
            </button>
          </div>
        </>
      ) : (
        <form
          className="form invite-form"
          noValidate
          onSubmit={(event) => {
            void start(event);
          }}
        >
          <p className="muted">{t("sharing.inviteCode.formIntro")}</p>
          {failure !== null && (
            <p className="error-text" role="alert">
              {failure}
            </p>
          )}
          <div className="field">
            <label htmlFor={roleId}>{t("sharing.invites.roleLabel")}</label>
            <select
              id={roleId}
              className="select"
              value={chosen}
              disabled={starting}
              onChange={(event) => {
                setChosen(event.target.value as MemberRole);
              }}
            >
              {INVITE_ROLES.map((option) => (
                <option key={option} value={option}>
                  {roleLabel(option)}
                </option>
              ))}
            </select>
          </div>
          {startError !== null && (
            <p className="error-text" role="alert">
              {startError}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={starting} onClick={close}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-primary" disabled={starting}>
              {starting ? t("sharing.inviteCode.starting") : t("sharing.inviteCode.start")}
            </button>
          </div>
        </form>
      )}
    </Modal>
  );
}
