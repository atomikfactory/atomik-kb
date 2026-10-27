import { useId } from "react";

import { BackupExportResult, useBackupExport } from "../components/BackupExport";
import { t } from "../i18n";
import { useAppStore } from "../store-context";

/**
 * Instead of the app once this device learnt it was removed from the account (ADR-0022):
 * the notes stay here and sync is off; a backup is the way to take them elsewhere.
 */
export function RevokedScreen() {
  const canExport = useAppStore((state) => state.capabilities.backups);
  const backup = useBackupExport();
  const titleId = useId();

  return (
    <main className="center-screen">
      <section className="card revoked-card" aria-labelledby={titleId}>
        <h1 className="title" id={titleId}>
          {t("revoked.title")}
        </h1>
        <p className="lead">{t("revoked.body")}</p>
        {canExport && (
          <>
            <p className="muted">{t("revoked.backup")}</p>
            <div className="row-actions">
              <button
                type="button"
                className="button button-primary"
                disabled={backup.state.kind === "busy"}
                onClick={() => {
                  void backup.run();
                }}
              >
                {backup.state.kind === "busy" ? t("backupExport.working") : t("revoked.export")}
              </button>
            </div>
            <BackupExportResult state={backup.state} className="revoked-result" />
          </>
        )}
      </section>
    </main>
  );
}
