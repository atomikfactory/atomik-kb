import { useState } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";
import { BackupExportResult, useBackupExport } from "./BackupExport";
import { SyncDisconnectDialog } from "./SyncDisconnectDialog";

/**
 * The warning on Home and in Settings while sync detected a fork or a rollback (the server's
 * history does not match this device's, ADR-0021): it stays until the fork is gone, and
 * offers what protects the notes: a backup (the existing export, where the shell can write
 * files) and turning sync off.
 */
export function SyncForkBanner() {
  const forked = useAppStore((state) => state.sync?.forkDetected === true);
  const canExport = useAppStore((state) => state.capabilities.backups);
  const backup = useBackupExport();
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);

  return (
    <>
      {forked && (
        <aside className="banner banner-warning sync-banner" aria-label={t("syncBanner.label")}>
          <p>{t("syncBanner.body")}</p>
          <div className="sync-banner-actions">
            {canExport && (
              <button
                type="button"
                className="button button-primary"
                disabled={backup.state.kind === "busy"}
                onClick={() => {
                  void backup.run();
                }}
              >
                {backup.state.kind === "busy" ? t("backupExport.working") : t("syncBanner.export")}
              </button>
            )}
            <button
              type="button"
              className="button"
              onClick={() => {
                setError(null);
                setConfirming(true);
              }}
            >
              {t("syncBanner.disconnect")}
            </button>
          </div>
          <BackupExportResult state={backup.state} className="sync-banner-result" />
          {error !== null && (
            <p className="sync-banner-result error-text" role="alert">
              {error}
            </p>
          )}
        </aside>
      )}
      {/* Stays up while disconnecting, although the banner goes once the fork is gone. */}
      {confirming && (
        <SyncDisconnectDialog
          onClose={() => {
            setConfirming(false);
          }}
          onError={setError}
        />
      )}
    </>
  );
}
