// "Invite with a link" (the member panel, Milestone 13, ADR-0046): the Owner picks the role (never Owner) and how long the link
// lives, and the dialog shows the link once, with a Copy button: the core keeps no copy of its secret. The first person who opens
// it joins with that role, once; they are not verified until the safety numbers are compared, which the dialog says.

import { toCoreError } from "@atomik-kb/core-client";
import type { InviteLinkInfo, MemberRole, SpaceId } from "@atomik-kb/core-client";
import { useId, useRef, useState } from "react";
import type { FormEvent } from "react";

import { writeClipboard } from "../clipboard";
import { t, tp } from "../i18n";
import { INVITE_ROLES, LINK_DAYS, LINK_DEFAULT_DAYS, formatDay, roleLabel } from "../sharing";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";

export interface InviteLinkDialogProps {
  spaceId: SpaceId;
  /** The role the form starts on. */
  role?: MemberRole;
  onClose: () => void;
}

/** A line with a read-only box that selects itself and a Copy button (the view link dialog uses it too). */
export function CopyLine({ label, value, hint }: { label: string; value: string; hint?: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const box = useRef<HTMLInputElement>(null);
  const boxId = useId();
  const hintId = useId();

  async function copy(): Promise<void> {
    try {
      await writeClipboard(value);
      setState("copied");
    } catch {
      setState("failed");
      box.current?.select();
    }
  }

  return (
    <div className="field invite-link-line">
      <label htmlFor={boxId}>{label}</label>
      <div className="row">
        <input
          ref={box}
          id={boxId}
          className="input invite-link-box"
          type="text"
          readOnly
          value={value}
          aria-describedby={hint === undefined ? undefined : hintId}
          onFocus={(event) => {
            event.currentTarget.select();
          }}
        />
        <button
          type="button"
          className="button"
          onClick={() => {
            void copy();
          }}
        >
          {state === "copied" ? t("sharing.inviteLink.copied") : t("sharing.inviteLink.copy")}
        </button>
      </div>
      {hint !== undefined && (
        <p className="muted" id={hintId}>
          {hint}
        </p>
      )}
      {state === "failed" && (
        <p className="error-text" role="alert">
          {t("sharing.inviteLink.copyError")}
        </p>
      )}
    </div>
  );
}

export function InviteLinkDialog({ spaceId, role = "viewer", onClose }: InviteLinkDialogProps) {
  const createInviteLink = useAppStore((state) => state.createInviteLink);
  const [chosen, setChosen] = useState<MemberRole>(role);
  const [days, setDays] = useState(LINK_DEFAULT_DAYS);
  const [link, setLink] = useState<InviteLinkInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const introId = useId();
  const roleId = useId();
  const daysId = useId();

  async function create(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    setError(null);
    setBusy(true);
    try {
      setLink(await createInviteLink(spaceId, chosen, days));
    } catch (failure) {
      setError(t("sharing.inviteLink.error", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  return (
    <Modal labelledBy={titleId} describedBy={introId} onDismiss={busy ? undefined : onClose} className="invite-dialog">
      <h2 className="subtitle" id={titleId}>
        {t("sharing.inviteLink.title")}
      </h2>
      {link === null ? (
        <form
          className="form invite-form"
          noValidate
          onSubmit={(event) => {
            void create(event);
          }}
        >
          <p id={introId}>{t("sharing.inviteLink.intro")}</p>
          <div className="field">
            <label htmlFor={roleId}>{t("sharing.invites.roleLabel")}</label>
            <select
              id={roleId}
              className="select"
              value={chosen}
              disabled={busy}
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
          <div className="field">
            <label htmlFor={daysId}>{t("sharing.inviteLink.expiry")}</label>
            <select
              id={daysId}
              className="select"
              value={days}
              disabled={busy}
              onChange={(event) => {
                setDays(Number(event.target.value));
              }}
            >
              {LINK_DAYS.map((option) => (
                <option key={option} value={option}>
                  {tp("sharing.days", option)}
                </option>
              ))}
            </select>
          </div>
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={busy} onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-primary" disabled={busy}>
              {busy ? t("sharing.inviteLink.creating") : t("sharing.inviteLink.create")}
            </button>
          </div>
        </form>
      ) : (
        <div className="invite-result">
          <p id={introId} className="note invite-once" role="status">
            {t("sharing.inviteLink.once")}
          </p>
          <CopyLine label={t("sharing.inviteLink.linkLabel")} value={link.link} />
          <CopyLine label={t("sharing.inviteLink.webLabel")} value={link.webLink} hint={t("sharing.inviteLink.webHint")} />
          <p className="muted">
            {t("sharing.inviteLink.summary", { role: roleLabel(link.role), date: formatDay(link.expiresAt) })}
          </p>
          <div className="dialog-actions">
            <button type="button" className="button button-primary" onClick={onClose}>
              {t("dialog.close")}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
