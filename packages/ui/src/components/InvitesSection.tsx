// The invitations of a space in the member panel (Milestone 13, ADR-0046; Owners only): the open and the used, expired and
// revoked ones with their role, status and when they end, "Revoke" for an open one, and the two ways to invite, with a code
// (the other person types it, both compare six digits) or with a link (shown once).

import { toCoreError } from "@atomik-kb/core-client";
import type { InviteInfo, MembersInfo, SpaceId } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import { formatDay, inviteStatusLabel, roleLabel } from "../sharing";
import { useAppStore } from "../store-context";

export interface InvitesSectionProps {
  spaceId: SpaceId;
  info: MembersInfo;
  onCode: () => void;
  onLink: () => void;
}

function InviteItem({
  invite,
  info,
  onRevoke,
  busy,
}: {
  invite: InviteInfo;
  info: MembersInfo;
  onRevoke: () => void;
  busy: boolean;
}) {
  const usedBy = invite.usedBy === undefined ? undefined : info.members.find((member) => member.accountId === invite.usedBy);
  const open = invite.status === "Open";
  let detail = "";
  if (invite.status === "Used") {
    detail = t("sharing.invites.usedBy", { name: usedBy?.name ?? t("sharing.notice.aMember") });
  } else if (open || invite.status === "Expired") {
    detail = t("sharing.invites.expires", { date: formatDay(invite.expiresAt) });
  }
  return (
    <li className="invite-item">
      <span className="tag">{invite.kind === "Code" ? t("sharing.invites.kind.code") : t("sharing.invites.kind.link")}</span>
      <span className="invite-role">{roleLabel(invite.role)}</span>
      <span className={open ? "tag tag-accent" : "tag"}>{inviteStatusLabel(invite.status)}</span>
      {detail !== "" && <span className="muted invite-detail">{detail}</span>}
      {open && (
        <button type="button" className="link-button link-danger invite-revoke" disabled={busy} onClick={onRevoke}>
          {t("sharing.invites.revoke")}
        </button>
      )}
    </li>
  );
}

export function InvitesSection({ spaceId, info, onCode, onLink }: InvitesSectionProps) {
  const revokeInvite = useAppStore((state) => state.revokeInvite);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The newest first: what was just made is on top.
  const invites = [...info.invites].sort((a, b) => b.createdAt - a.createdAt);

  async function revoke(invite: InviteInfo): Promise<void> {
    setError(null);
    setBusy(true);
    try {
      await revokeInvite(spaceId, invite.inviteId);
    } catch (failure) {
      setError(t("sharing.invites.revokeError", { message: toCoreError(failure).message }));
    }
    setBusy(false);
  }

  return (
    <section className="members-section" aria-labelledby={`invites-${spaceId}`}>
      <h3 className="members-heading" id={`invites-${spaceId}`}>
        {t("sharing.invites.title")}
      </h3>
      <div className="row-actions invite-actions">
        <button type="button" className="button" onClick={onCode}>
          {t("sharing.invites.code")}
        </button>
        <button type="button" className="button" onClick={onLink}>
          {t("sharing.invites.link")}
        </button>
      </div>
      {invites.length === 0 ? (
        <p className="muted">{t("sharing.invites.none")}</p>
      ) : (
        <ul className="invite-list" aria-label={t("sharing.invites.title")}>
          {invites.map((invite) => (
            <InviteItem
              key={invite.inviteId}
              invite={invite}
              info={info}
              busy={busy}
              onRevoke={() => {
                void revoke(invite);
              }}
            />
          ))}
        </ul>
      )}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
