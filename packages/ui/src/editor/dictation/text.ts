// The words and numbers of dictation (ADR-0053, `docs/architecture/dictation.md` sections 4.2 and 6): a sentence for every
// error code of the shell, the names of the engines, why one is unavailable, a download size in MB and a list of
// languages named in the active language (`Intl.DisplayNames`).

import { dictationReasonOf, toCoreError } from "@atomik-kb/core-client";
import type { DictationErrorCode, SpeechEngine, UnavailableReason } from "@atomik-kb/core-client";

import { activeLocale, t } from "../../i18n";
import type { MessageKey } from "../../i18n";

/** The sentence of each error code of section 4.2. */
export const ERROR_KEYS: Readonly<Record<DictationErrorCode, MessageKey>> = {
  modelMissing: "dictation.error.modelMissing",
  engineMissing: "dictation.error.engineMissing",
  engineFailed: "dictation.error.engineFailed",
  engineCrashed: "dictation.error.engineCrashed",
  ollamaUnavailable: "dictation.error.ollamaUnavailable",
  busy: "dictation.error.busy",
  downloadFailed: "dictation.error.downloadFailed",
  hashMismatch: "dictation.error.hashMismatch",
  cancelled: "dictation.error.cancelled",
  consentDeclined: "dictation.error.consentDeclined",
  importInvalid: "dictation.error.importInvalid",
  storeFailed: "dictation.error.storeFailed",
};

/** What an engine is called. */
export const ENGINE_KEYS: Readonly<Record<SpeechEngine, MessageKey>> = {
  accurate: "dictation.engine.accurate",
  light: "dictation.engine.light",
  ollama: "dictation.engine.ollama",
};

/** What each engine is, in a sentence. */
export const ENGINE_ABOUT_KEYS: Readonly<Record<SpeechEngine, MessageKey>> = {
  accurate: "dictation.engine.accurate.about",
  light: "dictation.engine.light.about",
  ollama: "dictation.engine.ollama.about",
};

/** Why an engine cannot be used (`EngineStatus.unavailableReason`). */
export const UNAVAILABLE_KEYS: Readonly<Record<UnavailableReason, MessageKey>> = {
  engineMissing: "dictation.unavailable.engineMissing",
  aiOff: "dictation.unavailable.aiOff",
  noTranscriptionModel: "dictation.unavailable.noTranscriptionModel",
  consentRequired: "dictation.unavailable.consentRequired",
};

/** The name of an engine in the active language. */
export function engineName(engine: SpeechEngine): string {
  return t(ENGINE_KEYS[engine]);
}

/** The sentence for an error code. */
export function codeText(code: DictationErrorCode): string {
  return t(ERROR_KEYS[code]);
}

/** The sentence for whatever a call to the shell rejected with: its code when it has one, else a general sentence. */
export function errorText(error: unknown): string {
  const code = dictationReasonOf(error);
  return code === undefined ? t("dictation.error.unknown", { message: toCoreError(error).message }) : codeText(code);
}

/** The size of a download as the dialog of the shell words it: decimal megabytes, rounded up ("671 MB", "671 Mo"). */
export function formatSize(bytes: number): string {
  const megabytes = Math.max(1, Math.ceil(bytes / 1_000_000));
  return new Intl.NumberFormat(activeLocale(), { style: "unit", unit: "megabyte", unitDisplay: "short", maximumFractionDigits: 0 }).format(
    megabytes,
  );
}

/** A percentage of `done` in `total`, as a whole number from 0 to 100. */
export function percentOf(done: number, total: number): number {
  return total <= 0 ? 0 : Math.max(0, Math.min(100, Math.floor((done / total) * 100)));
}

/** "2 %": a percentage in the active language's own form. */
export function formatPercent(percent: number): string {
  return new Intl.NumberFormat(activeLocale(), { style: "percent", maximumFractionDigits: 0 }).format(percent / 100);
}

/**
 * The languages of an engine, named in the active language and listed in its alphabetical order ("Bulgarian, Croatian and
 * Czech"). Names come from the platform (`Intl.DisplayNames`); a code it does not know stays as written.
 */
export function languageList(codes: readonly string[]): string {
  const locale = activeLocale();
  let names: Intl.DisplayNames | null = null;
  try {
    names = new Intl.DisplayNames([locale], { type: "language" });
  } catch {
    names = null;
  }
  const collator = new Intl.Collator(locale);
  const named = codes.map((code) => {
    try {
      return names?.of(code) ?? code;
    } catch {
      return code;
    }
  });
  return new Intl.ListFormat(locale, { style: "long", type: "conjunction" }).format(named.sort((a, b) => collator.compare(a, b)));
}
