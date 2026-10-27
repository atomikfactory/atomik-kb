import { t } from "../i18n";

export function LoadingScreen() {
  return (
    <main className="center-screen" aria-busy="true">
      <div className="card card-narrow" role="status">
        <span className="spinner" aria-hidden="true" />
        <h1 className="title">{t("loading.title")}</h1>
        <p className="muted">{t("loading.body")}</p>
      </div>
    </main>
  );
}
