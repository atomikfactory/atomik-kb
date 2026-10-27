// "Restore from a backup" on the first onboarding screen (ADR-0017, ADR-0059): the button opens the restore wizard
// (`RestoreWizard.tsx`), where the person picks a backup file, a folder of backups or Google Drive, and the core restores the
// backup into this empty installation; the app then starts again with the restored workspace. A backup of another account needs
// its recovery kit: the core answers `InvalidInput` for the field `kit` and the kit dialog (`RestoreKit.tsx`) asks for it.

import { useRef, useState } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { RestoreWizard } from "./RestoreWizard";

// The kit dialog and its live hint live in `RestoreKit.tsx`; "Recover with my kit" imports them from here.
export { KitHint, RestoreKitDialog } from "./RestoreKit";
export type { RestoreKitDialogProps } from "./RestoreKit";

/**
 * The way in for someone who already has a workspace elsewhere: the button opens the wizard that picks the backup (a file, a
 * folder, Google Drive) and asks for the kit when the backup needs it. Not offered where the shell has no backup files.
 */
export function RestoreFromBackup() {
  const available = useAppStore((state) => state.capabilities.backups);
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);

  if (!available) {
    return null;
  }

  return (
    <div className="onboarding-restore">
      <p className="muted">{t("onboarding.restore.intro")}</p>
      <button
        ref={button}
        type="button"
        className="button"
        aria-haspopup="dialog"
        onClick={() => {
          setOpen(true);
        }}
      >
        {t("onboarding.restore.button")}
      </button>
      {open && (
        <RestoreWizard
          returnFocus={button}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </div>
  );
}
