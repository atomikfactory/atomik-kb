// The recovery kit (ADR-0006, ADR-0017) as the user meets it: created and shown once,
// written down or printed, then checked by typing three of its words back. The same flow
// runs in the onboarding, in a dialog from the banner or Settings, and after a re-issue.
//
// The kit lives only in the state of these components and is gone when they unmount; it
// never reaches the store or any storage. Its words are rendered twice while it is shown:
// on screen and in a print-only sheet (see the print styles in app.css).

import { toCoreError } from "@atomik-kb/core-client";
import type { RecoveryKit } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent, RefObject } from "react";
import { createPortal } from "react-dom";

import { writeClipboard } from "../clipboard";
import { formatTimestamp } from "../format";
import { t } from "../i18n";
import { CONFIRM_WORDS, KIT_WORDS, kitText, matchesWord, pickPositions } from "../kit";
import type { KitForm } from "../kit";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { TextDialog } from "./TextDialog";

/** The kit's words in a numbered grid (four columns, read down each column), or its code. */
export function KitView({ kit, form }: { kit: RecoveryKit; form: KitForm }) {
  if (form === "code") {
    return (
      <div className="kit-code-box" role="group" aria-label={t("kit.codeLabel")}>
        <code className="kit-code">{kit.code}</code>
      </div>
    );
  }
  return <KitWords words={kit.words} label={t("kit.wordsLabel")} />;
}

function KitWords({ words, label }: { words: readonly string[]; label?: string }) {
  return (
    <ol className="kit-words" aria-label={label}>
      {words.map((word, index) => (
        // Words may repeat, so the position is the key.
        <li key={index} className="kit-word">
          <span className="kit-word-number" aria-hidden="true">
            {index + 1}
          </span>
          <span className="kit-word-text">{word}</span>
        </li>
      ))}
    </ol>
  );
}

/**
 * The kit laid out for paper: hidden on screen, and the only thing printed while it is
 * mounted (the print styles hide every other child of the body then).
 */
export function KitPrintSheet({ kit }: { kit: RecoveryKit }) {
  const created = formatTimestamp(kit.createdAt);
  return createPortal(
    <div className="kit-print-sheet" aria-hidden="true">
      <h1 className="kit-print-title">{t("kit.print.title")}</h1>
      {created !== null && <p>{t("kit.print.created", { date: created.text })}</p>}
      <p className="kit-print-account">{t("kit.print.account", { account: kit.accountId })}</p>
      <KitWords words={kit.words} />
      <p className="kit-print-label">{t("kit.print.code")}</p>
      <p className="kit-code">{kit.code}</p>
      <p className="kit-print-warning">{t("kit.print.warning")}</p>
    </div>,
    document.body,
  );
}

/** Moves focus to an element when it mounts (a step that replaces the one that had focus). */
function useFocusOnMount<T extends HTMLElement>(): RefObject<T | null> {
  const ref = useRef<T>(null);
  useEffect(() => {
    ref.current?.focus();
  }, []);
  return ref;
}

function describeCreateError(error: unknown): string {
  const failure = toCoreError(error);
  if (failure.code === "AlreadyExists") {
    return t("kit.error.alreadyExists");
  }
  return t("kit.error.create", { message: failure.message });
}

/** Before the kit exists: what it is, the warning, and the choice to create it now or later. */
function KitIntro({ onCreated, onSkip }: { onCreated: (kit: RecoveryKit) => void; onSkip?: () => void }) {
  const createRecoveryKit = useAppStore((state) => state.createRecoveryKit);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const step = useFocusOnMount<HTMLDivElement>();

  async function create(): Promise<void> {
    setBusy(true);
    setError(null);
    try {
      onCreated(await createRecoveryKit());
    } catch (failure) {
      setError(describeCreateError(failure));
      setBusy(false);
    }
  }

  let label = t("kit.create");
  if (busy) {
    label = t("kit.creating");
  } else if (error !== null) {
    label = t("kit.retry");
  }

  return (
    <div className="kit-step" ref={step} tabIndex={-1}>
      <p>{t("kit.intro")}</p>
      <p className="banner banner-warning">{t("kit.warning")}</p>
      <p className="muted">{t("kit.privacy")}</p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      <div className="kit-actions">
        <button
          type="button"
          className="button button-primary"
          disabled={busy}
          onClick={() => {
            void create();
          }}
        >
          {label}
        </button>
        {onSkip !== undefined && (
          <button type="button" className="link-button" disabled={busy} onClick={onSkip}>
            {t("kit.later")}
          </button>
        )}
      </div>
    </div>
  );
}

