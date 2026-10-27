// The table layout, the filters and the column widths of the list view block (ADR-0016, amendment of
// 2026-10-08): the pure rules (reading the new attributes, every filter operator, the widths, moving
// a column, the window of rows), the HTML form and the schema, the layout switch, the grid (columns,
// header menu: sort, hide, move; resizing), editing the cells of every kind through the editors of
// the properties panel, the keyboard, "New page", the filter editor, the windowed rows, a read-only
// editor, a layout or filter this version does not know, and that nothing is written on render.

import { CoreClient, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, PropertyDef, SeedSpace } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { fullDate } from "./dates";
import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { viewSelect, viewToolbar } from "../test/page-list";
import type { User } from "../test/render";
import { K, kindsHub, kindsSpace } from "../test/view-kinds";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { loroSyncPluginKey } from "./loro-binding";
import { applyFilters, filterOps, matchFilter, newFilter, readDateRange, readFilters } from "./page-list-filters";
import type { PageListFilter } from "./page-list-filters";
import { clampColumnWidth, readColumnWidths, readLayout } from "./page-list-model";
import {
  DEFAULT_PROPERTY_WIDTH,
  DEFAULT_TITLE_WIDTH,
  dragWidth,
  gridLayout,
  isWindowed,
  moveProperty,
  ROW_HEIGHT,
  scrollToReveal,
  visibleRows,
  widthOf,
} from "./page-table-model";
import { SELECT_COLUMN_WIDTH } from "./PageRowCheckbox";
import { createEditorSchema, createSchemaExtensions } from "./schema";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

// ----- the space ---------------------------------------------------------------------------

const PROP = {
  status: "de000000-0000-4000-8000-00000000c001",
  due: "de000000-0000-4000-8000-00000000c002",
  done: "de000000-0000-4000-8000-00000000c003",
  priority: "de000000-0000-4000-8000-00000000c004",
  summary: "de000000-0000-4000-8000-00000000c005",
  stage: "de000000-0000-4000-8000-00000000c006",
  areas: "de000000-0000-4000-8000-00000000c007",
  link: "de000000-0000-4000-8000-00000000c008",
  todo: "de000000-0000-4000-8000-00000000c009",
};
const TAG = {
  idea: "de000000-0000-4000-8000-00000000d001",
  project: "de000000-0000-4000-8000-00000000d002",
};

const DEFAULTS = {
  source: { kind: "children" },
  layout: "table",
  properties: [] as string[],
  sort: { by: "title", dir: "asc" },
};

/**
 * A view that opens a page as a page: the side peek is the default (ADR-0054) and `openIn` is a part of `views`, which the older
 * attributes cannot say. For the tests that follow a click on a row to the page it opens.
 */
function asPages(extra: { [key: string]: JsonValue } = {}): { [key: string]: JsonValue } {
  return { views: [{ id: "v1", name: "", layout: "table", properties: [], sort: { by: "title", dir: "asc" }, openIn: "page", ...extra }] };
}

function paragraph(n: number, value: string): JsonValue {
  return value === ""
    ? { type: "paragraph", attrs: { id: pid(n) } }
    : { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

/** An intro, the view (id `pid(2)`, a table unless `list` says otherwise) and an empty paragraph. */
function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [paragraph(1, "Intro"), { type: "pageList", attrs: { id: pid(2), ...DEFAULTS, ...list } }, paragraph(3, "")],
  };
}

/**
 * Hub (holds the view) with the children Alpha, beta, Gamma and delta (the title order is Alpha,
 * beta, delta, Gamma), `extra` more pages when given; Notes (an empty page for the `/` menu).
 */
function space(content: JsonValue = hubContent(), extra = 0): SeedSpace {
  const more = Array.from({ length: extra }, (_, index) => ({ title: `Row ${String(index).padStart(3, "0")}` }));
  return {
    name: "Personal",
    tagOptions: [
      { id: TAG.idea, name: "idea", color: "green" },
      { id: TAG.project, name: "project", color: "blue" },
    ],
    properties: [
      { id: PROP.status, name: "Status", kind: "status" },
      { id: PROP.due, name: "Due", kind: "date" },
      { id: PROP.done, name: "Done", kind: "checkbox" },
      { id: PROP.priority, name: "Priority", kind: "number" },
      { id: PROP.summary, name: "Summary", kind: "text" },
      {
        id: PROP.stage,
        name: "Stage",
        kind: "select",
        options: [
          { name: "Review", color: "orange" },
          { name: "Draft", color: "gray" },
        ],
      },
      {
        id: PROP.areas,
        name: "Areas",
        kind: "multi_select",
        options: [
          { name: "Home", color: "pink" },
          { name: "Work", color: "purple" },
        ],
      },
      { id: PROP.link, name: "Link", kind: "url" },
      { id: PROP.todo, name: "Todo", kind: "checklist" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          {
            title: "Alpha",
            tags: ["idea"],
            properties: {
              Status: "In progress",
              Due: { date: "2026-10-15" },
              Done: true,
              Priority: 3,
              Summary: "A short note",
              Stage: "Draft",
              Areas: ["Home", "Work"],
              Link: "https://example.com",
              Todo: [{ text: "one", done: true }, { text: "two", done: true }],
            },
          },
          {
            title: "beta",
            tags: ["project"],
            properties: {
              Status: "Done",
              Due: { date: "2026-10-09", time: "09:30" },
              Done: false,
              Priority: 10,
              Summary: "Second",
              Stage: "Review",
              Todo: [{ text: "one", done: true }, { text: "two", done: false }],
            },
          },
          { title: "Gamma" },
          { title: "delta", tags: ["idea", "project"], properties: { Due: { date: "2026-10-04" }, Priority: 2, Areas: ["Work"], Summary: "zzz" } },
          ...more,
        ],
      },
      { title: "Notes", content: { type: "doc", content: [paragraph(10, "Intro"), paragraph(11, "")] } },
    ],
  };
}

function ticking(): () => number {
  let now = Date.UTC(2026, 9, 3, 12, 0, 0);
  return () => {
    now += 1000;
    return now;
  };
}

async function openHub(content: JsonValue = hubContent(), extra = 0) {
  const app = await renderPages([space(content, extra)], { now: ticking() });
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const client = new CoreClient(app.memory);
  const ids = await idsOf(app.memory);
  return { ...app, editor, client, ids, spaceId: ids.space("Personal") };
}

// ----- looking at the view -----------------------------------------------------------------

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

function grid(): HTMLElement {
  return within(block()).getByRole("grid", { name: t("pageTable.grid") });
}

function bodyRows(): HTMLElement[] {
  return within(grid())
    .getAllByRole("row")
    .filter((row) => row.hasAttribute("data-page"));
}

/** The titles of the rows, in order. */
function titles(): string[] {
  return bodyRows().map((row) => row.querySelector(".page-table-title")?.textContent ?? "");
}

function rowOf(title: string): HTMLElement {
  const row = bodyRows().find((candidate) => candidate.querySelector(".page-table-title")?.textContent === title);
  if (row === undefined) {
    throw new Error(`no row for ${title}`);
  }
  return row;
}

function cell(title: string, column: string): HTMLElement {
  return within(rowOf(title)).getByRole("gridcell", { name: column });
}

function header(name: string): HTMLElement {
  return within(grid()).getByRole("columnheader", { name });
}

/** The names of the columns of the grid, the checkbox column of the selection (ADR-0058) left out: it is not a property. */
function headers(): string[] {
  return within(grid())
    .getAllByRole("columnheader")
    .map((entry) => entry.getAttribute("aria-label") ?? "")
    .filter((name) => name !== t("pageList.bulk.selectAll"));
}

function toolbar(): HTMLElement {
  return viewToolbar(block());
}

/** The select `label` of the toolbar's popovers (the layout and the layout's own: view settings; Sort by: Sort); opens the one that holds it. */
function select(user: User, label: string): Promise<HTMLSelectElement> {
  return viewSelect(user, block(), label);
}

function attrsOf(editor: Editor): Record<string, unknown> {
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageList") {
      found = node.attrs;
    }
    return true;
  });
  return found;
}

function docOf(editor: Editor): LoroDoc {
  const doc = loroSyncPluginKey.getState(editor.state)?.doc;
  if (doc === undefined) {
    throw new Error("the editor is not bound to a document");
  }
  return doc;
}

function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

/** The value a page holds for a property, as the core has it. */
async function stored(client: CoreClient, pageId: string, propertyId: string) {
  return (await client.pagePropertiesGet(pageId)).find(([id]) => id === propertyId)?.[1];
}

/** Opens the menu of a column header and picks an item. */
async function pickFromHeader(user: User, column: string, item: string) {
  await user.click(within(header(column)).getByRole("button", { name: new RegExp(`^${column}`, "u") }));
  const menu = await screen.findByRole("menu", { name: t("pageTable.menu", { name: column }) });
  await user.click(within(menu).getByRole("menuitem", { name: item }));
}

// ----- the rules ---------------------------------------------------------------------------

