// The sources of a database block (ADR-0058, `docs/architecture/database-power-ups.md` section 2): reading the two linked kinds from the
// attribute, the select value of each source, which pages every kind lists (`sourcePages`) and what is left of them as rows and as templates
// (`splitTemplates`), where "New" makes a page (`sourceParentId`) and which pages the chooser's picker offers (`sourceChoices`). Pure
// functions over a page tree; the chooser and the missing state are `page-list-source-chooser.test.tsx`'s.

import type { PageId, PageInfo, PageNode, SpaceId, TreeView } from "@atomik-kb/core-client";
import { describe, expect, it } from "vitest";

import { templatesOfSpace } from "../templates";
import { splitTemplates } from "./database-templates";
import { choiceSource, readSource, sourceChoice } from "./page-list-model";
import type { PageListSource } from "./page-list-model";
import { sourceChoices, sourceParentId, sourcePages } from "./page-list-sources";

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const id = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;

function page(n: number, title: string, extra: Partial<PageInfo> = {}): PageInfo {
  return { id: id(n), spaceId: SPACE, title, createdAt: n, updatedAt: n, ...extra };
}

function node(info: PageInfo, ...children: PageNode[]): PageNode {
  return { page: info, children };
}

function treeOf(...roots: PageNode[]): TreeView {
  return { spaceId: SPACE, roots, trashed: [], undo: { canUndo: false, canRedo: false } };
}

const titles = (pages: readonly PageInfo[]): string[] => pages.map((entry) => entry.title);

// Hub (1) holds the block: Alpha (2) with Deep (3), Weekly (4, a template) with Weekly notes (5), Beta (6), Brief (7, a template).
// Tasks (8): Task A (9), Task B (10), Task template (11, a template) with Task template notes (12), Task C (13) with Subtask (14).
// Archive (15): Old (16). Empty (17) has no sub-pages.
const hub = page(1, "Hub");
const alpha = page(2, "Alpha");
const deep = page(3, "Deep");
const weekly = page(4, "Weekly", { template: true });
const weeklyNotes = page(5, "Weekly notes");
const beta = page(6, "Beta");
const brief = page(7, "Brief", { template: true });
const tasks = page(8, "Tasks");
const taskA = page(9, "Task A");
const taskB = page(10, "Task B");
const taskTemplate = page(11, "Task template", { template: true });
const taskTemplateNotes = page(12, "Task template notes");
const taskC = page(13, "Task C");
const subtask = page(14, "Subtask");
const archive = page(15, "Archive");
const old = page(16, "Old");
const empty = page(17, "Empty");

const tree = treeOf(
  node(hub, node(alpha, node(deep)), node(weekly, node(weeklyNotes)), node(beta), node(brief)),
  node(tasks, node(taskA), node(taskB), node(taskTemplate, node(taskTemplateNotes)), node(taskC, node(subtask))),
  node(archive, node(old)),
  node(empty),
);

describe("reading a source", () => {
  it("reads a page source with its page and a space source, and nothing else it does not know", () => {
    expect(readSource({ kind: "page", pageId: "p1" })).toEqual({ kind: "page", pageId: "p1" });
    expect(readSource({ kind: "space" })).toEqual({ kind: "space" });
    expect(readSource({ kind: "space", extra: 1 })).toEqual({ kind: "space" });
    for (const odd of [{ kind: "page" }, { kind: "page", pageId: "" }, { kind: "page", pageId: 4 }, { kind: "pages" }, { kind: "everything" }]) {
      expect(readSource(odd), JSON.stringify(odd)).toBeNull();
    }
  });

  it("still reads the first two kinds as before", () => {
    expect(readSource({ kind: "children" })).toEqual({ kind: "children" });
    expect(readSource({ kind: "tag", optionId: "o1" })).toEqual({ kind: "tag", optionId: "o1" });
    expect(readSource({ kind: "tag" })).toBeNull();
  });

  it("turns each source into a select value and back", () => {
    const sources: PageListSource[] = [
      { kind: "children" },
      { kind: "tag", optionId: "o1" },
      { kind: "page", pageId: "p1" },
      { kind: "space" },
    ];
    expect(sources.map(sourceChoice)).toEqual(["children", "tag:o1", "page:p1", "space"]);
    for (const source of sources) {
      expect(choiceSource(sourceChoice(source))).toEqual(source);
    }
    for (const odd of ["page:", "page", "tag:", "pick-page", "everything", ""]) {
      expect(choiceSource(odd), odd).toBeNull();
    }
  });
});

describe("the pages of a children source", () => {
  it("are the sub-pages of the page that holds the block, templates among them, in tree order", () => {
    const { pages, missing } = sourcePages(tree, { kind: "children" }, hub.id, null);
    expect(titles(pages)).toEqual(["Alpha", "Weekly", "Beta", "Brief"]);
    expect(missing).toBe(false);
  });

  it("are none while the host is unknown or the tree is not read", () => {
    expect(sourcePages(tree, { kind: "children" }, null, null).pages).toEqual([]);
    expect(sourcePages(null, { kind: "children" }, hub.id, null).pages).toEqual([]);
  });
});

