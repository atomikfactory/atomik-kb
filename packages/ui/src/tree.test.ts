import type { PageNode } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import {
  countPages,
  dropZone,
  findPage,
  flattenVisible,
  indentTarget,
  isInSubtree,
  isNoop,
  locate,
  moveDownTarget,
  moveUpTarget,
  neighborAfterRemoval,
  outdentTarget,
  resolveDrop,
  rootEndTarget,
} from "./tree";

/** A node whose id is its title, so expectations read like the tree. */
function node(title: string, ...children: PageNode[]): PageNode {
  return { page: { id: title, spaceId: "space", title, createdAt: 0, updatedAt: 0 }, children };
}

//   A
//     A1
//       A1x
//     A2
//   B
//   C
const roots = [node("A", node("A1", node("A1x")), node("A2")), node("B"), node("C")];

describe("flattenVisible", () => {
  it("lists every page in display order with its place among its siblings", () => {
    const rows = flattenVisible(roots, new Set());
    expect(rows.map((row) => row.id)).toEqual(["A", "A1", "A1x", "A2", "B", "C"]);
    expect(rows.map((row) => row.depth)).toEqual([0, 1, 2, 1, 0, 0]);
    expect(rows.map((row) => row.parentId)).toEqual([null, "A", "A1", "A", null, null]);
    expect(rows.map((row) => row.index)).toEqual([0, 0, 0, 1, 1, 2]);
    expect(rows.map((row) => row.siblingCount)).toEqual([3, 2, 1, 2, 3, 3]);
    expect(rows.map((row) => row.hasChildren)).toEqual([true, true, false, false, false, false]);
    expect(rows.map((row) => row.expanded)).toEqual([true, true, false, false, false, false]);
    expect(rows[0]?.page).toBe(roots[0]?.page);
  });

  it("hides the children of collapsed pages, however deep", () => {
    expect(flattenVisible(roots, new Set(["A1"])).map((row) => row.id)).toEqual(["A", "A1", "A2", "B", "C"]);
    expect(flattenVisible(roots, new Set(["A"])).map((row) => row.id)).toEqual(["A", "B", "C"]);
    const collapsed = flattenVisible(roots, new Set(["A"]))[0];
    expect(collapsed).toMatchObject({ hasChildren: true, expanded: false });
  });

  it("ignores collapsed ids that are unknown or belong to pages without children", () => {
    expect(flattenVisible(roots, new Set(["B", "nope"])).map((row) => row.id)).toEqual(["A", "A1", "A1x", "A2", "B", "C"]);
  });

  it("is empty for an empty tree", () => {
    expect(flattenVisible([], new Set())).toEqual([]);
  });
});

describe("locating pages", () => {
  it("finds a page with its parent, siblings and position", () => {
    const found = locate(roots, "A2");
    expect(found?.node.page.id).toBe("A2");
    expect(found?.parent?.page.id).toBe("A");
    expect(found?.index).toBe(1);
    expect(found?.siblings.map((sibling) => sibling.page.id)).toEqual(["A1", "A2"]);
    expect(locate(roots, "C")?.parent).toBeNull();
    expect(locate(roots, "nope")).toBeNull();
  });

  it("reads a page, counts pages and checks subtrees", () => {
    expect(findPage(roots, "A1x")?.title).toBe("A1x");
    expect(findPage(roots, "nope")).toBeNull();
    expect(countPages(roots)).toBe(6);
    expect(countPages([])).toBe(0);
    const a = roots[0] as PageNode;
    expect(isInSubtree(a, "A")).toBe(true);
    expect(isInSubtree(a, "A1x")).toBe(true);
    expect(isInSubtree(a, "B")).toBe(false);
  });
});

describe("dropZone", () => {
  it("splits a row into an upper quarter, a middle half and a lower quarter", () => {
    expect(dropZone(0, 28)).toBe("before");
    expect(dropZone(6.9, 28)).toBe("before");
    expect(dropZone(7, 28)).toBe("inside");
    expect(dropZone(14, 28)).toBe("inside");
    expect(dropZone(21, 28)).toBe("inside");
    expect(dropZone(21.1, 28)).toBe("after");
    expect(dropZone(28, 28)).toBe("after");
    expect(dropZone(-3, 28)).toBe("before");
  });

  it("counts everything as inside when there is no geometry", () => {
    expect(dropZone(0, 0)).toBe("inside");
    expect(dropZone(5, -1)).toBe("inside");
  });
});

