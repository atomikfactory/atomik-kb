// "Share this space..." (the space menu, Milestone 13, ADR-0041 and ADR-0047): what sharing sends, said in plain words before
// anything is sent. The four roles, the history warning (everyone who is invited receives the whole history, including the
// trash and everything ever deleted; members keep copies, and removing one cannot take them back), the counts the core's plan
// gives, the note that members may turn on local AI on their own devices, and what is missing (a sync server, the recovery
// kit, the name other members see). "Share" asks the core, which answers `Pending` until the server accepts the space; the
// dialog follows the space's state and then offers to invite people.

import { toCoreError } from "@atomik-kb/core-client";
import type { SpaceId, SpaceSharePlan } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { formatBytes } from "../format";
import { t } from "../i18n";
import { ROLES, roleHint, roleLabel } from "../sharing";
import { useSpace } from "../sharing-hooks";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { PairingProgress } from "./Pairing";

export interface ShareDialogProps {
  spaceId: SpaceId;
  /** The space's name as the sidebar shows it. */
  name: string;
  /** Where focus goes when the dialog closes (the control that opened the menu). */
  returnFocus?: Element | null;
  /** Opens the member panel (the "Invite people" button of a space that is shared now). */
  onInvite: () => void;
  onClose: () => void;
}

/** What the plan read gives: on its way, there, or refused. */
type Plan = { status: "loading" } | { status: "ready"; plan: SpaceSharePlan } | { status: "error"; message: string };

function describeShareError(error: unknown): string {
  const failure = toCoreError(error);
  if (isFieldError(failure, "displayName")) {
    return t("form.fieldError", { message: failure.message });
  }
  return t("sharing.share.error", { message: failure.message });
}

export function ShareDialog({ spaceId, name, returnFocus, onInvite, onClose }: ShareDialogProps) {
  const planShare = useAppStore((state) => state.planShare);
  const shareSpace = useAppStore((state) => state.shareSpace);
  const setProfileName = useAppStore((state) => state.setProfileName);
  const profileName = useAppStore((state) => state.profile?.displayName);
  const space = useSpace(spaceId);
  const [plan, setPlan] = useState<Plan>({ status: "loading" });
  const [typedName, setTypedName] = useState(profileName ?? "");
  const [started, setStarted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const titleId = useId();
  const introId = useId();
  const nameId = useId();
  const nameHintId = useId();
  const returnRef = useRef<HTMLElement | null>(returnFocus instanceof HTMLElement ? returnFocus : null);

  useEffect(() => {
    let current = true;
    planShare(spaceId).then(
      (value) => {
        if (current) {
          setPlan({ status: "ready", plan: value });
        }
      },
      (failure: unknown) => {
        if (current) {
          setPlan({ status: "error", message: toCoreError(failure).message });
        }
      },
    );
    return () => {
      current = false;
    };
  }, [planShare, spaceId]);

  const ready = plan.status === "ready" ? plan.plan : null;
  const needName = ready !== null && !ready.hasProfileName;
  const blocked = ready !== null && (!ready.serverConfigured || !ready.hasAccount);
  const shared = space?.shareState === "Shared";
  const sharing = started && !shared;

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy || ready === null || blocked) {
      return;
    }
    if (needName && typedName.trim() === "") {
      setError(t("sharing.share.error.name"));
      return;
    }
    setError(null);
    setBusy(true);
    try {
      if (needName) {
        await setProfileName(typedName);
      }
      await shareSpace(spaceId);
      setStarted(true);
    } catch (failure) {
      setError(describeShareError(failure));
    }
    setBusy(false);
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={introId}
      onDismiss={busy || sharing ? undefined : onClose}
      className="share-dialog"
      returnFocus={returnRef}
    >
      <h2 className="subtitle" id={titleId}>
        {t("sharing.share.title", { name })}
      </h2>
      {started ? (
        <div className="share-result">
          {shared ? (
            <>
              <p id={introId} role="status">
                {t("sharing.share.done", { name })}
              </p>
              <div className="dialog-actions">
                <button type="button" className="button" onClick={onClose}>
                  {t("dialog.close")}
                </button>
                <button
                  type="button"
                  className="button button-primary"
                  onClick={() => {
                    onClose();
                    onInvite();
                  }}
                >
                  {t("sharing.share.invite")}
                </button>
              </div>
            </>
          ) : (
            <div id={introId}>
              <PairingProgress text={t("sharing.share.pending")} />
            </div>
          )}
        </div>
      ) : (
        <form
          className="form share-form"
          noValidate
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <p id={introId}>{t("sharing.share.intro")}</p>
          <section aria-label={t("sharing.share.roles")}>
            <h3 className="share-heading">{t("sharing.share.roles")}</h3>
            <dl className="role-list">
              {ROLES.map((role) => (
                <div key={role} className="role-item">
                  <dt>{roleLabel(role)}</dt>
                  <dd className="muted">{roleHint(role)}</dd>
                </div>
              ))}
            </dl>
          </section>
          <div className="banner banner-warning share-history" role="note">
            <h3 className="share-heading">{t("sharing.share.historyTitle")}</h3>
            <p>{t("sharing.share.history")}</p>
            {plan.status === "loading" && <p className="muted">{t("sharing.share.loading")}</p>}
            {plan.status === "error" && (
              <p className="error-text" role="alert">
                {t("sharing.share.planError", { message: plan.message })}
              </p>
            )}
            {ready !== null && (
              <dl className="facts share-counts">
                <dt>{t("sharing.share.fact.pages")}</dt>
                <dd>{ready.pageCount}</dd>
                <dt>{t("sharing.share.fact.trashed")}</dt>
                <dd>{ready.trashedPageCount}</dd>
                <dt>{t("sharing.share.fact.files")}</dt>
                <dd>
                  {ready.attachmentCount > 0
                    ? t("sharing.share.fact.filesValue", {
                        count: ready.attachmentCount,
                        size: formatBytes(ready.attachmentBytes),
                      })
                    : ready.attachmentCount}
                </dd>
                <dt>{t("sharing.share.fact.history")}</dt>
                <dd>{formatBytes(ready.historyBytes)}</dd>
              </dl>
            )}
          </div>
          <p className="muted share-ai">{t("sharing.share.ai")}</p>
          {ready !== null && !ready.serverConfigured && (
            <p className="note" role="alert">
              {t("sharing.share.needSync")}
            </p>
          )}
          {ready !== null && ready.serverConfigured && !ready.hasAccount && (
            <p className="note" role="alert">
              {t("sharing.share.needKit")}
            </p>
          )}
          {needName && !blocked && (
            <div className="field">
              <label htmlFor={nameId}>{t("sharing.share.nameLabel")}</label>
              <input
                id={nameId}
                className="input"
                type="text"
                value={typedName}
                placeholder={t("sharing.share.namePlaceholder")}
                autoComplete="off"
                readOnly={busy}
                aria-describedby={nameHintId}
                onChange={(event) => {
                  setTypedName(event.target.value);
                  setError(null);
                }}
              />
              <p className="muted" id={nameHintId}>
                {t("sharing.share.nameHint")}
              </p>
            </div>
          )}
          {error !== null && (
            <p className="error-text" role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={busy} onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-primary" disabled={busy || ready === null || blocked}>
              {busy ? t("sharing.share.working") : t("sharing.share.submit")}
            </button>
          </div>
        </form>
      )}
    </Modal>
  );
}
