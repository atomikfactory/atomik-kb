// The device banners of Milestone 7 (ADR-0022), on Home and in Settings: a key rotation made
// on another device that waits for the app-lock passphrase here, and the reminder to
// re-issue the kit after this device joined with it.

import { toCoreError } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { ReissueKitDialog } from "../screens/SecuritySettings";
import type { AppState } from "../store";
import { useAppStore } from "../store-context";

/**
 * Another device rotated the keys (it removed a device) and the app lock keeps this one from
 * storing the new root key until the passphrase is typed: the passphrase, then "Finish".
 */
export function RotationBanner() {
  const pending = useAppStore((state) => state.security?.rotationPending === true);
  const lockEnabled = useAppStore((state) => state.security?.appLock.enabled === true);
  const applyRotation = useAppStore((state) => state.applyRotation);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const fieldId = useId();
  const bodyId = useId();
  const errorId = useId();

  if (!pending) {
    return null;
  }

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (lockEnabled && passphrase === "") {
      setError(t("rotationBanner.error.empty"));
      return;
    }
    const typed = passphrase;
    setPassphrase("");
    setError(null);
    setBusy(true);
    try {
      await applyRotation(lockEnabled ? typed : undefined);
      // Done: the status no longer says pending, and the banner goes.
    } catch (failure) {
      const reason = toCoreError(failure);
      setError(
        isFieldError(reason, "passphrase")
          ? t("rotationBanner.error.wrong")
          : t("rotationBanner.error.generic", { message: reason.message }),
      );
    }
    setBusy(false);
  }

  return (
    <aside className="banner banner-warning device-banner" aria-label={t("rotationBanner.label")}>
      <p id={bodyId}>{t("rotationBanner.body")}</p>
      <form
        className="device-banner-form"
        noValidate
        onSubmit={(event) => {
          void submit(event);
        }}
      >
        {lockEnabled && (
          <div className="field">
            <label htmlFor={fieldId}>{t("rotationBanner.passphraseLabel")}</label>
            <input
              id={fieldId}
              className="input"
              type="password"
              value={passphrase}
              autoComplete="current-password"
              readOnly={busy}
              aria-invalid={error !== null}
              aria-describedby={[bodyId, error !== null ? errorId : undefined].filter((id) => id !== undefined).join(" ")}
              onChange={(event) => {
                setPassphrase(event.target.value);
                setError(null);
              }}
            />
          </div>
        )}
        <button type="submit" className="button button-primary" disabled={busy}>
          {busy ? t("rotationBanner.working") : t("rotationBanner.submit")}
        </button>
      </form>
      {error !== null && (
        <p className="error-text device-banner-error" id={errorId} role="alert">
          {error}
        </p>
      )}
    </aside>
  );
}

/**
 * True while the core says the kit should be re-issued (`RecoveryKitInfo.stale`: it was typed
 * on this device for a recovery or a restore, or a rotation was interrupted; ADR-0022: the UI
 * insists on a re-issue). A core or mirror that leaves the flag out falls back to how this
 * device joined: added with the kit, and the kit not re-issued since.
 */
export function selectKitToReissue(state: AppState): boolean {
  const kit = state.security?.recoveryKit;
  if (kit === undefined || state.security?.thisDeviceRevoked === true) {
    return false;
  }
  const stale: boolean | undefined = (kit as { stale?: boolean }).stale;
  if (stale !== undefined) {
    return stale;
  }
  const me = state.devices.find((device) => device.isThisDevice);
  if (me === undefined || !me.addedViaKit) {
    return false;
  }
  return kit.reissuedAt === undefined || Number(kit.reissuedAt) < Number(me.addedAt);
}

/** Device-local (`ui.*`, never synced): the user chose to keep the kit and hide the reminder here. */
export const KIT_REMINDER_DISMISSED_KEY = "ui.kit-reminder-dismissed";

/**
 * The reminder to re-issue the kit after joining with it; its button opens the re-issue
 * dialog, "Dismiss" hides it on this device (re-issuing on another device would only bring
 * the reminder back there, so it cannot be a one-way street).
 */
export function StaleKitBanner() {
  const stale = useAppStore(
    (state) => selectKitToReissue(state) && state.settings[KIT_REMINDER_DISMISSED_KEY] !== true,
  );
  const setSetting = useAppStore((state) => state.setSetting);
  const lockEnabled = useAppStore((state) => state.security?.appLock.enabled === true);
  const [open, setOpen] = useState(false);

  return (
    <>
      {stale && (
        <aside className="banner banner-warning kit-banner" aria-label={t("staleKitBanner.label")}>
          <p>{t("staleKitBanner.body")}</p>
          <button
            type="button"
            className="button button-primary"
            onClick={() => {
              setOpen(true);
            }}
          >
            {t("staleKitBanner.action")}
          </button>
          <button
            type="button"
            className="button"
            onClick={() => {
              setSetting(KIT_REMINDER_DISMISSED_KEY, true).catch((failure: unknown) => {
                console.warn("could not remember the dismissed kit reminder", failure);
              });
            }}
          >
            {t("staleKitBanner.dismiss")}
          </button>
        </aside>
      )}
      {/* Stays up once the banner went: it is showing the new kit. */}
      {open && (
        <ReissueKitDialog
          lockEnabled={lockEnabled}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </>
  );
}
