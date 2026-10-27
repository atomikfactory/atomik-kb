// "Share a view link..." (the page menu, Milestone 13, slice 4; ADR-0051): says what a view link shows (this page only, as it reads
// now, with no history and no comments) and who can open it (anyone who holds it), makes one with an optional expiry, shows its
// URL once with a Copy button (the dialog keeps it in its own state and nowhere else; it cannot be shown again, and the lists of the
// store never hold one), and lists the links of the page with when they were made, by whom and when they expire, and "Revoke" (after
// a question) for the creator of a link and for Owners. It is offered where the role allows it (`viewLinkRight`).

import { toCoreError } from "@atomik-kb/core-client";
import type { PageId, SpaceId, ViewLinkInfo, ViewLinkStatus } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { FormEvent } from "react";

import { t, tp } from "../i18n";
import type { MessageKey } from "../i18n";
import { VIEW_LINK_DAYS, formatMoment } from "../sharing";
import { useMembers, useSpaceAccess } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import type { ViewLinkEntry } from "../store-links";
import { ConfirmDialog } from "./ConfirmDialog";
import { CopyLine } from "./InviteLinkDialog";
import { Modal } from "./Modal";

export interface ViewLinkDialogProps {
  spaceId: SpaceId;
  pageId: PageId;
  onClose: () => void;
}

const STATUS_KEYS: Readonly<Record<ViewLinkStatus, MessageKey>> = {
  Publishing: "viewLink.status.publishing",
  Live: "viewLink.status.live",
  Expired: "viewLink.status.expired",
  Revoked: "viewLink.status.revoked",
};

/** The sentence for a failure to create a link: sync being off has its own, the rest is the core's message. */
function createErrorText(failure: unknown): string {
  const error = toCoreError(failure);
  const details: unknown = error.details;
  const field = typeof details === "object" && details !== null && "field" in details ? details.field : undefined;
  return error.code === "InvalidInput" && field === "serverUrl"
    ? t("viewLink.error.noSync")
    : t("viewLink.error.create", { message: error.message });
}

/** When a link ends, in words. */
function expiryText(link: ViewLinkEntry): string {
  if (link.expiresAt === undefined) {
    return t("viewLink.row.never");
  }
  return link.status === "Expired"
    ? t("viewLink.row.expired", { when: formatMoment(link.expiresAt) })
    : t("viewLink.row.expires", { when: formatMoment(link.expiresAt) });
}

