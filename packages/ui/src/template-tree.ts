// Template pages as the UI shows them (ADR-0056, `docs/architecture/templates.md` section 6): pure helpers over the page tree on
// top of `templates.ts`. The sidebar and the pickers draw `withoutTemplates(tree.roots)`; a move the person makes there is
// computed on that tree and put back in terms of the whole one (`toFullTarget`), so "Move up" never lands "before a hidden
// template" and looks like nothing happened. The banner needs the template a page belongs to (`templateRootOf`), the Templates
// panel the templates of a space by where they live (`templateGroups`).

import type { PageId, PageInfo, PageNode, SpaceId, TreeView } from "@atomik-kb/core-client";

import { templatesOfSpace } from "./templates";
import { locate, findPage, pathTo } from "./tree";
import type { MoveTarget } from "./tree";

/**
 * The tree without the templates and the pages inside them. The very array when nothing is hidden (and the same node for a
 * branch with nothing hidden), so a memo over it does not run again for nothing.
 */
export function withoutTemplates(roots: readonly PageNode[]): readonly PageNode[] {
  let changed = false;
  const kept: PageNode[] = [];
  for (const node of roots) {
    if (node.page.template === true) {
      changed = true;
      continue;
    }
    const children = withoutTemplates(node.children);
    if (children === node.children) {
      kept.push(node);
    } else {
      changed = true;
      kept.push({ ...node, children: [...children] });
    }
  }
  return changed ? kept : roots;
}

/** The template a page belongs to: the page itself when it has the flag, else its outermost ancestor that has it; `null` for a page outside templates. */
export function templateRootOf(tree: TreeView | null, pageId: PageId): PageInfo | null {
  if (tree === null) {
    return null;
  }
  return pathTo(tree.roots, pageId)?.find((page) => page.template === true) ?? null;
}

/**
 * A move computed on the tree without templates (`target`: a position among the visible siblings, counted without the page that
 * moves, as every `MoveTarget`) in terms of the whole tree: the visible sibling it goes before keeps its place, and the end of
 * the visible ones is the end of all. The target itself when the parent has no hidden child.
 */
export function toFullTarget(roots: readonly PageNode[], id: PageId, target: MoveTarget): MoveTarget {
  const siblings = target.parentId === null ? roots : (locate(roots, target.parentId)?.node.children ?? []);
  const all = siblings.filter((sibling) => sibling.page.id !== id);
  const visible = all.filter((sibling) => sibling.page.template !== true);
  if (visible.length === all.length) {
    return target;
  }
  const before = visible[target.index];
  return { parentId: target.parentId, index: before === undefined ? all.length : all.indexOf(before) };
}

/** The templates that live under one parent (the top level for `parentId` `null`). */
export interface TemplateGroup {
  parentId: PageId | null;
  /** The parent's title; empty for the top level and for an untitled parent. */
  title: string;
  templates: PageInfo[];
}

/**
 * The templates of a space grouped by where they live: the top level first (the "Page templates"), then each parent that holds
 * some (a database's host page, for the templates of its rows) in the order the tree meets them.
 */
export function templateGroups(tree: TreeView | null, spaceId: SpaceId): TemplateGroup[] {
  if (tree === null) {
    return [];
  }
  const groups = new Map<PageId | null, TemplateGroup>();
  for (const template of templatesOfSpace(tree, spaceId)) {
    const parentId = template.parentId ?? null;
    let group = groups.get(parentId);
    if (group === undefined) {
      group = { parentId, title: parentId === null ? "" : (findPage(tree.roots, parentId)?.title ?? ""), templates: [] };
      groups.set(parentId, group);
    }
    group.templates.push(template);
  }
  const top = groups.get(null);
  return [...(top === undefined ? [] : [top]), ...[...groups.values()].filter((group) => group.parentId !== null)];
}
