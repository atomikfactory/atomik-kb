import { readFileSync, readdirSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  LANGUAGES,
  activeLanguage,
  activeLocale,
  isLanguage,
  loadLanguage,
  localeFor,
  parseLanguageChoice,
  resetLanguage,
  resolveLanguage,
  t,
  tp,
} from "./i18n";
import messages from "./i18n/en.json";
import type { Dictionary, Language, MessageKey } from "./i18n";

/** Makes `navigator.languages` (and `navigator.language`) say this; `afterEach` puts the real ones back. */
function systemSays(languages: readonly string[]): void {
  Object.defineProperty(navigator, "languages", { configurable: true, value: languages });
  Object.defineProperty(navigator, "language", { configurable: true, value: languages[0] ?? "" });
}

afterEach(() => {
  resetLanguage();
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "languages");
  Reflect.deleteProperty(navigator, "language");
});

describe("t", () => {
  it("returns the string for a key", () => {
    expect(t("nav.settings")).toBe("Settings");
  });

  it("fills named placeholders", () => {
    expect(t("diagnostics.warning.apiVersion", { expected: 1, actual: 2 })).toContain(
      "built for core API version 1, but the core reports version 2.",
    );
  });

  it("leaves a placeholder visible when its parameter is missing", () => {
    expect(t("onboarding.error.generic", { message: "boom" })).toBe("The workspace could not be created: boom");
    expect(t("onboarding.error.generic", { other: "x" })).toBe("The workspace could not be created: {message}");
    expect(t("onboarding.error.generic", {})).toBe("The workspace could not be created: {message}");
    expect(t("onboarding.error.generic")).toBe("The workspace could not be created: {message}");
  });

  it("does not interpret parameter values as templates or replacement patterns", () => {
    expect(t("onboarding.error.generic", { message: "{message} $& $1" })).toBe(
      "The workspace could not be created: {message} $& $1",
    );
  });
});

/** A small dictionary of one language, as a loader answers it. */
const GERMAN_LIKE: Dictionary = {
  "nav.settings": "Einstellungen",
  "spaces.pageCount.one": "{count} Seite",
  "spaces.pageCount.other": "{count} Seiten",
};

const loaderOf =
  (dictionary: Dictionary) =>
  (): Promise<Dictionary> =>
    Promise.resolve(dictionary);

