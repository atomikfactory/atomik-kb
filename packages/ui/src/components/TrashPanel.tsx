import { TRASH_RETENTION_SETTING, toCoreError, trashRetentionMs } from "@atomik-kb/core-client";
import type { TrashedPage } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";

import { formatTrashExpiry } from "../format";
import { t, tp } from "../i18n";
import { useCurrentAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { useRunAction } from "./actions";
import { ConfirmDialog } from "./ConfirmDialog";
import { ChevronIcon, TrashIcon } from "./icons";
import { PageIcon } from "./PageIcon";

function titleOf(entry: TrashedPage): string {
  return entry.page.title === "" ? t("page.untitled") : entry.page.title;
}

function insideLabel(count: number): string {
  return tp("trash.inside", count);
}

/** What permanently deleting a trashed page takes with it, in words. */
function purgeBody(entry: TrashedPage): string {
  const title = titleOf(entry);
  switch (entry.descendantCount) {
    case 0:
      return t("trash.purgeBody.none", { title });
    default:
      return tp("trash.purgeBody", entry.descendantCount, { title });
  }
}

function TrashItem({
  entry,
  retentionMs,
  onRestore,
  onPurge,
}: {
  entry: TrashedPage;
  retentionMs: number | null;
  /** Restoring needs an Editor (`undefined`: not offered). */
  onRestore?: () => void;
  /** Deleting for good is an Owner's (`undefined`: not offered). */
  onPurge?: () => void;
}) {
  const titleId = useId();
  const expiry = entry.page.trashedAt === undefined ? null : formatTrashExpiry(entry.page.trashedAt, retentionMs);
  return (
    <li className="trash-item">
      <div className="trash-info">
        <PageIcon icon={entry.page.icon} className="tree-icon" />
        <span
          className={entry.page.title === "" ? "trash-title is-untitled" : "trash-title"}
          id={titleId}
          title={titleOf(entry)}
        >
          {titleOf(entry)}
        </span>
        {entry.descendantCount > 0 && <span className="trash-count">{insideLabel(entry.descendantCount)}</span>}
      </div>
      {expiry !== null && <div className="trash-expiry">{expiry}</div>}
      <div className="trash-actions">
        {onRestore !== undefined && (
          <button type="button" className="link-button" aria-describedby={titleId} onClick={onRestore}>
            {t("trash.restore")}
          </button>
        )}
        {onPurge !== undefined && (
          <button type="button" className="link-button link-danger" aria-describedby={titleId} onClick={onPurge}>
            {t("trash.purge")}
          </button>
        )}
      </div>
    </li>
  );
}

/**
 * The trash of the current space, at the bottom of the sidebar: a collapsed header with
 * the number of trashed pages, and when opened the pages that can be restored or deleted
 * permanently (after a confirmation that says it cannot be undone), each with the day it will
 * be deleted by itself (ADR-0040; nothing when the device setting keeps trashed pages).
 *
 * In a shared space (Milestone 13) restoring is an Editor's and deleting for good an Owner's (ADR-0047: an Editor's "Delete forever"
 * is hidden); a read-only role only sees what is in the trash.
 */
export function TrashPanel() {
  const run = useRunAction();
  const tree = useAppStore((state) => state.tree);
  const access = useCurrentAccess();
  const restorePage = useAppStore((state) => state.restorePage);
  const purgePage = useAppStore((state) => state.purgePage);
  const storedLimit = useAppStore((state) => state.settings[TRASH_RETENTION_SETTING]);
  const loadSetting = useAppStore((state) => state.loadSetting);
  const [open, setOpen] = useState(false);
  const [purging, setPurging] = useState<TrashedPage | null>(null);
  const listId = useId();
  const unread = storedLimit === undefined;

  // The limit is read when the list is first opened; until then (or when the read failed) the
  // entries say nothing about their deletion.
  useEffect(() => {
    if (open && unread) {
      loadSetting(TRASH_RETENTION_SETTING).catch((failure: unknown) => {
        console.error("could not read the trash limit", toCoreError(failure).code);
      });
    }
  }, [open, unread, loadSetting]);

  if (tree === null) {
    return null;
  }
  const trashed = tree.trashed;
  const retentionMs = unread ? null : trashRetentionMs(storedLimit);

  return (
    <div className="trash">
      <button
        type="button"
        className="trash-toggle"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => {
          setOpen((current) => !current);
        }}
      >
        <TrashIcon />
        <span className="trash-toggle-label">{t("trash.title", { count: trashed.length })}</span>
        <span className="trash-toggle-chevron" data-expanded={open}>
          <ChevronIcon />
        </span>
      </button>
      {open && (
        <div className="trash-body" id={listId}>
          {trashed.length === 0 ? (
            <p className="muted sidebar-note">{t("trash.empty")}</p>
          ) : (
            <ul className="trash-list" aria-label={t("trash.listLabel")}>
              {trashed.map((entry) => (
                <TrashItem
                  key={entry.page.id}
                  entry={entry}
                  retentionMs={retentionMs}
                  onRestore={
                    access.canEdit
                      ? () => {
                          void run(restorePage(entry.page.id), "tree.error.restore");
                        }
                      : undefined
                  }
                  onPurge={
                    access.isOwner
                      ? () => {
                          setPurging(entry);
                        }
                      : undefined
                  }
                />
              ))}
            </ul>
          )}
        </div>
      )}
      {purging !== null && (
        <ConfirmDialog
          title={t("trash.purgeTitle")}
          body={purgeBody(purging)}
          confirmLabel={t("trash.purge")}
          onConfirm={() => run(purgePage(purging.page.id), "tree.error.purge")}
          onClose={() => {
            setPurging(null);
          }}
        />
      )}
    </div>
  );
}