describe("reading the new attributes", () => {
  it("reads a layout: table, and anything else as the list", () => {
    expect(readLayout("table")).toBe("table");
    for (const odd of ["list", "timeline", "", null, undefined, 3, {}]) {
      expect(readLayout(odd), JSON.stringify(odd)).toBe("list");
    }
  });

  it("reads the widths: the entries that are numbers above zero, nothing else", () => {
    expect(readColumnWidths({ title: 300, a: 120.5, b: 0, c: -3, d: "9", e: null, f: Number.POSITIVE_INFINITY })).toEqual({ title: 300, a: 120.5 });
    for (const odd of [null, undefined, [], "x", 3]) {
      expect(readColumnWidths(odd), JSON.stringify(odd)).toEqual({});
    }
  });

  it("draws a width as a whole number between 80 and 800, and the default when there is none", () => {
    expect(clampColumnWidth(10)).toBe(80);
    expect(clampColumnWidth(80)).toBe(80);
    expect(clampColumnWidth(200.4)).toBe(200);
    expect(clampColumnWidth(5000)).toBe(800);
    expect(widthOf({}, "title")).toBe(DEFAULT_TITLE_WIDTH);
    expect(widthOf({}, PROP.due)).toBe(DEFAULT_PROPERTY_WIDTH);
    expect(widthOf({ title: 40, [PROP.due]: 333 }, "title")).toBe(80);
    expect(widthOf({ title: 40, [PROP.due]: 333 }, PROP.due)).toBe(333);
  });

  it("reads the filters: objects with a property id and an operator, anything else reads as none", () => {
    expect(
      readFilters([
        { propertyId: "a", op: "isSet" },
        { propertyId: "b", op: "equals", value: 3 },
        { propertyId: "", op: "isSet" },
        { propertyId: "c" },
        { op: "isSet" },
        "x",
        null,
        { propertyId: "d", op: "future", value: { any: 1 } },
      ]),
    ).toEqual([
      { propertyId: "a", op: "isSet" },
      { propertyId: "b", op: "equals", value: 3 },
      { propertyId: "d", op: "future", value: { any: 1 } },
    ]);
    for (const odd of ["a", {}, null, undefined, 3]) {
      expect(readFilters(odd), JSON.stringify(odd)).toEqual([]);
    }
    expect(readDateRange({ from: "2026-10-01", to: "" })).toEqual({ from: "2026-10-01" });
    expect(readDateRange("x")).toEqual({});
  });
});

describe("moving a column and the window of rows", () => {
  it("moves a column past its visible neighbour, keeping the ids of properties the space lacks where they are", () => {
    expect(moveProperty(["a", "b", "c"], ["a", "b", "c"], "b", -1)).toEqual(["b", "a", "c"]);
    expect(moveProperty(["a", "b", "c"], ["a", "b", "c"], "b", 1)).toEqual(["a", "c", "b"]);
    expect(moveProperty(["a", "b", "c"], ["a", "b", "c"], "a", -1)).toBeNull();
    expect(moveProperty(["a", "b", "c"], ["a", "b", "c"], "c", 1)).toBeNull();
    expect(moveProperty(["a", "b", "c"], ["a", "b", "c"], "x", 1)).toBeNull();
    // "gone" is no column: "c" moves past "a", and "gone" keeps its place in the list.
    expect(moveProperty(["a", "gone", "c"], ["a", "c"], "c", -1)).toEqual(["c", "a", "gone"]);
  });

  it("draws every row up to 100 and the rows in view beyond", () => {
    expect(isWindowed(100)).toBe(false);
    expect(isWindowed(101)).toBe(true);
    expect(visibleRows(500, 0)).toEqual({ start: 0, end: 15 + 8 });
    const middle = visibleRows(500, ROW_HEIGHT * 200);
    expect(middle.start).toBe(200 - 8);
    expect(middle.end).toBe(200 + 15 + 8);
    expect(visibleRows(210, ROW_HEIGHT * 200).end).toBe(210);
    expect(scrollToReveal(3, 0)).toBe(0);
    expect(scrollToReveal(2, ROW_HEIGHT * 10)).toBe(ROW_HEIGHT * 2);
    expect(scrollToReveal(300, 0)).toBe(ROW_HEIGHT * 301 - (480 - ROW_HEIGHT));
  });
});

describe("the filters", () => {
  const def = (id: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({ id, name: id, kind, options, createdAt: 0 });
  const defs = [
    def("txt", "text"),
    def("num", "number"),
    def("sel", "select", [{ id: "o1", name: "One", color: "gray" }, { id: "o2", name: "Two", color: "red" }]),
    def("multi", "multi_select", [{ id: "o1", name: "One", color: "gray" }, { id: "o2", name: "Two", color: "red" }]),
    def("day", "date"),
    def("box", "checkbox"),
    def("todo", "checklist"),
    def("file", "files"),
    def("mystery", "hologram" as never),
  ];
  const page = (title: string, values: Record<string, unknown> = {}): PageInfo => ({
    id: title,
    spaceId: "s",
    title,
    createdAt: 0,
    updatedAt: 0,
    properties: Object.entries(values).map(([id, value]) => [id, value as never]),
  });
  const pass = (p: PageInfo, filter: PageListFilter): boolean | null => matchFilter(p, filter, defs);
  const item = (done: boolean) => ({ id: "a0000000-0000-4000-8000-000000000001", text: "x", done });

  it("offers the operators of each kind, and a new filter starts with the first one", () => {
    expect(filterOps("text")).toEqual(["contains", "notContains", "equals", "notEquals", "startsWith", "endsWith", "isSet", "isEmpty"]);
    expect(filterOps("number")).toEqual(["equals", "notEquals", "greater", "less", "atLeast", "atMost", "isSet", "isEmpty"]);
    expect(filterOps("status")).toEqual(["hasOption", "lacksOption", "isSet", "isEmpty"]);
    expect(filterOps("date")).toEqual(["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet", "isEmpty"]);
    expect(filterOps("checkbox")).toEqual(["checked", "unchecked"]);
    expect(filterOps("checklist")).toEqual(["complete", "incomplete", "isSet", "isEmpty"]);
    expect(filterOps("files")).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("hologram" as never)).toEqual(["isSet", "isEmpty"]);
    expect(newFilter(def("p", "date"))).toEqual({ propertyId: "p", op: "equals" });
  });

  it("is set / is empty: a blank text, an empty list and no value are empty; a zero and false are values", () => {
    const set: PageListFilter = { propertyId: "txt", op: "isSet" };
    const empty: PageListFilter = { propertyId: "txt", op: "isEmpty" };
    expect(pass(page("a", { txt: { kind: "text", value: "x" } }), set)).toBe(true);
    expect(pass(page("a"), set)).toBe(false);
    expect(pass(page("a", { txt: { kind: "text", value: "" } }), set)).toBe(false);
    expect(pass(page("a"), empty)).toBe(true);
    expect(pass(page("a", { txt: { kind: "text", value: "x" } }), empty)).toBe(false);
    expect(pass(page("a", { num: { kind: "number", value: 0 } }), { propertyId: "num", op: "isSet" })).toBe(true);
    expect(pass(page("a", { multi: { kind: "multi_select", value: [] } }), { propertyId: "multi", op: "isEmpty" })).toBe(true);
    expect(pass(page("a", { file: { kind: "files", value: ["f1"] } }), { propertyId: "file", op: "isSet" })).toBe(true);
    // A kind this version does not know can still be tested for a value.
    expect(pass(page("a", { mystery: { kind: "hologram", value: 1 } }), { propertyId: "mystery", op: "isSet" })).toBe(true);
  });

  it("text contains (ASCII case-insensitive, trimmed) and equals; number equals", () => {
    const hello = page("a", { txt: { kind: "text", value: "Hello World" } });
    expect(pass(hello, { propertyId: "txt", op: "contains", value: "  world " })).toBe(true);
    expect(pass(hello, { propertyId: "txt", op: "contains", value: "xyz" })).toBe(false);
    expect(pass(page("b"), { propertyId: "txt", op: "contains", value: "a" })).toBe(false);
    expect(pass(hello, { propertyId: "txt", op: "equals", value: "hello world" })).toBe(true);
    expect(pass(hello, { propertyId: "txt", op: "equals", value: "hello" })).toBe(false);
    expect(pass(page("a", { num: { kind: "number", value: 3 } }), { propertyId: "num", op: "equals", value: 3 })).toBe(true);
    expect(pass(page("a", { num: { kind: "number", value: 3 } }), { propertyId: "num", op: "equals", value: 4 })).toBe(false);
    expect(pass(page("a"), { propertyId: "num", op: "equals", value: 0 })).toBe(false);
  });

  it("has an option: a select, a status or any option of a multi-select", () => {
    expect(pass(page("a", { sel: { kind: "select", value: "o1" } }), { propertyId: "sel", op: "hasOption", value: "o1" })).toBe(true);
    expect(pass(page("a", { sel: { kind: "select", value: "o2" } }), { propertyId: "sel", op: "hasOption", value: "o1" })).toBe(false);
    expect(pass(page("a", { multi: { kind: "multi_select", value: ["o1", "o2"] } }), { propertyId: "multi", op: "hasOption", value: "o2" })).toBe(true);
    expect(pass(page("a", { multi: { kind: "multi_select", value: ["o1"] } }), { propertyId: "multi", op: "hasOption", value: "o2" })).toBe(false);
    expect(pass(page("a"), { propertyId: "multi", op: "hasOption", value: "o2" })).toBe(false);
  });

  it("dates: before and after a day (the time does not count), between inclusive with one or both ends", () => {
    const on = (date: string, time?: string): PageInfo => page("a", { day: { kind: "date", value: time === undefined ? { date } : { date, time } } });
    expect(pass(on("2026-10-09"), { propertyId: "day", op: "before", value: "2026-10-10" })).toBe(true);
    expect(pass(on("2026-10-10"), { propertyId: "day", op: "before", value: "2026-10-10" })).toBe(false);
    expect(pass(on("2026-10-10", "23:00"), { propertyId: "day", op: "after", value: "2026-10-10" })).toBe(false);
    expect(pass(on("2026-10-11"), { propertyId: "day", op: "after", value: "2026-10-10" })).toBe(true);
    const between = (value: unknown): PageListFilter => ({ propertyId: "day", op: "between", value });
    expect(pass(on("2026-10-10"), between({ from: "2026-10-10", to: "2026-10-12" }))).toBe(true);
    expect(pass(on("2026-10-12"), between({ from: "2026-10-10", to: "2026-10-12" }))).toBe(true);
    expect(pass(on("2026-10-13"), between({ from: "2026-10-10", to: "2026-10-12" }))).toBe(false);
    expect(pass(on("2026-10-09"), between({ from: "2026-10-10" }))).toBe(false);
    expect(pass(on("2026-10-09"), between({ to: "2026-10-10" }))).toBe(true);
    expect(pass(page("a"), between({ from: "2026-10-10" }))).toBe(false);
    expect(pass(page("a"), { propertyId: "day", op: "before", value: "2026-10-10" })).toBe(false);
  });

  it("checked / not checked, and checklist complete / not complete", () => {
    expect(pass(page("a", { box: { kind: "checkbox", value: true } }), { propertyId: "box", op: "checked" })).toBe(true);
    expect(pass(page("a", { box: { kind: "checkbox", value: false } }), { propertyId: "box", op: "checked" })).toBe(false);
    expect(pass(page("a"), { propertyId: "box", op: "checked" })).toBe(false);
    expect(pass(page("a"), { propertyId: "box", op: "unchecked" })).toBe(true);
    expect(pass(page("a", { box: { kind: "checkbox", value: true } }), { propertyId: "box", op: "unchecked" })).toBe(false);
    const list = (...done: boolean[]): PageInfo => page("a", { todo: { kind: "checklist", value: done.map(item) } });
    expect(pass(list(true, true), { propertyId: "todo", op: "complete" })).toBe(true);
    expect(pass(list(true, false), { propertyId: "todo", op: "complete" })).toBe(false);
    expect(pass(list(), { propertyId: "todo", op: "complete" })).toBe(false);
    expect(pass(list(true, false), { propertyId: "todo", op: "incomplete" })).toBe(true);
    expect(pass(list(true), { propertyId: "todo", op: "incomplete" })).toBe(false);
    expect(pass(page("a"), { propertyId: "todo", op: "incomplete" })).toBe(false);
  });

  it("cannot be applied (null) when the property is gone, the operator is not the kind's, or the value is missing", () => {
    const a = page("a", { txt: { kind: "text", value: "x" } });
    expect(pass(a, { propertyId: "gone", op: "isSet" })).toBeNull();
    expect(pass(a, { propertyId: "txt", op: "checked" })).toBeNull();
    expect(pass(a, { propertyId: "txt", op: "future" })).toBeNull();
    expect(pass(a, { propertyId: "txt", op: "contains" })).toBeNull();
    expect(pass(a, { propertyId: "txt", op: "contains", value: "  " })).toBeNull();
    expect(pass(a, { propertyId: "txt", op: "contains", value: 3 })).toBeNull();
    expect(pass(a, { propertyId: "num", op: "equals", value: "3" })).toBeNull();
    expect(pass(a, { propertyId: "sel", op: "hasOption" })).toBeNull();
    expect(pass(a, { propertyId: "day", op: "between", value: {} })).toBeNull();
    expect(pass(a, { propertyId: "mystery", op: "equals", value: "x" })).toBeNull();
  });

  it("combines the filters with AND and ignores (keeps) the ones that cannot be applied", () => {
    const pages = [
      page("a", { num: { kind: "number", value: 1 }, box: { kind: "checkbox", value: true } }),
      page("b", { num: { kind: "number", value: 1 }, box: { kind: "checkbox", value: false } }),
      page("c", { num: { kind: "number", value: 2 }, box: { kind: "checkbox", value: true } }),
    ];
    const filters: PageListFilter[] = [
      { propertyId: "num", op: "equals", value: 1 },
      { propertyId: "box", op: "checked" },
    ];
    expect(applyFilters(pages, filters, defs).map((entry) => entry.title)).toEqual(["a"]);
    expect(applyFilters(pages, [...filters, { propertyId: "gone", op: "isSet" }, { propertyId: "num", op: "future" }], defs).map((entry) => entry.title)).toEqual(["a"]);
    expect(applyFilters(pages, [], defs)).toHaveLength(3);
  });
});