describe("the active language", () => {
  it("starts in English", () => {
    expect(activeLanguage()).toBe("en");
    expect(t("nav.settings")).toBe("Settings");
  });

  it("looks a key up in the active dictionary and falls back to English for a key it lacks", async () => {
    await loadLanguage("fr", loaderOf(GERMAN_LIKE));
    expect(activeLanguage()).toBe("fr");
    expect(t("nav.settings")).toBe("Einstellungen");
    // Not in the dictionary: English, never the bare key.
    expect(t("nav.home")).toBe("Home");
    expect(t("onboarding.error.generic", { message: "boom" })).toBe("The workspace could not be created: boom");
  });

  it("keeps the {name} placeholder behaviour in another language", async () => {
    await loadLanguage("fr", loaderOf({ "onboarding.error.generic": "Erreur : {message}" }));
    expect(t("onboarding.error.generic", { message: "boom" })).toBe("Erreur : boom");
    expect(t("onboarding.error.generic")).toBe("Erreur : {message}");
    expect(t("onboarding.error.generic", { message: "{message} $&" })).toBe("Erreur : {message} $&");
  });

  it("stays in English when the language has no file, or its file cannot be read", async () => {
    const logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
    const missing = (): Promise<Dictionary> => Promise.reject(new Error("Unknown variable dynamic import: ./es.json"));
    await expect(loadLanguage("es", missing)).resolves.toBe("en");
    expect(activeLanguage()).toBe("en");
    expect(activeLocale()).toBe(localeFor("en"));
    expect(t("nav.settings")).toBe("Settings");
    // The language is logged, never what the error said.
    expect(logged).toHaveBeenCalledWith("a language file could not be loaded", "es");
  });

  it("reads no file for English", async () => {
    const load = vi.fn(loaderOf(GERMAN_LIKE));
    await expect(loadLanguage("en", load)).resolves.toBe("en");
    expect(load).not.toHaveBeenCalled();
  });

  it("lets the newest request win, whatever order the files arrive in", async () => {
    let finishSlow: (dictionary: Dictionary) => void = () => undefined;
    const slow = new Promise<Dictionary>((resolve) => {
      finishSlow = resolve;
    });
    const older = loadLanguage("fr", () => slow);
    const newer = loadLanguage("en");
    await newer;
    finishSlow(GERMAN_LIKE);
    await older;
    expect(activeLanguage()).toBe("en");
    expect(t("nav.settings")).toBe("Settings");
  });

  it("is the language of the real French file", async () => {
    await expect(loadLanguage("fr")).resolves.toBe("fr");
    expect(activeLanguage()).toBe("fr");
    expect(t("nav.settings")).toBe("Paramètres");
  });

  it("is the language of the real Spanish file", async () => {
    await expect(loadLanguage("es")).resolves.toBe("es");
    expect(activeLanguage()).toBe("es");
    expect(activeLocale()).toBe("es");
    expect(t("nav.settings")).toBe("Ajustes");
    // The placeholders are filled as in every language, and a name is quoted with « ».
    expect(t("spaces.deleteBody", { name: "Trabajo" })).toBe("El espacio vacío «Trabajo» se eliminará. Esta acción no se puede deshacer.");
  });

  it("follows the system into the real Spanish file: es-ES, es-MX and es_AR are Spanish, the first language the app has wins", async () => {
    await expect(loadLanguage("system", undefined, ["es-ES", "en-US"])).resolves.toBe("es");
    expect(activeLocale()).toBe("es-ES");
    expect(t("nav.home")).toBe("Inicio");
    await expect(loadLanguage("system", undefined, ["de-DE", "es-MX", "fr"])).resolves.toBe("es");
    expect(activeLocale()).toBe("es-MX");
    await expect(loadLanguage("system", undefined, ["es_AR"])).resolves.toBe("es");
    expect(t("nav.home")).toBe("Inicio");
    // A language earlier in the system's list wins over Spanish.
    await expect(loadLanguage("system", undefined, ["fr-FR", "es-ES"])).resolves.toBe("fr");
    expect(t("nav.home")).toBe("Accueil");
    // A chosen language is not the system's.
    await expect(loadLanguage("en", undefined, ["es-ES"])).resolves.toBe("en");
    expect(t("nav.home")).toBe("Home");
  });
});

describe("tp", () => {
  it("picks one for 1 and other for everything else in English", () => {
    expect(tp("spaces.pageCount", 1)).toBe("1 page");
    expect(tp("spaces.pageCount", 0)).toBe("0 pages");
    expect(tp("spaces.pageCount", 2)).toBe("2 pages");
    expect(tp("spaces.pageCount", 1.5)).toBe("1.5 pages");
  });

  it("fills {count} unless the caller gives its own (a formatted number)", () => {
    expect(tp("spaces.pageCount", 1234)).toBe("1234 pages");
    expect(tp("spaces.pageCount", 1234, { count: "1,234" })).toBe("1,234 pages");
    expect(tp("moveToSpace.confirm", 3, { space: "Work" })).toBe("Move 3 pages to “Work”");
    expect(tp("moveToSpace.confirm", 1, { space: "Work" })).toBe("Move 1 page to “Work”");
  });

  it("asks the plural rules of the active language: in French 0 and 1 are one", async () => {
    await loadLanguage("fr");
    expect(tp("spaces.pageCount", 0)).toBe("0 page");
    expect(tp("spaces.pageCount", 1)).toBe("1 page");
    expect(tp("spaces.pageCount", 1.5)).toBe("1.5 page");
    expect(tp("spaces.pageCount", 2)).toBe("2 pages");
    expect(tp("spaces.pageCount", 21)).toBe("21 pages");
  });

  it("asks the plural rules of Spanish: 1 is one, 0 and 2 and 1.5 are other", async () => {
    await loadLanguage("es");
    expect(tp("spaces.pageCount", 1)).toBe("1 página");
    expect(tp("spaces.pageCount", 0)).toBe("0 páginas");
    expect(tp("spaces.pageCount", 2)).toBe("2 páginas");
    expect(tp("spaces.pageCount", 1.5)).toBe("1.5 páginas");
    expect(tp("spaces.pageCount", 21)).toBe("21 páginas");
    expect(tp("moveToSpace.confirm", 1, { space: "Trabajo" })).toBe("Mover 1 página a «Trabajo»");
    expect(tp("moveToSpace.confirm", 3, { space: "Trabajo" })).toBe("Mover 3 páginas a «Trabajo»");
    expect(tp("trash.purgeBody", 1, { title: "Notas" })).toContain("la página que contiene");
    expect(tp("trash.purgeBody", 4, { title: "Notas" })).toContain("las 4 páginas que contiene");
    // The `many` category of Spanish (multiples of a million) has no text of its own: it reads like .other.
    expect(new Intl.PluralRules("es").select(1_000_000)).toBe("many");
    expect(tp("spaces.pageCount", 1_000_000)).toBe("1000000 páginas");
  });

  it("falls back to .other when the language has no text for the category", async () => {
    // French says "one" for 0 and 1; a dictionary with only an .other answers them too.
    await loadLanguage("fr", loaderOf({ "spaces.pageCount.other": "{count} Seiten" }));
    expect(activeLanguage()).toBe("fr");
    expect(tp("spaces.pageCount", 1)).toBe("1 Seiten");
    expect(tp("spaces.pageCount", 0)).toBe("0 Seiten");
    expect(tp("spaces.pageCount", 7)).toBe("7 Seiten");
  });

  it("falls back to English for a plural the dictionary lacks altogether", async () => {
    await loadLanguage("fr", loaderOf({ "nav.settings": "Einstellungen" }));
    expect(tp("spaces.pageCount", 3)).toBe("3 pages");
  });

  it("uses a category the language needs (French and Spanish have `many`, for millions) when the dictionary has it", async () => {
    // Whatever category the runtime's rules give a million in French, the dictionary's text for it is the one used.
    const category = new Intl.PluralRules("fr").select(1_000_000);
    await loadLanguage("fr", loaderOf({ [`spaces.pageCount.${category}`]: "{count} de pages", "spaces.pageCount.other": "{count} pages" }));
    expect(tp("spaces.pageCount", 1_000_000)).toBe("1000000 de pages");
    expect(tp("spaces.pageCount", 5)).toBe("5 pages");
  });
});

