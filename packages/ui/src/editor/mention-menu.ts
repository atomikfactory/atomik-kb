// `@` opens the mention menu: Today, Tomorrow and "Pick a date..." first, then the people of the space that match
// what was typed (its members, `findPeople`: ADR-0049; a personal space lists the person themself), then the pages that
// match (the source of the `[[` picker, `findPages`). Choosing a day inserts a date mention (`dateMention`);
// "Pick a date..." inserts today's and opens the picker on it; choosing a person inserts a `personMention` chip (the
// account id and the name as the fallback label); choosing a page inserts a `pageRef` chip exactly as `[[` does. `[[` is
// unchanged. Escape closes the menu and leaves what was typed. `/mention` of the `/` menu types the `@` for you
// (`openMentionMenu`). On a page in a template (ADR-0056) the people start with "Me (whoever creates)": a person mention of
// this account with `relative: "me"`, filled in with whoever makes a page from the template.

import { Extension } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";
import { PluginKey } from "@tiptap/pm/state";
import { exitSuggestion, Suggestion } from "@tiptap/suggestion";

import type { PersonInfo } from "@atomik-kb/core-client";

import { t } from "../i18n";
import type { MessageKey } from "../i18n";
import type { PageChoice } from "../store-content";
import { takesInlineAtom } from "./date-mention";
import { addDays, todayIso } from "./dates";
import { personLabel } from "./person-mention";
import type { SuggestionMenuStore } from "./suggestion-menu";

/** One line of the menu: a day to mention, the calendar, a person of the space, or a page. */
export type MentionChoice =
  | { kind: "today" }
  | { kind: "tomorrow" }
  | { kind: "pick" }
  | { kind: "person"; person: PersonInfo }
  /** "Me (whoever creates)" of a page in a template (ADR-0056): `person` is this account, the mention's written value. */
  | { kind: "me"; person: PersonInfo }
  | { kind: "page"; page: PageChoice };

/** The groups of the menu, shown as headers while nothing is typed. */
export type MentionSection = "dates" | "people" | "pages";

export function mentionSection(choice: MentionChoice): MentionSection {
  return choice.kind === "page" ? "pages" : choice.kind === "person" || choice.kind === "me" ? "people" : "dates";
}

/** A key that tells two choices apart (the day entries and "me" are the same whatever is typed). */
export function mentionItemKey(choice: MentionChoice): string {
  return choice.kind === "page"
    ? `page:${choice.page.pageId}`
    : choice.kind === "person"
      ? `person:${choice.person.accountId}`
      : choice.kind;
}

/** Most people the menu lists at once. */
export const PEOPLE_LIMIT = 8;

/**
 * The people that match what was typed after `@`, as menu lines: all of them (up to {@link PEOPLE_LIMIT}) when nothing is typed,
 * else the ones whose name holds it, those whose name or one of whose words starts with it first. Case is ignored.
 */
export function personChoices(people: readonly PersonInfo[], query: string): MentionChoice[] {
  const wanted = query.trim().toLowerCase();
  const found = people.filter((person) => wanted === "" || person.name.toLowerCase().includes(wanted));
  const starts = (person: PersonInfo): boolean =>
    wanted !== "" && person.name.toLowerCase().split(/\s+/u).some((word) => word.startsWith(wanted));
  return [...found.filter(starts), ...found.filter((person) => !starts(person))]
    .slice(0, PEOPLE_LIMIT)
    .map((person): MentionChoice => ({ kind: "person", person }));
}

/**
 * "Me (whoever creates)" for the `@` menu of a page in a template: offered when nothing is typed or what is typed starts a word of
 * its label ("me", "who", "creat"), and only when this account is among `people` (the mention keeps its account and name as
 * the value it was written with). Case is ignored.
 */
export function meChoices(people: readonly PersonInfo[], query: string): MentionChoice[] {
  const me = people.find((person) => person.isMe);
  if (me === undefined) {
    return [];
  }
  const wanted = query.trim().toLowerCase();
  const words = t("template.mention.me").toLowerCase().split(/[\s()]+/u).filter((word) => word !== "");
  return wanted === "" || words.some((word) => word.startsWith(wanted)) ? [{ kind: "me", person: me }] : [];
}

interface DateEntry {
  choice: MentionChoice;
  label: MessageKey;
  /** Extra words that find the entry (lowercase). */
  keywords: readonly string[];
}

/** The date entries, in the order the menu lists them. */
const DATE_ENTRIES: readonly DateEntry[] = [
  { choice: { kind: "today" }, label: "date.today", keywords: ["now"] },
  { choice: { kind: "tomorrow" }, label: "date.tomorrow", keywords: [] },
  { choice: { kind: "pick" }, label: "mention.pick", keywords: ["date", "calendar", "day", "when"] },
];

function normalise(text: string): string {
  return text.toLowerCase().replace(/\s+/g, "").replace(/…|\.{3}/g, "");
}

/**
 * The date entries that match what was typed after `@`, in the menu's order: all three when
 * nothing is typed, else the ones whose label or one of whose words starts with it (`to` finds
 * Today and Tomorrow, `tom` Tomorrow, `pick` and `cal` the calendar). Case and spaces are ignored.
 */
