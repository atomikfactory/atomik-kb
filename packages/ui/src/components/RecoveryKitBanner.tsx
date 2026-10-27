import { useState } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { RecoveryKitDialog } from "./RecoveryKit";

/**
 * The reminder on Home and in Settings while the workspace has no recovery kit (the user
 * chose "Do this later"): its button runs the same kit flow as the onboarding, in a dialog.
 */
export function RecoveryKitBanner() {
  const missing = useAppStore((state) => state.security !== null && state.security.recoveryKit === undefined);
  const [open, setOpen] = useState(false);

  return (
    <>
      {missing && (
        <aside className="banner banner-warning kit-banner" aria-label={t("kitBanner.label")}>
          <p>{t("kitBanner.body")}</p>
          <button
            type="button"
            className="button button-primary"
            onClick={() => {
              setOpen(true);
            }}
          >
            {t("kitBanner.action")}
          </button>
        </aside>
      )}
      {/* Stays up when the kit now exists and the banner went: it is showing that kit. */}
      {open && (
        <RecoveryKitDialog
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </>
  );
}
