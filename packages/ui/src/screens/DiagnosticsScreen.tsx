import { API_VERSION, toCoreError } from "@atomik-kb/core-client";
import { useState } from "react";

import { t } from "../i18n";
import { useAppStore } from "../store-context";

/** Everything the core reports in `AppStatus`, plus a warning when the API versions differ. */
export function DiagnosticsScreen() {
  const status = useAppStore((state) => state.status);
  const refreshStatus = useAppStore((state) => state.refreshStatus);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function refresh(): void {
    setRefreshing(true);
    setError(null);
    refreshStatus()
      .catch((failure: unknown) => {
        setError(t("diagnostics.refreshError", { message: toCoreError(failure).message }));
      })
      .finally(() => {
        setRefreshing(false);
      });
  }

  return (
    <section aria-labelledby="diagnostics-title">
      <h1 className="title" id="diagnostics-title">
        {t("diagnostics.title")}
      </h1>
      <p className="muted">{t("diagnostics.intro")}</p>

      {status !== null && status.apiVersion !== API_VERSION && (
        <p className="banner banner-warning" role="alert">
          {t("diagnostics.warning.apiVersion", { expected: API_VERSION, actual: status.apiVersion })}
        </p>
      )}

      {status !== null && (
        <dl className="facts">
          <dt>{t("diagnostics.field.workspace")}</dt>
          <dd>{status.workspaceInitialized ? t("common.yes") : t("common.no")}</dd>
          <dt>{t("diagnostics.field.schemaVersion")}</dt>
          <dd>{status.schemaVersion}</dd>
          <dt>{t("diagnostics.field.encrypted")}</dt>
          <dd>{status.dbEncrypted ? t("common.yes") : t("common.no")}</dd>
          <dt>{t("diagnostics.field.cipher")}</dt>
          <dd>{status.dbCipher}</dd>
          <dt>{t("diagnostics.field.deviceId")}</dt>
          <dd className="mono">{status.deviceId}</dd>
          <dt>{t("diagnostics.field.coreVersion")}</dt>
          <dd>{status.coreVersion}</dd>
          <dt>{t("diagnostics.field.apiVersion")}</dt>
          <dd>{status.apiVersion}</dd>
          <dt>{t("diagnostics.field.platform")}</dt>
          <dd>{status.platform}</dd>
          <dt>{t("diagnostics.field.dataDir")}</dt>
          <dd className="mono">{status.dataDir}</dd>
        </dl>
      )}

      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}

      <button type="button" className="button" disabled={refreshing} onClick={refresh}>
        {t("diagnostics.refresh")}
      </button>
    </section>
  );
}
