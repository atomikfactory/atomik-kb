// "New sub-page" in the `/` menu: creates a child page of the open page, puts a `pageLink` card
// to it where the command was typed and opens it, as Notion does. The same service sorts the open
// page's sub-pages once ("Sort subpages", ADR-0052). The editor knows nothing of
// the app's pages: the page editor hands it a service (`SubPages`, `createEditorExtensions`),
// and a schema-only editor has none, which makes the commands unavailable. `/template` (ADR-0056) is the same flow with a page
// made from a template or a starter: the service asks which one (a small picker at the caret) and makes the page.

import { Extension } from "@tiptap/core";
import type { Editor, Range } from "@tiptap/core";

import type { Anchor } from "../components/FloatingPanel";
import type { MessageKey } from "../i18n";
import type { SortKey } from "../sort-order";
import type { TemplateChoice } from "../template-choices";
import { insertPageLinkCard } from "./insert-page-link";
import { BLOCK_ID_ATTRIBUTE } from "./schema";

/** What `/template` needs of the app (ADR-0056). */
export interface SubPageTemplates {
  /** Asks which template or starter to use (a picker opens at `anchor`); resolves with the choice, `null` when it was dismissed. */
  choose: (anchor: Anchor) => Promise<TemplateChoice | null>;
  /** Makes a page under the open one from the choice (it shows in the sidebar) without opening it, and resolves with its id; rejects when it could not be made. */
  create: (choice: TemplateChoice) => Promise<string>;
  /** Opens a page `create` made, once the open page is saved. */
  open: (pageId: string) => Promise<void>;
}

export interface SubPageService {
  /** `/template`; absent where templates cannot be used. */
  templates?: SubPageTemplates;
  /**
   * Creates a page under the open one (it shows in the sidebar, untitled) without opening it,
   * and resolves with its id; rejects when it could not be created.
   */
  create: () => Promise<string>;
  /** Opens a page `create` made, with its title ready to be typed, once the open page is saved. */
  open: (pageId: string) => Promise<void>;
  /** How many live sub-pages the open page has right now (sorting them needs two). */
  count: () => number;
  /** Sorts the open page's sub-pages once (ADR-0052); the service reports a failure itself. */
  sort: (by: SortKey) => Promise<void>;
}

export interface SubPagesOptions {
  /** `null` adds nothing (schema-only editors): the command is then unavailable. */
  service: SubPageService | null;
}

export interface SubPagesStorage {
  service: SubPageService | null;
}

declare module "@tiptap/core" {
  interface Storage {
    subPages: SubPagesStorage;
  }
}

export const SubPages = Extension.create<SubPagesOptions, SubPagesStorage>({
  name: "subPages",

  addOptions() {
    return { service: null };
  },

  addStorage() {
    return { service: this.options.service };
  },
});

/** The service of an editor, if it has the extension and a service. */
function serviceOf(editor: Editor): SubPageService | null {
  // Absent in editors built without the extension (schema-only uses).
  const storage = (editor.storage as unknown as { subPages?: SubPagesStorage }).subPages;
  return storage?.service ?? null;
}

/** Why "New sub-page" cannot run in this editor (read-only, or no page behind it), else `null`. */
export function subPageUnavailable(editor: Editor): MessageKey | null {
  if (!editor.isEditable) {
    return "slash.subpage.readOnly";
  }
  return serviceOf(editor) === null ? "slash.subpage.noPage" : null;
}

/** Why `/template` cannot run in this editor (read-only, or no page behind it), else `null`. */
export function templatePageUnavailable(editor: Editor): MessageKey | null {
  if (!editor.isEditable) {
    return "slash.subpage.readOnly";
  }
  return serviceOf(editor)?.templates === undefined ? "slash.subpage.noPage" : null;
}

/** Why "Sort subpages" cannot run in this editor (read-only, no page behind it, fewer than two sub-pages), else `null`. */
export function sortSubpagesUnavailable(editor: Editor): MessageKey | null {
  if (!editor.isEditable) {
    return "slash.subpage.readOnly";
  }
  const service = serviceOf(editor);
  if (service === null) {
    return "slash.subpage.noPage";
  }
  return service.count() < 2 ? "slash.sortSubpages.few" : null;
}

