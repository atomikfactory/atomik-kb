// What the space switcher shows beside a shared space (Milestone 13): two people and the member count, "Sharing..." while the
// server has not accepted the space yet, and a lock with "Read-only copy" for an archive. A personal space has none.

import type { SpaceInfo } from "@atomik-kb/core-client";

import { t, tp } from "../i18n";
import { accessOf } from "../sharing";
import { LockIcon, UsersIcon } from "./icons";

export function SharedBadge({ space }: { space: SpaceInfo }) {
  const access = accessOf(space);
  if (!access.shared) {
    return null;
  }
  if (access.archived) {
    return (
      <span className="space-badge space-badge-archive" title={t("sharing.archive.badge")}>
        <LockIcon />
        <span className="visually-hidden">{t("sharing.archive.badge")}</span>
      </span>
    );
  }
  if (access.pending) {
    return (
      <span className="space-badge" title={t("sharing.pending.badge")}>
        <UsersIcon />
        <span className="visually-hidden">{t("sharing.pending.badge")}</span>
      </span>
    );
  }
  const count = space.memberCount ?? 0;
  const label = tp("sharing.badge", count);
  return (
    <span className="space-badge" title={label}>
      <UsersIcon />
      <span aria-hidden="true">{count}</span>
      <span className="visually-hidden">{label}</span>
    </span>
  );
}