describe("the language choice", () => {
  it("reads only the four values and takes anything else for system", () => {
    for (const value of ["system", "en", "fr", "es"] as const) {
      expect(parseLanguageChoice(value)).toBe(value);
    }
    for (const value of [undefined, null, "", "de", "FR", 3, true, ["fr"], { language: "fr" }]) {
      expect(parseLanguageChoice(value)).toBe("system");
    }
    expect(LANGUAGES).toEqual(["en", "fr", "es"]);
    expect(isLanguage("fr")).toBe(true);
    expect(isLanguage("system")).toBe(false);
  });

  it("is a language itself when it names one", () => {
    expect(resolveLanguage("fr", ["en-US"])).toBe("fr");
    expect(resolveLanguage("en", ["fr-FR"])).toBe("en");
    expect(resolveLanguage("es", [])).toBe("es");
  });

  it("follows the system: the first of its languages that the app has", () => {
    expect(resolveLanguage("system", ["fr-FR", "en-US"])).toBe("fr");
    expect(resolveLanguage("system", ["de-DE", "es-MX", "fr"])).toBe("es");
    expect(resolveLanguage("system", ["FR"])).toBe("fr");
    expect(resolveLanguage("system", ["fr_CA"])).toBe("fr");
    expect(resolveLanguage("system", ["en-GB", "fr"])).toBe("en");
  });

  it("is English when the system's languages are none that the app has, or none at all", () => {
    expect(resolveLanguage("system", ["de-DE", "ja"])).toBe("en");
    expect(resolveLanguage("system", [])).toBe("en");
    expect(resolveLanguage("system", ["", "-"])).toBe("en");
  });

  it("reads navigator.languages by default, and navigator.language when that is empty", () => {
    systemSays(["fr-CA", "en"]);
    expect(resolveLanguage("system")).toBe("fr");
    Object.defineProperty(navigator, "languages", { configurable: true, value: [] });
    Object.defineProperty(navigator, "language", { configurable: true, value: "es-AR" });
    expect(resolveLanguage("system")).toBe("es");
  });
});

