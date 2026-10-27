// Which pages a database block's source lists (ADR-0016, ADR-0054, and ADR-0058 for the linked sources;
// `docs/architecture/database-power-ups.md` section 2). Pure functions over the page tree, so that the node view
// (`PageListView.tsx`), the template actions and the tests share them:
//
// - `children`: the sub-pages of the page that holds the block;
// - `tag`: the pages of the space that carry one Tags option (the ids come from the core, `useTaggedPageIds`);
// - `page`: the sub-pages of another page of the space (a linked view of that page's database);
// - `space`: every page of the space, at any depth, except the page that holds the block (the view's filters narrow it).
//
// The pages returned still hold the templates and the pages inside one: `splitTemplates` (`database-templates.ts`) takes
// them out of the rows and gives the templates of the source for the menu next to "New", for every kind alike. Trashed
// pages are never in the tree, so they are never listed.

import type { PageId, PageInfo, PageNode, TreeView } from "@atomik-kb/core-client";

import { locate } from "../tree";
import { hiddenTemplatePageIds } from "./database-templates";
import { childPages, pagesIn } from "./page-list-model";
import type { PageListSource } from "./page-list-model";

const NO_IDS: ReadonlySet<PageId> = new Set();

/** The pages of a source, and whether the source points at a page that cannot be used. */
export interface SourcePages {
  /** In tree order (depth first); templates among them are told apart by `splitTemplates`. */
  readonly pages: PageInfo[];
  /**
   * True for a `page` source whose page is not a live page of the space (gone, trashed, in another space) or is a template
   * or inside one: the view says its pages are no longer available and lists nothing. Never true for the other kinds, nor
   * while the tree is not loaded.
   */
  readonly missing: boolean;
}

/**
 * The pages of `source` for the block held by the page `hostId` (`null` when unknown): `taggedIds` are the pages that carry
 * the option of a tag source (`null` while the answer is on its way). A template that holds the block shows the pages inside
 * it as ordinary ones (`hiddenTemplatePageIds`), so a source page inside that template is usable.
 */
export function sourcePages(
  tree: TreeView | null,
  source: PageListSource,
  hostId: PageId | null,
  taggedIds: ReadonlySet<PageId> | null,
): SourcePages {
  switch (source.kind) {
    case "children":
      return { pages: childPages(tree, hostId), missing: false };
    case "tag":
      return { pages: pagesIn(tree, taggedIds ?? NO_IDS), missing: false };
    case "page": {
      if (tree === null) {
        return { pages: [], missing: false };
      }
      const found = locate(tree.roots, source.pageId);
      if (found === null || hiddenTemplatePageIds(tree, hostId).has(source.pageId)) {
        return { pages: [], missing: true };
      }
      return { pages: found.node.children.map((node) => node.page), missing: false };
    }
    case "space": {
      const pages: PageInfo[] = [];
      const visit = (nodes: readonly PageNode[]): void => {
        for (const node of nodes) {
          // The page that holds the block is not its own row; its sub-pages are.
          if (node.page.id !== hostId) {
            pages.push(node.page);
          }
          visit(node.children);
        }
      };
      if (tree !== null) {
        visit(tree.roots);
      }
      return { pages, missing: false };
    }
  }
}

/**
 * The page "New" makes a row (or a template) under: the source page of a `page` source, the page that holds the block
 * for the other kinds (`null` when that is unknown). A tag source also gives the new page its tag, which the caller does.
 */
export function sourceParentId(source: PageListSource, hostId: PageId | null): PageId | null {
  return source.kind === "page" ? source.pageId : hostId;
}

/** The most pages the chooser's picker lists at once (a search narrows them). */
export const SOURCE_PICKER_LIMIT = 100;

/** A page the picker offers as the source page of a `page` source, with the page it sits under (for pages that share a title). */
export interface SourceChoice {
  readonly page: PageInfo;
  readonly parent: PageInfo | null;
}

/**
 * The pages the chooser offers as "Sub-pages of another page": the live pages of the space in tree order without templates and
 * the pages inside one (as the rows of a view, `hiddenTemplatePageIds`) and without the page that holds the block (`hostId`),
 * whose title contains `query` (case folded; an untitled page matches `untitled`, the text it is shown as). At most `limit`.
 */
export function sourceChoices(
  tree: TreeView | null,
  hostId: PageId | null,
  query: string,
  untitled: string,
  limit: number = SOURCE_PICKER_LIMIT,
): SourceChoice[] {
  const choices: SourceChoice[] = [];
  if (tree === null) {
    return choices;
  }
  const hidden = hiddenTemplatePageIds(tree, hostId);
  const wanted = query.trim().toLowerCase();
  const visit = (nodes: readonly PageNode[], parent: PageInfo | null): void => {
    for (const node of nodes) {
      if (choices.length >= limit) {
        return;
      }
      const { page } = node;
      const shown = page.title === "" ? untitled : page.title;
      if (page.id !== hostId && !hidden.has(page.id) && (wanted === "" || shown.toLowerCase().includes(wanted))) {
        choices.push({ page, parent });
      }
      visit(node.children, page);
    }
  };
  visit(tree.roots, null);
  return choices;
}
