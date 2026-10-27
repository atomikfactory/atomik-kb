// What the space in use says on the home screen, above its pages (Milestone 13): the archive banner for a read-only copy (a closed
// space, or one this account was removed from: "You were removed from this space. This is a read-only copy; it will not update."
// with "Remove from this device"), the notice that the server delivered a change of someone who is not a member, and "N changes
// from Bob were refused" for what this device refused. A notice can be dismissed; a new refusal brings it back.

import { toCoreError } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import { useCurrentAccess, useMembers } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { selectCurrentSpace } from "../store-structure";
import { refusedText } from "./MemberList";
import { ConfirmDialog } from "./ConfirmDialog";

export function SpaceBanners() {
  const space = useAppStore(selectCurrentSpace);
  const access = useCurrentAccess();
  const info = useMembers(space?.id ?? null);
  const dismissed = useAppStore((state) => state.dismissedNotices);
  const dismiss = useAppStore((state) => state.dismissSharingNotice);
  const leaveSpace = useAppStore((state) => state.leaveSpace);
  const [removing, setRemoving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (space === null || !access.shared) {
    return null;
  }
  const flaggedKey = `flagged:${space.id}`;
  const refused = info?.members.filter((member) => member.refusedChanges > 0) ?? [];

  return (
    <>
      {access.archived && (
        <div className="banner banner-warning archive-banner" role="status">
          <p>{access.archiveReason === "Closed" ? t("sharing.archive.closed") : t("sharing.archive.removed")}</p>
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <button
            type="button"
            className="button"
            onClick={() => {
              setRemoving(true);
            }}
          >
            {t("sharing.archive.remove")}
          </button>
        </div>
      )}
      {info?.serverFlagged === true && !dismissed.includes(flaggedKey) && (
        <div className="banner banner-warning space-notice" role="alert">
          <p>{t("sharing.notice.serverFlagged")}</p>
          <button
            type="button"
            className="link-button"
            onClick={() => {
              dismiss(flaggedKey);
            }}
          >
            {t("toast.dismiss")}
          </button>
        </div>
      )}
      {refused.map((member) => {
        const key = `refused:${space.id}:${member.accountId}:${String(member.refusedChanges)}`;
        return dismissed.includes(key) ? null : (
          <div key={key} className="banner space-notice" role="status">
            <p>{refusedText(member)}</p>
            <button
              type="button"
              className="link-button"
              onClick={() => {
                dismiss(key);
              }}
            >
              {t("toast.dismiss")}
            </button>
          </div>
        );
      })}
      {removing && (
        <ConfirmDialog
          title={t("sharing.archive.removeTitle", { name: space.name })}
          body={t("sharing.archive.removeBody")}
          confirmLabel={t("sharing.archive.removeConfirm")}
          onConfirm={async () => {
            setError(null);
            await leaveSpace(space.id).catch((failure: unknown) => {
              setError(t("sharing.archive.error", { message: toCoreError(failure).message }));
            });
          }}
          onClose={() => {
            setRemoving(false);
          }}
        />
      )}
    </>
  );
}