describe("the locale", () => {
  it("is the regional variant the system lists for the language, else the language", () => {
    expect(localeFor("fr", ["en-GB", "fr-CA"])).toBe("fr-CA");
    expect(localeFor("en", ["en-GB", "fr-CA"])).toBe("en-GB");
    expect(localeFor("fr", ["en-GB"])).toBe("fr");
    expect(localeFor("es", [])).toBe("es");
    // A malformed tag is skipped, not thrown.
    expect(localeFor("fr", ["fr-!!", "fr-BE"])).toBe("fr-BE");
    expect(localeFor("fr", ["fr-!!"])).toBe("fr");
  });

  it("keeps the system's own region formats when it speaks none of the app's languages, and only then", async () => {
    await loadLanguage("system", loaderOf({}), ["de-DE", "ja-JP"]);
    expect(activeLanguage()).toBe("en");
    expect(activeLocale()).toBe("de-DE");
    // A system that speaks a language the app has uses that language's regional tag.
    await loadLanguage("system", loaderOf({}), ["de-DE", "fr-BE"]);
    expect(activeLanguage()).toBe("fr");
    expect(activeLocale()).toBe("fr-BE");
    // A chosen language is not the system's: English is English, whatever the region of the system.
    await loadLanguage("en", loaderOf({}), ["de-DE"]);
    expect(activeLocale()).toBe("en");
    // A language that cannot be loaded leaves English with the regional formats of the choice.
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await loadLanguage("fr", () => Promise.reject(new Error("no file")), ["en-GB", "fr-CA"]);
    expect(activeLanguage()).toBe("en");
    expect(activeLocale()).toBe("en-GB");
  });

  it("is what dates and numbers use once a language is active", async () => {
    systemSays(["fr-CA", "en-US"]);
    await loadLanguage("fr", loaderOf({}));
    expect(activeLocale()).toBe("fr-CA");
    resetLanguage();
    expect(activeLocale()).toBe("en-US");
  });
});

// --- the language files ---

const FILES = readdirSync("src/i18n").filter((name) => name.endsWith(".json"));
const LANGUAGE_FILES = FILES.filter((name) => name !== "en.json");

function dictionaryOf(file: string): Record<string, string> {
  return JSON.parse(readFileSync(`src/i18n/${file}`, "utf8")) as Record<string, string>;
}

/** The set of `{name}` placeholders of a text, sorted. */
function placeholders(text: string): string[] {
  return [...new Set([...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1] ?? ""))].sort();
}

describe("en.json", () => {
  const entries = Object.entries(messages) as [MessageKey, string][];

  it("has no empty strings", () => {
    for (const [key, text] of entries) {
      expect(text.trim(), key).not.toBe("");
    }
  });

  it("has a dedicated fatal hint for the error codes that need one, and a default", () => {
    for (const code of ["KeystoreUnavailable", "Storage", "Crypto"]) {
      expect(messages).toHaveProperty([`fatal.hint.${code}`]);
    }
    expect(messages).toHaveProperty(["fatal.hint.default"]);
  });

  it("gives every plural .one an .other, which tp falls back to", () => {
    const keys = new Set(Object.keys(messages));
    for (const key of keys) {
      if (key.endsWith(".one")) {
        expect(keys.has(key.replace(/\.one$/, ".other")), `${key} has no .other`).toBe(true);
      }
    }
  });

  it("writes the number of a plural as {count}, so that a language where 0 is one can say 0", () => {
    // `.one` is not always 1 (French says "one" for 0 and 1.5): a text that wrote "1" would be wrong there.
    for (const [key, text] of entries) {
      if (key.endsWith(".one") && !key.startsWith("trash.purgeBody")) {
        expect(text, key).not.toMatch(/(?<![\w{])1(?![\w}])/);
      }
    }
  });
});

