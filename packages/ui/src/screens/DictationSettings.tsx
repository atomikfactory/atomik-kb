// Settings -> Dictation (ADR-0053, `docs/architecture/dictation.md` section 6): the engine of this device as a radio
// ("Accurate" first, recommended), and for each engine its status, the download size, its languages (named by
// `Intl.DisplayNames` in the language of the app) and its credits, with Download (a progress bar and Cancel while it runs),
// Remove and "Use model files…" (a folder of the model for a computer that is offline). Ollama is disabled unless AI
// transcription is set up, and says which step is missing, with a link to the AI section. Shown only where the
// transport can dictate.
//
// The shell owns every file, the folder dialog and the consent dialog of a download; this screen asks and shows.

import type { EngineStatus, SpeechEngine } from "@atomik-kb/core-client";
import { useEffect, useId } from "react";

import { useObservable } from "../editor/ai/observable";
import { openAiSettings, useDictationApi } from "../editor/dictation/api";
import { EngineOption } from "../editor/dictation/EngineOption";
import { setupOf } from "../editor/dictation/setup";
import type { DictationSetup } from "../editor/dictation/setup";
import { formatPercent, percentOf } from "../editor/dictation/text";
import { t } from "../i18n";
import { useStoreApi } from "../store-context";

/** What a download shows while it runs: the bar, a sentence and Cancel. */
function InstallProgress({ setup, engine }: { setup: DictationSetup; engine: SpeechEngine }) {
  const { install } = useObservable(setup.state);
  if (install === null || install.engine !== engine) {
    return null;
  }
  let text: string;
  if (install.phase === "verifying") {
    text = t("dictation.download.verifying");
  } else if (install.phase === "starting" || install.total === 0) {
    text = t("dictation.download.waiting");
  } else {
    text = t("dictation.download.percent", { percent: formatPercent(percentOf(install.done, install.total)) });
  }
  return (
    <div className="dictation-progress">
      <progress
        max={install.total > 0 ? install.total : undefined}
        value={install.phase === "downloading" ? install.done : undefined}
        aria-label={t("dictation.download.progress")}
      />
      <p role="status">{text}</p>
      <button
        type="button"
        className="button"
        disabled={install.phase === "verifying"}
        onClick={() => {
          setup.cancelInstall();
        }}
      >
        {t("settings.dictation.cancel")}
      </button>
    </div>
  );
}

interface EngineExtrasProps {
  setup: DictationSetup;
  engine: EngineStatus;
  /** An installation, an import or a removal runs (the shell does one thing at a time). */
  busy: boolean;
  selected: SpeechEngine | null;
}

/** Under an engine: its credits, the way to the AI settings for Ollama, and the buttons of a downloadable model. */
function EngineExtras({ setup, engine, busy, selected }: EngineExtrasProps) {
  const store = useStoreApi();
  const { install } = useObservable(setup.state);
  const installing = install?.engine === engine.engine;

  async function download(): Promise<void> {
    const outcome = await setup.install(engine.engine);
    // A first model on a device with no choice yet becomes the choice: it is what the person just asked for.
    if (outcome === "done" && selected === null) {
      await setup.select(engine.engine);
    }
  }

  return (
    <div className="dictation-option-extras">
      {engine.attribution !== "" && (
        <p className="muted dictation-attribution">
          <span className="dictation-credits-label">{t("settings.dictation.credits")}</span> {engine.attribution}
        </p>
      )}
      {engine.engine === "ollama" && !engine.available && engine.unavailableReason !== "engineMissing" && (
        <button
          type="button"
          className="link-button dictation-ai-link"
          onClick={() => {
            openAiSettings(store);
          }}
        >
          {t("dictation.openAiSettings")}
        </button>
      )}
      {engine.engine !== "ollama" && engine.available && (
        <>
          <InstallProgress setup={setup} engine={engine.engine} />
          {!installing && (
            <div className="row-actions dictation-actions">
              {engine.installed ? (
                <button
                  type="button"
                  className="button"
                  disabled={busy}
                  onClick={() => {
                    void setup.remove(engine.engine);
                  }}
                >
                  {t("settings.dictation.remove")}
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    className="button"
                    disabled={busy}
                    onClick={() => {
                      void download();
                    }}
                  >
                    {t("settings.dictation.download")}
                  </button>
                  <button
                    type="button"
                    className="button"
                    disabled={busy}
                    onClick={() => {
                      void setup.importModel(engine.engine);
                    }}
                  >
                    {t("settings.dictation.useFiles")}
                  </button>
                </>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** Settings -> Dictation. */
export function DictationSettings() {
  const api = useDictationApi();
  const setup = api === null ? null : setupOf(api);
  const titleId = useId();
  const group = useId();

  useEffect(() => {
    void setup?.refresh();
  }, [setup]);

  if (setup === null) {
    return null;
  }
  return <Engines setup={setup} titleId={titleId} group={group} />;
}

function Engines({ setup, titleId, group }: { setup: DictationSetup; titleId: string; group: string }) {
  const model = useObservable(setup.state);
  const engines = model.status?.engines ?? [];
  const busy = model.install !== null || model.working;
  const selected = model.status?.selected ?? null;
  return (
    <section className="subpanel dictation-settings" aria-labelledby={titleId}>
      <h3 className="subheading" id={titleId}>
        {t("settings.dictation.engine.title")}
      </h3>
      <p className="muted">{t("settings.dictation.intro")}</p>
      {model.status === null && (
        <p className="muted" role="status">
          {model.loading ? t("settings.dictation.loading") : ""}
        </p>
      )}
      {engines.length > 0 && (
        <ul className="dictation-options" role="radiogroup" aria-labelledby={titleId}>
          {engines.map((engine) => (
            <EngineOption
              key={engine.engine}
              engine={engine}
              group={group}
              checked={selected === engine.engine}
              disabled={busy}
              onSelect={(choice) => {
                void setup.select(choice);
              }}
            >
              <EngineExtras setup={setup} engine={engine} busy={busy} selected={selected} />
            </EngineOption>
          ))}
        </ul>
      )}
      {model.notice !== null && (
        <p className="error-text" role="alert">
          {model.notice}
        </p>
      )}
    </section>
  );
}