describe("the pages of a tag source", () => {
  it("are the tagged pages in tree order, whatever order the ids come in", () => {
    const { pages, missing } = sourcePages(tree, { kind: "tag", optionId: "o1" }, hub.id, new Set([taskC.id, alpha.id, old.id]));
    expect(titles(pages)).toEqual(["Alpha", "Task C", "Old"]);
    expect(missing).toBe(false);
  });

  it("are none while the core has not answered", () => {
    expect(sourcePages(tree, { kind: "tag", optionId: "o1" }, hub.id, null).pages).toEqual([]);
  });
});

describe("the pages of a page source", () => {
  const source = (n: number): PageListSource => ({ kind: "page", pageId: id(n) });

  it("are the sub-pages of that page in tree order, and the page itself is not among them", () => {
    const { pages, missing } = sourcePages(tree, source(8), hub.id, null);
    expect(titles(pages)).toEqual(["Task A", "Task B", "Task template", "Task C"]);
    expect(missing).toBe(false);
  });

  it("leave out the templates and what is inside them from the rows, and list the templates of that page", () => {
    const { pages } = sourcePages(tree, source(8), hub.id, null);
    const { rows, templates } = splitTemplates(tree, pages, hub.id);
    expect(titles(rows)).toEqual(["Task A", "Task B", "Task C"]);
    expect(titles(templates)).toEqual(["Task template"]);
  });

  it("reach only one level: the sub-pages of a sub-page are not listed", () => {
    expect(titles(sourcePages(tree, source(8), hub.id, null).pages)).not.toContain("Subtask");
    expect(titles(sourcePages(tree, source(13), hub.id, null).pages)).toEqual(["Subtask"]);
  });

  it("can name the page that holds the block, which lists the same pages as a children source", () => {
    expect(sourcePages(tree, source(1), hub.id, null)).toEqual(sourcePages(tree, { kind: "children" }, hub.id, null));
  });

  it("is empty, not missing, for a page without sub-pages", () => {
    expect(sourcePages(tree, source(17), hub.id, null)).toEqual({ pages: [], missing: false });
  });

  it("is missing when the page is not in the live tree (gone, trashed or in another space)", () => {
    expect(sourcePages(tree, source(99), hub.id, null)).toEqual({ pages: [], missing: true });
  });

  it("is missing when the page is a template or inside one", () => {
    expect(sourcePages(tree, source(4), hub.id, null)).toEqual({ pages: [], missing: true });
    expect(sourcePages(tree, source(5), hub.id, null)).toEqual({ pages: [], missing: true });
    expect(sourcePages(tree, source(12), hub.id, null)).toEqual({ pages: [], missing: true });
  });

  it("is not missing while the tree is not read", () => {
    expect(sourcePages(null, source(8), hub.id, null)).toEqual({ pages: [], missing: false });
  });

  it("can use a page inside the template that holds the block, as the block's rows are drawn there", () => {
    const board = page(30, "Board template", { template: true });
    const column = page(31, "Column");
    const card = page(32, "Card");
    const inTemplate = treeOf(node(board, node(column, node(card))));
    expect(titles(sourcePages(inTemplate, { kind: "page", pageId: column.id }, board.id, null).pages)).toEqual(["Card"]);
    expect(sourcePages(inTemplate, { kind: "page", pageId: board.id }, board.id, null).missing).toBe(true);
    // From a page outside the template the same page is inside a template, so it is missing.
    expect(sourcePages(inTemplate, { kind: "page", pageId: column.id }, null, null).missing).toBe(true);
  });
});

