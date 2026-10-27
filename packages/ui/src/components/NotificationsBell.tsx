// The bell with the count of unread notifications, and the inbox it opens (ADR-0048): the notifications of this device, newest
// first, each saying who did what on which page (never the words of a comment) with its age. A click on a row opens what it is
// about at the anchor (the page at its block, and the comments panel on the thread for a comment) and marks it read; "Mark all as
// read" clears the count. The rows are raised by other people's changes only; the toasts and the OS notification come from the
// same events.

import { toCoreError } from "@atomik-kb/core-client";
import { useEffect, useState } from "react";

import type { NotificationInfo } from "@atomik-kb/core-client";

import { personName } from "../comments";
import { usePeople } from "../comments-hooks";
import { formatAgo } from "../format";
import { t, tp } from "../i18n";
import { useAppStore } from "../store-context";
import { notificationText } from "../store-comments";
import { FloatingPanel, anchorBelow } from "./FloatingPanel";
import type { Anchor } from "./FloatingPanel";
import { BellIcon } from "./icons";

/** The count the badge writes: up to 99, then "99+". */
export function badgeText(unread: number): string {
  return unread > 99 ? "99+" : String(unread);
}

/** One row: the phrase of its kind, the actor named as the space says now ("Former member (name)" once they left), its age. */
function InboxRow({
  row,
  onOpen,
  onMark,
}: {
  row: NotificationInfo;
  onOpen: (row: NotificationInfo) => void;
  onMark: (ids: number[]) => void;
}) {
  const people = usePeople(row.spaceId);
  const text = notificationText(row, personName(people, row.actor, row.actorName));
  const when = formatAgo(row.at);
  return (
    <li className={`inbox-row${row.read ? "" : " is-unread"}`}>
      <button
        type="button"
        className="inbox-open"
        onClick={() => {
          onOpen(row);
        }}
      >
        <span className="inbox-text">{text}</span>
        {when !== null && <span className="inbox-time muted">{when}</span>}
      </button>
      {!row.read && (
        <button
          type="button"
          className="link-button inbox-mark"
          aria-label={t("inbox.markRead.label", { text })}
          onClick={() => {
            onMark([row.id]);
          }}
        >
          {t("inbox.markRead")}
        </button>
      )}
    </li>
  );
}

function Inbox({ anchor, anchorElement, onClose }: { anchor: Anchor; anchorElement: Element; onClose: () => void }) {
  const items = useAppStore((state) => state.inbox.items);
  const unread = useAppStore((state) => state.inbox.unread);
  const status = useAppStore((state) => state.inbox.status);
  const loadInbox = useAppStore((state) => state.loadInbox);
  const markRead = useAppStore((state) => state.markNotificationsRead);
  const openNotification = useAppStore((state) => state.openNotification);
  const [failure, setFailure] = useState<string | null>(null);

  // What arrived while the popover was closed is read when it opens.
  useEffect(() => {
    loadInbox().catch((error: unknown) => {
      console.error("could not read the inbox", toCoreError(error).code);
    });
  }, [loadInbox]);

  async function mark(ids?: number[]): Promise<void> {
    setFailure(null);
    try {
      await markRead(ids);
    } catch (error) {
      setFailure(t("inbox.error", { message: toCoreError(error).message }));
    }
  }

  return (
    <FloatingPanel anchor={anchor} anchorElement={anchorElement} label={t("inbox.title")} className="inbox" onClose={onClose}>
      <header className="inbox-header">
        <h2 className="inbox-title">{t("inbox.title")}</h2>
        <button
          type="button"
          className="link-button"
          disabled={unread === 0}
          onClick={() => {
            void mark();
          }}
        >
          {t("inbox.markAll")}
        </button>
      </header>
      {failure !== null && (
        <p className="error-text" role="alert">
          {failure}
        </p>
      )}
      {items.length === 0 ? (
        <p className="muted inbox-empty">{status === "loading" ? t("inbox.loading") : t("inbox.empty")}</p>
      ) : (
        <ul className="inbox-list" aria-label={t("inbox.list")}>
          {items.map((row) => (
            <InboxRow
              key={row.id}
              row={row}
              onOpen={openNotification}
              onMark={(ids) => {
                void mark(ids);
              }}
            />
          ))}
        </ul>
      )}
    </FloatingPanel>
  );
}

/** The bell of the sidebar: its badge counts the unread notifications; a click opens the inbox. */
export function NotificationsBell() {
  const unread = useAppStore((state) => state.inbox.unread);
  const open = useAppStore((state) => state.inboxOpen);
  const setInboxOpen = useAppStore((state) => state.setInboxOpen);
  // Where the inbox opens and the button that opened it (set by the click, so that nothing reads a ref while drawing).
  const [opener, setOpener] = useState<{ anchor: Anchor; element: Element } | null>(null);
  const label = unread === 0 ? t("inbox.open") : tp("inbox.openUnread", unread);

  return (
    <>
      <button
        type="button"
        className="icon-button notifications-bell"
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={(event) => {
          setOpener({ anchor: anchorBelow(event.currentTarget), element: event.currentTarget });
          setInboxOpen(!open);
        }}
      >
        <BellIcon />
        {unread > 0 && (
          <span className="notifications-badge" aria-hidden="true">
            {badgeText(unread)}
          </span>
        )}
      </button>
      {open && opener !== null && (
        <Inbox
          anchor={opener.anchor}
          anchorElement={opener.element}
          onClose={() => {
            setInboxOpen(false);
          }}
        />
      )}
    </>
  );
}
