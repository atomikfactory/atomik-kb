// One engine of dictation as a radio with its facts (ADR-0053, `docs/architecture/dictation.md` section 6): the name
// ("Accurate" first, marked recommended), what it is, whether it is installed, how much it downloads, the languages it
// knows, and, for an engine that cannot be used, why. Settings and the first-use dialog list the engines with it; the
// parts that differ (download, remove, import) come in as children.

import type { EngineStatus } from "@atomik-kb/core-client";
import { useId } from "react";
import type { ReactNode } from "react";

import { t } from "../../i18n";
import { ENGINE_ABOUT_KEYS, engineName, formatSize, languageList, UNAVAILABLE_KEYS } from "./text";

export interface EngineOptionProps {
  engine: EngineStatus;
  /** The radio group's name. */
  group: string;
  checked: boolean;
  /** The radio cannot be changed now (an installation runs). An engine that is not available is disabled anyway. */
  disabled?: boolean;
  onSelect: (engine: EngineStatus["engine"]) => void;
  /** What goes under the facts: reasons with links, buttons, a progress bar. */
  children?: ReactNode;
}

/** The status line of an engine: "Installed", "Not installed", "Ready" (Ollama) or why it cannot be used. */
export function engineStatusText(engine: EngineStatus): string {
  if (!engine.available) {
    return t(UNAVAILABLE_KEYS[engine.unavailableReason ?? "engineMissing"]);
  }
  if (engine.engine === "ollama") {
    return t("dictation.engine.ollama.ready");
  }
  return engine.installed ? t("dictation.engine.installed") : t("dictation.engine.notInstalled");
}

export function EngineOption({ engine, group, checked, disabled = false, onSelect, children }: EngineOptionProps) {
  const titleId = useId();
  const factsId = useId();
  const downloadable = engine.engine !== "ollama";
  return (
    <li className={checked ? "dictation-option is-checked" : "dictation-option"} data-engine={engine.engine}>
      <label className="dictation-option-label">
        <input
          type="radio"
          name={group}
          value={engine.engine}
          checked={checked}
          disabled={disabled || !engine.available}
          aria-labelledby={titleId}
          aria-describedby={factsId}
          onChange={() => {
            onSelect(engine.engine);
          }}
        />
        <span className="dictation-option-body">
          <span className="dictation-option-title" id={titleId}>
            {engineName(engine.engine)}
            {engine.engine === "accurate" && <span className="ai-badge ai-badge-local dictation-recommended">{t("dictation.recommended")}</span>}
          </span>
          <span className="muted">{t(ENGINE_ABOUT_KEYS[engine.engine])}</span>
          <span className="muted dictation-option-facts" id={factsId}>
            <span className="dictation-option-status">{engineStatusText(engine)}</span>
            {downloadable && engine.downloadBytes > 0 && <span>{t("dictation.size", { size: formatSize(engine.downloadBytes) })}</span>}
            <span>
              {engine.languages.length === 0
                ? t("dictation.languages.model")
                : t("dictation.languages", { languages: languageList(engine.languages) })}
            </span>
          </span>
        </span>
      </label>
      {children}
    </li>
  );
}
