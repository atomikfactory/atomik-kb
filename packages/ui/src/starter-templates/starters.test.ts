// The built-in starter templates (ADR-0056, `docs/architecture/templates.md` section 8) against the in-memory core: every
// starter in every language imports as one page with its title (the date resolved), icon and properties; English stands
// in for a language the starters lack; a starter added as a template keeps `{{date}}` in its title and is flagged.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, PageInfo, SpaceId } from "@atomik-kb/core-client";
import { describe, expect, it, vi } from "vitest";

import { DAILY_JOURNAL } from "./daily-journal";
import {
  STARTER_TEMPLATES,
  instantiateStarter,
  resolveStarterTokens,
  starterDatePlaceholders,
  starterLanguage,
} from "./index";
import { MEETING_NOTES } from "./meeting-notes";
import { PROJECT_BRIEF } from "./project-brief";
import { READING_NOTES } from "./reading-notes";
import { NBSP, STARTER_LANGUAGES } from "./source";
import type { StarterLanguage } from "./source";
import { WEEKLY_REVIEW } from "./weekly-review";

const TODAY = "2026-10-18";
const NOW = "14:30";

interface Expected {
  readonly icon: string;
  /** The page title in each language, with the date resolved. */
  readonly titles: Record<StarterLanguage, string>;
  /** The page title in each language as a template keeps it. */
  readonly templateTitles: Record<StarterLanguage, string>;
  /** The name in each language of the date property the starter fills in with "today", when it has one. */
  readonly dateProperty?: Record<StarterLanguage, string>;
  /** The name in each language of the text property the starter fills in, with its value. */
  readonly textProperty?: Record<StarterLanguage, readonly [name: string, value: string]>;
  readonly toggles: boolean;
}

const EXPECTED: Record<string, Expected> = {
  "meeting-notes": {
    icon: "📝",
    titles: {
      en: `Meeting notes ${TODAY}`,
      fr: `Notes de réunion ${TODAY}`,
      es: `Notas de reunión ${TODAY}`,
    },
    templateTitles: {
      en: "Meeting notes {{date}}",
      fr: "Notes de réunion {{date}}",
      es: "Notas de reunión {{date}}",
    },
    dateProperty: { en: "Date", fr: "Date", es: "Fecha" },
    toggles: true,
  },
  "daily-journal": {
    icon: "📔",
    titles: { en: `Journal ${TODAY}`, fr: `Journal ${TODAY}`, es: `Diario ${TODAY}` },
    templateTitles: { en: "Journal {{date}}", fr: "Journal {{date}}", es: "Diario {{date}}" },
    dateProperty: { en: "Date", fr: "Date", es: "Fecha" },
    toggles: true,
  },
  "weekly-review": {
    icon: "📅",
    titles: {
      en: `Weekly review ${TODAY}`,
      fr: `Bilan de la semaine ${TODAY}`,
      es: `Revisión semanal ${TODAY}`,
    },
    templateTitles: {
      en: "Weekly review {{date}}",
      fr: "Bilan de la semaine {{date}}",
      es: "Revisión semanal {{date}}",
    },
    dateProperty: { en: "Date", fr: "Date", es: "Fecha" },
    toggles: true,
  },
  "project-brief": {
    icon: "🎯",
    titles: { en: "Project brief", fr: "Fiche projet", es: "Resumen del proyecto" },
    templateTitles: { en: "Project brief", fr: "Fiche projet", es: "Resumen del proyecto" },
    textProperty: {
      en: ["Status", "Draft"],
      fr: ["Statut", "Brouillon"],
      es: ["Estado", "Borrador"],
    },
    toggles: true,
  },
  "reading-notes": {
    icon: "📚",
    titles: { en: "Reading notes", fr: "Notes de lecture", es: "Notas de lectura" },
    templateTitles: { en: "Reading notes", fr: "Notes de lecture", es: "Notas de lectura" },
    dateProperty: { en: "Started", fr: "Début", es: "Inicio" },
    textProperty: {
      en: ["Status", "Reading"],
      fr: ["Statut", "En cours"],
      es: ["Estado", "Leyendo"],
    },
    toggles: false,
  },
};