/** The kit itself: words or code, copy, print, and on to the check. */
function KitShow({ kit, onContinue }: { kit: RecoveryKit; onContinue: () => void }) {
  const [form, setForm] = useState<KitForm>("words");
  const [notice, setNotice] = useState("");
  const [manualCopy, setManualCopy] = useState<string | null>(null);
  const step = useFocusOnMount<HTMLDivElement>();

  async function copy(): Promise<void> {
    setNotice("");
    const text = kitText(kit, form);
    try {
      await writeClipboard(text);
      setNotice(t("kit.copied"));
    } catch {
      setManualCopy(text); // no clipboard here: show the text to copy by hand
    }
  }

  return (
    <div className="kit-step" ref={step} tabIndex={-1}>
      <p>{t("kit.show.lead")}</p>
      <div className="segmented" role="group" aria-label={t("kit.formLabel")}>
        {(["words", "code"] as const).map((value) => (
          <button
            key={value}
            type="button"
            className="segment"
            aria-pressed={form === value}
            onClick={() => {
              setForm(value);
              setNotice("");
            }}
          >
            {value === "words" ? t("kit.form.words") : t("kit.form.code")}
          </button>
        ))}
      </div>
      <KitView kit={kit} form={form} />
      <div className="kit-toolbar">
        <button
          type="button"
          className="button"
          onClick={() => {
            void copy();
          }}
        >
          {t("kit.copy")}
        </button>
        <button
          type="button"
          className="button"
          onClick={() => {
            window.print();
          }}
        >
          {t("kit.print")}
        </button>
        <p className="muted kit-notice" role="status">
          {notice}
        </p>
      </div>
      <p className="banner banner-warning">{t("kit.warning")}</p>
      <div className="kit-actions">
        <button type="button" className="button button-primary" onClick={onContinue}>
          {t("kit.saved")}
        </button>
      </div>
      <KitPrintSheet kit={kit} />
      {manualCopy !== null && (
        <TextDialog
          title={t("kit.copyDialog.title")}
          body={t("kit.copyDialog.body")}
          text={manualCopy}
          onClose={() => {
            setManualCopy(null);
          }}
        />
      )}
    </div>
  );
}

/**
 * The check: three words typed back, compared here with the kit (case does not matter,
 * the first four letters are enough), then the whole kit verified by the core.
 */
function KitConfirm({
  kit,
  positions,
  onBack,
  onVerified,
}: {
  kit: RecoveryKit;
  positions: readonly number[];
  onBack: () => void;
  onVerified: () => void;
}) {
  const verifyRecoveryKit = useAppStore((state) => state.verifyRecoveryKit);
  const [answers, setAnswers] = useState<readonly string[]>(() => positions.map(() => ""));
  const [wrong, setWrong] = useState<readonly boolean[]>(() => positions.map(() => false));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fieldId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const empty = answers.map((answer) => answer.trim() === "");
    if (empty.some(Boolean)) {
      setWrong(empty);
      setError(t("kit.confirm.error.empty"));
      return;
    }
    const mismatched = positions.map((position, index) => !matchesWord(answers[index] ?? "", kit.words[position] ?? ""));
    if (mismatched.some(Boolean)) {
      setWrong(mismatched);
      setError(t("kit.confirm.error.mismatch"));
      return;
    }
    setWrong(positions.map(() => false));
    setError(null);
    setBusy(true);
    try {
      await verifyRecoveryKit(kitText(kit, "words"));
      onVerified();
    } catch (failure) {
      setError(t("kit.confirm.error.generic", { message: toCoreError(failure).message }));
      setBusy(false);
    }
  }

  return (
    <form
      className="kit-step"
      noValidate
      onSubmit={(event) => {
        void submit(event);
      }}
    >
      <p>{t("kit.confirm.lead")}</p>
      <div className="kit-confirm-fields">
        {positions.map((position, index) => (
          <div className="kit-confirm-field" key={position}>
            <label htmlFor={`${fieldId}-${String(index)}`}>{t("kit.confirm.word", { position: position + 1 })}</label>
            <input
              id={`${fieldId}-${String(index)}`}
              className="input"
              type="text"
              value={answers[index] ?? ""}
              autoComplete="off"
              autoCapitalize="none"
              spellCheck={false}
              autoFocus={index === 0}
              aria-invalid={wrong[index] === true}
              aria-describedby={error !== null ? errorId : undefined}
              onChange={(event) => {
                const value = event.target.value;
                setAnswers((current) => current.map((answer, at) => (at === index ? value : answer)));
                setWrong((current) => current.map((flag, at) => (at === index ? false : flag)));
                setError(null);
              }}
            />
          </div>
        ))}
      </div>
      {error !== null && (
        <p className="error-text" id={errorId} role="alert">
          {error}
        </p>
      )}
      <div className="kit-actions">
        <button type="submit" className="button button-primary" disabled={busy}>
          {busy ? t("kit.confirm.submitting") : t("kit.confirm.submit")}
        </button>
        <button type="button" className="link-button" disabled={busy} onClick={onBack}>
          {t("kit.confirm.back")}
        </button>
      </div>
    </form>
  );
}

