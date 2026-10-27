import type { ApiError, ErrorCode } from "@atomik-kb/core-client";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";

const HINTS: Partial<Record<ErrorCode, MessageKey>> = {
  KeystoreUnavailable: "fatal.hint.KeystoreUnavailable",
  Storage: "fatal.hint.Storage",
  Crypto: "fatal.hint.Crypto",
};

const UNKNOWN_ERROR: ApiError = { code: "Internal", message: t("fatal.unknown") };

/** Shown when the core cannot open or stops answering; there is nothing else to do here. */
export function FatalErrorScreen() {
  const error = useAppStore((state) => state.fatalError) ?? UNKNOWN_ERROR;
  const boot = useAppStore((state) => state.boot);

  return (
    <main className="center-screen">
      <section className="card" role="alert" aria-labelledby="fatal-title">
        <h1 className="title" id="fatal-title">
          {t("fatal.title")}
        </h1>
        <p>{t("fatal.body")}</p>
        <p>{t(HINTS[error.code] ?? "fatal.hint.default")}</p>
        <dl className="details">
          <dt>{t("fatal.code")}</dt>
          <dd>
            <code>{error.code}</code>
          </dd>
          <dt>{t("fatal.message")}</dt>
          <dd>{error.message}</dd>
        </dl>
        <button
          type="button"
          className="button"
          onClick={() => {
            void boot();
          }}
        >
          {t("fatal.retry")}
        </button>
      </section>
    </main>
  );
}