const IDS = Object.keys(EXPECTED);

interface Fixture {
  readonly client: CoreClient;
  readonly spaceId: SpaceId;
  readonly parentId: PageId;
}

async function setup(): Promise<Fixture> {
  const transport = new MemoryTransport({
    workspaceName: "Starters",
    spaces: [{ name: "Home", pages: [{ title: "Existing" }] }],
  });
  const client = new CoreClient(transport);
  const space = (await client.spaceList())[0];
  if (space === undefined) {
    throw new Error("no space");
  }
  const parent = (await client.treeGet(space.id)).roots[0]?.page;
  if (parent === undefined) {
    throw new Error("no page");
  }
  return { client, spaceId: space.id, parentId: parent.id };
}

/** The titles of the pages at the top level of the space, and the number of pages in all. */
async function topLevel(client: CoreClient, spaceId: SpaceId): Promise<{ titles: string[]; pages: number }> {
  const tree = await client.treeGet(spaceId);
  const count = (nodes: typeof tree.roots): number => nodes.reduce((sum, node) => sum + 1 + count(node.children), 0);
  return { titles: tree.roots.map((node) => node.page.title), pages: count(tree.roots) };
}

function starter(id: string) {
  const found = STARTER_TEMPLATES.find((candidate) => candidate.id === id);
  if (found === undefined) {
    throw new Error(`no starter ${id}`);
  }
  return found;
}

describe("the list of starters", () => {
  it("has the five starters in the order the pickers show them", () => {
    expect(STARTER_TEMPLATES.map((s) => s.id)).toEqual([
      "meeting-notes",
      "daily-journal",
      "weekly-review",
      "project-brief",
      "reading-notes",
    ]);
    for (const s of STARTER_TEMPLATES) {
      expect(s.icon).toBe(EXPECTED[s.id]?.icon);
    }
  });

  it("names and describes each starter in every language, and in English for another", () => {
    const names = new Set<string>();
    for (const s of STARTER_TEMPLATES) {
      for (const language of STARTER_LANGUAGES) {
        expect(s.title(language).trim()).not.toBe("");
        expect(s.description(language).trim()).not.toBe("");
        expect(s.title(language)).not.toContain("{{");
        names.add(`${language}:${s.title(language)}`);
      }
      expect(s.title("de")).toBe(s.title("en"));
      expect(s.description("pt-BR")).toBe(s.description("en"));
      expect(s.title("fr")).not.toBe(s.title("en"));
    }
    expect(names.size).toBe(STARTER_TEMPLATES.length * STARTER_LANGUAGES.length);
    expect(starter("meeting-notes").title("en")).toBe("Meeting notes");
    expect(starter("meeting-notes").title("fr")).toBe("Notes de réunion");
    expect(starter("meeting-notes").title("es")).toBe("Notas de reunión");
  });

  it("reads a regional or upper-case language as its language", () => {
    expect(starterLanguage("fr-CA")).toBe("fr");
    expect(starterLanguage("es_MX")).toBe("es");
    expect(starterLanguage("EN-gb")).toBe("en");
    expect(starterLanguage("")).toBe("en");
    expect(starterLanguage("ja")).toBe("en");
    expect(starter("daily-journal").title("fr-CA")).toBe("Journal quotidien");
  });
});