/** Clears the typed `/sort...` and sorts the open page's sub-pages by `by` (the sidebar's action, once). */
export async function sortSubpages(editor: Editor, range: Range, by: SortKey): Promise<void> {
  const service = serviceOf(editor);
  if (service === null) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  await service.sort(by);
}

/**
 * Where the card goes after the page was created: where the caret was in the block the command
 * was typed in (found again by its id, the caret clamped to what is left of the block), else the
 * end of the page.
 */
function cardPosition(editor: Editor, blockId: unknown, offset: number): number {
  const { doc } = editor.state;
  let found: number | null = null;
  if (typeof blockId === "string") {
    doc.descendants((node, pos) => {
      if (found !== null) {
        return false;
      }
      if (node.attrs[BLOCK_ID_ATTRIBUTE] === blockId) {
        found = node.isTextblock ? pos + 1 + Math.min(offset, node.content.size) : pos;
        return false;
      }
      return true;
    });
  }
  return found ?? doc.content.size;
}

/** Where a picker for the caret opens: below the block the caret is in. */
function caretAnchor(editor: Editor): Anchor {
  try {
    const { node } = editor.view.domAtPos(editor.state.selection.from);
    const element = node instanceof Element ? node : node.parentElement;
    const rect = element?.getBoundingClientRect();
    return rect === undefined ? { x: 0, y: 0 } : { x: rect.left, y: rect.bottom + 4 };
  } catch {
    return { x: 0, y: 0 };
  }
}

/**
 * Clears the typed `/template`, asks which template or starter to use, makes the child page from it, puts a card to it where the
 * command was typed and opens the page (ADR-0056): `/page` with a page that starts from a template. Nothing is inserted when
 * the picker is dismissed or the page cannot be made (the service reports it).
 */
export async function insertSubPageFromTemplate(editor: Editor, range: Range): Promise<void> {
  const templates = serviceOf(editor)?.templates;
  if (templates === undefined) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  const { $from } = editor.state.selection;
  const blockId: unknown = $from.parent.attrs[BLOCK_ID_ATTRIBUTE];
  const offset = $from.parentOffset;
  const choice = await templates.choose(caretAnchor(editor));
  if (editor.isDestroyed) {
    return;
  }
  if (choice === null) {
    editor.commands.focus();
    return;
  }
  let pageId: string;
  try {
    pageId = await templates.create(choice);
  } catch {
    return;
  }
  if (editor.isDestroyed) {
    return; // the user went elsewhere meanwhile: the page exists, the card has nowhere to go
  }
  insertPageLinkCard(editor, { pageId }, cardPosition(editor, blockId, offset));
  await templates.open(pageId);
}

/**
 * Clears the typed `/page`, creates the child page, puts a card to it in place of the empty
 * paragraph the command was typed in (after the block when there was text in it), and opens
 * the new page. The page is created first, so that the card never links to nothing; the block
 * is found again by its id afterwards, since the user may have typed meanwhile. Does nothing
 * when the editor has no service, or when the page cannot be created (the service reports it);
 * nothing is inserted and nobody navigates then.
 */
export async function insertSubPage(editor: Editor, range: Range): Promise<void> {
  const service = serviceOf(editor);
  if (service === null) {
    return;
  }
  editor.chain().focus().deleteRange(range).run();
  const { $from } = editor.state.selection;
  const blockId: unknown = $from.parent.attrs[BLOCK_ID_ATTRIBUTE];
  const offset = $from.parentOffset;
  let pageId: string;
  try {
    pageId = await service.create();
  } catch {
    return;
  }
  if (editor.isDestroyed) {
    return; // the user went elsewhere meanwhile: the page exists, the card has nowhere to go
  }
  insertPageLinkCard(editor, { pageId }, cardPosition(editor, blockId, offset));
  await service.open(pageId);
}
