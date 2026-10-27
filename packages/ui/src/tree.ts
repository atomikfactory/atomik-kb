// Pure helpers over the page tree the core returns (`TreeView.roots`): what the sidebar
// shows, where a page sits, and where a move, a drop or a menu command puts it.
//
// Positions follow the API: `index` is the 0-based position among the new siblings
// counted WITHOUT the page that moves, so the position a page ends up at.

import type { PageId, PageInfo, PageNode } from "@atomik-kb/core-client";

/** One visible row of the tree, in display order. */
export interface FlatRow {
  id: PageId;
  page: PageInfo;
  /** Nesting level, 0 for root pages. */
  depth: number;
  parentId: PageId | null;
  /** 0-based position among the siblings. */
  index: number;
  siblingCount: number;
  hasChildren: boolean;
  /** Only true for a page that has children and is not collapsed. */
  expanded: boolean;
}

/** Where a page sits in the tree. */
export interface Location {
  node: PageNode;
  parent: PageNode | null;
  siblings: readonly PageNode[];
  index: number;
}

/** Where a page goes: under `parentId` (`null` for the space root) at `index`. */
export interface MoveTarget {
  parentId: PageId | null;
  index: number;
}

/** Which part of a row a drop lands on. */
export type DropZone = "before" | "inside" | "after";

/** The rows that are visible when the pages in `collapsed` hide their children. */
export function flattenVisible(roots: readonly PageNode[], collapsed: ReadonlySet<PageId>): FlatRow[] {
  const rows: FlatRow[] = [];
  const visit = (nodes: readonly PageNode[], depth: number, parentId: PageId | null): void => {
    nodes.forEach((node, index) => {
      const hasChildren = node.children.length > 0;
      const expanded = hasChildren && !collapsed.has(node.page.id);
      rows.push({
        id: node.page.id,
        page: node.page,
        depth,
        parentId,
        index,
        siblingCount: nodes.length,
        hasChildren,
        expanded,
      });
      if (expanded) {
        visit(node.children, depth + 1, node.page.id);
      }
    });
  };
  visit(roots, 0, null);
  return rows;
}

/** Finds a page anywhere in the tree. */
export function locate(roots: readonly PageNode[], id: PageId): Location | null {
  const search = (siblings: readonly PageNode[], parent: PageNode | null): Location | null => {
    for (const [index, node] of siblings.entries()) {
      if (node.page.id === id) {
        return { node, parent, siblings, index };
      }
      const inside = search(node.children, node);
      if (inside !== null) {
        return inside;
      }
    }
    return null;
  };
  return search(roots, null);
}

/** The page's metadata, or `null` when it is not in the live tree. */
export function findPage(roots: readonly PageNode[], id: PageId): PageInfo | null {
  return locate(roots, id)?.node.page ?? null;
}

/**
 * The pages from the top of the tree down to `id` (the page itself last), or `null` when it is
 * not in the live tree.
 */
export function pathTo(roots: readonly PageNode[], id: PageId): PageInfo[] | null {
  for (const node of roots) {
    if (node.page.id === id) {
      return [node.page];
    }
    const below = pathTo(node.children, id);
    if (below !== null) {
      return [node.page, ...below];
    }
  }
  return null;
}

/** Every live page of the tree by id, for looking many pages up at once. */
export function indexPages(roots: readonly PageNode[]): Map<PageId, PageInfo> {
  const pages = new Map<PageId, PageInfo>();
  const visit = (nodes: readonly PageNode[]): void => {
    for (const node of nodes) {
      pages.set(node.page.id, node.page);
      visit(node.children);
    }
  };
  visit(roots);
  return pages;
}

/** Number of pages in the tree, counting every level. */
export function countPages(roots: readonly PageNode[]): number {
  return roots.reduce((total, node) => total + 1 + countPages(node.children), 0);
}

/** True when `id` is `node` itself or lies anywhere below it. */
export function isInSubtree(node: PageNode, id: PageId): boolean {
  return node.page.id === id || node.children.some((child) => isInSubtree(child, id));
}

