import { toCoreError } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { RecoveryKitFlow } from "../components/RecoveryKit";
import { RestoreFromBackup } from "../components/Restore";
import { isFieldError } from "../errors";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { pairingUnderWay, recoveryUnderWay } from "../store-devices";
import { JoinAccount } from "./JoinAccount";
import { RecoverAccount } from "./RecoverAccount";

/** The ways into the app on a new installation. */
type Way = "create" | "join" | "recover";

function describeCreateError(error: unknown): string {
  const failure = toCoreError(error);
  if (failure.code === "AlreadyExists") {
    return t("onboarding.error.alreadyExists");
  }
  if (isFieldError(failure, "name")) {
    return t("form.fieldError", { message: failure.message });
  }
  return t("onboarding.error.generic", { message: failure.message });
}

/**
 * The other ways in, for someone who has an account already: join it from a device that
 * has it, or recover it from the sync server with the kit.
 */
function OtherWays({ onChoose }: { onChoose: (way: Way) => void }) {
  return (
    <div className="onboarding-restore onboarding-other">
      <p className="muted">{t("onboarding.other.intro")}</p>
      <div className="row-actions">
        <button
          type="button"
          className="button"
          onClick={() => {
            onChoose("join");
          }}
        >
          {t("onboarding.join.button")}
        </button>
        <button
          type="button"
          className="button"
          onClick={() => {
            onChoose("recover");
          }}
        >
          {t("onboarding.recover.button")}
        </button>
      </div>
    </div>
  );
}

/**
 * Step 1: name the workspace, restore one from a backup, or join an account another way.
 * Only the obvious mistake (nothing typed) is caught here; length and character rules
 * belong to the core, whose answer is shown as is.
 */
function WorkspaceStep({ onChoose }: { onChoose: (way: Way) => void }) {
  const createWorkspace = useAppStore((state) => state.createWorkspace);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const inputId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (submitting) {
      return;
    }
    if (name.trim() === "") {
      setError(t("onboarding.error.empty"));
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      // The first space exists right away, so the sidebar has somewhere to put pages.
      await createWorkspace(name, t("onboarding.defaultSpaceName"));
      // On success the onboarding moves on to the recovery kit and this step unmounts.
    } catch (failure) {
      setError(describeCreateError(failure));
      setSubmitting(false);
    }
  }

  return (
    <section className="card" aria-labelledby="onboarding-title">
      <h1 className="title" id="onboarding-title">
        {t("onboarding.title")}
      </h1>
      <p className="muted">{t("onboarding.intro")}</p>
      <form
        className="form"
        noValidate
        onSubmit={(event) => {
          void submit(event);
        }}
      >
        <label htmlFor={inputId}>{t("onboarding.nameLabel")}</label>
        <input
          id={inputId}
          className="input"
          type="text"
          value={name}
          placeholder={t("onboarding.namePlaceholder")}
          autoComplete="off"
          autoFocus
          aria-invalid={error !== null}
          aria-describedby={error !== null ? errorId : undefined}
          onChange={(event) => {
            setName(event.target.value);
            setError(null);
          }}
        />
        {error !== null && (
          <p className="error-text" id={errorId} role="alert">
            {error}
          </p>
        )}
        <button type="submit" className="button button-primary" disabled={submitting}>
          {submitting ? t("onboarding.submitting") : t("onboarding.submit")}
        </button>
      </form>
      <RestoreFromBackup />
      <OtherWays onChoose={onChoose} />
    </section>
  );
}

/**
 * Step 2, right after the workspace exists: the recovery kit (create, write down, check).
 * "Do this later" goes to the app without one; Home and Settings then remind of it.
 */
function KitStep() {
  const finishOnboarding = useAppStore((state) => state.finishOnboarding);
  const titleId = useId();
  return (
    <main className="center-screen">
      <section className="card card-wide" aria-labelledby={titleId}>
        <h1 className="title" id={titleId}>
          {t("kit.title")}
        </h1>
        <RecoveryKitFlow onSkip={finishOnboarding} onDone={finishOnboarding} />
      </section>
    </main>
  );
}

/**
 * First run: the workspace, then its recovery kit; or this device joins an account (pairing)
 * or recovers one (the kit and the sync server). A pairing or recovery under way when the
 * app started opens its flow again.
 */
export function OnboardingScreen() {
  const step = useAppStore((state) => state.onboardingStep);
  const resumed = useAppStore((state): Way => {
    if (pairingUnderWay(state.pairing, "Joiner") !== null) {
      return "join";
    }
    return recoveryUnderWay(state.recovery) !== null ? "recover" : "create";
  });
  const [chosen, setChosen] = useState<Way | null>(null);
  const way = chosen ?? resumed;

  if (step === "kit") {
    return <KitStep />;
  }
  const back = (): void => {
    setChosen("create");
  };
  return (
    <main className="center-screen">
      {way === "join" && <JoinAccount onBack={back} />}
      {way === "recover" && <RecoverAccount onBack={back} />}
      {way === "create" && <WorkspaceStep onChoose={setChosen} />}
    </main>
  );
}