// ----- the schema and the HTML form --------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("lists filters and columnWidths after the four attributes of the list view, like the fixture", () => {
    expect(live.nodes.pageList?.attrs).toEqual(["id", "source", "layout", "properties", "sort", "filters", "columnWidths", "groupBy", "hiddenGroups", "calculations", "views"]);
    expect(fixture.nodes.pageList).toEqual(live.nodes.pageList);
  });

  it("gives a block made without them no filters and no widths (absent, so that they are not written)", () => {
    const node = createEditorSchema().nodes.pageList?.create({ id: pid(9) });
    expect(node?.attrs.filters).toBeNull();
    expect(node?.attrs.columnWidths).toBeNull();
  });
});

describe("the HTML form", () => {
  function mount(content: JsonValue): Editor {
    const element = document.createElement("div");
    document.body.append(element);
    return new Editor({ element, extensions: createSchemaExtensions(), content: content as never });
  }

  it("writes the filters and the widths as JSON text when the block has them, and reads them back", () => {
    const filters = [{ propertyId: PROP.priority, op: "equals", value: 3 }];
    const columnWidths = { title: 320, [PROP.due]: 140 };
    const editor = mount({ type: "doc", content: [{ type: "pageList", attrs: { id: pid(2), ...DEFAULTS, filters, columnWidths } }] });
    const holder = document.createElement("div");
    holder.innerHTML = editor.getHTML();
    const element = holder.querySelector('[data-type="pageList"]');
    expect(JSON.parse(element?.getAttribute("data-filters") ?? "null")).toEqual(filters);
    expect(JSON.parse(element?.getAttribute("data-column-widths") ?? "null")).toEqual(columnWidths);
    expect(element?.getAttribute("data-layout")).toBe("table");

    const copy = mount({ type: "doc", content: [{ type: "paragraph" }] });
    copy.commands.setContent(editor.getHTML());
    expect(copy.state.doc.child(0).attrs).toMatchObject({ layout: "table", filters, columnWidths });
    editor.destroy();
    copy.destroy();
  });

  it("leaves them out when the block has none, and a copy stays without", () => {
    const editor = mount({ type: "doc", content: [{ type: "pageList", attrs: { id: pid(2), ...DEFAULTS } }] });
    const holder = document.createElement("div");
    holder.innerHTML = editor.getHTML();
    const element = holder.querySelector('[data-type="pageList"]');
    expect(element?.hasAttribute("data-filters")).toBe(false);
    expect(element?.hasAttribute("data-column-widths")).toBe(false);
    const copy = mount({ type: "doc", content: [{ type: "paragraph" }] });
    copy.commands.setContent(editor.getHTML());
    expect(copy.state.doc.child(0).attrs).toMatchObject({ filters: null, columnWidths: null, groupBy: null, hiddenGroups: null });
    editor.destroy();
    copy.destroy();
  });
});

// ----- the layout switch -------------------------------------------------------------------

describe("the layout", () => {
  it("is switched by the toolbar, writing the layout alone, and back", async () => {
    const { user, editor } = await openHub(hubContent({ layout: "list" }));
    expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
    expect(block()).toHaveAttribute("data-layout", "list");
    expect((await select(user, t("pageList.layout.label"))).value).toBe("list");
    const doc = docOf(editor);

    let before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageList.layout.label")), t("pageList.layout.table"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("table");
    });
    expect(grid()).toBeInTheDocument();
    expect(block()).toHaveAttribute("data-layout", "table");
    let changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"layout"');
    expect(attrsOf(editor)).toMatchObject({ id: pid(2), sort: { by: "title", dir: "asc" }, properties: [] });

    before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageList.layout.label")), t("pageList.layout.list"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("list");
    });
    changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
  });

  it("shows a layout it does not know as the list and keeps it until a control replaces it", async () => {
    const { user, editor, memory, ids } = await openHub(hubContent({ layout: "timeline" }));
    expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
    expect(within(block()).getAllByRole("listitem")).toHaveLength(4);
    expect((await select(user, t("pageList.layout.label"))).value).toBe("list");
    const doc = docOf(editor);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(hubContent({ layout: "timeline" }));
    await user.selectOptions(await select(user, t("pageList.layout.label")), t("pageList.layout.table"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("table");
    });
  });
});

// ----- the grid ----------------------------------------------------------------------------