describe("the tokens of a starter", () => {
  it("resolves the title and the date tokens, or keeps the title's for a template", () => {
    const text = 'title: "A {{date}} {{time}}"\nDate: "{{today}}"\n<time datetime="{{today}} {{now}}">x</time>';
    expect(resolveStarterTokens(text, { today: TODAY, now: NOW }, false)).toBe(
      `title: "A ${TODAY} ${NOW}"\nDate: "${TODAY}"\n<time datetime="${TODAY} ${NOW}">x</time>`,
    );
    expect(resolveStarterTokens(text, { today: TODAY, now: NOW }, true)).toBe(
      `title: "A {{date}} {{time}}"\nDate: "${TODAY}"\n<time datetime="${TODAY} ${NOW}">x</time>`,
    );
  });

  it("finds the front matter properties that hold the day or the moment of creation", () => {
    const text = '---\ntitle: "T {{date}}"\nDate: "{{today}}"\nDébut de lecture: "{{today}} {{now}}"\nStatus: "Draft"\n---\n\nbody "{{today}}"';
    expect(starterDatePlaceholders(text)).toEqual([
      ["Date", "today"],
      ["Début de lecture", "now"],
    ]);
    expect(starterDatePlaceholders("no front matter\nDate: \"{{today}}\"")).toEqual([]);
  });

  it("is written the same way in every language: the same tokens, the front matter first, an icon and a title", () => {
    for (const id of IDS) {
      const counts = new Set<string>();
      for (const language of STARTER_LANGUAGES) {
        const markdown = resolveStarterTokens(
          sourceMarkdown(id, language),
          { today: TODAY, now: NOW },
          false,
        );
        expect(markdown).not.toContain("{{");
        expect(markdown.startsWith('---\ntitle: "')).toBe(true);
        expect(markdown).toContain(`\nicon: "${EXPECTED[id]?.icon ?? "?"}"\n`);
        counts.add(
          ["{{today}}", "{{date}}", "<details>", "- [ ]"]
            .map((token) => String(sourceMarkdown(id, language).split(token).length - 1))
            .join(","),
        );
      }
      // The three versions have the same number of dates, toggles and tasks.
      expect(counts.size).toBe(1);
    }
  });
});

describe("the French text", () => {
  it("has a no-break space before a question mark or a colon, never a plain one", () => {
    for (const id of IDS) {
      const french = sourceMarkdown(id, "fr");
      expect(french).not.toMatch(/ [?!:;]/u);
    }
    expect(sourceMarkdown("meeting-notes", "fr")).toContain(`présent${NBSP}?`);
    expect(sourceMarkdown("project-brief", "fr")).toContain(`En une phrase${NBSP}:`);
    expect(starter("daily-journal").description("fr")).toContain(`par jour${NBSP}:`);
  });
});

/** The Markdown of a starter in a language, read from the module the starters are made of. */
function sourceMarkdown(id: string, language: StarterLanguage): string {
  const found = SOURCE_TEXTS.find((source) => source.id === id);
  if (found === undefined) {
    throw new Error(`no source ${id}`);
  }
  return found.markdown[language];
}

const SOURCE_TEXTS = [MEETING_NOTES, DAILY_JOURNAL, WEEKLY_REVIEW, PROJECT_BRIEF, READING_NOTES];