export function dateChoices(query: string): MentionChoice[] {
  const wanted = normalise(query);
  return DATE_ENTRIES.filter(
    ({ label, keywords }) =>
      wanted === "" ||
      normalise(t(label)).startsWith(wanted) ||
      keywords.some((word) => word.startsWith(wanted)),
  ).map(({ choice }) => choice);
}


/** Types the `@` where the `/mention` command was, which opens the menu: the `[[` picker is opened the same way. */
export function openMentionMenu(editor: Editor, range: Range): void {
  editor.chain().focus().deleteRange(range).insertContent("@").run();
}

export interface MentionMenuOptions {
  /** Where the menu shows; `null` disables it (schema-only editors). */
  store: SuggestionMenuStore<MentionChoice> | null;
  /** The pages to offer for what was typed after `@` (the `[[` picker's). */
  findPages: (query: string) => Promise<PageChoice[]>;
  /** The people of the page's space (its members, this account among them); none offered by default. */
  findPeople: () => Promise<readonly PersonInfo[]>;
  /** Whether the page is a template or inside one (ADR-0056), asked at every query; no by default. */
  inTemplate: () => boolean;
}

export const mentionMenuKey = new PluginKey("mentionMenu");

/** The longest text after `@` that is still a search (a sentence after a stray `@` is not). */
const MAX_QUERY = 60;

/** Inserts a date mention in place of the typed `@query`. */
function insertDate(editor: Editor, range: Range, date: string, openPicker: boolean): void {
  editor.chain().focus().deleteRange(range).insertDateMention({ date, openPicker }).run();
}

export const MentionMenu = Extension.create<MentionMenuOptions>({
  name: "mentionMenu",

  addOptions() {
    return {
      store: null,
      findPages: () => Promise.resolve([]),
      findPeople: () => Promise.resolve([]),
      inTemplate: () => false,
    };
  },

  addProseMirrorPlugins() {
    const { store, findPages, findPeople, inTemplate } = this.options;
    if (store === null) {
      return [];
    }
    return [
      Suggestion<MentionChoice, MentionChoice>({
        editor: this.editor,
        pluginKey: mentionMenuKey,
        char: "@",
        // A page title has spaces; the menu ends when a query with one finds nothing (below).
        allowSpaces: true,
        // After a space or at the start of a block: `ben@example.com` is not a mention.
        allowedPrefixes: [" "],
        // The days are there before the pages are found.
        initialItems: dateChoices(""),
        items: async ({ query }) => {
          let pages: PageChoice[] = [];
          let people: readonly PersonInfo[] = [];
          try {
            [pages, people] = await Promise.all([findPages(query.trim()), findPeople().catch((): PersonInfo[] => [])]);
          } catch {
            // The days are still offered.
          }
          return [
            ...dateChoices(query),
            ...(inTemplate() ? meChoices(people, query) : []),
            ...personChoices(people, query),
            ...pages.map((page): MentionChoice => ({ kind: "page", page })),
          ];
        },
        command: ({ editor, range, props }) => {
          switch (props.kind) {
            case "today":
              insertDate(editor, range, todayIso(), false);
              return;
            case "tomorrow":
              insertDate(editor, range, addDays(todayIso(), 1), false);
              return;
            case "pick":
              // Today's date, with its picker open: a day picked there replaces it.
              insertDate(editor, range, todayIso(), true);
              return;
            case "person":
              editor
                .chain()
                .focus()
                .insertContentAt(range, [
                  { type: "personMention", attrs: { accountId: props.person.accountId, label: personLabel(props.person.name) } },
                  { type: "text", text: " " },
                ])
                .run();
              return;
            case "me":
              editor
                .chain()
                .focus()
                .insertContentAt(range, [
                  {
                    type: "personMention",
                    attrs: { accountId: props.person.accountId, label: personLabel(props.person.name), relative: "me" },
                  },
                  { type: "text", text: " " },
                ])
                .run();
              return;
            case "page":
              editor
                .chain()
                .focus()
                .insertContentAt(range, [
                  { type: "pageRef", attrs: { pageId: props.page.pageId } },
                  { type: "text", text: " " },
                ])
                .run();
              return;
          }
        },
        allow: ({ editor }) =>
          takesInlineAtom(editor, "dateMention") || takesInlineAtom(editor, "pageRef") || takesInlineAtom(editor, "personMention"),
        // `@ ` (a space right after it) is a stray at sign, and a long text is no search.
        shouldShow: ({ query }) => !/^\s/u.test(query) && query.length <= MAX_QUERY,
        render: () => {
          const renderer = store.renderer();
          return {
            ...renderer,
            onUpdate: (props) => {
              renderer.onUpdate?.(props);
              // A query with a space that finds nothing is not a mention: the menu ends there, and
              // the text stays as typed.
              if (props.items.length === 0 && /\s/u.test(props.query.trim())) {
                exitSuggestion(props.editor.view, mentionMenuKey);
              }
            },
          };
        },
      }),
    ];
  },
});
