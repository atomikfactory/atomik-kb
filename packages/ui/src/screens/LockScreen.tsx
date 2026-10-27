import { toCoreError } from "@atomik-kb/core-client";
import { useEffect, useId, useRef, useState } from "react";
import type { FormEvent } from "react";

import { LockIcon } from "../components/icons";
import { isFieldError } from "../errors";
import { t } from "../i18n";
import { useAppStore } from "../store-context";

/**
 * The app is locked (ADR-0018): the core holds no key and nothing from the workspace is in
 * memory, so this screen shows nothing but the passphrase field. Unlocking starts the app
 * again like at launch.
 */
export function LockScreen() {
  const unlock = useAppStore((state) => state.unlock);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const titleId = useId();
  const inputId = useId();
  const errorId = useId();

  // Back to the field after a failed attempt, ready to type again.
  useEffect(() => {
    if (error !== null) {
      input.current?.focus();
    }
  }, [error]);

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (passphrase === "") {
      setError(t("lock.error.empty"));
      return;
    }
    const typed = passphrase;
    // Kept no longer than needed: the field empties as soon as the attempt starts.
    setPassphrase("");
    setError(null);
    setBusy(true);
    try {
      await unlock(typed);
      // The app starts again and this screen goes away.
    } catch (failure) {
      const reason = toCoreError(failure);
      setError(
        isFieldError(reason, "passphrase") ? t("lock.error.wrong") : t("lock.error.generic", { message: reason.message }),
      );
      setBusy(false);
    }
  }

  return (
    <main className="center-screen">
      <section className="card card-narrow lock-card" aria-labelledby={titleId}>
        <span className="lock-badge" aria-hidden="true">
          <LockIcon />
        </span>
        <h1 className="title" id={titleId}>
          {t("lock.title")}
        </h1>
        <p className="muted">{t("lock.body")}</p>
        <form
          className="form"
          noValidate
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <label htmlFor={inputId}>{t("lock.passphraseLabel")}</label>
          <input
            ref={input}
            id={inputId}
            className="input"
            type="password"
            value={passphrase}
            autoComplete="current-password"
            autoFocus
            readOnly={busy}
            aria-invalid={error !== null}
            aria-describedby={error !== null ? errorId : undefined}
            onChange={(event) => {
              setPassphrase(event.target.value);
              setError(null);
            }}
          />
          {error !== null && (
            <p className="error-text" id={errorId} role="alert">
              {error}
            </p>
          )}
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? t("lock.submitting") : t("lock.submit")}
          </button>
        </form>
      </section>
    </main>
  );
}
