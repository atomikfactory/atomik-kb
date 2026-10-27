// What a page can be made from (ADR-0056): a template page of the space, or one of the built-in starters. The Templates panel,
// the compact picker and the editor's `/template` list the same choices and make the page the same way.

import type { PageId, PageInfo } from "@atomik-kb/core-client";

import { activeLanguage } from "./i18n";
import { STARTER_TEMPLATES } from "./starter-templates";
import type { AppState } from "./store";

/** A template of the space, or a starter that ships with the app. */
export type TemplateChoice =
  | { kind: "template"; templateId: PageId }
  | { kind: "starter"; starterId: string };

/** The part of the store that makes a page from a choice. */
type Maker = Pick<AppState, "createFromTemplate" | "createFromStarter">;

export interface MakeOptions {
  /** The parent of the new page; the top level when absent. */
  parentId?: PageId;
  /** Open the new page in the active tab (default). */
  open?: boolean;
}

/** Makes a page from a choice: a copy of the template (its placeholders resolved), or the starter. Rejects with a `CoreError`. */
export function createFromChoice(maker: Maker, choice: TemplateChoice, options: MakeOptions = {}): Promise<PageInfo> {
  const { parentId, open } = options;
  const shared = { ...(parentId === undefined ? {} : { parentId }), ...(open === undefined ? {} : { open }) };
  return choice.kind === "template"
    ? maker.createFromTemplate(choice.templateId, shared)
    : maker.createFromStarter(choice.starterId, shared);
}

/** One built-in starter as the lists show it, in the language of the app. */
export interface StarterEntry {
  id: string;
  icon: string;
  title: string;
  description: string;
}

/** The starters in the order the lists give them (none until they ship). */
export function starterEntries(): StarterEntry[] {
  const language = activeLanguage();
  return STARTER_TEMPLATES.map((starter) => ({
    id: starter.id,
    icon: starter.icon,
    title: starter.title(language),
    description: starter.description(language),
  }));
}