describe("resolveDrop", () => {
  it("drops inside a page as its last child, not counting the dragged page itself", () => {
    expect(resolveDrop(roots, "B", "A", "inside")).toEqual({ parentId: "A", index: 2 });
    expect(resolveDrop(roots, "B", "C", "inside")).toEqual({ parentId: "C", index: 0 });
    // A1 is already under A: the other children are just A2.
    expect(resolveDrop(roots, "A1", "A", "inside")).toEqual({ parentId: "A", index: 1 });
    expect(resolveDrop(roots, "B", "A1x", "inside")).toEqual({ parentId: "A1x", index: 0 });
  });

  it("drops before a page as its sibling, at its position", () => {
    expect(resolveDrop(roots, "B", "A", "before")).toEqual({ parentId: null, index: 0 });
    expect(resolveDrop(roots, "C", "B", "before")).toEqual({ parentId: null, index: 1 });
    expect(resolveDrop(roots, "B", "A2", "before")).toEqual({ parentId: "A", index: 1 });
    expect(resolveDrop(roots, "B", "A1x", "before")).toEqual({ parentId: "A1", index: 0 });
    expect(resolveDrop(roots, "A2", "A1", "before")).toEqual({ parentId: "A", index: 0 });
  });

  it("drops after a page as its sibling, counting positions without the dragged page", () => {
    expect(resolveDrop(roots, "C", "A", "after")).toEqual({ parentId: null, index: 1 });
    expect(resolveDrop(roots, "A", "C", "after")).toEqual({ parentId: null, index: 2 });
    expect(resolveDrop(roots, "A1", "A2", "after")).toEqual({ parentId: "A", index: 1 });
    expect(resolveDrop(roots, "B", "A1", "after")).toEqual({ parentId: "A", index: 1 });
  });

  it("refuses a drop on the page itself or anywhere inside it", () => {
    for (const zone of ["before", "inside", "after"] as const) {
      expect(resolveDrop(roots, "A", "A", zone)).toBeNull();
      expect(resolveDrop(roots, "A", "A1", zone)).toBeNull();
      expect(resolveDrop(roots, "A", "A1x", zone)).toBeNull();
      expect(resolveDrop(roots, "A1", "A1x", zone)).toBeNull();
    }
  });

  it("refuses pages that are not in the tree", () => {
    expect(resolveDrop(roots, "nope", "A", "inside")).toBeNull();
    expect(resolveDrop(roots, "A", "nope", "inside")).toBeNull();
  });

  it("puts a page at the end of the root", () => {
    expect(rootEndTarget(roots, "B")).toEqual({ parentId: null, index: 2 });
    expect(rootEndTarget(roots, "A1")).toEqual({ parentId: null, index: 3 });
  });
});

describe("isNoop", () => {
  it("recognises a move to the place the page already has", () => {
    expect(isNoop(roots, "B", { parentId: null, index: 1 })).toBe(true);
    expect(isNoop(roots, "B", { parentId: null, index: 0 })).toBe(false);
    expect(isNoop(roots, "A1", { parentId: "A", index: 0 })).toBe(true);
    expect(isNoop(roots, "A1", { parentId: "A", index: 1 })).toBe(false);
    expect(isNoop(roots, "A1", { parentId: null, index: 0 })).toBe(false);
    expect(isNoop(roots, "nope", { parentId: null, index: 0 })).toBe(false);
  });

  it("agrees with resolveDrop about drops next to the page", () => {
    // Dropping B right after A or right before C changes nothing.
    expect(isNoop(roots, "B", resolveDrop(roots, "B", "A", "after") ?? { parentId: "x", index: 0 })).toBe(true);
    expect(isNoop(roots, "B", resolveDrop(roots, "B", "C", "before") ?? { parentId: "x", index: 0 })).toBe(true);
    expect(isNoop(roots, "B", resolveDrop(roots, "B", "A", "before") ?? { parentId: "x", index: 0 })).toBe(false);
  });
});

describe("menu commands", () => {
  it("moves up and down by one position among the siblings", () => {
    expect(moveUpTarget(roots, "B")).toEqual({ parentId: null, index: 0 });
    expect(moveUpTarget(roots, "A2")).toEqual({ parentId: "A", index: 0 });
    expect(moveUpTarget(roots, "A")).toBeNull();
    expect(moveUpTarget(roots, "A1x")).toBeNull();
    expect(moveDownTarget(roots, "A")).toEqual({ parentId: null, index: 1 });
    expect(moveDownTarget(roots, "A1")).toEqual({ parentId: "A", index: 1 });
    expect(moveDownTarget(roots, "C")).toBeNull();
    expect(moveDownTarget(roots, "A2")).toBeNull();
    expect(moveUpTarget(roots, "nope")).toBeNull();
    expect(moveDownTarget(roots, "nope")).toBeNull();
  });

  it("indents into the previous sibling as its last child", () => {
    expect(indentTarget(roots, "B")).toEqual({ parentId: "A", index: 2 });
    expect(indentTarget(roots, "C")).toEqual({ parentId: "B", index: 0 });
    expect(indentTarget(roots, "A2")).toEqual({ parentId: "A1", index: 1 });
    expect(indentTarget(roots, "A")).toBeNull();
    expect(indentTarget(roots, "A1")).toBeNull();
    expect(indentTarget(roots, "nope")).toBeNull();
  });

  it("outdents to be the sibling right after the parent", () => {
    expect(outdentTarget(roots, "A1")).toEqual({ parentId: null, index: 1 });
    expect(outdentTarget(roots, "A2")).toEqual({ parentId: null, index: 1 });
    expect(outdentTarget(roots, "A1x")).toEqual({ parentId: "A", index: 1 });
    expect(outdentTarget(roots, "A")).toBeNull();
    expect(outdentTarget(roots, "nope")).toBeNull();
  });
});

describe("neighborAfterRemoval", () => {
  const rows = flattenVisible(roots, new Set());

  it("is the next row that is not inside the removed page", () => {
    expect(neighborAfterRemoval(rows, 0)).toBe("B");
    expect(neighborAfterRemoval(rows, 1)).toBe("A2");
    expect(neighborAfterRemoval(rows, 2)).toBe("A2");
    expect(neighborAfterRemoval(rows, 4)).toBe("C");
  });

  it("is the row before when the removed page is the last one", () => {
    expect(neighborAfterRemoval(rows, 5)).toBe("B");
    expect(neighborAfterRemoval(rows.slice(0, 4), 3)).toBe("A1x");
  });

  it("is nothing when no row is left or the row does not exist", () => {
    expect(neighborAfterRemoval(rows.slice(0, 1), 0)).toBeNull();
    expect(neighborAfterRemoval(rows, 99)).toBeNull();
    expect(neighborAfterRemoval([], 0)).toBeNull();
  });
});
