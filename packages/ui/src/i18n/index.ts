// The UI's strings and the language they are shown in (ADR-0040 item 13).
//
// - **Files.** `en.json` is the source: its keys are the type of every `t("...")` call (`MessageKey`) and it is
//   always loaded (it is the fallback). `fr.json` and `es.json` sit beside it, with the same keys, and are loaded
//   lazily with a dynamic `import()` (the bundler emits one chunk per language), so a person who reads English never
//   downloads them. A test checks that every language file has every key of `en.json` and its `{placeholders}`.
// - **The choice.** The device setting `ui.language` (`system`, `en`, `fr` or `es`; absent means `system`). `system`
//   takes the first language of `navigator.languages` that the app has, else English. The store reads the setting
//   at boot with the theme and the accent and loads the language *before* the app shows (`loadLanguage`), so no frame
//   is drawn in English first; `t()` itself stays synchronous.
// - **Lookup.** `t(key, params)` looks the key up in the active dictionary and falls back to English when the key is
//   missing, then fills the `{name}` placeholders. `tp(base, count, params)` is the plural form: it asks
//   `Intl.PluralRules` of the active locale for the category of `count` and looks up `<base>.<category>`, falling
//   back to `<base>.other` (every plural has an `.other`; a language adds `.one`, `.many`, ... as it needs them).
// - **Locale.** `activeLocale()` is the BCP 47 tag that dates, numbers, plurals and sorting use: the regional
//   variant the person's system lists for the language when there is one (`fr-CA`), else the language (`fr`); on a
//   system that speaks none of the app's languages the words are English and the formats stay the system's own
//   (`de-DE`). One function, so no formatter guesses its own.
// - **Missing files.** A language the app lists but has no file for (or whose file cannot be read) leaves the app in
//   English, never half in another language. A locked app at launch cannot read the stored choice (the settings are in
//   the encrypted database), so its lock screen follows the system until the app is opened.
// - **Not translated.** Core error messages stay English (they are the core's, ADR-0040), and so do product names.

import en from "./en.json";

/** Every key of `en.json`; a typo in a `t("...")` call fails the type check. */
export type MessageKey = keyof typeof en;

type PluralBaseOf<K> = K extends `${infer Base}.other` ? Base : never;

/** The base of every plural of `en.json`: `x` for the keys `x.one` and `x.other`. A typo in a `tp("...")` call fails the type check. */
export type PluralKey = PluralBaseOf<MessageKey>;

/** A language the UI is translated into (or will be). */
export type Language = "en" | "fr" | "es";

/** The values of the `ui.language` setting: a language, or `system` to follow the operating system. */
export type LanguageChoice = "system" | Language;

/** Every language, in the order Settings lists them (after "System"). */
export const LANGUAGES: readonly Language[] = ["en", "fr", "es"];

/** The setting that stores the language choice (`ui.*` keys belong to the UI). Never synced. */
export const LANGUAGE_KEY = "ui.language";

/** What each language is called in itself: the picker writes them like this whatever language is active. */
export const LANGUAGE_NAMES: Readonly<Record<Language, string>> = {
  en: "English",
  fr: "Français",
  es: "Español",
};

type Params = Readonly<Record<string, string | number>>;

/** The strings of one language, by key. A language may lack keys (a newer `en.json`); English fills the gaps. */
export type Dictionary = Readonly<Record<string, string>>;

/** Whether `value` names a language. */
export function isLanguage(value: unknown): value is Language {
  return typeof value === "string" && (LANGUAGES as readonly string[]).includes(value);
}

/** Reads a stored choice; anything unexpected (unset, wrong type, a language of a newer version) means `system`. */
export function parseLanguageChoice(value: unknown): LanguageChoice {
  return value === "system" || isLanguage(value) ? value : "system";
}

/** The languages the person's system lists, most preferred first (empty without a `navigator`). */
export function systemLanguages(): readonly string[] {
  if (typeof navigator === "undefined") {
    return [];
  }
  const listed: readonly string[] = navigator.languages;
  return listed.length > 0 ? listed : navigator.language === "" ? [] : [navigator.language];
}

/** The language of a BCP 47 tag (`fr-CA` is `fr`), lower case; `undefined` when the tag is not a language the app has. */
function languageOfTag(tag: string): Language | undefined {
  const primary = tag.split(/[-_]/u)[0]?.toLowerCase();
  return isLanguage(primary) ? primary : undefined;
}

/**
 * The language a choice stands for: the language itself, or for `system` the first of `preferred` (the system's
 * languages, `navigator.languages` by default) that the app has, else English.
 */
export function resolveLanguage(choice: LanguageChoice, preferred: readonly string[] = systemLanguages()): Language {
  if (choice !== "system") {
    return choice;
  }
  for (const tag of preferred) {
    const language = languageOfTag(tag);
    if (language !== undefined) {
      return language;
    }
  }
  return "en";
}

/** A BCP 47 tag in its canonical form, `undefined` for a malformed one (the system's list is not ours to trust). */
function canonical(tag: string): string | undefined {
  try {
    return Intl.getCanonicalLocales(tag)[0];
  } catch {
    return undefined;
  }
}

/** The locale for `language`: the first tag of `preferred` that is in that language (`fr-CA`), else the language itself. */
export function localeFor(language: Language, preferred: readonly string[] = systemLanguages()): string {
  for (const tag of preferred) {
    if (languageOfTag(tag) === language) {
      const found = canonical(tag);
      if (found !== undefined) {
        return found;
      }
    }
  }
  return language;
}

