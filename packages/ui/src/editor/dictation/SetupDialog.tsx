// The first-use dialog of dictation (ADR-0053, `docs/architecture/dictation.md` section 6): `/microphone` with no engine
// chosen on this device, or one that is not installed, asks which engine to use. The engines are listed as in Settings
// ("Accurate" first and recommended, with size, languages and, for Ollama, why it cannot be used); a downloadable one
// shows "Download 671 MB". The button chooses the engine, downloads its model (the shell's own dialog asks first: this
// one cannot fake it), shows the progress and "Verifying…", and starts dictating when the model is in. Cancel in the
// shell's dialog returns to the list with nothing said; an installed engine starts at once with no dialog at all.

import type { EngineStatus, SpeechEngine } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";

import { Modal } from "../../components/Modal";
import { t } from "../../i18n";
import { useStoreApi } from "../../store-context";
import { useObservable } from "../ai/observable";
import { openAiSettings, useDictationApi } from "./api";
import type { DictationController } from "./controller";
import { EngineOption } from "./EngineOption";
import { engineOf, isReady, setupOf } from "./setup";
import { formatPercent, formatSize, percentOf } from "./text";

/**
 * The engine the list starts on: the one chosen on this device if it can run, else the first model that is installed (no
 * download), else the recommended one, else the first that can run at all. Ollama is never the default: it is the person's
 * own set-up, chosen on purpose.
 */
function initialChoice(engines: readonly EngineStatus[], selected: SpeechEngine | null): SpeechEngine | null {
  const runnable = engines.filter((engine) => engine.available);
  return (
    runnable.find((engine) => engine.engine === selected)?.engine ??
    runnable.find((engine) => engine.engine !== "ollama" && isReady(engine))?.engine ??
    runnable.find((engine) => engine.engine === "accurate")?.engine ??
    runnable[0]?.engine ??
    null
  );
}

export function SetupDialog({ controller }: { controller: DictationController }) {
  const phase = useObservable(controller.state).phase;
  return phase === "setup" ? <SetupDialogBody controller={controller} /> : null;
}

function SetupDialogBody({ controller }: { controller: DictationController }) {
  const api = useDictationApi();
  const store = useStoreApi();
  const titleId = useId();
  const introId = useId();
  const [chosen, setChosen] = useState<SpeechEngine | null>(null);
  // Without a dictation there is nothing to ask (the controller does not get here without one).
  const setup = api === null ? null : setupOf(api);

  useEffect(() => {
    if (setup === null) {
      return;
    }
    void setup.refresh();
    return () => {
      // A download that is under way when the dialog goes (the page closed under it) is not left running unseen.
      if (setup.state.get().install !== null) {
        setup.cancelInstall();
      }
      setup.clearNotice();
    };
  }, [setup]);

  if (setup === null) {
    return null;
  }
  return <Chooser controller={controller} setup={setup} chosen={chosen} setChosen={setChosen} titleId={titleId} introId={introId} store={store} />;
}

interface ChooserProps {
  controller: DictationController;
  setup: ReturnType<typeof setupOf>;
  chosen: SpeechEngine | null;
  setChosen: (engine: SpeechEngine) => void;
  titleId: string;
  introId: string;
  store: ReturnType<typeof useStoreApi>;
}

function Chooser({ controller, setup, chosen, setChosen, titleId, introId, store }: ChooserProps) {
  const model = useObservable(setup.state);
  const group = useId();
  const engines = model.status?.engines ?? [];
  const choice = chosen ?? initialChoice(engines, model.status?.selected ?? null);
  const picked = choice === null ? undefined : engineOf(model.status, choice);
  const busy = model.install !== null || model.working;
  const ready = isReady(picked);
  const canGo = picked !== undefined && picked.available && !busy;

  async function go(): Promise<void> {
    if (picked === undefined) {
      return;
    }
    const engine = picked.engine;
    setup.clearNotice();
    if (!(await setup.select(engine))) {
      return;
    }
    if (!isReady(engineOf(setup.state.get().status, engine))) {
      // The shell's own dialog asks first; a "Cancel" there, or ours, leaves the list as it was.
      if ((await setup.install(engine)) !== "done") {
        return;
      }
    }
    await controller.proceed(engine);
  }

  let progress: string | null = null;
  if (model.install !== null) {
    if (model.install.phase === "verifying") {
      progress = t("dictation.download.verifying");
    } else if (model.install.phase === "starting" || model.install.total === 0) {
      progress = t("dictation.download.waiting");
    } else {
      progress = t("dictation.download.percent", { percent: formatPercent(percentOf(model.install.done, model.install.total)) });
    }
  }

  return (
    <Modal
      labelledBy={titleId}
      describedBy={introId}
      className="dictation-setup"
      {...(busy
        ? {}
        : {
            onDismiss: () => {
              controller.dismissSetup();
            },
          })}
    >
      <h2 className="subtitle" id={titleId}>
        {t("dictation.setup.title")}
      </h2>
      <p className="muted" id={introId}>
        {t("dictation.setup.intro")}
      </p>
      {model.status === null && <p role="status">{model.loading ? t("settings.dictation.loading") : ""}</p>}
      {engines.length > 0 && (
        <ul className="dictation-options" aria-label={t("dictation.setup.engines")}>
          {engines.map((engine) => (
            <EngineOption
              key={engine.engine}
              engine={engine}
              group={group}
              checked={choice === engine.engine}
              disabled={busy}
              onSelect={setChosen}
            >
              {!engine.available && engine.unavailableReason !== undefined && engine.unavailableReason !== "engineMissing" && (
                <button
                  type="button"
                  className="link-button dictation-ai-link"
                  onClick={() => {
                    controller.dismissSetup();
                    openAiSettings(store);
                  }}
                >
                  {t("dictation.openAiSettings")}
                </button>
              )}
            </EngineOption>
          ))}
        </ul>
      )}
      {model.install !== null && (
        <div className="dictation-progress">
          <progress
            max={model.install.total > 0 ? model.install.total : undefined}
            value={model.install.phase === "downloading" ? model.install.done : undefined}
            aria-label={t("dictation.download.progress")}
          />
          <p role="status">{progress}</p>
        </div>
      )}
      {model.notice !== null && (
        <p className="error-text" role="alert">
          {model.notice}
        </p>
      )}
      <div className="dialog-actions">
        {model.install === null ? (
          <button
            type="button"
            className="button"
            onClick={() => {
              controller.dismissSetup();
            }}
          >
            {t("dialog.cancel")}
          </button>
        ) : (
          <button
            type="button"
            className="button"
            disabled={model.install.phase === "verifying"}
            onClick={() => {
              setup.cancelInstall();
            }}
          >
            {t("dictation.setup.cancelDownload")}
          </button>
        )}
        <button
          type="button"
          className="button button-primary"
          disabled={!canGo}
          onClick={() => {
            void go();
          }}
        >
          {ready || picked === undefined || picked.engine === "ollama"
            ? t("dictation.setup.start")
            : t("dictation.setup.download", { size: formatSize(picked.downloadBytes) })}
        </button>
      </div>
    </Modal>
  );
}