describe("the language files", () => {
  it("are beside en.json, one per language the app lists: French and Spanish at least", () => {
    expect(FILES).toContain("en.json");
    expect(LANGUAGE_FILES).toContain("fr.json");
    expect(LANGUAGE_FILES).toContain("es.json");
    for (const file of LANGUAGE_FILES) {
      expect(LANGUAGES as readonly string[], file).toContain(file.replace(/\.json$/, ""));
    }
  });

  for (const file of LANGUAGE_FILES) {
    describe(file, () => {
      const language = file.replace(/\.json$/, "");
      const dictionary = dictionaryOf(file);
      const english = messages as Record<string, string>;

      it("has every key of en.json", () => {
        const missing = Object.keys(english).filter((key) => !Object.hasOwn(dictionary, key));
        expect(missing).toEqual([]);
      });

      it("has no key that en.json does not have", () => {
        const extra = Object.keys(dictionary).filter((key) => !Object.hasOwn(english, key));
        expect(extra).toEqual([]);
      });

      it("has no empty string", () => {
        for (const [key, text] of Object.entries(dictionary)) {
          expect(typeof text, key).toBe("string");
          expect(text.trim(), key).not.toBe("");
        }
      });

      it("keeps the {placeholders} of every string, no more and no fewer", () => {
        for (const [key, text] of Object.entries(english)) {
          expect(placeholders(dictionary[key] ?? ""), key).toEqual(placeholders(text));
        }
      });

      it("keeps a placeholder as {name}: no other braces", () => {
        for (const [key, text] of Object.entries(dictionary)) {
          const stray = text.replace(/\{\w+\}/g, "");
          expect(stray, key).not.toMatch(/[{}]/);
        }
      });

      it("gives every native menu label one mnemonic (&), and no two items of a menu the same letter", () => {
        const menus: Record<string, string[]> = {
          bar: ["file", "edit", "view", "window", "help"],
          file: ["newPage", "import", "export", "print", "lock", "quit"],
          edit: ["undo", "redo", "cut", "copy", "paste", "selectAll", "find"],
          view: ["toggleSidebar", "back", "forward"],
          window: ["minimize", "closeWindow"],
          help: ["about"],
        };
        for (const [menu, names] of Object.entries(menus)) {
          const letters = names.map((name) => {
            const text = (dictionary[`menu.${name}`] ?? "").replace(/&&/g, "");
            expect(text.match(/&/g)?.length, `menu.${name}`).toBe(1);
            return /&([^&\s])/.exec(text)?.[1]?.toLocaleLowerCase(language);
          });
          expect(letters.every((letter) => letter !== undefined), menu).toBe(true);
          expect(new Set(letters).size, `${menu}: ${letters.join(", ")}`).toBe(letters.length);
        }
      });

      it("keeps the native labels short enough for the shell (1 to 64 characters; its dialog texts up to 400)", () => {
        for (const key of Object.keys(english)) {
          const text = dictionary[key] ?? "";
          if (key.startsWith("tray.") || key.startsWith("menu.") || key.startsWith("fileFilter.")) {
            if (key === "menu.noPage" || key === "menu.unavailable") {
              continue; // toasts of the page, not labels of the shell
            }
            expect(text.length, key).toBeLessThanOrEqual(64);
          }
          if (key.startsWith("shellDialog.")) {
            expect(text.length, key).toBeLessThanOrEqual(/^shellDialog\.(?:allow|cancel|yes|no|untitled|\w+Title)$/.test(key) ? 64 : 400);
          }
        }
      });

      it("has the same plural categories pairs as English: every .one has its .other", () => {
        for (const key of Object.keys(dictionary)) {
          if (key.endsWith(".one")) {
            expect(Object.hasOwn(dictionary, key.replace(/\.one$/, ".other")), key).toBe(true);
          }
        }
      });

      it("loads through the loader of the app", async () => {
        await expect(loadLanguage(language as Language)).resolves.toBe(language);
        const sample = Object.keys(english)[5] as MessageKey;
        expect(t(sample)).toBe(dictionary[sample]);
      });
    });
  }
});

describe("es.json", () => {
  const spanish = dictionaryOf("es.json");

  it("speaks to the person as tú: never usted", () => {
    for (const [key, text] of Object.entries(spanish)) {
      expect(text, key).not.toMatch(/\bustedes?\b/iu);
    }
  });

  it("writes a quoted name with « », never with the English “ ”, and opens every question with ¿", () => {
    for (const [key, text] of Object.entries(spanish)) {
      expect(text, key).not.toMatch(/[“”]/u);
      if (text.includes("?")) {
        expect(text, key).toContain("¿");
      }
    }
  });

  it("keeps the product names and the keyboard keys as they are", () => {
    expect(spanish["settings.ai.enabled"]).toContain("Ollama");
    expect(spanish["mcp.title"]).toContain("MCP");
    expect(spanish["settings.desktop.shortcut.hint"]).toContain("Ctrl, Alt o Super");
    expect(spanish["export.format.markdown"]).toBe("Markdown");
    expect(spanish["app.name"]).toBe("atomik-kb");
  });
});
