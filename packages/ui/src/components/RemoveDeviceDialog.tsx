// "Remove..." next to a device in Settings -> Security -> Devices (ADR-0022): the warning
// (keys rotate, a new kit replaces the old one, the removed device keeps what it has), the
// app-lock passphrase while the lock is on, then the new recovery kit, shown and checked
// like any kit. The passphrase and the kit stay in this dialog's state.

import { toCoreError } from "@atomik-kb/core-client";
import type { DeviceEntry, RecoveryKit } from "@atomik-kb/core-client";
import { useId, useState } from "react";
import type { FormEvent } from "react";

import { isFieldError } from "../errors";
import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { Modal } from "./Modal";
import { RecoveryKitFlow } from "./RecoveryKit";

export function RemoveDeviceDialog({
  device,
  lockEnabled,
  onClose,
}: {
  device: DeviceEntry;
  lockEnabled: boolean;
  onClose: () => void;
}) {
  const revokeDevice = useAppStore((state) => state.revokeDevice);
  const [kit, setKit] = useState<RecoveryKit | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const titleId = useId();
  const bodyId = useId();
  const fieldId = useId();
  const errorId = useId();

  async function submit(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) {
      return;
    }
    if (lockEnabled && passphrase === "") {
      setError(t("removeDevice.error.empty"));
      return;
    }
    const typed = passphrase;
    setPassphrase("");
    setError(null);
    setBusy(true);
    try {
      setKit(await revokeDevice(device.deviceId, lockEnabled ? typed : undefined));
    } catch (failure) {
      const reason = toCoreError(failure);
      setError(
        isFieldError(reason, "passphrase")
          ? t("removeDevice.error.wrong")
          : t("removeDevice.error.generic", { message: reason.message }),
      );
      setBusy(false);
    }
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={kit === null ? bodyId : undefined}
      onDismiss={kit !== null || busy ? undefined : onClose}
      className={kit === null ? undefined : "kit-dialog"}
    >
      <h2 className="subtitle" id={titleId}>
        {kit === null ? t("removeDevice.title", { name: device.name }) : t("removeDevice.newKitTitle")}
      </h2>
      {kit === null ? (
        <form
          className="form reissue-form"
          noValidate
          onSubmit={(event) => {
            void submit(event);
          }}
        >
          <p id={bodyId}>{t("removeDevice.body")}</p>
          {lockEnabled && (
            <>
              <label htmlFor={fieldId}>{t("removeDevice.passphraseLabel")}</label>
              <input
                id={fieldId}
                className="input"
                type="password"
                value={passphrase}
                autoComplete="current-password"
                readOnly={busy}
                aria-invalid={error !== null}
                aria-describedby={error !== null ? errorId : undefined}
                onChange={(event) => {
                  setPassphrase(event.target.value);
                  setError(null);
                }}
              />
            </>
          )}
          {error !== null && (
            <p className="error-text" id={errorId} role="alert">
              {error}
            </p>
          )}
          <div className="dialog-actions">
            <button type="button" className="button" disabled={busy} onClick={onClose}>
              {t("dialog.cancel")}
            </button>
            <button type="submit" className="button button-danger" disabled={busy}>
              {busy ? t("removeDevice.working") : t("removeDevice.confirm")}
            </button>
          </div>
        </form>
      ) : (
        <RecoveryKitFlow initialKit={kit} onDone={onClose} />
      )}
    </Modal>
  );
}
