// The built-in starter templates (ADR-0056, `docs/architecture/templates.md` section 8): a few templates shipped with the
// app in English, French and Spanish, as Markdown with front matter, usable in every space without being stored in it.
// "Use" makes a page from one (through the import session of the core, ADR-0036); "Add to my templates" makes a template
// page of the space from it, which the person can then change.
//
// The text of each starter is in its own file (`./meeting-notes.ts`, ...), written for the importer (`./source.ts` says
// how). `instantiateStarter` resolves the tokens of the language's text, imports it as one page under the parent, and,
// for a template, flags the page and turns the properties that held "today" into placeholders.

import type { CoreClient, PageId, PageInfo, PropertyDef, SpaceId, TemplatePlaceholder } from "@atomik-kb/core-client";

import { DAILY_JOURNAL } from "./daily-journal";
import { MEETING_NOTES } from "./meeting-notes";
import { PROJECT_BRIEF } from "./project-brief";
import { READING_NOTES } from "./reading-notes";
import { STARTER_LANGUAGES } from "./source";
import type { StarterLanguage, StarterSource } from "./source";
import { WEEKLY_REVIEW } from "./weekly-review";

/** One built-in starter. */
export interface StarterTemplate {
  /** Stable id, e.g. `meeting-notes`. */
  readonly id: string;
  /** An emoji or an icon reference, as a page icon is stored. */
  readonly icon: string;
  /** The name, in the UI's language. */
  readonly title: (language: string) => string;
  /** One line saying what it is for, in the UI's language. */
  readonly description: (language: string) => string;
}

/** The starters with their text, in the order the pickers list them. */
const SOURCES: readonly StarterSource[] = [MEETING_NOTES, DAILY_JOURNAL, WEEKLY_REVIEW, PROJECT_BRIEF, READING_NOTES];

/**
 * The starter language for a UI language: `en`, `fr` or `es` (a regional form such as `fr-CA` or `es_MX` counts as its
 * language), English for any other.
 */
export function starterLanguage(language: string): StarterLanguage {
  const base = language.trim().toLowerCase().split(/[-_]/u)[0];
  return STARTER_LANGUAGES.find((candidate) => candidate === base) ?? "en";
}

/** Every starter, in the order the pickers list them. */
export const STARTER_TEMPLATES: readonly StarterTemplate[] = SOURCES.map((source) => ({
  id: source.id,
  icon: source.icon,
  title: (language) => source.title[starterLanguage(language)],
  description: (language) => source.description[starterLanguage(language)],
}));

/** Where and how to make a page from a starter. */
export interface InstantiateOptions {
  readonly spaceId: SpaceId;
  /** The parent of the new page; the space's top level when absent. */
  readonly parentId?: PageId;
  /** Make a template page of the space (Add to my templates) rather than a page. */
  readonly asTemplate?: boolean;
  /** The UI's language (`en`, `fr`, `es`): the starter's text in it, English otherwise. */
  readonly language: string;
  /** The local day `YYYY-MM-DD` and time `HH:mm` that replace the starter's placeholders (not for `asTemplate`). */
  readonly today: string;
  readonly now: string;
}

/** Most pages one `importCommit` call writes (a starter is one page). */
const COMMIT_PAGES = 50;

/**
 * Replaces the tokens of a starter's Markdown. `{{today}}` and `{{now}}` (property values and date mentions) are always
 * replaced. `{{date}}` and `{{time}}` (the title) are replaced too, unless `keepTitleTokens`: a template keeps them, for
 * the core to resolve each time a page is made from it (templates.md 2.3).
 */
export function resolveStarterTokens(
  markdown: string,
  values: { readonly today: string; readonly now: string },
  keepTitleTokens: boolean,
): string {
  const resolved = markdown.replaceAll("{{today}}", values.today).replaceAll("{{now}}", values.now);
  return keepTitleTokens ? resolved : resolved.replaceAll("{{date}}", values.today).replaceAll("{{time}}", values.now);
}