/**
 * The locale a choice formats dates and numbers in once its language is `language`. It is `localeFor`, except for
 * `system` on a computer that speaks none of the app's languages (German, Japanese): the words are English then, and
 * the formats stay the system's own (`de-DE`: the day first, a week that starts on Monday), as they were before the
 * app had languages.
 */
function localeOf(choice: LanguageChoice, language: Language, preferred: readonly string[]): string {
  if (choice === "system" && !preferred.some((tag) => languageOfTag(tag) !== undefined)) {
    for (const tag of preferred) {
      const found = canonical(tag);
      if (found !== undefined) {
        return found;
      }
    }
  }
  return localeFor(language, preferred);
}

interface Active {
  language: Language;
  locale: string;
  dictionary: Dictionary | null;
  plurals: Intl.PluralRules;
}

function activation(
  language: Language,
  dictionary: Dictionary | null,
  choice: LanguageChoice = "system",
  preferred: readonly string[] = systemLanguages(),
): Active {
  const locale = localeOf(choice, language, preferred);
  return { language, locale, dictionary, plurals: new Intl.PluralRules(locale) };
}

let active: Active = activation("en", null);

/** The language whose strings `t()` uses now. */
export function activeLanguage(): Language {
  return active.language;
}

/**
 * The locale of dates, numbers, plurals and sorting now (a BCP 47 tag, `fr-CA`, `en`): give it to every
 * `Intl.*` constructor and `toLocale*` call instead of `undefined`, so that they follow the app's language.
 */
export function activeLocale(): string {
  return active.locale;
}

/** English at once, with no file to load (what a store starts with when the system speaks English, and what a test starts from). */
export function resetLanguage(): void {
  active = activation("en", null, "system");
}

/**
 * Reads the file of a language other than English: a dynamic `import()`, so the bundler emits one chunk per file and
 * the module system keeps it after the first read. Rejects when there is no such file (a language the app lists
 * before its file is written).
 */
async function importDictionary(language: Language): Promise<Dictionary> {
  const file = (await import(`./${language}.json`)) as { default: Dictionary };
  return file.default;
}

/** How a language file is read; a test gives its own. */
export type DictionaryLoader = (language: Language) => Promise<Dictionary>;

// The newest `loadLanguage` call wins: a slower, older one must not undo it.
let latest = 0;

/**
 * Makes the language of `choice` the active one: loads its dictionary (nothing to load for English) and answers
 * the language that is now active. A language with no file yet, or a file that cannot be read, leaves the app in
 * English, which is always right, never in a half-loaded language. When a newer call came meanwhile, this one
 * changes nothing and answers what is active.
 */
export async function loadLanguage(
  choice: LanguageChoice,
  load: DictionaryLoader = importDictionary,
  preferred: readonly string[] = systemLanguages(),
): Promise<Language> {
  latest += 1;
  const mine = latest;
  const wanted = resolveLanguage(choice, preferred);
  let dictionary: Dictionary | null = null;
  if (wanted !== "en") {
    try {
      dictionary = await load(wanted);
    } catch {
      // No file for this language yet, or it could not be read: English. Nothing to log that says more.
      console.error("a language file could not be loaded", wanted);
    }
  }
  if (mine === latest) {
    // A language that could not be loaded is English with the regional formats of the choice.
    active = activation(dictionary === null ? "en" : wanted, dictionary, choice, preferred);
  }
  return active.language;
}

const english = en as Readonly<Record<string, string>>;

/** The template of a key in the active language, else in English. */
function template(key: string): string | undefined {
  return active.dictionary?.[key] ?? english[key];
}

/** Fills `{name}` placeholders from `params`; one that has no value is left as written, so that a missing parameter is visible. */
function fill(text: string, params: Params | undefined): string {
  if (params === undefined) {
    return text;
  }
  return text.replace(/\{(\w+)\}/g, (placeholder, name: string) =>
    Object.hasOwn(params, name) ? String(params[name]) : placeholder,
  );
}

/**
 * Looks up a UI string in the active language (English for a key it lacks) and fills `{name}` placeholders from
 * `params`. A placeholder without a value is left as written so a missing parameter is visible, not silent.
 */
export function t(key: MessageKey, params?: Params): string {
  return fill(template(key) ?? key, params);
}

/**
 * The plural form of a string: `Intl.PluralRules` of the active locale gives the category of `count` (`one`,
 * `other`, and `many`, `few`, ... where a language has them; in French 0 and 1 are `one`, in English only 1 is),
 * and the text is the key `<base>.<category>`, else `<base>.other`, in the language's dictionary before English's. `{count}` is filled with `count` unless
 * `params` gives its own (a formatted number); a `.one` text may use it too, because "one" is not always 1.
 */
export function tp(base: PluralKey, count: number, params?: Params): string {
  const category = active.plurals.select(count);
  // The language's own texts first (its category, else its .other), English only for a plural it lacks altogether.
  const text =
    active.dictionary?.[`${base}.${category}`] ??
    active.dictionary?.[`${base}.other`] ??
    english[`${base}.${category}`] ??
    english[`${base}.other`] ??
    `${base}.other`;
  return fill(text, { count, ...params });
}
