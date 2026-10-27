// Settings -> Appearance -> Language (ADR-0040 item 13): "Match system" and one choice per language, each written in
// its own language (English, Français, Español) whatever language is active, so that a person who landed in a
// language they do not read can still find theirs. The choice is the device setting `ui.language`; `system` (the
// default) is the setting's absence or the text `system`. Choosing one loads its file and draws the app again in it
// (`setSetting` in the store), and the shell gets the tray's, the menu bar's and the dialogs' words in it.

import { toCoreError } from "@atomik-kb/core-client";
import { useState } from "react";

import { LANGUAGES, LANGUAGE_KEY, LANGUAGE_NAMES, t } from "../i18n";
import type { Language, LanguageChoice } from "../i18n";
import { selectLanguageChoice } from "../store";
import { useAppStore } from "../store-context";

export function LanguagePicker() {
  const choice = useAppStore(selectLanguageChoice);
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);

  function choose(next: LanguageChoice): void {
    if (next === choice) {
      return;
    }
    setError(null);
    setSetting(LANGUAGE_KEY, next).catch((failure: unknown) => {
      setError(t("settings.language.error", { message: toCoreError(failure).message }));
    });
  }

  const options: readonly { value: LanguageChoice; label: string; language?: Language }[] = [
    { value: "system", label: t("settings.language.system") },
    ...LANGUAGES.map((language) => ({ value: language, label: LANGUAGE_NAMES[language], language })),
  ];

  return (
    <fieldset className="fieldset">
      <legend>{t("settings.language.legend")}</legend>
      <p className="muted">{t("settings.language.hint")}</p>
      <div className="radio-group">
        {options.map(({ value, label, language }) => (
          <label className="radio" key={value}>
            <input
              type="radio"
              name="language"
              value={value}
              checked={choice === value}
              onChange={() => {
                choose(value);
              }}
            />
            <span lang={language}>{label}</span>
          </label>
        ))}
      </div>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}