export function ViewLinkDialog({ spaceId, pageId, onClose }: ViewLinkDialogProps) {
  const loadViewLinks = useAppStore((state) => state.loadViewLinks);
  const createViewLink = useAppStore((state) => state.createViewLink);
  const revokeViewLink = useAppStore((state) => state.revokeViewLink);
  const links = useAppStore((state) => state.viewLinks[spaceId]);
  const access = useSpaceAccess(spaceId);
  const members = useMembers(spaceId);
  const [days, setDays] = useState<number | null>(null);
  // The link just made, with its URL: the only place the URL is ever held, gone with the dialog.
  const [created, setCreated] = useState<ViewLinkInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<ViewLinkEntry | null>(null);
  const [revokeError, setRevokeError] = useState<string | null>(null);
  const titleId = useId();
  const introId = useId();
  const expiryId = useId();

  useEffect(() => {
    let current = true;
    loadViewLinks(spaceId).catch((failure: unknown) => {
      if (current) {
        setLoadError(t("viewLink.error.load", { message: toCoreError(failure).message }));
      }
    });
    return () => {
      current = false;
    };
  }, [loadViewLinks, spaceId]);

  const mine = members?.members.find((member) => member.isMe)?.accountId;
  const forPage = (links ?? []).filter((link) => link.pageId === pageId);
  const publishing = created !== null && forPage.some((link) => link.linkId === created.linkId && link.status === "Publishing");

  /** The creator of a link and an Owner revoke it; a personal space is all this account's. */
  function mayRevoke(link: ViewLinkEntry): boolean {
    return !access.shared || access.isOwner || mine === undefined || link.createdBy === mine;
  }

  async function create(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    setError(null);
    setBusy(true);
    try {
      setCreated(await createViewLink(spaceId, pageId, days ?? undefined));
    } catch (failure) {
      setError(createErrorText(failure));
    }
    setBusy(false);
  }

  return (
    <Modal labelledBy={titleId} describedBy={introId} onDismiss={busy ? undefined : onClose} className="invite-dialog">
      <h2 className="subtitle" id={titleId}>
        {t("viewLink.title")}
      </h2>
      <p id={introId}>{t("viewLink.intro")}</p>
      <div className="view-link-facts">
        <h3 className="members-heading">{t("viewLink.shows.title")}</h3>
        <ul>
          <li>{t("viewLink.shows.page")}</li>
          <li>{t("viewLink.shows.text")}</li>
          <li>{t("viewLink.shows.comments")}</li>
        </ul>
        <h3 className="members-heading">{t("viewLink.who.title")}</h3>
        <ul>
          <li>{t("viewLink.who.anyone")}</li>
          <li>{t("viewLink.who.server")}</li>
        </ul>
        <p className="muted">{t("viewLink.fresh")}</p>
      </div>

      {created === null ? (
        <form
          className="form invite-form"
          noValidate
          onSubmit={(event) => {
            void create(event);
          }}
        >
          <div className="field">
            <label htmlFor={expiryId}>{t("viewLink.expiry")}</label>
            <select
              id={expiryId}
              className="select"
              value={days === null ? "" : String(days)}
              disabled={busy}
              onChange={(event) => {
                setDays(event.target.value === "" ? null : Number(event.target.value));
              }}
            >
              <option value="">{t("viewLink.expiry.never")}</option>
              {VIEW_LINK_DAYS.map((option) => (
                <option key={option} value={option}>
                  {tp("viewLink.expiry.days", option)}
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
            <button type="submit" className="button button-primary" disabled={busy}>
              {busy ? t("viewLink.creating") : t("viewLink.create")}
            </button>
          </div>
        </form>
      ) : (
        <div className="invite-result">
          <p className="note invite-once" role="status">
            {t("viewLink.once")}
          </p>
          <CopyLine label={t("viewLink.linkLabel")} value={created.url} />
          {publishing && <p className="muted">{t("viewLink.publishing.note")}</p>}
        </div>
      )}

      <section className="members-section" aria-labelledby={`${introId}-links`}>
        <h3 className="members-heading" id={`${introId}-links`}>
          {t("viewLink.list.title")}
        </h3>
        {loadError !== null ? (
          <p className="error-text" role="alert">
            {loadError}
          </p>
        ) : links === undefined ? null : forPage.length === 0 ? (
          <p className="muted">{t("viewLink.list.empty")}</p>
        ) : (
          <ul className="invite-list" aria-label={t("viewLink.list.title")}>
            {forPage.map((link) => (
              <li className="invite-item" key={link.linkId}>
                <span className={link.status === "Live" ? "tag tag-accent" : "tag"}>{t(STATUS_KEYS[link.status])}</span>
                <span className="invite-detail">
                  {t("viewLink.row.created", { when: formatMoment(link.createdAt), name: link.createdByName })}
                </span>
                <span className="muted invite-detail">{expiryText(link)}</span>
                <button
                  type="button"
                  className="link-button link-danger invite-revoke"
                  aria-label={t("viewLink.revoke.for", { when: formatMoment(link.createdAt) })}
                  title={mayRevoke(link) ? undefined : t("viewLink.revoke.only")}
                  disabled={!mayRevoke(link)}
                  onClick={() => {
                    setRevokeError(null);
                    setRevoking(link);
                  }}
                >
                  {t("viewLink.revoke")}
                </button>
              </li>
            ))}
          </ul>
        )}
        {revokeError !== null && (
          <p className="error-text" role="alert">
            {revokeError}
          </p>
        )}
      </section>

      <div className="dialog-actions">
        <button type="button" className="button" disabled={busy} onClick={onClose}>
          {t("dialog.close")}
        </button>
      </div>

      {revoking !== null && (
        <ConfirmDialog
          title={t("viewLink.revoke.confirmTitle")}
          body={t("viewLink.revoke.confirmBody")}
          confirmLabel={t("viewLink.revoke.confirm")}
          onConfirm={async () => {
            try {
              await revokeViewLink(spaceId, revoking.linkId);
            } catch (failure) {
              setRevokeError(t("viewLink.revoke.error", { message: toCoreError(failure).message }));
            }
          }}
          onClose={() => {
            setRevoking(null);
          }}
        />
      )}
    </Modal>
  );
}