describe("the pages of a space source", () => {
  it("are every page of the space at any depth in tree order, without the page that holds the block (its sub-pages stay)", () => {
    const { pages, missing } = sourcePages(tree, { kind: "space" }, hub.id, null);
    expect(titles(pages)).toEqual([
      "Alpha",
      "Deep",
      "Weekly",
      "Weekly notes",
      "Beta",
      "Brief",
      "Tasks",
      "Task A",
      "Task B",
      "Task template",
      "Task template notes",
      "Task C",
      "Subtask",
      "Archive",
      "Old",
      "Empty",
    ]);
    expect(missing).toBe(false);
  });

  it("lists every page when the host is unknown, and none while the tree is not read", () => {
    expect(sourcePages(tree, { kind: "space" }, null, null).pages).toHaveLength(17);
    expect(sourcePages(null, { kind: "space" }, hub.id, null)).toEqual({ pages: [], missing: false });
  });

  it("leaves the templates and what is inside them out of the rows", () => {
    const { pages } = sourcePages(tree, { kind: "space" }, hub.id, null);
    const { rows } = splitTemplates(tree, pages, hub.id);
    expect(titles(rows)).toEqual(["Alpha", "Deep", "Beta", "Tasks", "Task A", "Task B", "Task C", "Subtask", "Archive", "Old", "Empty"]);
  });

  it("has every template of the space as its templates, as the Templates panel lists them", () => {
    const { pages } = sourcePages(tree, { kind: "space" }, hub.id, null);
    const { templates } = splitTemplates(tree, pages, hub.id);
    expect(titles(templates)).toEqual(["Weekly", "Brief", "Task template"]);
    expect(templates).toEqual(templatesOfSpace(tree, SPACE));
  });

  it("does not list the host among its templates when the host is itself a template", () => {
    const flagged = page(40, "Project template", { template: true });
    const task = page(41, "Task");
    const other = page(42, "Other template", { template: true });
    const inTemplate = treeOf(node(flagged, node(task)), node(other));
    const { pages } = sourcePages(inTemplate, { kind: "space" }, flagged.id, null);
    const { rows, templates } = splitTemplates(inTemplate, pages, flagged.id);
    expect(titles(rows)).toEqual(["Task"]);
    expect(titles(templates)).toEqual(["Other template"]);
  });
});

describe("where New makes a page", () => {
  it("is under the source page for a page source, and under the page that holds the block for the others", () => {
    expect(sourceParentId({ kind: "children" }, hub.id)).toBe(hub.id);
    expect(sourceParentId({ kind: "tag", optionId: "o1" }, hub.id)).toBe(hub.id);
    expect(sourceParentId({ kind: "space" }, hub.id)).toBe(hub.id);
    expect(sourceParentId({ kind: "page", pageId: tasks.id }, hub.id)).toBe(tasks.id);
  });

  it("needs the host except for a page source", () => {
    expect(sourceParentId({ kind: "children" }, null)).toBeNull();
    expect(sourceParentId({ kind: "space" }, null)).toBeNull();
    expect(sourceParentId({ kind: "page", pageId: tasks.id }, null)).toBe(tasks.id);
  });
});

describe("the pages the chooser offers", () => {
  it("are the live pages in tree order, with the page each sits under, without templates, what is inside them and the host", () => {
    const choices = sourceChoices(tree, hub.id, "", "Untitled");
    expect(choices.map((choice) => choice.page.title)).toEqual([
      "Alpha",
      "Deep",
      "Beta",
      "Tasks",
      "Task A",
      "Task B",
      "Task C",
      "Subtask",
      "Archive",
      "Old",
      "Empty",
    ]);
    expect(choices.map((choice) => choice.parent?.title ?? null)).toEqual([
      "Hub",
      "Alpha",
      "Hub",
      null,
      "Tasks",
      "Tasks",
      "Tasks",
      "Task C",
      null,
      "Archive",
      null,
    ]);
  });

  it("keeps the host's sub-pages, and offers the host when it is unknown", () => {
    expect(sourceChoices(tree, hub.id, "", "Untitled").map((choice) => choice.page.id)).toContain(deep.id);
    expect(sourceChoices(tree, null, "", "Untitled").map((choice) => choice.page.title)).toContain("Hub");
  });

  it("narrows by the title, ignoring case and the spaces around the query", () => {
    expect(sourceChoices(tree, hub.id, "  TASK ", "Untitled").map((choice) => choice.page.title)).toEqual(["Tasks", "Task A", "Task B", "Task C", "Subtask"]);
    expect(sourceChoices(tree, hub.id, "zzz", "Untitled")).toEqual([]);
  });

  it("matches an untitled page by the text it is shown as", () => {
    const untitled = page(50, "");
    const withBlank = treeOf(node(hub, node(untitled)));
    expect(sourceChoices(withBlank, null, "untit", "Untitled").map((choice) => choice.page.id)).toEqual([untitled.id]);
  });

  it("stops at the limit, and has none while the tree is not read", () => {
    expect(sourceChoices(tree, hub.id, "", "Untitled", 3).map((choice) => choice.page.title)).toEqual(["Alpha", "Deep", "Beta"]);
    expect(sourceChoices(null, hub.id, "", "Untitled")).toEqual([]);
  });

  it("offers the pages inside the template that holds the block", () => {
    const board = page(30, "Board template", { template: true });
    const column = page(31, "Column");
    const inTemplate = treeOf(node(board, node(column)), node(page(32, "Elsewhere")));
    expect(sourceChoices(inTemplate, board.id, "", "Untitled").map((choice) => choice.page.title)).toEqual(["Column", "Elsewhere"]);
    expect(sourceChoices(inTemplate, null, "", "Untitled").map((choice) => choice.page.title)).toEqual(["Elsewhere"]);
  });
});