describe("making a page from a starter", () => {
  for (const id of IDS) {
    for (const language of STARTER_LANGUAGES) {
      it(`${id} in ${language} imports as one page with its title, icon, properties and content`, async () => {
        const { client, spaceId } = await setup();
        const before = await topLevel(client, spaceId);
        const expected = EXPECTED[id];
        if (expected === undefined) {
          throw new Error("no expectation");
        }

        const page = await instantiateStarter(client, id, { spaceId, language, today: TODAY, now: NOW });

        expect(page.title).toBe(expected.titles[language]);
        expect(page.icon).toBe(expected.icon);
        expect(page.template).toBeFalsy();
        expect(page.spaceId).toBe(spaceId);
        const after = await topLevel(client, spaceId);
        expect(after.pages).toBe(before.pages + 1);
        expect(after.titles).toEqual([...before.titles, expected.titles[language]]);
        expect(await client.pageGet(page.id)).toEqual(page);

        const schema = await client.propertySchemaGet(spaceId);
        const values = await client.pagePropertiesGet(page.id);
        const valueOf = (name: string) => {
          const def = schema.find((property) => property.name === name);
          return values.find(([propertyId]) => propertyId === def?.id)?.[1];
        };
        if (expected.dateProperty !== undefined) {
          const name = expected.dateProperty[language];
          expect(schema.find((property) => property.name === name)?.kind).toBe("date");
          expect(valueOf(name)).toEqual({ kind: "date", value: { date: TODAY } });
        }
        if (expected.textProperty !== undefined) {
          const [name, value] = expected.textProperty[language];
          expect(valueOf(name)).toEqual({ kind: "text", value });
        }

        const markdown = await client.pageExportMarkdown(page.id);
        expect(markdown).not.toContain("{{");
        expect(markdown).toContain("- [ ] ");
        expect(markdown.includes("<details>")).toBe(expected.toggles);
        expect(markdown).toMatch(/^## /mu);
      });
    }
  }

  it("answers the page under the parent when one is given", async () => {
    const { client, spaceId, parentId } = await setup();

    const page = await instantiateStarter(client, "project-brief", {
      spaceId,
      parentId,
      language: "en",
      today: TODAY,
      now: NOW,
    });

    const tree = await client.treeGet(spaceId);
    expect(tree.roots).toHaveLength(1);
    expect(tree.roots[0]?.children.map((node) => node.page.id)).toEqual([page.id]);
  });

  it("makes a page of every starter side by side, sharing the properties they have in common", async () => {
    const { client, spaceId } = await setup();

    for (const id of IDS) {
      await instantiateStarter(client, id, { spaceId, language: "en", today: TODAY, now: NOW });
    }

    const { titles } = await topLevel(client, spaceId);
    expect(titles).toEqual([
      "Existing",
      `Meeting notes ${TODAY}`,
      `Journal ${TODAY}`,
      `Weekly review ${TODAY}`,
      "Project brief",
      "Reading notes",
    ]);
    const names = (await client.propertySchemaGet(spaceId)).map((property) => property.name);
    expect(names.filter((name) => name === "Date")).toHaveLength(1);
    expect(names.filter((name) => name === "Status")).toHaveLength(1);
    expect(names).toContain("Started");
  });

  it("writes the language's text, English when the starters lack it", async () => {
    const { client, spaceId } = await setup();

    const fallback = await instantiateStarter(client, "reading-notes", { spaceId, language: "de", today: TODAY, now: NOW });
    const regional = await instantiateStarter(client, "reading-notes", { spaceId, language: "fr-CA", today: TODAY, now: NOW });

    expect(fallback.title).toBe("Reading notes");
    expect(regional.title).toBe("Notes de lecture");
    expect(await client.pageExportMarkdown(fallback.id)).toContain("## Key ideas");
    expect(await client.pageExportMarkdown(regional.id)).toContain("## Idées clés");
  });

  it("rejects an unknown starter and a malformed day or time before anything is made", async () => {
    const { client, spaceId } = await setup();
    const before = await topLevel(client, spaceId);

    await expect(instantiateStarter(client, "nope", { spaceId, language: "en", today: TODAY, now: NOW })).rejects.toThrow(/unknown starter/u);
    await expect(instantiateStarter(client, "daily-journal", { spaceId, language: "en", today: "18/10/2026", now: NOW })).rejects.toThrow(/YYYY-MM-DD/u);
    await expect(instantiateStarter(client, "daily-journal", { spaceId, language: "en", today: TODAY, now: "2:30 PM" })).rejects.toThrow(/HH:mm/u);

    expect(await topLevel(client, spaceId)).toEqual(before);
  });

  it("rejects, and leaves no import session open, when the parent does not exist", async () => {
    const { client, spaceId } = await setup();
    const before = await topLevel(client, spaceId);

    await expect(
      instantiateStarter(client, "daily-journal", {
        spaceId,
        parentId: "00000000-0000-4000-8000-00000000dead",
        language: "en",
        today: TODAY,
        now: NOW,
      }),
    ).rejects.toMatchObject({ code: "NotFound" });

    expect(await topLevel(client, spaceId)).toEqual(before);
  });
});

describe("adding a starter as a template", () => {
  /** A client whose template requests answer with the page unchanged (the in-memory core may not implement them yet). */
  function mocked(client: CoreClient) {
    const setTemplate = vi.spyOn(client, "pageSetTemplate").mockImplementation((pageId: PageId): Promise<PageInfo> => client.pageGet(pageId));
    const setPlaceholder = vi
      .spyOn(client, "pageSetTemplatePlaceholder")
      .mockImplementation((pageId: PageId): Promise<PageInfo> => client.pageGet(pageId));
    return { setTemplate, setPlaceholder };
  }

  for (const language of STARTER_LANGUAGES) {
    it(`keeps {{date}} in the title of the meeting notes in ${language} and flags the page`, async () => {
      const { client, spaceId } = await setup();
      const { setTemplate, setPlaceholder } = mocked(client);

      const page = await instantiateStarter(client, "meeting-notes", { spaceId, language, asTemplate: true, today: TODAY, now: NOW });

      expect(page.title).toBe(EXPECTED["meeting-notes"]?.templateTitles[language]);
      expect(page.icon).toBe("📝");
      expect(setTemplate).toHaveBeenCalledTimes(1);
      expect(setTemplate).toHaveBeenCalledWith(page.id, true);
      // The Date property held the day of the import, and becomes the placeholder "today".
      const dateName = EXPECTED["meeting-notes"]?.dateProperty?.[language];
      const date = (await client.propertySchemaGet(spaceId)).find((property) => property.name === dateName);
      expect(date?.kind).toBe("date");
      expect(setPlaceholder).toHaveBeenCalledTimes(1);
      expect(setPlaceholder).toHaveBeenCalledWith(page.id, date?.id, "today");
      expect((await topLevel(client, spaceId)).pages).toBe(2);
    });
  }

  it("flags every starter and puts a placeholder on each date property it has, none on the others", async () => {
    for (const id of IDS) {
      const { client, spaceId } = await setup();
      const { setTemplate, setPlaceholder } = mocked(client);

      const page = await instantiateStarter(client, id, { spaceId, language: "es", asTemplate: true, today: TODAY, now: NOW });

      expect(page.title).toBe(EXPECTED[id]?.templateTitles.es);
      expect(setTemplate).toHaveBeenCalledWith(page.id, true);
      expect(setPlaceholder).toHaveBeenCalledTimes(EXPECTED[id]?.dateProperty === undefined ? 0 : 1);
    }
  });

  it("answers the page the placeholder request answered", async () => {
    const { client, spaceId } = await setup();
    mocked(client);
    const flagged = { template: true } as const;
    vi.spyOn(client, "pageSetTemplatePlaceholder").mockImplementation(async (pageId: PageId) => ({
      ...(await client.pageGet(pageId)),
      ...flagged,
      templatePlaceholders: [["date-property", "today"]],
    }));

    const page = await instantiateStarter(client, "daily-journal", { spaceId, language: "en", asTemplate: true, today: TODAY, now: NOW });

    expect(page.template).toBe(true);
    expect(page.templatePlaceholders).toEqual([["date-property", "today"]]);
  });

  it("still answers the template when a placeholder cannot be set", async () => {
    const { client, spaceId } = await setup();
    const { setTemplate } = mocked(client);
    vi.spyOn(client, "pageSetTemplatePlaceholder").mockRejectedValue(new Error("refused"));

    const page = await instantiateStarter(client, "weekly-review", { spaceId, language: "en", asTemplate: true, today: TODAY, now: NOW });

    expect(page.title).toBe("Weekly review {{date}}");
    expect(setTemplate).toHaveBeenCalledWith(page.id, true);
  });

  it("does not flag a page it made as a page", async () => {
    const { client, spaceId } = await setup();
    const { setTemplate, setPlaceholder } = mocked(client);

    await instantiateStarter(client, "meeting-notes", { spaceId, language: "en", today: TODAY, now: NOW });

    expect(setTemplate).not.toHaveBeenCalled();
    expect(setPlaceholder).not.toHaveBeenCalled();
  });

  it("skips the placeholder of a date property the space already has as another kind", async () => {
    const transport = new MemoryTransport({
      workspaceName: "Starters",
      spaces: [{ name: "Home", properties: [{ name: "Date", kind: "text" }] }],
    });
    const client = new CoreClient(transport);
    const space = (await client.spaceList())[0];
    if (space === undefined) {
      throw new Error("no space");
    }
    const { setTemplate, setPlaceholder } = mocked(client);

    const page = await instantiateStarter(client, "meeting-notes", {
      spaceId: space.id,
      language: "en",
      asTemplate: true,
      today: TODAY,
      now: NOW,
    });

    expect(page.title).toBe("Meeting notes {{date}}");
    expect(setTemplate).toHaveBeenCalledWith(page.id, true);
    expect(setPlaceholder).not.toHaveBeenCalled();
  });
});