function KitDone({ onDone }: { onDone: () => void }) {
  const done = useFocusOnMount<HTMLButtonElement>();
  return (
    <div className="kit-step">
      <p className="kit-done" role="status">
        {t("kit.done.lead")}
      </p>
      <div className="kit-actions">
        <button ref={done} type="button" className="button button-primary" onClick={onDone}>
          {t("kit.done")}
        </button>
      </div>
    </div>
  );
}

export interface RecoveryKitFlowProps {
  /** A kit to show right away (a re-issued one). Without it the flow explains the kit and creates it on request. */
  initialKit?: RecoveryKit;
  /** Offered as "Do this later" while no kit was created; the flow ends without one. */
  onSkip?: () => void;
  /** "Done", once the kit was checked. */
  onDone: () => void;
  /**
   * Tells the container whether the flow is showing a kit that exists only here (from its
   * creation until the check): leaving then would lose the only display of it.
   */
  onHoldsKit?: (holdsKit: boolean) => void;
}

type Stage =
  | { kind: "intro" }
  /** `positions` are the words asked for, once the user went on to the check. */
  | { kind: "show"; kit: RecoveryKit; positions: readonly number[] | null }
  | { kind: "confirm"; kit: RecoveryKit; positions: readonly number[] }
  /** Checked: nothing needs the words any more. */
  | { kind: "done" };

/** Create (or receive), show, check: the recovery kit from start to finish. */
export function RecoveryKitFlow({ initialKit, onSkip, onDone, onHoldsKit }: RecoveryKitFlowProps) {
  const [stage, setStage] = useState<Stage>(() =>
    initialKit === undefined ? { kind: "intro" } : { kind: "show", kit: initialKit, positions: null },
  );

  switch (stage.kind) {
    case "intro":
      return (
        <KitIntro
          onSkip={onSkip}
          onCreated={(kit) => {
            setStage({ kind: "show", kit, positions: null });
            onHoldsKit?.(true);
          }}
        />
      );
    case "show":
      return (
        <KitShow
          kit={stage.kit}
          onContinue={() => {
            // The same three words when the user comes back to the kit and tries again.
            const positions = stage.positions ?? pickPositions(KIT_WORDS, CONFIRM_WORDS);
            setStage({ kind: "confirm", kit: stage.kit, positions });
          }}
        />
      );
    case "confirm":
      return (
        <KitConfirm
          kit={stage.kit}
          positions={stage.positions}
          onBack={() => {
            setStage({ kind: "show", kit: stage.kit, positions: stage.positions });
          }}
          onVerified={() => {
            setStage({ kind: "done" });
            onHoldsKit?.(false);
          }}
        />
      );
    case "done":
      return <KitDone onDone={onDone} />;
  }
}

export interface RecoveryKitDialogProps {
  /** A kit to show right away; without it the dialog starts by explaining and creating one. */
  initialKit?: RecoveryKit;
  /** The heading; "Your recovery kit" by default. */
  title?: string;
  onClose: () => void;
}

/**
 * The recovery kit flow in a modal (from the banner, Settings, or after a re-issue).
 * While it shows a kit that exists nowhere else, Escape and the backdrop do not close it.
 */
export function RecoveryKitDialog({ initialKit, title, onClose }: RecoveryKitDialogProps) {
  const titleId = useId();
  const [holdsKit, setHoldsKit] = useState(initialKit !== undefined);
  return (
    <Modal labelledBy={titleId} onDismiss={holdsKit ? undefined : onClose} className="kit-dialog">
      <h2 className="subtitle" id={titleId}>
        {title ?? t("kit.title")}
      </h2>
      <RecoveryKitFlow initialKit={initialKit} onSkip={onClose} onDone={onClose} onHoldsKit={setHoldsKit} />
    </Modal>
  );
}
