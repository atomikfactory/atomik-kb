// What one built-in starter is made of (ADR-0056, `docs/architecture/templates.md` section 8): its icon, its names, and
// its text as Markdown with front matter in each language, written for the importer of ADR-0036.
//
// The Markdown carries four tokens, resolved by `instantiateStarter` (`./index.ts`) before the text goes to the core:
//
// - `{{date}}` (and `{{time}}`) in the title: the local day (and time) a page is made. A starter added as a template keeps
//   these two, which the core resolves each time a page is made from the template (templates.md 2.3);
// - `{{today}}` (and `{{now}}`) elsewhere: a property value (`Date: "{{today}}"`) or the `datetime` of a date mention
//   (`<time datetime="{{today}}">{{today}}</time>`). Always resolved with the day (and time) the starter is used or added.
//
// Front matter values are double-quoted so that no title or name can be misread as YAML, and the file has no level-1
// heading (the title is the front matter's). Properties are written by name only: the import matches them in the
// space or creates them, from the kind the value suggests (a date for `YYYY-MM-DD`).

/** A no-break space: French writes one before `:`, `?` and `!`. Spelled out so that the source has no invisible character. */
export const NBSP = String.fromCharCode(0xa0);

/** The languages the starters are written in. */
export type StarterLanguage = "en" | "fr" | "es";

/** The languages, English first (the fallback). */
export const STARTER_LANGUAGES: readonly StarterLanguage[] = ["en", "fr", "es"];

/** One text per language. */
export type PerLanguage<T> = Readonly<Record<StarterLanguage, T>>;

/** One built-in starter as data. */
export interface StarterSource {
  /** Stable id, e.g. `meeting-notes`. */
  readonly id: string;
  /** The page icon (an emoji). */
  readonly icon: string;
  /** The name in a picker, per language (no tokens). */
  readonly title: PerLanguage<string>;
  /** One line saying what the starter is for, per language. */
  readonly description: PerLanguage<string>;
  /** The page as Markdown with front matter, per language. */
  readonly markdown: PerLanguage<string>;
}
