// The words of dictation in the three languages (ADR-0053): a sentence for every error code of the shell, the names of the
// engines, why one cannot be used, the three labels of the shell's download dialog and their placeholders. (That every key
// of English is in French and Spanish, with the same placeholders, is `i18n.test.ts`.)

import { DICTATION_ERROR_CODES, CoreError, SPEECH_ENGINES } from "@atomik-kb/core-client";
import type { UnavailableReason } from "@atomik-kb/core-client";
import { afterEach, describe, expect, it } from "vitest";

import { loadLanguage, resetLanguage, t } from "../../i18n";
import type { Language } from "../../i18n";
import en from "../../i18n/en.json";
import es from "../../i18n/es.json";
import fr from "../../i18n/fr.json";
import { dialogLabels } from "../../store-shell";
import { codeText, ENGINE_ABOUT_KEYS, ENGINE_KEYS, ERROR_KEYS, errorText, formatPercent, percentOf, UNAVAILABLE_KEYS } from "./text";

afterEach(() => {
  resetLanguage();
});

const files: Readonly<Record<Language, Record<string, string>>> = { en, fr, es };
const UNAVAILABLE: readonly UnavailableReason[] = ["engineMissing", "aiOff", "noTranscriptionModel", "consentRequired"];

describe("the sentences", () => {
  it.each(["en", "fr", "es"] as const)("cover every code of the shell, every engine and every reason in %s", async (language) => {
    await loadLanguage(language);
    expect(Object.keys(ERROR_KEYS).sort()).toEqual([...DICTATION_ERROR_CODES].sort());
    for (const code of DICTATION_ERROR_CODES) {
      const sentence = codeText(code);
      expect(sentence, code).not.toBe(ERROR_KEYS[code]);
      expect(sentence, code).toBe(files[language][ERROR_KEYS[code]]);
      expect(sentence.length, code).toBeGreaterThan(15);
      if (language !== "en") {
        expect(sentence, `${language} ${code}`).not.toBe(en[ERROR_KEYS[code]]);
      }
    }
    for (const engine of SPEECH_ENGINES) {
      expect(files[language][ENGINE_KEYS[engine]], engine).toBeTruthy();
      expect(files[language][ENGINE_ABOUT_KEYS[engine]], engine).toBeTruthy();
    }
    for (const reason of UNAVAILABLE) {
      expect(files[language][UNAVAILABLE_KEYS[reason]], reason).toBeTruthy();
    }
  });

  it("are the same sentences in French and Spanish as the keys name, never English left over", () => {
    for (const code of DICTATION_ERROR_CODES) {
      for (const language of ["fr", "es"] as const) {
        expect(files[language][ERROR_KEYS[code]], `${language} ${code}`).not.toBe(en[ERROR_KEYS[code]]);
      }
    }
  });

  it("say what the code is when the shell rejects with one, and what the error is when it does not", () => {
    const refusal = new CoreError({ code: "InvalidInput", message: "the model is not installed", details: { field: "engine", reason: "modelMissing" } });
    expect(errorText(refusal)).toBe(t("dictation.error.modelMissing"));
    expect(errorText(new CoreError({ code: "Internal", message: "boom" }))).toBe(t("dictation.error.unknown", { message: "boom" }));
    // A reason that is not a code (the shell's `ended`) is not one.
    const ended = new CoreError({ code: "InvalidInput", message: "no session", details: { field: "sessionId", reason: "ended" } });
    expect(errorText(ended)).toBe(t("dictation.error.unknown", { message: "no session" }));
  });
});

describe("the labels of the shell's download dialog", () => {
  it("are sent from the three shellDialog keys, with the placeholders the shell fills", () => {
    const labels = dialogLabels();
    expect(labels.dialogDictationTitle).toBe(en["shellDialog.dictationTitle"]);
    expect(labels.dialogDictationMessage).toBe(en["shellDialog.dictationMessage"]);
    expect(labels.dialogDownload).toBe(en["shellDialog.download"]);
    expect(labels.dialogDictationMessage).toContain("{model}");
    expect(labels.dialogDictationMessage).toContain("{size}");
    expect(labels.dialogDictationMessage).toContain("{host}");
  });

  it.each(["en", "fr", "es"] as const)("keep {size} and {host} (and {model}) in %s, and fit the shell's limits", (language) => {
    const message = files[language]["shellDialog.dictationMessage"] ?? "";
    for (const name of ["model", "size", "host"]) {
      expect(message, language).toContain(`{${name}}`);
    }
    expect(message.length).toBeLessThanOrEqual(400);
    expect((files[language]["shellDialog.dictationTitle"] ?? "").length).toBeLessThanOrEqual(64);
    expect((files[language]["shellDialog.download"] ?? "").length).toBeLessThanOrEqual(64);
  });

  it("send the words of the person's language", async () => {
    await loadLanguage("fr");
    const labels = dialogLabels();
    expect(labels.dialogDictationTitle).toBe(fr["shellDialog.dictationTitle"]);
    expect(labels.dialogDownload).toBe("Télécharger");
    await loadLanguage("es");
    expect(dialogLabels().dialogDictationTitle).toBe("¿Descargar un modelo de voz?");
  });

  it("say the same as the contract's English", () => {
    expect(en["shellDialog.dictationTitle"]).toBe("Download a speech model?");
    expect(en["shellDialog.dictationMessage"]).toBe(
      "Dictation needs to download {model} ({size}) from {host}. {host} will see this computer's IP address and which model you download. The files are checked against a fixed fingerprint, and your recordings are never stored or sent anywhere.",
    );
    expect(en["shellDialog.download"]).toBe("Download");
  });
});

describe("the figures", () => {
  it("are whole percentages from 0 to 100", () => {
    expect(percentOf(0, 100)).toBe(0);
    expect(percentOf(50, 200)).toBe(25);
    expect(percentOf(300, 200)).toBe(100);
    expect(percentOf(-5, 200)).toBe(0);
    expect(percentOf(5, 0)).toBe(0);
  });

  it("are written in the active language", async () => {
    expect(formatPercent(42)).toBe("42%");
    await loadLanguage("fr");
    expect(formatPercent(42)).toMatch(/^42\s%$/u);
  });
});
