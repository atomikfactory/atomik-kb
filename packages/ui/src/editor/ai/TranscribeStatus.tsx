// What an audio or video block shows while its file is transcribed (ADR-0037, `docs/architecture/ai.md`
// section 8.6): "Transcribing 3 of 42" with a bar and a Cancel; afterwards, the message of a failure
// until it is dismissed. It reads the editor's jobs (EditorAi.tsx) by the block's id.

import { t } from "../../i18n";
import { useEditorAi } from "./EditorAi";
import { useObservable } from "./observable";
import type { JobView } from "./transcribe-jobs";
import type { TranscribeJobs } from "./transcribe-jobs";

/** The line a running job shows. */
export function describeJob(view: Extract<JobView, { status: "running" }>): string {
  switch (view.step) {
    case "reading":
      return t("transcribe.reading", { percent: Math.floor((view.done / Math.max(1, view.total)) * 100) });
    case "decoding":
      return t("transcribe.decoding");
    case "transcribing":
      return t("transcribe.progress", { done: Math.min(view.done + 1, Math.max(1, view.total)), total: Math.max(1, view.total) });
    case "summarising":
      return t("transcribe.summarising");
  }
}

export function TranscribeStatus({ blockId }: { blockId: string | null }) {
  const ai = useEditorAi();
  if (ai === null || blockId === null) {
    return null;
  }
  return <Status jobs={ai.jobs} blockId={blockId} />;
}

function Status({ jobs, blockId }: { jobs: TranscribeJobs; blockId: string }) {
  const views = useObservable(jobs.views);
  const view = views.get(blockId);
  if (view === undefined) {
    return null;
  }
  if (view.status === "error") {
    return (
      <div className="transcribe-status is-error" contentEditable={false}>
        <p className="transcribe-status-text error-text" role="alert">
          {view.message}
        </p>
        <button
          type="button"
          className="button"
          onClick={() => {
            jobs.dismiss(blockId);
          }}
        >
          {t("transcribe.dismiss")}
        </button>
      </div>
    );
  }
  const determinate = view.step === "transcribing" || view.step === "reading";
  return (
    <div className="transcribe-status" contentEditable={false}>
      <p className="transcribe-status-text" role="status">
        {describeJob(view)}
      </p>
      {determinate ? (
        <progress
          className="transcribe-status-bar"
          max={Math.max(1, view.total)}
          value={Math.min(view.done, Math.max(1, view.total))}
          aria-label={describeJob(view)}
        />
      ) : (
        <progress className="transcribe-status-bar" aria-label={describeJob(view)} />
      )}
      <button
        type="button"
        className="button"
        onClick={() => {
          jobs.cancel(blockId);
        }}
      >
        {t("transcribe.cancel")}
      </button>
    </div>
  );
}