describe("the grid", () => {
  it("has the page as its first column and one column per property of the block, with the rows sorted", async () => {
    await openHub(hubContent({ properties: [PROP.priority, PROP.status] }));
    expect(headers()).toEqual([t("pageList.sort.title"), "Priority", "Status"]);
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(grid()).toHaveAttribute("aria-rowcount", "6"); // the header, four rows and the footer (ADR-0040)
    expect(grid()).toHaveAttribute("aria-colcount", "4"); // the checkbox column, the page and two properties
    // The header has the glyph of the kind, and the cells hold the values.
    expect(header("Priority").querySelector(".property-glyph svg")).not.toBeNull();
    expect(cell("Alpha", "Priority")).toHaveTextContent("3");
    expect(cell("beta", "Status")).toHaveTextContent("Done");
    expect(within(cell("Alpha", "Status")).getByText("In progress")).toBeInTheDocument();
    // A property the space no longer has has no column (and stays in the list).
    expect(rowOf("Alpha")).toBeInTheDocument();
  });

  it("skips a property the space does not have", async () => {
    await openHub(hubContent({ properties: ["de000000-0000-4000-8000-00000000eeee", PROP.priority] }));
    expect(headers()).toEqual([t("pageList.sort.title"), "Priority"]);
  });

  it("opens a page from the button of its row, and shows the icon", async () => {
    const { user } = await openHub(hubContent(asPages()));
    await user.click(within(rowOf("beta")).getByRole("button", { name: t("pageTable.open", { title: "beta" }) }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("beta");
    });
  });

  it("marks the sorted column, and sorts by the header menu, writing the sort alone", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    expect(header(t("pageList.sort.title"))).toHaveAttribute("aria-sort", "ascending");
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await pickFromHeader(user, "Priority", t("pageTable.sortDesc"));
    await waitFor(() => {
      expect(attrsOf(editor).sort).toEqual({ by: PROP.priority, dir: "desc" });
    });
    expect(titles()).toEqual(["beta", "Alpha", "delta", "Gamma"]);
    expect(header("Priority")).toHaveAttribute("aria-sort", "descending");
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"sort"');

    await pickFromHeader(user, "Priority", t("pageTable.sortAsc"));
    await waitFor(() => {
      expect(titles()).toEqual(["delta", "Alpha", "beta", "Gamma"]);
    });
    await pickFromHeader(user, t("pageList.sort.title"), t("pageTable.sortDesc"));
    await waitFor(() => {
      expect(titles()).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    });
    expect(attrsOf(editor).sort).toEqual({ by: "title", dir: "desc" });
  });

  it("sorts by the several keys of a view, marks the first one, and sorts by the one key the header menu picks (ADR-0058)", async () => {
    const keys = [
      { by: PROP.stage, dir: "asc" },
      { by: "title", dir: "asc" },
    ];
    const { user, editor } = await openHub(
      hubContent({
        properties: [PROP.stage],
        sort: keys[0] ?? null,
        views: [{ id: "v1", name: "", layout: "table", properties: [PROP.stage], sort: keys[0] ?? null, sorts: keys, openIn: "page" }],
      }),
    );
    // Gamma and delta have no stage: the title tells them apart (the source order is Gamma, delta).
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(header("Stage")).toHaveAttribute("aria-sort", "ascending");
    expect(header(t("pageList.sort.title"))).not.toHaveAttribute("aria-sort");

    await pickFromHeader(user, "Stage", t("pageTable.sortDesc"));
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "Alpha", "Gamma", "delta"]);
    });
    // The header's sort is the view's only key: `sorts` is gone, `sort` (and the mirror) is that key.
    const [view] = attrsOf(editor).views as Record<string, unknown>[];
    expect(view).not.toHaveProperty("sorts");
    expect(view?.sort).toEqual({ by: PROP.stage, dir: "desc" });
    expect(attrsOf(editor).sort).toEqual({ by: PROP.stage, dir: "desc" });
  });

  it("hides a column from its menu, writing the list of properties alone", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority, PROP.status, PROP.due] }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await pickFromHeader(user, "Status", t("pageTable.hide"));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.priority, PROP.due]);
    });
    expect(headers()).toEqual([t("pageList.sort.title"), "Priority", "Due"]);
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"properties"');
  });

  it("moves a column left or right, and disables the moves at the ends (the page column has none)", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority, PROP.status, PROP.due] }));
    await pickFromHeader(user, "Status", t("pageTable.moveLeft"));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.status, PROP.priority, PROP.due]);
    });
    expect(headers()).toEqual([t("pageList.sort.title"), "Status", "Priority", "Due"]);
    await pickFromHeader(user, "Status", t("pageTable.moveRight"));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.priority, PROP.status, PROP.due]);
    });

    await user.click(within(header("Priority")).getByRole("button", { name: /^Priority/u }));
    const first = await screen.findByRole("menu", { name: t("pageTable.menu", { name: "Priority" }) });
    expect(within(first).getByRole("menuitem", { name: t("pageTable.moveLeft") })).toBeDisabled();
    expect(within(first).getByRole("menuitem", { name: t("pageTable.moveRight") })).toBeEnabled();
    await user.keyboard("{Escape}");
    await user.click(within(header("Due")).getByRole("button", { name: /^Due/u }));
    const last = await screen.findByRole("menu", { name: t("pageTable.menu", { name: "Due" }) });
    expect(within(last).getByRole("menuitem", { name: t("pageTable.moveRight") })).toBeDisabled();
    await user.keyboard("{Escape}");

    await user.click(within(header(t("pageList.sort.title"))).getByRole("button", { name: new RegExp(t("pageList.sort.title"), "u") }));
    const title = await screen.findByRole("menu", { name: t("pageTable.menu", { name: t("pageList.sort.title") }) });
    expect(within(title).queryByRole("menuitem", { name: t("pageTable.hide") })).not.toBeInTheDocument();
    expect(within(title).queryByRole("menuitem", { name: t("pageTable.moveLeft") })).not.toBeInTheDocument();
  });
});

// ----- the widths --------------------------------------------------------------------------

describe("the column widths", () => {
  const handle = (column: string): HTMLElement => within(header(column)).getByRole("separator");

  it("start from the defaults and come from the block", async () => {
    await openHub(hubContent({ properties: [PROP.priority, PROP.due], columnWidths: { title: 400, [PROP.due]: 10 } }));
    expect(header(t("pageList.sort.title"))).toHaveAttribute("data-width", "400");
    expect(header("Priority")).toHaveAttribute("data-width", String(DEFAULT_PROPERTY_WIDTH));
    expect(header("Due")).toHaveAttribute("data-width", "80"); // never narrower than 80
  });

  it("follow the pointer while it is dragged and are written once, when it is released", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    fireEvent.pointerDown(handle("Priority"), { clientX: 500, button: 0 });
    fireEvent.pointerMove(document, { clientX: 540 });
    fireEvent.pointerMove(document, { clientX: 560 });
    expect(header("Priority")).toHaveAttribute("data-width", String(DEFAULT_PROPERTY_WIDTH + 60));
    expect(changesSince(doc, before)).toHaveLength(0);
    expect(attrsOf(editor).columnWidths).toBeNull();
    fireEvent.pointerUp(document, { clientX: 560 });
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ [PROP.priority]: DEFAULT_PROPERTY_WIDTH + 60 });
    });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"columnWidths"');
    expect(header("Priority")).toHaveAttribute("data-width", String(DEFAULT_PROPERTY_WIDTH + 60));
  });

  it("stop at 80 pixels, keep the widths of the other columns, and a drag that moves nothing writes nothing", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.priority], columnWidths: { title: 300 } }));
    const doc = docOf(editor);
    fireEvent.pointerDown(handle("Priority"), { clientX: 500, button: 0 });
    fireEvent.pointerMove(document, { clientX: 100 });
    fireEvent.pointerUp(document, { clientX: 100 });
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ title: 300, [PROP.priority]: 80 });
    });
    const before = doc.oplogVersion();
    fireEvent.pointerDown(handle("Priority"), { clientX: 500, button: 0 });
    fireEvent.pointerUp(document, { clientX: 500 });
    expect(changesSince(doc, before)).toHaveLength(0);
    fireEvent.pointerDown(handle("Priority"), { clientX: 500, button: 0 });
    fireEvent.pointerMove(document, { clientX: 700 });
    fireEvent.pointerCancel(document);
    expect(header("Priority")).toHaveAttribute("data-width", "80");
    expect(changesSince(doc, before)).toHaveLength(0);
  });

  it("can be changed from the keyboard with the arrow keys on the handle", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    handle("Priority").focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ [PROP.priority]: DEFAULT_PROPERTY_WIDTH + 16 });
    });
  });
});

// ----- the grid fills its block (ADR-0035) -------------------------------------------------

describe("the tracks of the grid", () => {
  const keys = ["title", PROP.priority, PROP.due];

  it("give the page column the room the others leave, and every other column its exact width", () => {
    expect(gridLayout(keys, {}, null)).toEqual({
      template: `minmax(${String(DEFAULT_TITLE_WIDTH)}px, 1fr) ${String(DEFAULT_PROPERTY_WIDTH)}px ${String(DEFAULT_PROPERTY_WIDTH)}px`,
      minWidth: DEFAULT_TITLE_WIDTH + 2 * DEFAULT_PROPERTY_WIDTH,
    });
    // The widths of the block, clamped as they are drawn; the minimum of the grid is their sum.
    expect(gridLayout(keys, { title: 400, [PROP.priority]: 10, [PROP.due]: 900 }, null)).toEqual({
      template: "minmax(400px, 1fr) 80px 800px",
      minWidth: 1280,
    });
    expect(gridLayout(["title"], {}, null).template).toBe("minmax(280px, 1fr)");
  });

  it("are all fixed while a column is dragged, the page column at the width it had when the drag began", () => {
    expect(gridLayout(keys, { title: 400 }, { column: PROP.priority, width: 190, title: 900 })).toEqual({
      template: "900px 190px 170px",
      minWidth: 1260,
    });
    // The page column dragged: its own live width.
    expect(gridLayout(keys, {}, { column: "title", width: 1500, title: 1500 })).toEqual({
      template: "1500px 170px 170px",
      minWidth: 1840,
    });
    // Where the page column could not be measured, it keeps its stored width (but is not stretched).
    expect(gridLayout(keys, { title: 400 }, { column: PROP.due, width: 120, title: null })).toEqual({
      template: "400px 170px 120px",
      minWidth: 690,
    });
  });

  it("let a drag begin above the widest width a column keeps without jumping, and never end narrower than the narrowest", () => {
    expect(dragWidth(1500, 2000)).toBe(1500);
    expect(dragWidth(2100, 2000)).toBe(2000);
    expect(dragWidth(900, 170)).toBe(800);
    expect(dragWidth(900, 800)).toBe(800);
    expect(dragWidth(50, 2000)).toBe(80);
    expect(dragWidth(300.4, 170)).toBe(300);
  });

  it("are written to the grid, with the sum of the widths as its minimum", async () => {
    await openHub(hubContent({ properties: [PROP.priority], columnWidths: { title: 300 } }));
    const element = grid();
    // The first track is the checkbox column of the selection (ADR-0058).
    expect(element.style.getPropertyValue("--page-table-columns")).toBe(
      `${String(SELECT_COLUMN_WIDTH)}px minmax(300px, 1fr) ${String(DEFAULT_PROPERTY_WIDTH)}px`,
    );
    expect(element.style.minWidth).toBe(`${String(SELECT_COLUMN_WIDTH + 300 + DEFAULT_PROPERTY_WIDTH)}px`);
    // The rule that makes the grid fill its block (jsdom has no layout); the inline minimum makes it scroll when narrower.
    const css = readFileSync("src/styles/app.css", "utf8");
    const at = css.indexOf("\n.page-editor-content .page-table-grid {");
    expect(css.slice(at, css.indexOf("}", at))).toContain("width: 100%");
  });
});

