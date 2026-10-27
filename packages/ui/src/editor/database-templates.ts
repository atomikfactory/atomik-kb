// Database templates (ADR-0056, `docs/architecture/templates.md` section 7): the templates among the pages of a database
// block's source, the rows without them, which template "New" makes a page from, and what the menu next to "New" asks of
// the block. Pure functions and one context; `NewPageMenu.tsx` draws the menu, `PageListView.tsx` provides the context.
//
// A template is a page with the `template` flag; it stays in the tree (the peek and the tabs open pages through it) and every
// list that shows pages leaves it out, so the rows of a view are the source's pages *without* templates and without the pages
// inside one. The templates of the source are the flagged pages among its pages (`page-list-sources.ts`): the host page's
// children with the flag (`children` source), the flagged pages that carry the tag (`tag` source), the flagged children of the
// source page (`page` source) or every template of the space (`space` source, the Templates panel's list). A new template is
// made under `sourceParentId`.
//
// One exception: a database block that sits inside a template (a template of a project with its tasks table) draws the
// pages of that template as ordinary rows. They are inside a template, but it is the one the block belongs to, and an editor
// that hid them would show an empty table whatever the person adds.

import { TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { OptionId, PageId, PageInfo, PageNode, PropertyValue, TreeView } from "@atomik-kb/core-client";
import { createContext, useContext } from "react";

import { valueOfPage } from "./page-list-model";
import type { PageListViewSpec } from "./page-list-views";

/** What the menu next to "New" asks of the block (each reports its own failure; none throws). */
export interface DatabaseTemplateActions {
  /** Opens the template in the side peek (the page itself when the peek cannot show it). */
  readonly edit: (template: PageInfo) => void;
  /** Duplicates the template (the copy is a template too, in the same place); the page in front does not change. */
  readonly duplicate: (template: PageInfo) => Promise<void>;
  /** Moves the template to the trash. */
  readonly remove: (template: PageInfo) => Promise<void>;
  /** Makes a template under the source's parent (`sourceParentId`; with the tag of a tag source) and opens it in the side peek. */
  readonly create: () => Promise<void>;
}

/** The templates of the block's source and the actions of its menu. */
export interface DatabaseTemplates {
  /** The templates of the source, in tree order (never the host page itself). */
  readonly templates: readonly PageInfo[];
  readonly actions: DatabaseTemplateActions;
}

/** `null` outside a database block (a layout then offers no menu). */
export const DatabaseTemplatesContext = createContext<DatabaseTemplates | null>(null);

/** The templates and actions of the enclosing database block, `null` outside one. */
export function useDatabaseTemplates(): DatabaseTemplates | null {
  return useContext(DatabaseTemplatesContext);
}

/** The id of the outermost template that the page `hostId` is (or is inside), `null` when it is in none. */
function enclosingTemplate(nodes: readonly PageNode[], hostId: PageId, outer: PageId | null): PageId | null | undefined {
  for (const node of nodes) {
    const top = outer ?? (node.page.template === true ? node.page.id : null);
    if (node.page.id === hostId) {
      return top;
    }
    const inside = enclosingTemplate(node.children, hostId, top);
    if (inside !== undefined) {
      return inside;
    }
  }
  return undefined;
}

/**
 * The ids of the pages that are not rows: every template and every page inside one, except the pages inside the template that
 * holds the block (`hostId`), which are its ordinary pages.
 */
export function hiddenTemplatePageIds(tree: TreeView | null, hostId: PageId | null): ReadonlySet<PageId> {
  const hidden = new Set<PageId>();
  if (tree === null) {
    return hidden;
  }
  const scope = hostId === null ? null : (enclosingTemplate(tree.roots, hostId, null) ?? null);
  const visit = (nodes: readonly PageNode[], inside: boolean): void => {
    for (const node of nodes) {
      const here = inside || node.page.template === true;
      if (here) {
        hidden.add(node.page.id);
      }
      visit(node.children, node.page.id === scope ? false : here);
    }
  };
  visit(tree.roots, false);
  return hidden;
}

/** The pages of a source told apart: the ones that are rows, and the templates among them. */
export interface SplitPages {
  /** The pages without templates and without pages inside a template (the host page may be among them for a tag or a page source). */
  readonly rows: PageInfo[];
  /** The templates among the pages (never the host page itself), in tree order. */
  readonly templates: PageInfo[];
}

/** Splits the pages of a block's source (`hostId` is the page that holds the block, `null` when unknown). */
export function splitTemplates(tree: TreeView | null, pages: readonly PageInfo[], hostId: PageId | null): SplitPages {
  const hidden = hiddenTemplatePageIds(tree, hostId);
  const rows = pages.filter((page) => !hidden.has(page.id));
  const templates = pages.filter((page) => page.template === true && page.id !== hostId);
  return { rows, templates };
}

/**
 * The template "New" makes a page from in `view`: its `defaultTemplate` while that page is one of the source's templates,
 * `null` (an empty page) otherwise.
 */
export function resolveDefaultTemplate(
  view: Pick<PageListViewSpec, "defaultTemplate">,
  templates: readonly PageInfo[],
): PageId | null {
  const wanted = view.defaultTemplate;
  return wanted !== undefined && templates.some((template) => template.id === wanted) ? wanted : null;
}

/**
 * The Tags value a page made from `template` gets in a tag source: the tags the template has, then the source's tag (once). A
 * template made for the tag has it already; this keeps its other tags as they are.
 */
export function tagValueFor(template: PageInfo | undefined, optionId: OptionId): PropertyValue {
  const current = template === undefined ? undefined : valueOfPage(template, TAGS_PROPERTY_ID);
  const kept = current?.kind === "multi_select" ? current.value : [];
  return { kind: "multi_select", value: kept.includes(optionId) ? [...kept] : [...kept, optionId] };
}
