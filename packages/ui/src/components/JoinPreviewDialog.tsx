// The preview of an invitation link (Milestone 13, ADR-0046): "Alice invites you to Project X as Editor", that you will receive the
// whole history of the space, and that the invitation is not verified (anyone who held the link could have opened it; compare
// safety numbers with the person after joining). "Join the space" accepts, "Decline" cancels. The core opens the invitation
// first (a step: "Opening the invitation..."), and a dead one, which the core cannot tell from an unknown one, says only that
// it is invalid, expired or already used. The dialog is shown by `InviteSessionHost`, whichever way the link came in: a link
// of the operating system (`store-deep-links.ts`) or a pasted one.

import { toCoreError } from "@atomik-kb/core-client";
import type { InviteSessionInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";

import { t } from "../i18n";
import { inviteErrorText, roleLabel } from "../sharing";
import { useOpenJoinedSpace } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { PairingProgress } from "./Pairing";

export function JoinPreviewDialog({ session }: { session: InviteSessionInfo }) {
  const confirmInvite = useAppStore((state) => state.confirmInvite);
  const cancelInvite = useAppStore((state) => state.cancelInvite);
  const dismissInvite = useAppStore((state) => state.dismissInvite);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const bodyId = useId();
  const { state, spaceName, inviterName, role } = session;
  const joining = state === "Joining";
  const done = state === "Done";
  const failed = state === "Failed";
  const active = state === "Starting" || state === "Previewing" || joining;

  async function accept(): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      await confirmInvite();
    } catch (failure) {
      setError(t("sharing.preview.error", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  function close(): void {
    if (active) {
      cancelInvite().catch((failure: unknown) => {
        console.error("could not decline the invitation", toCoreError(failure).code);
      });
    }
    dismissInvite();
  }

  const { open, opening, late } = useOpenJoinedSpace(spaceName, close);

  return (
    <Modal labelledBy={titleId} describedBy={bodyId} onDismiss={joining ? undefined : close} className="invite-dialog">
      <h2 className="subtitle" id={titleId}>
        {t("sharing.preview.title")}
      </h2>
      <div id={bodyId} className="invite-preview">
        {state === "Starting" && <PairingProgress text={t("sharing.preview.opening")} />}
        {failed && (
          <p className="error-text" role="alert">
            {t("sharing.preview.failed", { message: inviteErrorText(session.error) })}
          </p>
        )}
        {(state === "Previewing" || joining) && spaceName !== undefined && inviterName !== undefined && role !== undefined && (
          <>
            <p className="pair-peer">
              {t("sharing.preview.invites", { inviter: inviterName, space: spaceName, role: roleLabel(role) })}
            </p>
            <p>{t("sharing.preview.history")}</p>
            <p className="note" role="note">
              {t("sharing.preview.unverified")}
            </p>
          </>
        )}
        {joining && <PairingProgress text={t("sharing.join.receiving")} />}
        {done && (
          <p role="status">{t("sharing.join.done", { space: spaceName ?? "" })}</p>
        )}
        {done && late && (
          <p className="note" role="status">
            {t("sharing.join.openLate")}
          </p>
        )}
        {error !== null && (
          <p className="error-text" role="alert">
            {error}
          </p>
        )}
      </div>
      <div className="dialog-actions">
        {done ? (
          <>
            <button type="button" className="button" onClick={close}>
              {t("dialog.close")}
            </button>
            <button type="button" className="button button-primary" disabled={opening} onClick={open}>
              {opening ? t("sharing.join.opening") : t("sharing.join.open")}
            </button>
          </>
        ) : failed ? (
          <button type="button" className="button button-primary" onClick={close}>
            {t("dialog.close")}
          </button>
        ) : (
          <>
            <button type="button" className="button" disabled={joining || busy} onClick={close}>
              {t("sharing.preview.decline")}
            </button>
            <button
              type="button"
              className="button button-primary"
              disabled={state !== "Previewing" || busy}
              onClick={() => {
                void accept();
              }}
            >
              {joining || busy ? t("sharing.preview.accepting") : t("sharing.preview.accept")}
            </button>
          </>
        )}
      </div>
    </Modal>
  );
}