describe("resizing a column of a grid that fills its block", () => {
  const handle = (column: string): HTMLElement => within(header(column)).getByRole("separator");
  const rect = (width: number): DOMRect => ({
    left: 0,
    top: 0,
    right: width,
    bottom: 34,
    width,
    height: 34,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  });
  // The tracks of the grid without the checkbox column of the selection (ADR-0058), which is not what these tests are about.
  const columns = (): string => grid().style.getPropertyValue("--page-table-columns").replace(`${String(SELECT_COLUMN_WIDTH)}px `, "");

  /** The page column is drawn `width` wide (it fills a wide block), whatever width it keeps. */
  function stretch(width: number): void {
    vi.spyOn(header(t("pageList.sort.title")), "getBoundingClientRect").mockReturnValue(rect(width));
  }

  it("starts the page column from the width it is drawn with, and the edge follows the pointer", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    stretch(600); // it keeps 280
    fireEvent.pointerDown(handle(t("pageList.sort.title")), { clientX: 700, button: 0 });
    expect(header(t("pageList.sort.title"))).toHaveAttribute("data-width", "600"); // no jump
    expect(columns()).toBe(`600px ${String(DEFAULT_PROPERTY_WIDTH)}px`); // fixed while it is dragged
    fireEvent.pointerMove(document, { clientX: 740 });
    expect(header(t("pageList.sort.title"))).toHaveAttribute("data-width", "640");
    expect(columns()).toBe(`640px ${String(DEFAULT_PROPERTY_WIDTH)}px`);
    expect(grid().style.minWidth).toBe(`${String(SELECT_COLUMN_WIDTH + 640 + DEFAULT_PROPERTY_WIDTH)}px`);
    expect(attrsOf(editor).columnWidths).toBeNull();
    fireEvent.pointerUp(document, { clientX: 740 });
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ title: 640 });
    });
    expect(columns()).toBe(`minmax(640px, 1fr) ${String(DEFAULT_PROPERTY_WIDTH)}px`);
  });

  it("writes nothing for a drag that ends where it began, though the width it is drawn with is not the one it keeps", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    stretch(600);
    fireEvent.pointerDown(handle(t("pageList.sort.title")), { clientX: 700, button: 0 });
    fireEvent.pointerUp(document, { clientX: 700 });
    fireEvent.pointerDown(handle(t("pageList.sort.title")), { clientX: 700, button: 0 });
    fireEvent.pointerMove(document, { clientX: 760 });
    fireEvent.pointerMove(document, { clientX: 700 });
    fireEvent.pointerUp(document, { clientX: 700 });
    expect(changesSince(doc, before)).toHaveLength(0);
    expect(attrsOf(editor).columnWidths).toBeNull();
    expect(columns()).toBe(`minmax(${String(DEFAULT_TITLE_WIDTH)}px, 1fr) ${String(DEFAULT_PROPERTY_WIDTH)}px`);
  });

  it("keeps the page column where it is drawn while another column is dragged", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    stretch(900);
    fireEvent.pointerDown(handle("Priority"), { clientX: 500, button: 0 });
    expect(columns()).toBe(`900px ${String(DEFAULT_PROPERTY_WIDTH)}px`);
    fireEvent.pointerMove(document, { clientX: 530 });
    expect(columns()).toBe(`900px ${String(DEFAULT_PROPERTY_WIDTH + 30)}px`);
    expect(header("Priority")).toHaveAttribute("data-width", String(DEFAULT_PROPERTY_WIDTH + 30));
    fireEvent.pointerUp(document, { clientX: 530 });
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ [PROP.priority]: DEFAULT_PROPERTY_WIDTH + 30 });
    });
    // Released, the page column is the one that gives and takes room again.
    expect(columns()).toBe(`minmax(${String(DEFAULT_TITLE_WIDTH)}px, 1fr) ${String(DEFAULT_PROPERTY_WIDTH + 30)}px`);
  });

  it("starts the arrow keys from the width a column is drawn with", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    stretch(600);
    handle(t("pageList.sort.title")).focus();
    await user.keyboard("{ArrowRight}");
    await waitFor(() => {
      expect(attrsOf(editor).columnWidths).toEqual({ title: 616 });
    });
  });
});

// ----- editing the cells -------------------------------------------------------------------

describe("editing a cell", () => {
  const propertyView = (): JsonValue =>
    hubContent({ properties: [PROP.summary, PROP.priority, PROP.done, PROP.stage, PROP.status, PROP.areas, PROP.due, PROP.link, PROP.todo] });

  it("text: a click edits in place, Enter writes through pagePropertySet and ends the editing, Escape drops the change", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(cell("Alpha", "Summary"));
    const field = await within(cell("Alpha", "Summary")).findByRole("textbox", { name: "Summary" });
    expect(field).toHaveFocus();
    await user.clear(field);
    await user.type(field, "Final{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.summary)).toEqual({ kind: "text", value: "Final" });
    });
    await waitFor(() => {
      expect(within(cell("Alpha", "Summary")).queryByRole("textbox")).not.toBeInTheDocument();
    });
    expect(cell("Alpha", "Summary")).toHaveTextContent("Final");
    expect(cell("Alpha", "Summary")).toHaveFocus();

    await user.click(cell("Alpha", "Summary"));
    const again = await within(cell("Alpha", "Summary")).findByRole("textbox", { name: "Summary" });
    await user.clear(again);
    await user.type(again, "Changed my mind{Escape}");
    await waitFor(() => {
      expect(within(cell("Alpha", "Summary")).queryByRole("textbox")).not.toBeInTheDocument();
    });
    expect(await stored(client, ids.page("Alpha"), PROP.summary)).toEqual({ kind: "text", value: "Final" });
  });

  it("text: leaving the cell writes the text, and a page without a value gets one", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(cell("Gamma", "Summary"));
    await user.type(await within(cell("Gamma", "Summary")).findByRole("textbox", { name: "Summary" }), "From blank");
    await user.click(cell("beta", "Summary")); // another cell takes the click
    await waitFor(async () => {
      expect(await stored(client, ids.page("Gamma"), PROP.summary)).toEqual({ kind: "text", value: "From blank" });
    });
    expect(within(cell("Gamma", "Summary")).queryByRole("textbox")).not.toBeInTheDocument();
    // The click went on to the other cell, which is being edited.
    expect(await within(cell("beta", "Summary")).findByRole("textbox", { name: "Summary" })).toBeInTheDocument();
  });

  it("number: a draft that is not a number is not saved and keeps the editor open to say why", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(cell("Alpha", "Priority"));
    const field = await within(cell("Alpha", "Priority")).findByRole("textbox", { name: "Priority" });
    await user.clear(field);
    await user.type(field, "twelve{Enter}");
    expect(await within(cell("Alpha", "Priority")).findByText(t("properties.problem.number"))).toBeInTheDocument();
    expect(await stored(client, ids.page("Alpha"), PROP.priority)).toEqual({ kind: "number", value: 3 });
    await user.clear(field);
    await user.type(field, "7{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.priority)).toEqual({ kind: "number", value: 7 });
    });
  });

  it("url: the editor refuses what is not a link", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(cell("Alpha", "Link"));
    const field = await within(cell("Alpha", "Link")).findByRole("textbox", { name: "Link" });
    await user.clear(field);
    await user.type(field, "not a link{Enter}");
    expect(await within(cell("Alpha", "Link")).findByText(t("properties.problem.url"))).toBeInTheDocument();
    await user.clear(field);
    await user.type(field, "https://atomik.dev/kb{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.link)).toEqual({ kind: "url", value: "https://atomik.dev/kb" });
    });
  });

  it("checkbox: the box is live in the cell, one click writes", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(within(cell("Alpha", "Done")).getByRole("checkbox", { name: "Done" }));
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.done)).toEqual({ kind: "checkbox", value: false });
    });
    await user.click(within(cell("Gamma", "Done")).getByRole("checkbox", { name: "Done" }));
    await waitFor(async () => {
      expect(await stored(client, ids.page("Gamma"), PROP.done)).toEqual({ kind: "checkbox", value: true });
    });
  });

  it("select and status: a click opens the picker, a pick writes", async () => {
    const { user, client, ids, memory } = await openHub(propertyView());
    await user.click(cell("Alpha", "Stage"));
    const list = await screen.findByRole("listbox", { name: "Stage" });
    await user.click(within(list).getByRole("option", { name: "Review" }));
    const stage = (await new CoreClient(memory).propertySchemaGet(ids.space("Personal"))).find((def) => def.id === PROP.stage);
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.stage)).toEqual({ kind: "select", value: stage?.options.find((o) => o.name === "Review")?.id });
    });

    await user.click(cell("beta", "Status"));
    const statuses = await screen.findByRole("listbox", { name: "Status" });
    await user.click(within(statuses).getByRole("option", { name: "In progress" }));
    await waitFor(async () => {
      expect(await stored(client, ids.page("beta"), PROP.status)).toMatchObject({ kind: "status" });
    });
    await waitFor(() => {
      expect(within(cell("beta", "Status")).getByText("In progress")).toBeInTheDocument();
    });
  });

  it("multi-select: adds an option to the list", async () => {
    const { user, client, ids, memory } = await openHub(propertyView());
    await user.click(cell("delta", "Areas"));
    const list = await screen.findByRole("listbox", { name: "Areas" });
    await user.click(within(list).getByRole("option", { name: "Home" }));
    const areas = (await new CoreClient(memory).propertySchemaGet(ids.space("Personal"))).find((def) => def.id === PROP.areas);
    const [home, work] = [areas?.options.find((o) => o.name === "Home")?.id, areas?.options.find((o) => o.name === "Work")?.id];
    await waitFor(async () => {
      expect(await stored(client, ids.page("delta"), PROP.areas)).toEqual({ kind: "multi_select", value: [work, home] });
    });
  });

  it("date: a click opens the calendar, a day writes", async () => {
    const { user, client, ids } = await openHub(propertyView());
    await user.click(cell("Alpha", "Due"));
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    await user.click(within(picker).getByRole("button", { name: fullDate("2026-10-20") }));
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.due)).toEqual({ kind: "date", value: { date: "2026-10-20" } });
    });
  });

  it("checklist: shows the progress and edits in a popover over the cell", async () => {
    const { user, client, ids } = await openHub(propertyView());
    expect(cell("Alpha", "Todo")).toHaveTextContent("2/2 done");
    expect(cell("beta", "Todo")).toHaveTextContent("1/2 done");
    await user.click(cell("beta", "Todo"));
    const popover = await screen.findByRole("dialog", { name: "Todo" });
    await user.click(within(popover).getByRole("checkbox", { name: t("properties.checklist.done", { number: 2 }) }));
    await waitFor(async () => {
      const value = await stored(client, ids.page("beta"), PROP.todo);
      expect(value?.kind === "checklist" && value.value.every((item) => item.done)).toBe(true);
    });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: "Todo" })).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(cell("beta", "Todo")).toHaveTextContent("2/2 done");
    });
  });

  it("the title: double click (or Enter) edits it in place and renames the page", async () => {
    const { user, client, ids } = await openHub();
    const row = rowOf("Gamma");
    await user.dblClick(cell("Gamma", t("pageList.sort.title")));
    const field = await within(row).findByRole("textbox", { name: t("pageTable.titleEdit") });
    await user.clear(field);
    await user.type(field, "Zulu{Enter}");
    await waitFor(async () => {
      expect((await client.pageGet(ids.page("Gamma"))).title).toBe("Zulu");
    });
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "delta", "Zulu"]);
    });
  });

  it("says why a value could not be written", async () => {
    const { user, memory } = await openHub(propertyView());
    const original = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PagePropertySet" ? Promise.reject(new Error("disk full")) : original(request),
    );
    await user.click(cell("Alpha", "Summary"));
    const field = await within(cell("Alpha", "Summary")).findByRole("textbox", { name: "Summary" });
    await user.clear(field);
    await user.type(field, "x{Enter}");
    // The table says it (the editor may be gone by then).
    await waitFor(() => {
      expect(within(block()).getAllByRole("alert")[0]).toHaveTextContent("disk full");
    });
  });
});

