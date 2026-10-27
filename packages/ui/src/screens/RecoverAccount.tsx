// "Recover with my kit" on the onboarding screen (ADR-0022): the sync server's address and
// the recovery kit; the core fetches the account's locked copy from the server, opens it
// with the kit and adds this device ("via kit"); its `WorkspaceCreated` starts the app,
// where the reminder to re-issue the kit waits. The kit stays in this form's state only.

import { toCoreError } from "@atomik-kb/core-client";
import type { RecoveryInfo } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { PairingProgress, useFocusOnMount } from "../components/Pairing";
import { KitHint } from "../components/Restore";
import { isFieldError } from "../errors";
import { t } from "../i18n";
import { kitInputText, readKitInput } from "../kit";
import { useAppStore } from "../store-context";
import { recoveryUnderWay } from "../store-devices";

/** A problem with what was typed; `field` says which input it belongs to. */
interface RecoverError {
  field: "url" | "kit" | null;
  text: string;
}

function describeStartError(error: unknown): RecoverError {
  const failure = toCoreError(error);
  if (isFieldError(failure, "serverUrl")) {
    return { field: "url", text: t("recover.error.url", { message: failure.message }) };
  }
  if (isFieldError(failure, "kit")) {
    return { field: "kit", text: t("recover.error.kit") };
  }
  switch (failure.code) {
    case "NotFound":
      return { field: null, text: t("recover.error.notFound") };
    case "AlreadyExists":
      return { field: null, text: t("recover.error.alreadyExists") };
    default:
      return { field: null, text: t("recover.error.generic", { message: failure.message }) };
  }
}

function RecoverForm({ previous, onBack }: { previous: RecoveryInfo | null; onBack: () => void }) {
  const startRecovery = useAppStore((state) => state.startRecovery);
  const [url, setUrl] = useState(previous?.serverUrl ?? "");
  const [kit, setKit] = useState("");
  const [error, setError] = useState<RecoverError | null>(null);
  const [busy, setBusy] = useState(false);
  const urlId = useId();
  const kitId = useId();
  const hintId = useId();
  const errorId = useId();
  const input = readKitInput(kit);

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    const serverUrl = url.trim();
    if (serverUrl === "") {
      setError({ field: "url", text: t("recover.error.emptyUrl") });
      return;
    }
    if (input.kind === "empty") {
      setError({ field: "kit", text: t("recover.error.emptyKit") });
      return;
    }
    setError(null);
    setBusy(true);
    try {
      // The recovery's steps replace this form (and the kit typed into it) once it started.
      await startRecovery(serverUrl, kitInputText(input));
    } catch (failure) {
      setError(describeStartError(failure));
    }
    setBusy(false);
  }

  const errorFor = (field: RecoverError["field"]): string | undefined =>
    error !== null && error.field === field ? errorId : undefined;

  return (
    <>
      <p className="muted">{t("recover.intro")}</p>
      {previous?.state === "Failed" && (
        <p className="error-text" role="alert">
          {t("recover.failed", { message: previous.error ?? "" })}
        </p>
      )}
      <form
        className="form"
        noValidate
        onSubmit={(event) => {
          void submit(event);
        }}
      >
        <label htmlFor={urlId}>{t("recover.urlLabel")}</label>
        <input
          id={urlId}
          className="input"
          type="url"
          inputMode="url"
          value={url}
          placeholder={t("join.urlPlaceholder")}
          autoComplete="url"
          spellCheck={false}
          autoFocus
          readOnly={busy}
          aria-invalid={error?.field === "url"}
          aria-describedby={errorFor("url")}
          onChange={(event) => {
            setUrl(event.target.value);
            setError(null);
          }}
        />
        <label htmlFor={kitId}>{t("recover.kitLabel")}</label>
        <textarea
          id={kitId}
          className="input kit-input"
          rows={4}
          value={kit}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          readOnly={busy}
          aria-invalid={error?.field === "kit"}
          aria-describedby={[hintId, errorFor("kit")].filter((id) => id !== undefined).join(" ")}
          onChange={(event) => {
            setKit(event.target.value);
            setError(null);
          }}
        />
        <KitHint input={input} id={hintId} />
        {error !== null && (
          <p className="error-text" id={errorId} role="alert">
            {error.text}
          </p>
        )}
        <div className="row-actions">
          <button type="submit" className="button button-primary" disabled={busy}>
            {busy ? t("recover.submitting") : t("recover.submit")}
          </button>
          <button type="button" className="button" disabled={busy} onClick={onBack}>
            {t("onboarding.back")}
          </button>
        </div>
      </form>
    </>
  );
}

function RecoverProgress({ recovery }: { recovery: RecoveryInfo }) {
  const cancelRecovery = useAppStore((state) => state.cancelRecovery);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const settingUp = recovery.state === "Done";
  const container = useFocusOnMount<HTMLDivElement>();

  return (
    <div className="pair-session" ref={container} tabIndex={-1}>
      {recovery.state === "Fetching" && <PairingProgress text={t("recover.fetching")} />}
      {recovery.state === "Joining" && <PairingProgress text={t("recover.joining")} />}
      {settingUp && <PairingProgress text={t("recover.settingUp")} />}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {!settingUp && (
        <div className="row-actions">
          <button
            type="button"
            className="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(null);
              cancelRecovery()
                .catch((failure: unknown) => {
                  setError(t("pair.error.cancel", { message: toCoreError(failure).message }));
                })
                .finally(() => {
                  setBusy(false);
                });
            }}
          >
            {t("pair.cancel")}
          </button>
        </div>
      )}
    </div>
  );
}

/** The recovery flow, in the onboarding card. `onBack` returns to creating a workspace. */
export function RecoverAccount({ onBack }: { onBack: () => void }) {
  const recovery = useAppStore((state) => state.recovery);
  const titleId = useId();
  const session = recoveryUnderWay(recovery);

  return (
    <section className="card card-wide join-card" aria-labelledby={titleId}>
      <h1 className="title" id={titleId}>
        {t("recover.title")}
      </h1>
      {session !== null ? (
        <RecoverProgress recovery={session} />
      ) : (
        <RecoverForm previous={recovery} onBack={onBack} />
      )}
    </section>
  );
}
