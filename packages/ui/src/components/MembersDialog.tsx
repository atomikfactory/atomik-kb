// "Members and sharing" (the space menu, Milestone 13, ADR-0041 to ADR-0047): the member panel of a shared space or of its archive.
// The member list with roles, verification and safety numbers (`MemberList`), for Owners the invitations (`InvitesSection`) and the
// space's settings (`SpaceSettingsSection`), the incomplete snapshots Owners should know of, every member's own AI switch, and at
// the bottom the ways out: "Leave this space" (not for the last Owner, who closes it or promotes someone first), "Close this space"
// (Owners) and, on an archive, "Remove from this device". Each says what it does before it does it.

import { toCoreError } from "@atomik-kb/core-client";
import type { MemberRole, SpaceId } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";

import { t } from "../i18n";
import { formatDay, roleLabel } from "../sharing";
import { useMembersRead, useSpace, useSpaceAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { ConfirmDialog } from "./ConfirmDialog";
import { InviteCodeDialog } from "./InviteCodeDialog";
import { InviteLinkDialog } from "./InviteLinkDialog";
import { InvitesSection } from "./InvitesSection";
import { MemberList } from "./MemberList";
import { Modal } from "./Modal";
import { SpaceAiSwitch } from "./SpaceAiSwitch";
import { SpaceSettingsSection } from "./SpaceSettingsSection";

export interface MembersDialogProps {
  spaceId: SpaceId;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  /** Opens with the invitation by code or by link already started (the share dialog's "Invite people..."). */
  start?: "code" | "link";
  onClose: () => void;
}

type Question = "leave" | "close" | "remove";

export function MembersDialog({ spaceId, returnFocus, start, onClose }: MembersDialogProps) {
  const space = useSpace(spaceId);
  const access = useSpaceAccess(spaceId);
  const { info, error: readError, retry } = useMembersRead(spaceId);
  const leaveSpace = useAppStore((state) => state.leaveSpace);
  const closeSpace = useAppStore((state) => state.closeSpace);
  const [invite, setInvite] = useState<"code" | "link" | null>(start ?? null);
  const [question, setQuestion] = useState<Question | null>(null);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);

  // A space that is gone (left from here, or removed from this device) takes its panel with it.
  const gone = space === undefined;
  useEffect(() => {
    if (gone) {
      onClose();
    }
  }, [gone, onClose]);
  if (space === undefined) {
    return null;
  }

  const role: MemberRole = access.role ?? "viewer";
  const owners = info?.members.filter((member) => member.role === "owner").length ?? 0;
  const lastOwner = access.isOwner && owners === 1;
  const manage = access.isOwner && info !== null;

  return (
    <Modal labelledBy={titleId} onDismiss={onClose} className="members-dialog" returnFocus={returnRef}>
      <h2 className="subtitle" id={titleId}>
        {t("sharing.members.title")}
      </h2>
      <p className="muted members-subtitle">
        {space.name}
        {" · "}
        {t("sharing.members.yourRole", { role: roleLabel(role) })}
        {info?.closedAt !== undefined && ` · ${t("sharing.members.closedOn", { date: formatDay(info.closedAt) })}`}
      </p>
      {access.archived && (
        <p className="banner banner-warning" role="status">
          {access.archiveReason === "Closed" ? t("sharing.archive.closed") : t("sharing.archive.removed")}
        </p>
      )}
      {info === null ? (
        readError !== null ? (
          <div className="note" role="alert">
            <p>{t("sharing.members.error", { message: readError })}</p>
            <button type="button" className="button" onClick={retry}>
              {t("sharing.members.retry")}
            </button>
          </div>
        ) : (
          <p className="muted" role="status">
            {t("sharing.members.loading")}
          </p>
        )
      ) : (
        <>
          {info.pendingRekey && !access.archived && (
            <p className="note" role="status">
              {t("sharing.members.pendingRekey")}
            </p>
          )}
          <section className="members-section" aria-labelledby={`${titleId}-members`}>
            <h3 className="members-heading" id={`${titleId}-members`}>
              {t("sharing.members.heading")}
            </h3>
            <MemberList spaceId={spaceId} info={info} canManage={manage} />
          </section>
          {manage && (
            <InvitesSection
              spaceId={spaceId}
              info={info}
              onCode={() => {
                setInvite("code");
              }}
              onLink={() => {
                setInvite("link");
              }}
            />
          )}
          {manage && <SpaceSettingsSection spaceId={spaceId} settings={info.settings} />}
          {manage && info.disputes.length > 0 && (
            <section className="members-section" aria-labelledby={`${titleId}-disputes`}>
              <h3 className="members-heading" id={`${titleId}-disputes`}>
                {t("sharing.disputes.title")}
              </h3>
              <ul className="dispute-list">
                {info.disputes.map((dispute, index) => {
                  const who = info.members.find((member) => member.accountId === dispute.accountId)?.name;
                  return (
                    <li key={index}>
                      {t("sharing.notice.disputedNoPage", { name: who ?? t("sharing.notice.aMember") })}{" "}
                      <span className="muted">{formatDay(dispute.at)}</span>
                    </li>
                  );
                })}
              </ul>
            </section>
          )}
        </>
      )}
      <SpaceAiSwitch spaceId={spaceId} />
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      <div className="members-footer">
        {access.archived ? (
          <button
            type="button"
            className="button button-danger"
            onClick={() => {
              setQuestion("remove");
            }}
          >
            {t("sharing.archive.remove")}
          </button>
        ) : (
          <>
            <button
              type="button"
              className="button"
              disabled={lastOwner}
              title={lastOwner ? t("sharing.members.lastOwner") : undefined}
              onClick={() => {
                setQuestion("leave");
              }}
            >
              {t("sharing.leave.button")}
            </button>
            {access.isOwner && (
              <button
                type="button"
                className="button button-danger"
                onClick={() => {
                  setQuestion("close");
                }}
              >
                {t("sharing.close.button")}
              </button>
            )}
          </>
        )}
        <span className="members-footer-spacer" />
        <button type="button" className="button button-primary" onClick={onClose}>
          {t("dialog.close")}
        </button>
      </div>
      {invite === "code" && (
        <InviteCodeDialog
          spaceId={spaceId}
          onClose={() => {
            setInvite(null);
          }}
        />
      )}
      {invite === "link" && (
        <InviteLinkDialog
          spaceId={spaceId}
          onClose={() => {
            setInvite(null);
          }}
        />
      )}
      {question === "leave" && (
        <ConfirmDialog
          title={t("sharing.leave.title", { name: space.name })}
          body={t("sharing.leave.body")}
          confirmLabel={t("sharing.leave.confirm")}
          onConfirm={async () => {
            setError(null);
            await leaveSpace(spaceId).catch((failure: unknown) => {
              setError(t("sharing.leave.error", { message: toCoreError(failure).message }));
            });
          }}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {question === "close" && (
        <ConfirmDialog
          title={t("sharing.close.title", { name: space.name })}
          body={t("sharing.close.body")}
          confirmLabel={t("sharing.close.confirm")}
          onConfirm={async () => {
            setError(null);
            await closeSpace(spaceId).catch((failure: unknown) => {
              setError(t("sharing.close.error", { message: toCoreError(failure).message }));
            });
          }}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
      {question === "remove" && (
        <ConfirmDialog
          title={t("sharing.archive.removeTitle", { name: space.name })}
          body={t("sharing.archive.removeBody")}
          confirmLabel={t("sharing.archive.removeConfirm")}
          onConfirm={async () => {
            setError(null);
            await leaveSpace(spaceId).catch((failure: unknown) => {
              setError(t("sharing.archive.error", { message: toCoreError(failure).message }));
            });
          }}
          onClose={() => {
            setQuestion(null);
          }}
        />
      )}
    </Modal>
  );
}