// ----- the keyboard ------------------------------------------------------------------------

describe("the keyboard", () => {
  const view = (): JsonValue => hubContent({ properties: [PROP.summary, PROP.done] });

  it("moves the active cell with the arrows, Home and End, and stays inside the table", async () => {
    const { user } = await openHub(view());
    await user.click(cell("Alpha", t("pageList.sort.title")));
    expect(cell("Alpha", t("pageList.sort.title"))).toHaveAttribute("data-active");
    await user.keyboard("{ArrowRight}");
    expect(cell("Alpha", "Summary")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(cell("beta", "Summary")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(cell("Gamma", "Summary")).toHaveFocus();
    await user.keyboard("{End}");
    expect(cell("Gamma", "Done")).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(cell("Gamma", "Done")).toHaveFocus();
    await user.keyboard("{Home}{ArrowUp}");
    expect(cell("delta", t("pageList.sort.title"))).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(cell("delta", t("pageList.sort.title"))).toHaveFocus();
  });

  it("edits the active cell on Enter, leaves the editor on Escape, and leaves the cell on a second Escape", async () => {
    const { user, client, ids } = await openHub(view());
    await user.click(cell("Alpha", t("pageList.sort.title")));
    await user.keyboard("{ArrowRight}{Enter}");
    const field = await within(cell("Alpha", "Summary")).findByRole("textbox", { name: "Summary" });
    await user.type(field, "!{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("Alpha"), PROP.summary)).toEqual({ kind: "text", value: "A short note!" });
    });
    await waitFor(() => {
      expect(cell("Alpha", "Summary")).toHaveFocus();
    });
    await user.keyboard("{Enter}");
    await within(cell("Alpha", "Summary")).findByRole("textbox", { name: "Summary" });
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(cell("Alpha", "Summary")).toHaveFocus();
    });
    expect(within(cell("Alpha", "Summary")).queryByRole("textbox")).not.toBeInTheDocument();
    expect(cell("Alpha", "Summary")).toHaveAttribute("data-active");
    await user.keyboard("{Escape}");
    expect(cell("Alpha", "Summary")).not.toHaveAttribute("data-active");
  });

  it("toggles a checkbox with Enter or Space, and edits a title with Enter", async () => {
    const { user, client, ids } = await openHub(view());
    const row = rowOf("beta");
    await user.click(cell("beta", t("pageList.sort.title")));
    await user.keyboard("{ArrowRight}{ArrowRight}{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("beta"), PROP.done)).toEqual({ kind: "checkbox", value: true });
    });
    await user.keyboard(" ");
    await waitFor(async () => {
      expect(await stored(client, ids.page("beta"), PROP.done)).toEqual({ kind: "checkbox", value: false });
    });
    await user.keyboard("{Home}{Enter}");
    expect(await within(row).findByRole("textbox", { name: t("pageTable.titleEdit") })).toBeInTheDocument();
  });

  it("has one cell in the tab order, the active one", async () => {
    const { user } = await openHub(view());
    const tabbable = (): HTMLElement[] => within(grid()).getAllByRole("gridcell").filter((entry) => entry.tabIndex === 0);
    expect(tabbable()).toHaveLength(1);
    expect(tabbable()[0]).toBe(cell("Alpha", t("pageList.sort.title")));
    await user.click(cell("delta", "Summary"));
    await user.keyboard("{Escape}");
    await user.click(cell("delta", t("pageList.sort.title")));
    expect(tabbable()).toEqual([cell("delta", t("pageList.sort.title"))]);
  });
});

// ----- New page ----------------------------------------------------------------------------

describe("New page", () => {
  it("is a row at the bottom of the table that makes a page under the page and opens it", async () => {
    const { user, client, ids, spaceId } = await openHub(hubContent(asPages()));
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
    });
    const hub = (await client.treeGet(spaceId)).roots.find((node) => node.page.id === ids.page("Hub"));
    expect(hub?.children.map((node) => node.page.title)).toEqual(["Alpha", "beta", "Gamma", "delta", ""]);
  });

  it("gives the page the tag of a tag source", async () => {
    const { user, client, ids, spaceId } = await openHub(hubContent({ source: { kind: "tag", optionId: TAG.project }, ...asPages() }));
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "delta"]);
    });
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
    });
    const hub = (await client.treeGet(spaceId)).roots.find((node) => node.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(await client.pagePropertiesGet(created?.id ?? "")).toEqual([[TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.project] }]]);
  });
});

// ----- the filters -------------------------------------------------------------------------

describe("the filters of a view", () => {
  it("apply to the rows of the table and of the list, an unknown one being ignored and kept", async () => {
    const filters: JsonValue[] = [
      { propertyId: PROP.priority, op: "equals", value: 3 },
      { propertyId: "de000000-0000-4000-8000-00000000eeee", op: "isSet" },
      { propertyId: PROP.summary, op: "future" },
    ];
    const { editor, memory, ids } = await openHub(hubContent({ filters }));
    expect(titles()).toEqual(["Alpha"]);
    expect(within(toolbar()).getByRole("button", { name: /^Filter/u })).toHaveTextContent("Filter 3");
    const doc = docOf(editor);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(hubContent({ filters }));
  });

  it("apply to the list layout too, and say so when they hide every page", async () => {
    await openHub(hubContent({ layout: "list", filters: [{ propertyId: PROP.done, op: "checked" }] }));
    expect(within(block()).getAllByRole("listitem")).toHaveLength(1);
    expect(within(block()).getByRole("button", { name: "Alpha" })).toBeInTheDocument();
  });

  it("says No pages match the filters when they hide every page (table and list)", async () => {
    await openHub(hubContent({ filters: [{ propertyId: PROP.priority, op: "equals", value: 99 }] }));
    expect(titles()).toEqual([]);
    expect(within(block()).getByText(t("pageList.empty.filtered"))).toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("pageList.newPage") })).toBeEnabled();
  });

  it("are edited in the toolbar popover: add, choose the property and the condition, give the value, remove; one write each", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.priority] }));
    const doc = docOf(editor);
    await user.click(within(toolbar()).getByRole("button", { name: t("pageList.filter") }));
    const panel = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    expect(within(panel).getByText(t("pageList.filter.none"))).toBeInTheDocument();

    let before = doc.oplogVersion();
    await user.click(within(panel).getByRole("button", { name: t("pageList.filter.add") }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: TAGS_PROPERTY_ID, op: "hasOption" }]);
    });
    let changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"filters"');
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]); // no option yet: nothing hidden

    const row = within(panel).getByRole("group", { name: t("pageList.filter.row", { n: 1 }) });
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Priority");
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.priority, op: "equals" }]);
    });
    before = doc.oplogVersion();
    const number = within(row).getByRole("textbox", { name: t("pageList.filter.number") });
    await user.type(number, "3");
    expect(changesSince(doc, before)).toHaveLength(0); // typing writes nothing
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.priority, op: "equals", value: 3 }]);
    });
    changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(titles()).toEqual(["Alpha"]);
    expect(within(toolbar()).getByRole("button", { name: /^Filter/u })).toHaveTextContent("Filter 1");

    await user.click(within(row).getByRole("button", { name: t("pageList.filter.remove") }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([]);
    });
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(within(panel).getByText(t("pageList.filter.none"))).toBeInTheDocument();
  });

  it("offers the option of a select property, a day for a date and a range between two days", async () => {
    const { user, editor } = await openHub(hubContent());
    await user.click(within(toolbar()).getByRole("button", { name: t("pageList.filter") }));
    const panel = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    await user.click(within(panel).getByRole("button", { name: t("pageList.filter.add") }));
    const row = await within(panel).findByRole("group", { name: t("pageList.filter.row", { n: 1 }) });
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Stage");
    await waitFor(() => {
      expect(within(row).getByRole("combobox", { name: t("pageList.filter.option") })).toBeInTheDocument();
    });
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.option") }), "Draft");
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha"]);
    });

    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Due");
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.op") }), t("pageList.op.between"));
    fireEvent.change(within(row).getByLabelText(t("pageList.filter.from")), { target: { value: "2026-10-05" } });
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.due, op: "between", value: { from: "2026-10-05" } }]);
    });
    fireEvent.change(within(row).getByLabelText(t("pageList.filter.to")), { target: { value: "2026-10-10" } });
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: PROP.due, op: "between", value: { from: "2026-10-05", to: "2026-10-10" } }]);
    });
    expect(titles()).toEqual(["beta"]);

    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.op") }), t("pageList.op.isEmpty"));
    await waitFor(() => {
      expect(titles()).toEqual(["Gamma"]);
    });
  });

  it("lists a filter it does not know with a remove button only", async () => {
    const odd = { propertyId: "de000000-0000-4000-8000-00000000eeee", op: "isSet" };
    const { user, editor } = await openHub(hubContent({ filters: [odd] }));
    await user.click(within(toolbar()).getByRole("button", { name: /^Filter/u }));
    const panel = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    expect(within(panel).getByText(t("pageList.filter.unknown"))).toBeInTheDocument();
    expect(within(panel).queryByRole("combobox")).not.toBeInTheDocument();
    expect(attrsOf(editor).filters).toEqual([odd]);
    await user.click(within(panel).getByRole("button", { name: t("pageList.filter.remove") }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([]);
    });
  });
});