/**
 * The zone of a row (or list item) the pointer is over: the upper quarter is "before",
 * the lower quarter "after" and the middle "inside". With no geometry (a zero height,
 * as in a test DOM) everything counts as "inside".
 */
export function dropZone(offsetY: number, height: number): DropZone {
  if (height <= 0) {
    return "inside";
  }
  const ratio = offsetY / height;
  if (ratio < 0.25) {
    return "before";
  }
  return ratio > 0.75 ? "after" : "inside";
}

/**
 * Where dropping `dragId` on the `zone` of `targetId` puts it, or `null` when that drop
 * is not allowed: on itself, or into its own subtree (that would make a cycle).
 */
export function resolveDrop(
  roots: readonly PageNode[],
  dragId: PageId,
  targetId: PageId,
  zone: DropZone,
): MoveTarget | null {
  const drag = locate(roots, dragId);
  const target = locate(roots, targetId);
  if (drag === null || target === null || isInSubtree(drag.node, targetId)) {
    return null;
  }
  if (zone === "inside") {
    const siblings = target.node.children.filter((child) => child.page.id !== dragId);
    return { parentId: targetId, index: siblings.length };
  }
  const siblings = target.siblings.filter((sibling) => sibling.page.id !== dragId);
  const at = siblings.findIndex((sibling) => sibling.page.id === targetId);
  return {
    parentId: target.parent?.page.id ?? null,
    index: zone === "before" ? at : at + 1,
  };
}

/** The position at the end of the root, for a page dropped below the last row. */
export function rootEndTarget(roots: readonly PageNode[], dragId: PageId): MoveTarget {
  return { parentId: null, index: roots.filter((node) => node.page.id !== dragId).length };
}

/** True when moving `id` to `target` would leave it where it is. */
export function isNoop(roots: readonly PageNode[], id: PageId, target: MoveTarget): boolean {
  const here = locate(roots, id);
  return here !== null && (here.parent?.page.id ?? null) === target.parentId && here.index === target.index;
}

/** One position up among the siblings; `null` for the first sibling. */
export function moveUpTarget(roots: readonly PageNode[], id: PageId): MoveTarget | null {
  const here = locate(roots, id);
  if (here === null || here.index === 0) {
    return null;
  }
  return { parentId: here.parent?.page.id ?? null, index: here.index - 1 };
}

/** One position down among the siblings; `null` for the last sibling. */
export function moveDownTarget(roots: readonly PageNode[], id: PageId): MoveTarget | null {
  const here = locate(roots, id);
  if (here === null || here.index >= here.siblings.length - 1) {
    return null;
  }
  return { parentId: here.parent?.page.id ?? null, index: here.index + 1 };
}

/** Becomes the last child of the previous sibling; `null` for the first sibling. */
export function indentTarget(roots: readonly PageNode[], id: PageId): MoveTarget | null {
  const here = locate(roots, id);
  const previous = here === null ? undefined : here.siblings[here.index - 1];
  if (previous === undefined) {
    return null;
  }
  return { parentId: previous.page.id, index: previous.children.length };
}

/** Becomes the sibling right after its parent; `null` for a root page. */
export function outdentTarget(roots: readonly PageNode[], id: PageId): MoveTarget | null {
  const here = locate(roots, id);
  if (here === null || here.parent === null) {
    return null;
  }
  const parent = locate(roots, here.parent.page.id);
  if (parent === null) {
    return null;
  }
  return { parentId: parent.parent?.page.id ?? null, index: parent.index + 1 };
}

/**
 * The row that should take focus when the row at `index` leaves the list (moved to the
 * trash): the next row that is not inside it, else the row before it.
 */
export function neighborAfterRemoval(rows: readonly FlatRow[], index: number): PageId | null {
  const removed = rows[index];
  if (removed === undefined) {
    return null;
  }
  for (const row of rows.slice(index + 1)) {
    if (row.depth <= removed.depth) {
      return row.id;
    }
  }
  return rows[index - 1]?.id ?? null;
}