/**
 * The properties of a starter's front matter whose value is the day (`"{{today}}"`) or the moment (`"{{today}} {{now}}"`)
 * of its creation, by name: what a template turns into placeholders (the value stays on the page as imported, then the
 * placeholder replaces it).
 */
export function starterDatePlaceholders(markdown: string): [name: string, placeholder: TemplatePlaceholder][] {
  const front = /^---\r?\n([\s\S]*?)\r?\n---/u.exec(markdown)?.[1] ?? "";
  const found: [string, TemplatePlaceholder][] = [];
  for (const line of front.split(/\r?\n/u)) {
    const match = /^([^:#"'][^:]*):\s*"\{\{today\}\}( \{\{now\}\})?"\s*$/u.exec(line);
    if (match?.[1] !== undefined) {
      found.push([match[1].trim(), match[2] === undefined ? "today" : "now"]);
    }
  }
  return found;
}

/** Runs the import session of one Markdown file and answers the page it made (the report's first root page). */
async function importPage(
  client: CoreClient,
  options: InstantiateOptions,
  path: string,
  markdown: string,
): Promise<PageId> {
  const importId = await client.importBegin(options.spaceId, options.parentId === undefined ? {} : { parentId: options.parentId });
  try {
    await client.importAddFile(importId, path, markdown);
    await client.importPlan(importId);
    // One page: the first commit is the last. The bound only keeps a core that never says `done` from looping.
    for (let call = 0; call < 10; call += 1) {
      const progress = await client.importCommit(importId, COMMIT_PAGES);
      if (progress.done) {
        const pageId = progress.report?.rootPageIds[0];
        if (pageId === undefined) {
          throw new Error("the import of the starter made no page");
        }
        return pageId;
      }
    }
    throw new Error("the import of the starter did not finish");
  } catch (error) {
    // The session may be closed already (a failed commit ends it); dropping one that is gone is fine. Pages already
    // written stay, as the import of a file does.
    await client.importAbort(importId).catch(() => undefined);
    throw error;
  }
}

/** The date property of the space called `name` (any case), when there is one. */
function dateProperty(schema: readonly PropertyDef[], name: string): PropertyDef | undefined {
  const wanted = name.toLowerCase();
  return schema.find((property) => property.kind === "date" && property.name.toLowerCase() === wanted);
}

/** Makes a page (or a template page) from a starter and answers it. */
export async function instantiateStarter(
  client: CoreClient,
  starterId: string,
  options: InstantiateOptions,
): Promise<PageInfo> {
  const source = SOURCES.find((candidate) => candidate.id === starterId);
  if (source === undefined) {
    throw new Error(`unknown starter: ${starterId}`);
  }
  if (!/^\d{4}-\d{2}-\d{2}$/u.test(options.today) || !/^\d{2}:\d{2}$/u.test(options.now)) {
    throw new Error("today must be YYYY-MM-DD and now HH:mm");
  }
  const asTemplate = options.asTemplate === true;
  const markdown = source.markdown[starterLanguage(options.language)];
  const text = resolveStarterTokens(markdown, options, asTemplate);

  const pageId = await importPage(client, options, `${source.id}.md`, text);
  if (!asTemplate) {
    return client.pageGet(pageId);
  }

  // A template: the page is flagged, and a date property that held "today" at the import stops holding that day and
  // takes the placeholder, so a page made from the template gets the day it is made. The mentions in the text stay as
  // written (the day of the import): turning them into relative mentions means editing the page document, which the
  // template's own editor does (templates.md 6).
  let page = await client.pageSetTemplate(pageId, true);
  const placeholders = starterDatePlaceholders(markdown);
  if (placeholders.length > 0) {
    const schema = await client.propertySchemaGet(options.spaceId);
    for (const [name, placeholder] of placeholders) {
      const property = dateProperty(schema, name);
      if (property === undefined) {
        continue; // The space had a property of that name of another kind: the value was kept as that kind allows.
      }
      try {
        page = await client.pageSetTemplatePlaceholder(pageId, property.id, placeholder);
      } catch {
        // The template exists and is usable without it: the date stays as imported rather than failing the whole call
        // (which would leave a page behind and invite a second one).
      }
    }
  }
  return page;
}
