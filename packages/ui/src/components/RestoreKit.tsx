// The recovery kit step of a restore (ADR-0017) and what the restore shows when it fails. `Restore.tsx` (the button on the first
// onboarding screen) and `RestoreWizard.tsx` (the wizard behind it, ADR-0059) share it: a backup of another account needs its
// recovery kit, the core then answers `InvalidInput` for the field `kit`, and `RestoreKitDialog` asks for it.

import { toCoreError } from "@atomik-kb/core-client";
import type { CoreError } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent, RefObject } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { kitInputText, readKitInput } from "../kit";
import type { KitInput } from "../kit";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";

export interface Notice {
  tone: "error" | "info";
  text: string;
}

export function describeRestoreError(failure: CoreError): Notice {
  if (isFieldError(failure, "path")) {
    return { tone: "info", text: t("onboarding.restore.cancelled") };
  }
  if (isFieldError(failure, "kit")) {
    return { tone: "error", text: t("restoreKit.error.wrong") };
  }
  switch (failure.code) {
    case "Crypto":
      return { tone: "error", text: t("onboarding.restore.error.damaged") };
    case "AlreadyExists":
      return { tone: "error", text: t("onboarding.restore.error.alreadyExists") };
    case "InvalidInput":
      return { tone: "error", text: t("onboarding.restore.error.unreadable", { message: failure.message }) };
    default:
      return { tone: "error", text: t("onboarding.restore.error.generic", { message: failure.message }) };
  }
}

export function NoticeText({ notice, id }: { notice: Notice; id?: string }) {
  return notice.tone === "error" ? (
    <p className="error-text" id={id} role="alert">
      {notice.text}
    </p>
  ) : (
    <p className="muted" id={id} role="status">
      {notice.text}
    </p>
  );
}

/** Live help under a kit field: how many words, which ones are not in the list (also used by "Recover with my kit"). */
export function KitHint({ input, id }: { input: KitInput; id: string }) {
  switch (input.kind) {
    case "empty":
      return (
        <p className="muted kit-hint" id={id}>
          {t("restoreKit.hint.prefix")}
        </p>
      );
    case "code":
      return (
        <p className="muted kit-hint" id={id}>
          {t("restoreKit.hint.code")}
        </p>
      );
    case "words":
      return (
        <div className="kit-hint" id={id}>
          <p className="muted">
            {t("restoreKit.hint.count", { count: input.words.length })} {t("restoreKit.hint.prefix")}
          </p>
          {input.unknown.length > 0 && (
            <p className="error-text">
              {t("restoreKit.hint.unknown", {
                words: input.unknown.map((unknown) => t("restoreKit.hint.unknownWord", unknown)).join(", "),
              })}
            </p>
          )}
        </div>
      );
  }
}

export interface RestoreKitDialogProps {
  /** The backup file, when the core said which one it read (or the wizard listed it); otherwise the shell asks again. */
  path?: string;
  /** Where focus goes when the dialog closes (the "Restore from a backup" button, or the wizard's step heading). */
  returnFocus?: RefObject<HTMLElement | null>;
  onClose: () => void;
}

/** Asks for the recovery kit of a backup (words or code) and restores with it. */
export function RestoreKitDialog({ path, returnFocus, onClose }: RestoreKitDialogProps) {
  const importBackup = useAppStore((state) => state.importBackup);
  const [text, setText] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [busy, setBusy] = useState(false);
  const titleId = useId();
  const bodyId = useId();
  const fieldId = useId();
  const hintId = useId();
  const noticeId = useId();
  const input = readKitInput(text);

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (input.kind === "empty") {
      setNotice({ tone: "error", text: t("restoreKit.error.empty") });
      return;
    }
    setNotice(null);
    setBusy(true);
    try {
      await importBackup(kitInputText(input), path);
      // Restored: the app starts again with the workspace, and this dialog goes with the onboarding.
    } catch (failure) {
      setNotice(describeRestoreError(toCoreError(failure)));
      setBusy(false);
    }
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={bodyId}
      onDismiss={busy ? undefined : onClose}
      returnFocus={returnFocus}
      className="restore-dialog"
    >
      <h2 className="subtitle" id={titleId}>
        {t("restoreKit.title")}
      </h2>
      <p id={bodyId}>{t("restoreKit.body")}</p>
      <form
        className="form"
        noValidate
        onSubmit={(event) => {
          void submit(event);
        }}
      >
        <label htmlFor={fieldId}>{t("restoreKit.label")}</label>
        <textarea
          id={fieldId}
          className="input kit-input"
          rows={4}
          value={text}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          readOnly={busy}
          aria-invalid={notice?.tone === "error"}
          aria-describedby={notice === null ? hintId : `${hintId} ${noticeId}`}
          onChange={(event) => {
            setText(event.target.value);
            setNotice(null);
          }}
        />
        <KitHint input={input} id={hintId} />
        {notice !== null && <NoticeText notice={notice} id={noticeId} />}
        <div className="dialog-actions">
          <button type="button" className="button" disabled={busy} onClick={onClose}>
            {t("dialog.cancel")}
          </button>
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? t("restoreKit.submitting") : t("restoreKit.submit")}
          </button>
        </div>
      </form>
    </Modal>
  );
}
