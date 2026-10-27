// Settings -> Desktop (ADR-0040): what only the desktop shell has. "Keep running in the tray when the window is closed"
// (`app.tray.close_to_tray`) and the quick-capture shortcut (`app.shortcut.capture`), a key combination recorded in a
// field: focus it and press the keys. Both are device settings the shell follows as they change; the shell also says
// whether the operating system accepted the shortcut (`shortcutState`), and Settings shows it, because another program
// may already own the combination. The section is only shown with a desktop shell (not in the in-memory dev UI or the
// web build), see `SettingsScreen`.

import { toCoreError } from "@atomik-kb/core-client";
import type { ShortcutState } from "@atomik-kb/core-client";
import { useEffect, useId, useState } from "react";
import type { KeyboardEvent } from "react";

import { DEFAULT_CAPTURE_SHORTCUT, captureKey } from "../accelerator";
import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import { useAppStore } from "../store-context";
import { CAPTURE_SHORTCUT_KEY, CLOSE_TO_TRAY_KEY, parseCaptureShortcut, parseCloseToTray } from "../store-shell";

/** What Settings says about the registration the shell reported; `registered` and `disabled` are no problem. */
const STATE_TEXT: Readonly<Record<ShortcutState, { key: MessageKey; problem: boolean }>> = {
  registered: { key: "settings.desktop.shortcut.state.registered", problem: false },
  disabled: { key: "settings.desktop.shortcut.state.disabled", problem: false },
  taken: { key: "settings.desktop.shortcut.state.taken", problem: true },
  invalid: { key: "settings.desktop.shortcut.state.invalid", problem: true },
};

/** Reads the settings of this section when the screen opens (the store has them only once they were read). */
function useLoadedSettings(): void {
  const loadSetting = useAppStore((state) => state.loadSetting);
  useEffect(() => {
    for (const key of [CLOSE_TO_TRAY_KEY, CAPTURE_SHORTCUT_KEY]) {
      loadSetting(key).catch((failure: unknown) => {
        console.error("could not read a desktop setting", toCoreError(failure).code);
      });
    }
  }, [loadSetting]);
}

function TrayChoice() {
  const closeToTray = useAppStore((state) => parseCloseToTray(state.settings[CLOSE_TO_TRAY_KEY]));
  const setSetting = useAppStore((state) => state.setSetting);
  const [error, setError] = useState<string | null>(null);
  const hintId = useId();

  return (
    <div className="field">
      <label className="checkbox-label">
        <input
          type="checkbox"
          role="switch"
          checked={closeToTray}
          aria-describedby={hintId}
          onChange={(event) => {
            setError(null);
            setSetting(CLOSE_TO_TRAY_KEY, event.target.checked).catch((failure: unknown) => {
              setError(t("settings.desktop.tray.error", { message: toCoreError(failure).message }));
            });
          }}
        />
        <span>{t("settings.desktop.tray.label")}</span>
      </label>
      <p className="muted" id={hintId}>
        {t("settings.desktop.tray.hint")}
      </p>
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function ShortcutChoice() {
  const shortcut = useAppStore((state) => parseCaptureShortcut(state.settings[CAPTURE_SHORTCUT_KEY]));
  const state = useAppStore((current) => current.shortcutState);
  const setSetting = useAppStore((current) => current.setSetting);
  // The field is recording while it has focus: what it shows is a prompt, and a key press is the new shortcut.
  const [recording, setRecording] = useState(false);
  // Why the last key press was not taken, and why the last save failed.
  const [problem, setProblem] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fieldId = useId();
  const hintId = useId();

  function save(value: string): void {
    setError(null);
    setProblem(null);
    setSetting(CAPTURE_SHORTCUT_KEY, value).catch((failure: unknown) => {
      setError(t("settings.desktop.shortcut.error.save", { message: toCoreError(failure).message }));
    });
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>): void {
    // Tab and Escape leave the field, as they do any other field, unless a modifier makes them part of a combination.
    const combined = event.ctrlKey || event.altKey || event.metaKey;
    if (event.code === "Tab" && !combined) {
      return;
    }
    if (event.code === "Escape" && !combined && !event.shiftKey) {
      event.currentTarget.blur();
      return;
    }
    event.preventDefault();
    const capture = captureKey(event);
    switch (capture.kind) {
      case "accelerator":
        save(capture.accelerator);
        break;
      case "pending":
        break;
      case "noModifier":
        setProblem(t("settings.desktop.shortcut.error.noModifier"));
        break;
      case "unsupported":
        setProblem(t("settings.desktop.shortcut.error.unsupported"));
        break;
    }
  }

  const shown = recording ? t("settings.desktop.shortcut.recording") : shortcut === "" ? t("settings.desktop.shortcut.none") : shortcut;
  const report = state === null ? null : STATE_TEXT[state];

  return (
    <fieldset className="fieldset desktop-shortcut">
      <legend>{t("settings.desktop.shortcut.legend")}</legend>
      <p className="muted" id={hintId}>
        {t("settings.desktop.shortcut.hint")}
      </p>
      <div className="row">
        <label htmlFor={fieldId} className="visually-hidden">
          {t("settings.desktop.shortcut.label")}
        </label>
        <input
          id={fieldId}
          className="input mono shortcut-field"
          type="text"
          readOnly
          autoComplete="off"
          spellCheck={false}
          value={shown}
          aria-describedby={hintId}
          aria-invalid={problem !== null || (report?.problem ?? false)}
          onFocus={() => {
            setRecording(true);
            setProblem(null);
          }}
          onBlur={() => {
            setRecording(false);
            setProblem(null);
          }}
          onKeyDown={onKeyDown}
        />
        <button
          type="button"
          className="button"
          disabled={shortcut === ""}
          onClick={() => {
            save("");
          }}
        >
          {t("settings.desktop.shortcut.clear")}
        </button>
        <button
          type="button"
          className="button"
          disabled={shortcut === DEFAULT_CAPTURE_SHORTCUT}
          onClick={() => {
            save(DEFAULT_CAPTURE_SHORTCUT);
          }}
        >
          {t("settings.desktop.shortcut.reset")}
        </button>
      </div>
      {problem !== null && (
        <p className="error-text" role="alert">
          {problem}
        </p>
      )}
      {error !== null && (
        <p className="error-text" role="alert">
          {error}
        </p>
      )}
      {report !== null &&
        (report.problem ? (
          <p className="error-text" role="alert">
            {t(report.key)}
          </p>
        ) : (
          <p className="muted" role="status">
            {t(report.key)}
          </p>
        ))}
    </fieldset>
  );
}

/** The tray and quick-capture settings of the desktop shell. */
export function DesktopSettings() {
  useLoadedSettings();
  return (
    <>
      <TrayChoice />
      <ShortcutChoice />
    </>
  );
}
