// Templates in the page tree (ADR-0056, `docs/architecture/templates.md` section 6): a template is a page with `template`
// set; its sub-pages are inside it without the flag. Templates stay in `store.tree` (so they open, sit in tabs and in the
// peek like any page) and every list that shows pages to choose or browse leaves them out with these helpers: the
// sidebar, the views of a database, recents, the Home lists, pickers.

import type { PageId, PageInfo, PageNode, SpaceId, TreeView } from "@atomik-kb/core-client";

/** Every page of the live tree with its parent's id, depth first. */
function* walk(nodes: readonly PageNode[], inTemplate: boolean): Generator<readonly [PageNode, boolean]> {
  for (const node of nodes) {
    const here = inTemplate || node.page.template === true;
    yield [node, here];
    yield* walk(node.children, here);
  }
}

/** The ids of every page that is a template or inside one. */
export function templatePageIds(tree: TreeView | null): ReadonlySet<PageId> {
  const ids = new Set<PageId>();
  if (tree === null) {
    return ids;
  }
  for (const [node, inTemplate] of walk(tree.roots, false)) {
    if (inTemplate) {
      ids.add(node.page.id);
    }
  }
  return ids;
}

/** Whether the page is a template or inside one. */
export function isInTemplate(tree: TreeView | null, pageId: PageId): boolean {
  return templatePageIds(tree).has(pageId);
}

/** The templates of a space (the pages with the flag, not their sub-pages), in tree order. */
export function templatesOfSpace(tree: TreeView | null, spaceId: SpaceId): PageInfo[] {
  if (tree === null) {
    return [];
  }
  const found: PageInfo[] = [];
  for (const [node] of walk(tree.roots, false)) {
    if (node.page.template === true && node.page.spaceId === spaceId) {
      found.push(node.page);
    }
  }
  return found;
}