// ----- many rows ---------------------------------------------------------------------------

describe("a table of many rows", () => {
  it("draws only the rows in view and the ones scrolled to", async () => {
    await openHub(hubContent(), 200);
    expect(grid()).toHaveAttribute("aria-rowcount", "206"); // the header, 204 rows and the footer
    const first = bodyRows();
    expect(first.length).toBeLessThan(40);
    expect(first.length).toBeGreaterThan(10);
    expect(titles()).toContain("Alpha");
    expect(titles()).not.toContain("Row 150");

    const scroller = grid().closest(".page-table-scroll");
    expect(scroller).not.toBeNull();
    fireEvent.scroll(scroller as Element, { target: { scrollTop: ROW_HEIGHT * 150 } });
    await waitFor(() => {
      expect(titles()).toContain("Row 150");
    });
    expect(titles()).not.toContain("Alpha");
    expect(bodyRows().length).toBeLessThan(40);
    expect(bodyRows()[0]?.getAttribute("aria-rowindex")).not.toBe("2");
  });

  it("draws every row of a table of 100", async () => {
    await openHub(hubContent(), 96);
    expect(bodyRows()).toHaveLength(100);
  });
});

// ----- a read-only editor ------------------------------------------------------------------

describe("a read-only editor", () => {
  it("shows the table and the values, and offers no menu, handle, editor, filter or New page", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent({ properties: [PROP.summary, PROP.done, PROP.stage] });
    (content as { content: JsonValue[] }).content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
    const { user, editor, memory, ids } = await openHub(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(cell("Alpha", "Summary")).toHaveTextContent("A short note");
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    expect(within(block()).queryByRole("separator")).not.toBeInTheDocument();
    expect(within(header("Summary")).queryByRole("button")).not.toBeInTheDocument();
    expect(within(block()).queryByRole("button", { name: t("pageList.newPage") })).not.toBeInTheDocument();
    expect(within(block()).queryByRole("checkbox")).not.toBeInTheDocument();

    await user.dblClick(cell("Alpha", t("pageList.sort.title")));
    await user.click(cell("Alpha", "Summary"));
    await user.keyboard("{Enter}");
    expect(within(block()).queryByRole("textbox", { name: /Summary|Page title/u })).not.toBeInTheDocument();
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(content);
  });

  it("still opens a page from its row", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent(asPages());
    (content as { content: JsonValue[] }).content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
    const { user } = await openHub(content);
    await screen.findByText(t("editor.readOnly"));
    await user.click(within(rowOf("Gamma")).getByRole("button", { name: t("pageTable.open", { title: "Gamma" }) }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("Gamma");
    });
  });
});

// ----- nothing is written ------------------------------------------------------------------

describe("the document", () => {
  it("records no change when the table is shown, scrolled, navigated and the tree changes", async () => {
    const { editor, client, ids, spaceId, user } = await openHub(hubContent({ properties: [PROP.summary], filters: [{ propertyId: PROP.priority, op: "isSet" }], columnWidths: { title: 300 } }));
    const doc = docOf(editor);
    const own = (): number => doc.oplogVersion().get(doc.peerIdStr) ?? 0;
    expect(own()).toBe(0);
    await user.click(cell("Alpha", t("pageList.sort.title")));
    await user.keyboard("{ArrowRight}{ArrowDown}{Escape}");
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "New one" });
    await client.pagePropertySet(ids.page("Gamma"), PROP.priority, { kind: "number", value: 5 });
    await waitFor(() => {
      expect(titles()).toContain("Gamma");
    });
    act(() => {
      fireEvent.focus(editor.view.dom);
    });
    expect(own()).toBe(0);
  });
});

// ----- the kinds of ADR-0026 (amendment of 2026-10-08) and ADR-0027 -----------------------------

describe("the filters on the new kinds", () => {
  const rollupDef = (id: string, fn: string): PropertyDef => ({
    id,
    name: id,
    kind: "rollup",
    options: [],
    createdAt: 0,
    config: { kind: "rollup", relationPropertyId: "rel", target: "title", function: fn } as never,
  });
  const def = (id: string, kind: PropertyDef["kind"]): PropertyDef => ({ id, name: id, kind, options: [], createdAt: 0 });
  const defs = [
    def("place", "place"),
    def("made", "created_time"),
    def("edit", "last_edited_time"),
    def("by", "created_by"),
    def("editBy", "last_edited_by"),
    def("rel", "relation"),
    rollupDef("sum", "sum"),
    rollupDef("late", "latest"),
  ];
  const page = (title: string, values: Record<string, unknown> = {}): PageInfo => ({
    id: title,
    spaceId: "s",
    title,
    createdAt: 0,
    updatedAt: 0,
    properties: Object.entries(values).map(([id, value]) => [id, value as never]),
  });
  const pass = (p: PageInfo, filter: PageListFilter): boolean | null => matchFilter(p, filter, defs);
  /** A local instant: the days of a filter are the person's days. */
  const at = (y: number, m: number, d: number, h = 12): number => new Date(y, m - 1, d, h).getTime();

  it("offers the operators of each new kind", () => {
    expect(filterOps("place")).toEqual(["contains", "notContains", "isSet", "isEmpty"]);
    expect(filterOps("created_time")).toEqual(["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet"]);
    expect(filterOps("last_edited_time")).toEqual(["equals", "before", "after", "onOrBefore", "onOrAfter", "between", "within", "isSet"]);
    expect(filterOps("created_by")).toEqual(["equals", "isSet"]);
    expect(filterOps("last_edited_by")).toEqual(["equals", "isSet"]);
    expect(filterOps("relation")).toEqual(["relatesTo", "notRelatesTo", "isSet", "isEmpty"]);
    expect(filterOps("rollup")).toEqual(["isSet", "isEmpty"]);
    expect(filterOps("rollup", rollupDef("x", "sum"))).toEqual(["equals", "notEquals", "greater", "less", "atLeast", "atMost", "isSet", "isEmpty"]);
    expect(filterOps("rollup", rollupDef("x", "earliest"))).toEqual(["isSet", "isEmpty"]);
    expect(newFilter(defs[5] ?? rollupDef("x", "sum"))).toEqual({ propertyId: "rel", op: "relatesTo" });
  });

  it("place: contains (the name, ASCII case-insensitive), is set, is empty", () => {
    const cafe = page("cafe", { place: { kind: "place", value: { name: "Café de Flore", lat: 1, lon: 2 } } });
    const none = page("none");
    expect(pass(cafe, { propertyId: "place", op: "contains", value: "FLORE" })).toBe(true);
    expect(pass(cafe, { propertyId: "place", op: "contains", value: "zoo" })).toBe(false);
    expect(pass(none, { propertyId: "place", op: "contains", value: "zoo" })).toBe(false);
    expect(pass(cafe, { propertyId: "place", op: "contains" })).toBeNull();
    expect(pass(cafe, { propertyId: "place", op: "isSet" })).toBe(true);
    expect(pass(none, { propertyId: "place", op: "isEmpty" })).toBe(true);
  });

  it("times: before, after and between are about the person's local days, inclusive of the whole day", () => {
    const noon = page("noon", { made: { kind: "created_time", value: at(2026, 10, 8) } });
    const lateNight = page("late", { made: { kind: "created_time", value: at(2026, 10, 8, 23) } });
    const none = page("none");
    expect(pass(noon, { propertyId: "made", op: "before", value: "2026-10-09" })).toBe(true);
    expect(pass(noon, { propertyId: "made", op: "before", value: "2026-10-08" })).toBe(false);
    expect(pass(noon, { propertyId: "made", op: "after", value: "2026-10-07" })).toBe(true);
    // After a day means a later day: the same day, whatever its time, is not after it.
    expect(pass(lateNight, { propertyId: "made", op: "after", value: "2026-10-08" })).toBe(false);
    expect(pass(lateNight, { propertyId: "made", op: "between", value: { from: "2026-10-08", to: "2026-10-08" } })).toBe(true);
    expect(pass(noon, { propertyId: "made", op: "between", value: { from: "2026-10-09" } })).toBe(false);
    expect(pass(noon, { propertyId: "made", op: "between", value: { to: "2026-10-07" } })).toBe(false);
    expect(pass(noon, { propertyId: "made", op: "between", value: {} })).toBeNull();
    expect(pass(noon, { propertyId: "made", op: "before", value: "tomorrow" })).toBeNull();
    expect(pass(none, { propertyId: "made", op: "before", value: "2026-10-09" })).toBe(false);
    expect(pass(noon, { propertyId: "made", op: "isSet" })).toBe(true);
    expect(pass(none, { propertyId: "made", op: "isSet" })).toBe(false);
    expect(pass(noon, { propertyId: "made", op: "isEmpty" })).toBeNull(); // not an operator of the kind
    const edited = page("edited", { edit: { kind: "last_edited_time", value: at(2026, 1, 2) } });
    expect(pass(edited, { propertyId: "edit", op: "between", value: { from: "2026-01-01", to: "2026-01-31" } })).toBe(true);
  });

  it("by a device: is that device, is set", () => {
    const mine = page("mine", { by: { kind: "created_by", value: "dev-1" }, editBy: { kind: "last_edited_by", value: "dev-2" } });
    expect(pass(mine, { propertyId: "by", op: "equals", value: "dev-1" })).toBe(true);
    expect(pass(mine, { propertyId: "by", op: "equals", value: "dev-2" })).toBe(false);
    expect(pass(mine, { propertyId: "editBy", op: "equals", value: "dev-2" })).toBe(true);
    expect(pass(mine, { propertyId: "by", op: "equals" })).toBeNull();
    expect(pass(page("none"), { propertyId: "by", op: "isSet" })).toBe(false);
    expect(pass(mine, { propertyId: "by", op: "isSet" })).toBe(true);
  });

  it("relation: links to a page, is set, is empty", () => {
    const linked = page("linked", { rel: { kind: "relation", value: ["p1", "p2"] } });
    expect(pass(linked, { propertyId: "rel", op: "relatesTo", value: "p2" })).toBe(true);
    expect(pass(linked, { propertyId: "rel", op: "relatesTo", value: "p3" })).toBe(false);
    expect(pass(page("none"), { propertyId: "rel", op: "relatesTo", value: "p1" })).toBe(false);
    expect(pass(linked, { propertyId: "rel", op: "relatesTo" })).toBeNull();
    expect(pass(linked, { propertyId: "rel", op: "isSet" })).toBe(true);
    expect(pass(page("none"), { propertyId: "rel", op: "isEmpty" })).toBe(true);
    expect(pass(page("empty", { rel: { kind: "relation", value: [] } }), { propertyId: "rel", op: "isEmpty" })).toBe(true);
  });

  it("rollup: is set / is empty, and a number result is compared with 'is'", () => {
    const seven = page("seven", {
      sum: { kind: "rollup", value: { function: "sum", result: { type: "number", data: 7.5 } } },
      late: { kind: "rollup", value: { function: "latest", result: { type: "date", data: { date: "2026-10-01" } } } },
    });
    expect(pass(seven, { propertyId: "sum", op: "isSet" })).toBe(true);
    expect(pass(page("none"), { propertyId: "sum", op: "isEmpty" })).toBe(true);
    expect(pass(seven, { propertyId: "sum", op: "equals", value: 7.5 })).toBe(true);
    expect(pass(seven, { propertyId: "sum", op: "equals", value: 7 })).toBe(false);
    expect(pass(page("none"), { propertyId: "sum", op: "equals", value: 7 })).toBe(false);
    expect(pass(seven, { propertyId: "sum", op: "equals" })).toBeNull();
    // A rollup that gives a day has no 'is'.
    expect(pass(seven, { propertyId: "late", op: "equals", value: 1 })).toBeNull();
    expect(pass(seven, { propertyId: "late", op: "isSet" })).toBe(true);
  });
});

