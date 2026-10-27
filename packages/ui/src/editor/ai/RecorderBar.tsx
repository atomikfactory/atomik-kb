// The recorder bar (ADR-0037, `docs/architecture/ai.md` section 8.6): a bar at the bottom of the page view
// with the timer, a level meter, Pause or Resume, Stop and Cancel, the last two lines of the live
// transcript, and what it waits for after Stop ("Transcribing 3 of 42", "Saving", "Summarising"). The
// page stays usable while it is open.

import { createPortal } from "react-dom";

import { t } from "../../i18n";
import { useObservable } from "./observable";
import type { RecorderController } from "./recorder";
import { formatTimestamp } from "./text";

export function RecorderBar({ controller }: { controller: RecorderController }) {
  const state = useObservable(controller.state);
  if (state.phase === "idle" && state.notice === null) {
    return null;
  }
  const recording = state.phase === "recording" || state.phase === "paused";
  const finishing = state.phase === "finishing";

  let status = "";
  if (state.phase === "starting") {
    status = t("recorder.starting");
  } else if (finishing) {
    switch (state.step) {
      case "transcribing":
        status = t("transcribe.progress", { done: Math.min((state.progress?.done ?? 0) + 1, state.progress?.total ?? 1), total: state.progress?.total ?? 1 });
        break;
      case "summarising":
        status = t("transcribe.summarising");
        break;
      default:
        status = t("recorder.saving");
    }
  }

  return createPortal(
    <section className="recorder-bar" aria-label={t("recorder.label")}>
      {(recording || state.phase === "starting" || finishing) && (
        <div className="recorder-row">
          {recording && (
            <>
              <span className={state.phase === "paused" ? "recorder-dot is-paused" : "recorder-dot"} aria-hidden="true" />
              <span className="recorder-time" role="timer" aria-label={t("recorder.time")}>
                {formatTimestamp(state.seconds * 1000)}
              </span>
              <span
                className="recorder-meter"
                role="meter"
                aria-label={t("recorder.level")}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(state.level * 100)}
              >
                <span className="recorder-meter-fill" style={{ width: `${String(Math.round(state.level * 100))}%` }} />
              </span>
              <span className="recorder-state muted">
                {state.phase === "paused" ? t("recorder.paused") : t("recorder.recording")}
              </span>
              <button
                type="button"
                className="button"
                onClick={() => {
                  if (state.phase === "paused") {
                    controller.resume();
                  } else {
                    controller.pause();
                  }
                }}
              >
                {state.phase === "paused" ? t("recorder.resume") : t("recorder.pause")}
              </button>
              <button
                type="button"
                className="button button-primary"
                onClick={() => {
                  void controller.stop();
                }}
              >
                {t("recorder.stop")}
              </button>
            </>
          )}
          {!recording && (
            <span className="recorder-status" role="status">
              {status}
            </span>
          )}
          {state.phase === "starting" && state.permissionHint && (
            <p className="recorder-hint muted">{t("recorder.permissionHint")}</p>
          )}
          <button
            type="button"
            className="button"
            onClick={() => {
              controller.cancel();
            }}
          >
            {t("recorder.cancel")}
          </button>
        </div>
      )}
      {state.lines.length > 0 && (recording || finishing) && (
        <div className="recorder-lines muted" aria-label={t("recorder.transcript")} aria-live="off">
          {state.lines.map((line) => (
            <p key={line} className="recorder-line">
              {line}
            </p>
          ))}
        </div>
      )}
      {state.notice !== null && (
        <div className="recorder-row recorder-notice">
          <p className="recorder-notice-text" role="alert">
            {state.notice}
          </p>
          {state.canRetry && (
            <button
              type="button"
              className="button"
              onClick={() => {
                void controller.retry();
              }}
            >
              {t("recorder.retry")}
            </button>
          )}
          <button
            type="button"
            className="button"
            onClick={() => {
              controller.dismiss();
            }}
          >
            {t("recorder.dismiss")}
          </button>
        </div>
      )}
    </section>,
    document.body,
  );
}
