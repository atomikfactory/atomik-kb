// The dictation bar (ADR-0053, `docs/architecture/dictation.md` section 6): a floating bar like the recorder's with the
// microphone's level, a timer, what the engine is doing ("Loading the model…", "Listening", "Recognising…"), the name of
// the engine and Stop. Esc stops too. A denied microphone and a failure of the engine are sentences in the bar until
// they are dismissed; the hint about the permission window and the denied text are the recorder's.

import { createPortal } from "react-dom";

import { t } from "../../i18n";
import type { MessageKey } from "../../i18n";
import { useObservable } from "../ai/observable";
import { formatTimestamp } from "../ai/text";
import type { DictationController, DictationPhase } from "./controller";
import { engineName } from "./text";

/** What the bar says of each phase in which the microphone is on or on its way. */
const STATUS_KEYS: Partial<Record<DictationPhase, MessageKey>> = {
  starting: "dictation.starting",
  loading: "dictation.loading",
  listening: "dictation.listening",
  recognising: "dictation.recognising",
  stopping: "dictation.stopping",
};

export function DictationBar({ controller }: { controller: DictationController }) {
  const state = useObservable(controller.state);
  const statusKey = STATUS_KEYS[state.phase];
  if (statusKey === undefined && state.notice === null) {
    return null;
  }
  const heard = state.phase === "loading" || state.phase === "listening" || state.phase === "recognising" || state.phase === "stopping";

  return createPortal(
    <section className="recorder-bar dictation-bar" aria-label={t("dictation.label")}>
      {statusKey !== undefined && (
        <div className="recorder-row">
          {heard && (
            <>
              <span className={state.phase === "stopping" ? "recorder-dot is-paused" : "recorder-dot"} aria-hidden="true" />
              <span className="recorder-time" role="timer" aria-label={t("dictation.time")}>
                {formatTimestamp(state.seconds * 1000)}
              </span>
              <span
                className="recorder-meter"
                role="meter"
                aria-label={t("dictation.level")}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(state.level * 100)}
              >
                <span className="recorder-meter-fill" style={{ width: `${String(Math.round(state.level * 100))}%` }} />
              </span>
            </>
          )}
          <span className="recorder-state muted" role="status">
            {t(statusKey)}
          </span>
          {state.engine !== null && <span className="dictation-engine muted">{engineName(state.engine)}</span>}
          <button
            type="button"
            className="button button-primary"
            disabled={state.phase === "stopping"}
            onClick={() => {
              void controller.stop();
            }}
          >
            {t("dictation.stop")}
          </button>
          {state.phase === "starting" && state.permissionHint && <p className="recorder-hint muted">{t("recorder.permissionHint")}</p>}
        </div>
      )}
      {state.notice !== null && (
        <div className="recorder-row recorder-notice">
          <p className="recorder-notice-text" role="alert">
            {state.notice}
          </p>
          <button
            type="button"
            className="button"
            onClick={() => {
              controller.dismiss();
            }}
          >
            {t("dictation.dismiss")}
          </button>
        </div>
      )}
    </section>,
    document.body,
  );
}
