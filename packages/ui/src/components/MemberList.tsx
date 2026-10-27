// The member list of a shared space (Milestone 13, ADR-0041 and ADR-0042): name, a "you" mark, verified or not, the role (a
// select for Owners), device count, when the person joined, a notice when their account added a device with the recovery kit,
// how many of their changes this device refused, a mark for a name two members share, and the person's safety number with
// "Mark verified". Owners also remove members, after a question that says what a removed member keeps.

import { toCoreError } from "@atomik-kb/core-client";
import type { MemberInfo, MembersInfo, MemberRole, SpaceId } from "@atomik-kb/core-client";
import { useId, useState } from "react";

import { t, tp } from "../i18n";
import { ROLES, formatDay, roleHint, roleLabel, safetyGroups } from "../sharing";
import { useAppStore } from "../store-context";
import { ConfirmDialog } from "./ConfirmDialog";

export interface MemberListProps {
  spaceId: SpaceId;
  info: MembersInfo;
  /** An Owner of a space that is not an archive: roles and removals are offered. */
  canManage: boolean;
}

/** How many of a member's changes were refused, as a sentence. */
export function refusedText(member: MemberInfo): string {
  return tp("sharing.notice.refused", member.refusedChanges, { name: member.name });
}

function MemberItem({
  spaceId,
  member,
  info,
  canManage,
  fail,
}: {
  spaceId: SpaceId;
  member: MemberInfo;
  info: MembersInfo;
  canManage: boolean;
  fail: (message: string | null) => void;
}) {
  const setMemberRole = useAppStore((state) => state.setMemberRole);
  const removeMember = useAppStore((state) => state.removeMember);
  const setPersonVerified = useAppStore((state) => state.setPersonVerified);
  const [removing, setRemoving] = useState(false);
  const [busy, setBusy] = useState(false);
  const roleId = useId();
  const owners = info.members.filter((candidate) => candidate.role === "owner").length;
  const lastOwner = member.role === "owner" && owners === 1;
  const joined = formatDay(member.joinedAt);
  const viaKit = member.viaKitAt === undefined ? "" : formatDay(member.viaKitAt);

  async function changeRole(role: MemberRole): Promise<void> {
    fail(null);
    setBusy(true);
    try {
      await setMemberRole(spaceId, member.accountId, role);
    } catch (error) {
      fail(t("sharing.members.roleError", { message: toCoreError(error).message }));
    }
    setBusy(false);
  }

  async function mark(verified: boolean): Promise<void> {
    fail(null);
    setBusy(true);
    try {
      await setPersonVerified(member.accountId, verified);
    } catch (error) {
      fail(t("sharing.members.verifyError", { message: toCoreError(error).message }));
    }
    setBusy(false);
  }

  return (
    <li className="member-item">
      <div className="member-head">
        <span className="member-name">{member.name}</span>
        {member.isMe && <span className="muted">({t("sharing.members.you")})</span>}
        {!member.isMe && (
          <span className={member.verified ? "tag tag-accent" : "tag"}>
            {member.verified ? t("sharing.members.verified") : t("sharing.members.unverified")}
          </span>
        )}
        {canManage ? (
          <select
            id={roleId}
            className="select member-role"
            aria-label={t("sharing.roleFor", { name: member.name })}
            value={member.role}
            disabled={busy || lastOwner}
            title={lastOwner ? t("sharing.members.lastOwner") : roleHint(member.role)}
            onChange={(event) => {
              void changeRole(event.target.value as MemberRole);
            }}
          >
            {ROLES.map((role) => (
              <option key={role} value={role}>
                {roleLabel(role)}
              </option>
            ))}
          </select>
        ) : (
          <span className="tag member-role-tag" title={roleHint(member.role)}>
            {roleLabel(member.role)}
          </span>
        )}
        {canManage && !member.isMe && (
          <button
            type="button"
            className="link-button link-danger member-remove"
            disabled={busy}
            onClick={() => {
              setRemoving(true);
            }}
          >
            {t("sharing.members.remove")}
          </button>
        )}
      </div>
      <div className="member-meta muted">
        <span>{tp("sharing.members.devices", member.deviceCount)}</span>
        {joined !== "" && <span>{t("sharing.members.joined", { date: joined })}</span>}
        {viaKit !== "" && <span>{t("sharing.members.viaKit", { date: viaKit })}</span>}
      </div>
      {member.nameShared && (
        <p className="member-note" role="note">
          {t("sharing.members.nameShared")}
        </p>
      )}
      {member.refusedChanges > 0 && (
        <p className="member-note" role="note">
          {refusedText(member)}
        </p>
      )}
      {!member.isMe && member.safetyNumber !== undefined && (
        <details className="member-safety">
          <summary>{t("sharing.members.safety")}</summary>
          <p className="member-safety-number" role="group" aria-label={t("sharing.members.safety")}>
            {safetyGroups(member.safetyNumber).map((group, index) => (
              <span key={index}>{group}</span>
            ))}
          </p>
          <p className="muted">{t("sharing.members.safetyHint", { name: member.name })}</p>
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              void mark(!member.verified);
            }}
          >
            {member.verified ? t("sharing.members.unmarkVerified") : t("sharing.members.markVerified")}
          </button>
        </details>
      )}
      {removing && (
        <ConfirmDialog
          title={t("sharing.members.removeTitle", { name: member.name })}
          body={t("sharing.members.removeBody", { name: member.name })}
          confirmLabel={t("sharing.members.removeConfirm", { name: member.name })}
          onConfirm={async () => {
            fail(null);
            try {
              await removeMember(spaceId, member.accountId);
            } catch (error) {
              fail(t("sharing.members.removeError", { message: toCoreError(error).message }));
            }
          }}
          onClose={() => {
            setRemoving(false);
          }}
        />
      )}
    </li>
  );
}

/** The members in join order. */
export function MemberList({ spaceId, info, canManage }: MemberListProps) {
  const [error, setError] = useState<string | null>(null);
  return (
    <>
      <ul className="member-list" aria-label={t("sharing.members.list")}>
        {info.members.map((member) => (
          <MemberItem
            key={member.accountId}
            spaceId={spaceId}
            member={member}
            info={info}
            canManage={canManage}
            fail={setError}
          />
        ))}
      </ul>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </>
  );
}