describe("the new kinds in a table", () => {
  async function openKinds(attrs: { [key: string]: JsonValue } = {}) {
    const app = await renderPages([kindsSpace(kindsHub(attrs))], { now: ticking() });
    const editor = await openPage(app.user, "Hub");
    await screen.findAllByRole("group", { name: t("pageList.label") });
    const client = new CoreClient(app.memory);
    const ids = await idsOf(app.memory);
    return { ...app, editor, client, ids };
  }

  it("shows every kind in its cell, the relation as chips and the rollup as its result", async () => {
    await openKinds();
    expect(headers()).toEqual([t("pageList.sort.title"), "Where", "Created", "Edited", "Made by", "Edited by", "Tasks", "Total", "Names"]);
    expect(cell("Cafe", "Where")).toHaveTextContent("Café de Flore");
    expect(cell("Cafe", "Created").textContent).not.toBe("");
    expect(cell("Zoo", "Tasks").querySelectorAll(".relation-chip-title")).toHaveLength(2);
    expect(cell("Zoo", "Total")).toHaveTextContent("7.5");
    expect(cell("Zoo", "Names").querySelectorAll(".rollup-item")).toHaveLength(2);
  });

  it("does not edit the computed kinds: no editor opens, nothing is written", async () => {
    const { user, editor } = await openKinds();
    const doc = docOf(editor);
    for (const column of ["Created", "Edited", "Made by", "Edited by", "Total", "Names"]) {
      await user.dblClick(cell("Cafe", column));
      expect(cell("Cafe", column)).not.toHaveAttribute("data-editing");
      expect(within(cell("Cafe", column)).queryByRole("textbox")).not.toBeInTheDocument();
    }
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
  });

  it("edits a place in a popover over the cell, writing through pagePropertySet", async () => {
    const { user, client, ids } = await openKinds();
    await user.click(cell("Plain", "Where"));
    const dialog = await screen.findByRole("dialog", { name: "Where" });
    const name = within(dialog).getByRole("textbox", { name: "Where" });
    await user.type(name, "Lisbon{Enter}");
    await waitFor(async () => {
      expect(await stored(client, ids.page("Plain"), K.where)).toEqual({ kind: "place", value: { name: "Lisbon" } });
    });
  });

  it("edits a relation in the cell with the page picker", async () => {
    const { user, client, ids } = await openKinds();
    await user.click(cell("Plain", "Tasks"));
    await user.click(within(cell("Plain", "Tasks")).getByRole("button", { name: t("properties.relation.add") }));
    await user.type(await screen.findByRole("combobox", { name: t("properties.relation.search") }), "Shi");
    await user.click(await screen.findByRole("option", { name: /Ship/u }));
    await waitFor(async () => {
      expect(await stored(client, ids.page("Plain"), K.tasks)).toEqual({ kind: "relation", value: [ids.page("Ship")] });
    });
    await waitFor(() => {
      expect(cell("Plain", "Tasks")).toHaveTextContent("Ship");
    });
  });

  it("shows the relation of a read-only editor without an add button", async () => {
    const { user, editor } = await openKinds();
    act(() => {
      editor.setEditable(false);
    });
    await user.click(cell("Plain", "Tasks"));
    expect(within(cell("Plain", "Tasks")).queryByRole("button", { name: t("properties.relation.add") })).not.toBeInTheDocument();
  });

  it("sorts from the column menu on a relation, a place and a rollup", async () => {
    const { editor } = await openKinds({ sort: { by: K.where, dir: "asc" } });
    expect(titles().slice(0, 3)).toEqual(["Write", "Zoo", "Cafe"]);
    expect(attrsOf(editor).sort).toEqual({ by: K.where, dir: "asc" });
  });

  it("filters by a place", async () => {
    await openKinds({ filters: [{ propertyId: K.where, op: "contains", value: "zoo" }] });
    expect(titles()).toEqual(["Zoo"]);
  });

  it("filters by a rollup that is a number", async () => {
    await openKinds({ filters: [{ propertyId: K.total, op: "equals", value: 3 }] });
    expect(titles()).toEqual(["Cafe"]);
  });

  it("filters by an empty relation", async () => {
    await openKinds({ filters: [{ propertyId: K.tasks, op: "isEmpty" }] });
    expect(titles()).toEqual(["Plain", "Ship", "Write"]);
  });

  it("edits the filters of the new kinds: the conditions of each, the device picker and the page picker", async () => {
    const { user, editor, ids } = await openKinds({ filters: [{ propertyId: K.tasks, op: "isSet" }] });
    expect(titles()).toEqual(["Cafe", "Zoo"]);

    // The editor lists the conditions of each kind.
    await user.click(within(toolbar()).getByRole("button", { name: /^Filter/u }));
    const panel = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    const row = within(panel).getByRole("group", { name: t("pageList.filter.row", { n: 1 }) });
    const ops = (): string[] =>
      Array.from(within(row).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.filter.op") }).options).map((option) => option.textContent ?? "");
    expect(ops()).toEqual([t("pageList.op.relatesTo"), t("pageList.filter.op.notRelatesTo"), t("pageList.op.isSet"), t("pageList.op.isEmpty")]);
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Total");
    expect(ops()).toEqual([
      t("pageList.op.equals"),
      t("formula.op.notEquals"),
      t("formula.op.greater"),
      t("formula.op.less"),
      t("formula.op.atLeast"),
      t("formula.op.atMost"),
      t("pageList.op.isSet"),
      t("pageList.op.isEmpty"),
    ]);
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Names");
    expect(ops()).toEqual([t("pageList.op.isSet"), t("pageList.op.isEmpty")]);
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Created");
    expect(ops()).toEqual([
      t("pageList.op.equals"),
      t("pageList.op.before"),
      t("pageList.op.after"),
      t("pageList.filter.op.onOrBefore"),
      t("pageList.filter.op.onOrAfter"),
      t("pageList.op.between"),
      t("pageList.filter.op.within"),
      t("pageList.op.isSet"),
    ]);
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Made by");
    expect(ops()).toEqual([t("pageList.op.equals"), t("pageList.op.isSet")]);
    // The device picker lists this device by its name.
    const device = within(row).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.filter.device") });
    expect(device.options.length).toBeGreaterThan(1);
    await user.selectOptions(device, device.options[1]?.value ?? "");
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: K.madeBy, op: "equals", value: device.options[1]?.value }]);
    });
    expect(titles()).toHaveLength(5);

    // The page picker of a relation filter.
    await user.selectOptions(within(row).getByRole("combobox", { name: t("pageList.filter.property") }), "Tasks");
    await user.click(within(row).getByRole("button", { name: t("pageList.filter.page") }));
    await user.type(await screen.findByRole("combobox", { name: t("properties.relation.search") }), "Wri");
    await user.click(await screen.findByRole("option", { name: /Write/u }));
    await waitFor(() => {
      expect(attrsOf(editor).filters).toEqual([{ propertyId: K.tasks, op: "relatesTo", value: ids.page("Write") }]);
    });
    expect(titles()).toEqual(["Cafe", "Zoo"]);
  });
});
