// The list view block (ADR-0016, amendment of 2026-10-07): the schema against the regenerated
// fixture, its attributes in HTML, the pure rules (reading the attributes, which pages, sorting by
// every kind of value), `/list view`, the rows of a children source and of a tag source, the property
// columns, the empty states, "New page", the toolbar (one attribute per change, the block keeps its
// id), the refresh when the tree or the schema changes, read-only editors, the block itself, and that
// the view writes nothing to the document.

import { CoreClient, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, PropertyDef, SeedSpace } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import { NodeSelection, TextSelection } from "@tiptap/pm/state";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { formatDateMention } from "./dates";
import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { openPanel, openSelect, viewSelect, viewToolbar } from "../test/page-list";
import type { User } from "../test/render";
import { K, kindsHub, kindsSpace } from "../test/view-kinds";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { duplicateBlock, turnInto, turnIntoKinds } from "./blocks";
import { loroSyncPluginKey } from "./loro-binding";
import {
  childPages,
  choiceSource,
  genericText,
  isKnownSort,
  pagesIn,
  readProperties,
  readSort,
  readSource,
  sortPages,
  sourceChoice,
} from "./page-list-model";
import type { PageListSort } from "./page-list-model";
import { PageListValue, oneLine, MAX_CELL_CHARS } from "./PageListValue";
import { formatInstant } from "../property-format";
import { createEditorSchema, createSchemaExtensions, ID_BEARING_NODES } from "./schema";
import { filterSlashCommands, SLASH_COMMANDS } from "./slash-commands";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// ----- the space ---------------------------------------------------------------------------

/** Ids the documents name before the space exists (the seed uses the ones it is given). */
const PROP = {
  status: "de000000-0000-4000-8000-00000000c001",
  due: "de000000-0000-4000-8000-00000000c002",
  done: "de000000-0000-4000-8000-00000000c003",
  priority: "de000000-0000-4000-8000-00000000c004",
  summary: "de000000-0000-4000-8000-00000000c005",
  stage: "de000000-0000-4000-8000-00000000c006",
  areas: "de000000-0000-4000-8000-00000000c007",
};
const TAG = {
  idea: "de000000-0000-4000-8000-00000000d001",
  project: "de000000-0000-4000-8000-00000000d002",
};

/** Every property of the space, Tags first (what a block shows when it is set to show them all). */
const ALL_PROPERTIES = [TAGS_PROPERTY_ID, PROP.status, PROP.due, PROP.done, PROP.priority, PROP.summary, PROP.stage, PROP.areas];

const ALPHA_ICON = "\u{1F170}️";
const LONG_TEXT = "The quick brown fox jumps over the lazy dog, again and again, until the text no longer fits on one line.";

/** The default configuration of a list view as the editor writes it. */
const DEFAULTS = {
  source: { kind: "children" },
  layout: "list",
  properties: [] as string[],
  sort: { by: "title", dir: "asc" },
};

/** The same as the editor holds it: `filters`, `columnWidths` and the attributes after them are absent from the document until a control sets them. */
const EDITOR_DEFAULTS = { ...DEFAULTS, filters: null, columnWidths: null, groupBy: null, hiddenGroups: null, calculations: null, views: null };

/**
 * A view that opens a page as a page: the side peek is the default (ADR-0054) and `openIn` is a part of `views`, which the older
 * attributes cannot say. For the tests that follow a click on a row to the page it opens.
 */
function asPages(extra: { [key: string]: JsonValue } = {}): { [key: string]: JsonValue } {
  return { views: [{ id: "v1", name: "", layout: "list", properties: [], sort: { by: "title", dir: "asc" }, openIn: "page", ...extra }] };
}

function paragraph(n: number, value: string): JsonValue {
  return value === ""
    ? { type: "paragraph", attrs: { id: pid(n) } }
    : { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

/** An intro, the list view (id `pid(2)`) and an empty paragraph; `list` overrides its attributes. */
function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [paragraph(1, "Intro"), { type: "pageList", attrs: { id: pid(2), ...DEFAULTS, ...list } }, paragraph(3, "")],
  };
}

/**
 * Personal: Hub (holds the list view) with the children Alpha, beta, Gamma and delta in this order
 * (the title order is Alpha, beta, delta, Gamma); Notes (an empty page for the `/` menu); Elsewhere,
 * outside Hub (with `elsewhere` as its content). Tags idea (Alpha, delta, Elsewhere) and project
 * (beta, delta).
 */
function space(content: JsonValue = hubContent(), elsewhere?: JsonValue): SeedSpace {
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
    ],
    pages: [
      {
        title: "Hub",
        icon: "🗂️",
        content,
        children: [
          {
            title: "Alpha",
            icon: ALPHA_ICON,
            tags: ["idea"],
            properties: {
              Status: "In progress",
              Due: { date: "2026-10-15" },
              Done: true,
              Priority: 3,
              Summary: "A short note",
              Stage: "Draft",
              Areas: ["Home", "Work"],
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
              Summary: LONG_TEXT,
              Stage: "Review",
            },
          },
          { title: "Gamma" },
          {
            title: "delta",
            tags: ["idea", "project"],
            properties: { Due: { date: "2026-10-04" }, Priority: 2, Areas: ["Work"], Summary: "zzz" },
          },
        ],
      },
      { title: "Notes", content: { type: "doc", content: [paragraph(10, "Intro"), paragraph(11, "")] } },
      { title: "Elsewhere", tags: ["idea"], properties: { Priority: 1 }, ...(elsewhere === undefined ? {} : { content: elsewhere }) },
    ],
  };
}

/** Today is 3 October 2026 for the test that calls this (only the clock is faked: timers stay real). */
function fakeToday(): void {
  vi.useFakeTimers({ now: new Date(2026, 9, 3, 12, 0, 0), toFake: ["Date"] });
}

/** A clock that moves on a second at every reading, so that pages are created and changed one after another. */
function ticking(): () => number {
  let now = Date.UTC(2026, 9, 3, 12, 0, 0);
  return () => {
    now += 1000;
    return now;
  };
}

/** Opens Hub (the page that holds `content`), or `title` when given: `Notes`, or `Elsewhere` with `elsewhere` as its content. */
async function openHub(content: JsonValue = hubContent(), title = "Hub", elsewhere?: JsonValue) {
  const app = await renderPages([space(content, elsewhere)], { now: ticking() });
  const editor = await openPage(app.user, title);
  if (title !== "Notes") {
    await screen.findAllByRole("group", { name: t("pageList.label") });
  }
  const client = new CoreClient(app.memory);
  const ids = await idsOf(app.memory);
  return { ...app, editor, client, ids, spaceId: ids.space("Personal") };
}

// ----- looking at the view -----------------------------------------------------------------

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

/** The titles of the rows, in order. */
function titles(): string[] {
  return Array.from(block().querySelectorAll(".page-list-row .page-list-name")).map((name) => name.textContent ?? "");
}

function rowOf(title: string): HTMLElement {
  const row = within(block()).getByRole("button", { name: title }).closest("li");
  if (row === null) {
    throw new Error(`no row for ${title}`);
  }
  return row;
}

/** The cell of a property in the row of a page, `null` when the page has no value for it. */
function cellOf(title: string, property: string): HTMLElement | null {
  return within(rowOf(title)).queryByRole("group", { name: property });
}

function toolbar(): HTMLElement {
  return viewToolbar(block());
}

/** The select `label` of the toolbar's popovers (Sort by, Sort order: Sort; the source: view settings); opens the one that holds it. */
function select(user: User, label: string): Promise<HTMLSelectElement> {
  return viewSelect(user, block(), label);
}

function propertiesButton(): HTMLElement {
  return within(toolbar()).getByRole("button", { name: /^Properties/u });
}

function emptyText(): string {
  return block().querySelector(".page-list-empty")?.textContent ?? "";
}

/** The attributes of the `n`-th list view of the editor's document. */
function attrsOf(editor: Editor, n = 0): Record<string, unknown> {
  let seen = -1;
  let found: Record<string, unknown> = {};
  editor.state.doc.descendants((node) => {
    if (node.type.name === "pageList") {
      seen += 1;
      if (seen === n) {
        found = node.attrs;
      }
    }
    return true;
  });
  return found;
}

function posOf(editor: Editor, n = 0): number {
  let seen = -1;
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === "pageList") {
      seen += 1;
      if (seen === n) {
        found = pos;
      }
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

/** Every change `doc` recorded since `since`, as the ops of each change. */
function changesSince(doc: LoroDoc, since: ReturnType<LoroDoc["oplogVersion"]>) {
  return doc.exportJsonUpdates(since).changes.map((change) => change.ops.map((op) => op.content));
}

/** The title of the open page (its icon is not part of it). */
function pageTitle(): string {
  return screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent ?? "";
}

function change(run: () => void): void {
  act(() => {
    run();
  });
}

// ----- the schema --------------------------------------------------------------------------

describe("the schema", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  it("has the list view of the fixture: a leaf atom block with an id and four attributes", () => {
    expect(live.nodes.pageList).toEqual({
      content: "",
      group: "block",
      inline: false,
      atom: true,
      attrs: ["id", "source", "layout", "properties", "sort", "filters", "columnWidths", "groupBy", "hiddenGroups", "calculations", "views"],
    });
    expect(fixture.nodes.pageList).toEqual(live.nodes.pageList);
    expect(fixture.idBearing).toContain("pageList");
    expect(ID_BEARING_NODES).toContain("pageList");
    expect(Object.keys(fixture.nodes)).toEqual(Object.keys(live.nodes));
  });

  it("is a selectable leaf, and gets the defaults when it is made without attributes", () => {
    const type = createEditorSchema().nodes.pageList;
    expect(type).toBeDefined();
    const node = type?.create({ id: pid(9) });
    expect(node?.isLeaf).toBe(true);
    expect(node?.isAtom).toBe(true);
    expect(type?.spec.selectable).toBe(true);
    expect(node?.attrs).toEqual({ id: pid(9), ...EDITOR_DEFAULTS });
    expect(type?.spec.parseDOM).toMatchObject([{ tag: 'div[data-type="pageList"]' }]);
  });
});

describe("the HTML form", () => {
  function mount(content: JsonValue): Editor {
    const element = document.createElement("div");
    document.body.append(element);
    return new Editor({ element, extensions: createSchemaExtensions(), content: content as never });
  }

  it("is <div data-type=pageList> with the structured attributes as JSON text, and is read back from it", () => {
    const attrs = {
      id: pid(2),
      source: { kind: "tag", optionId: TAG.idea },
      layout: "list",
      properties: [PROP.status, PROP.due],
      sort: { by: PROP.due, dir: "desc" },
    };
    const editor = mount({ type: "doc", content: [{ type: "pageList", attrs }] });
    const holder = document.createElement("div");
    holder.innerHTML = editor.getHTML();
    const element = holder.querySelector('[data-type="pageList"]');
    expect(element).not.toBeNull();
    expect(JSON.parse(element?.getAttribute("data-source") ?? "null")).toEqual(attrs.source);
    expect(element?.getAttribute("data-layout")).toBe("list");
    expect(JSON.parse(element?.getAttribute("data-properties") ?? "null")).toEqual(attrs.properties);
    expect(JSON.parse(element?.getAttribute("data-sort") ?? "null")).toEqual(attrs.sort);
    expect(editor.getText()).toBe(""); // nothing of it is text

    // A copy keeps the settings (the block's id is made again by the copy).
    const copy = mount({ type: "doc", content: [{ type: "paragraph" }] });
    copy.commands.setContent(editor.getHTML());
    expect(copy.state.doc.child(0).type.name).toBe("pageList");
    expect(copy.state.doc.child(0).attrs).toMatchObject({
      source: attrs.source,
      layout: "list",
      properties: attrs.properties,
      sort: attrs.sort,
    });
    editor.destroy();
    copy.destroy();
  });

  it("takes the defaults for attributes that are missing or are not JSON, and keeps a source it does not know", () => {
    const editor = mount({ type: "doc", content: [{ type: "paragraph" }] });
    editor.commands.setContent(
      '<div data-type="pageList"></div>' +
        '<div data-type="pageList" data-source="not json" data-properties="{}" data-sort="[1]"></div>' +
        '<div data-type="pageList" data-source=\'{"kind":"query","text":"x"}\' data-layout="timeline"></div>',
    );
    const lists: Record<string, unknown>[] = [];
    editor.state.doc.forEach((node) => {
      if (node.type.name === "pageList") {
        lists.push(node.attrs);
      }
    });
    expect(lists).toHaveLength(3);
    for (const attrs of lists.slice(0, 2)) {
      expect(attrs).toMatchObject(DEFAULTS);
    }
    expect(lists[2]).toMatchObject({ source: { kind: "query", text: "x" }, layout: "timeline" });
    editor.destroy();
  });
});

// ----- the rules ---------------------------------------------------------------------------

describe("reading the attributes", () => {
  it("reads a source: children, a tag with an option, nothing else", () => {
    expect(readSource({ kind: "children" })).toEqual({ kind: "children" });
    expect(readSource({ kind: "tag", optionId: "o1" })).toEqual({ kind: "tag", optionId: "o1" });
    for (const odd of [null, undefined, "children", [], {}, { kind: "tag" }, { kind: "tag", optionId: "" }, { kind: "tag", optionId: 3 }, { kind: "query" }]) {
      expect(readSource(odd), JSON.stringify(odd)).toBeNull();
    }
  });

  it("reads the properties as a list of ids without repeats; anything else is none", () => {
    expect(readProperties(["a", "b", "a", "", 3, null, "c"])).toEqual(["a", "b", "c"]);
    expect(readProperties([])).toEqual([]);
    for (const odd of ["a", {}, null, undefined, 3]) {
      expect(readProperties(odd), JSON.stringify(odd)).toEqual([]);
    }
  });

  it("reads a sort, a part that is not usable becoming its default", () => {
    expect(readSort({ by: "updated", dir: "desc" })).toEqual({ by: "updated", dir: "desc" });
    expect(readSort({ by: "p1" })).toEqual({ by: "p1", dir: "asc" });
    expect(readSort({ by: "", dir: "sideways" })).toEqual({ by: "title", dir: "asc" });
    for (const odd of [null, undefined, "title", [], 3]) {
      expect(readSort(odd), JSON.stringify(odd)).toEqual({ by: "title", dir: "asc" });
    }
  });

  it("turns a source into a select value and back", () => {
    expect(sourceChoice({ kind: "children" })).toBe("children");
    expect(sourceChoice({ kind: "tag", optionId: "o1" })).toBe("tag:o1");
    expect(choiceSource("children")).toEqual({ kind: "children" });
    expect(choiceSource("tag:o1")).toEqual({ kind: "tag", optionId: "o1" });
    expect(choiceSource("tag:")).toBeNull();
    expect(choiceSource("")).toBeNull();
  });
});

describe("which pages and in which order", () => {
  const info = (id: string, title: string, extra: Partial<PageInfo> = {}): PageInfo => ({
    id,
    spaceId: "s",
    title,
    createdAt: 0,
    updatedAt: 0,
    ...extra,
  });
  const tree = {
    spaceId: "s",
    roots: [
      {
        page: info("hub", "Hub"),
        children: [
          { page: info("c", "C"), children: [{ page: info("c1", "C1"), children: [] }] },
          { page: info("a", "A"), children: [] },
          { page: info("b", "B"), children: [] },
        ],
      },
      { page: info("z", "Z"), children: [] },
    ],
    trashed: [],
    undo: { canUndo: false, canRedo: false },
  };

  it("lists the children of the page in tree order, and none while the tree or the page is missing", () => {
    expect(childPages(tree, "hub").map((page) => page.id)).toEqual(["c", "a", "b"]);
    expect(childPages(tree, "c").map((page) => page.id)).toEqual(["c1"]);
    expect(childPages(tree, "z")).toEqual([]);
    expect(childPages(tree, "missing")).toEqual([]);
    expect(childPages(tree, null)).toEqual([]);
    expect(childPages(null, "hub")).toEqual([]);
  });

  it("lists the pages of a set in tree order, depth first, wherever they are", () => {
    expect(pagesIn(tree, new Set(["z", "b", "c1", "nope"])).map((page) => page.id)).toEqual(["c1", "b", "z"]);
    expect(pagesIn(tree, new Set())).toEqual([]);
    expect(pagesIn(null, new Set(["z"]))).toEqual([]);
  });

  const def = (id: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({
    id,
    name: id,
    kind,
    options,
    createdAt: 0,
  });
  const withValue = (title: string, property: string, value: unknown, extra: Partial<PageInfo> = {}): PageInfo =>
    info(title, title, { properties: [[property, value as never]], ...extra });
  const order = (pages: PageInfo[], sort: PageListSort, schema: PropertyDef[] = []): string[] =>
    sortPages(pages, [sort], schema).map((page) => page.title);

  it("sorts by title, ASCII case-insensitive, then exact, and ties keep their order", () => {
    const pages = ["Zebra", "apple", "Éclair", "banana", "Apple", "Banana"].map((title) => info(title, title));
    expect(order(pages, { by: "title", dir: "asc" })).toEqual(["Apple", "apple", "Banana", "banana", "Zebra", "Éclair"]);
    expect(order(pages, { by: "title", dir: "desc" })).toEqual(["Éclair", "Zebra", "banana", "Banana", "apple", "Apple"]);
    // Pages with the same title stay as they came.
    const same = [info("1", "x"), info("2", "x"), info("3", "a")];
    expect(sortPages(same, [{ by: "title", dir: "asc" }], []).map((page) => page.id)).toEqual(["3", "1", "2"]);
    expect(sortPages(same, [{ by: "title", dir: "desc" }], []).map((page) => page.id)).toEqual(["1", "2", "3"]);
    // An untitled page sorts by its empty title: first.
    expect(order([info("a", "b"), info("u", "")], { by: "title", dir: "asc" })).toEqual(["", "b"]);
  });

  it("sorts by the time of the last change or of the creation", () => {
    const pages = [
      info("a", "a", { createdAt: 30, updatedAt: 10 }),
      info("b", "b", { createdAt: 10, updatedAt: 30 }),
      info("c", "c", { createdAt: 20, updatedAt: 20 }),
    ];
    expect(order(pages, { by: "updated", dir: "asc" })).toEqual(["a", "c", "b"]);
    expect(order(pages, { by: "updated", dir: "desc" })).toEqual(["b", "c", "a"]);
    expect(order(pages, { by: "created", dir: "asc" })).toEqual(["b", "c", "a"]);
    expect(order(pages, { by: "created", dir: "desc" })).toEqual(["a", "c", "b"]);
  });

  it("sorts numbers numerically, not as text", () => {
    const schema = [def("n", "number")];
    const pages = [10, 9, 100, -5, 2.5].map((value) => withValue(String(value), "n", { kind: "number", value }));
    expect(order(pages, { by: "n", dir: "asc" }, schema)).toEqual(["-5", "2.5", "9", "10", "100"]);
    expect(order(pages, { by: "n", dir: "desc" }, schema)).toEqual(["100", "10", "9", "2.5", "-5"]);
  });

  it("sorts dates by ISO, a day before the same day with a time, and times by the clock", () => {
    const schema = [def("d", "date")];
    const day = (title: string, date: string, time?: string): PageInfo =>
      withValue(title, "d", { kind: "date", value: time === undefined ? { date } : { date, time } });
    const pages = [
      day("c", "2026-10-09", "14:00"),
      day("a", "2026-10-09"),
      day("d", "2026-11-01"),
      day("b", "2026-10-09", "09:30"),
      day("e", "2025-12-31"),
    ];
    expect(order(pages, { by: "d", dir: "asc" }, schema)).toEqual(["e", "a", "b", "c", "d"]);
    expect(order(pages, { by: "d", dir: "desc" }, schema)).toEqual(["d", "c", "b", "a", "e"]);
  });

  it("sorts text, url, email and phone by the text, ignoring ASCII case", () => {
    const schema = [def("t", "text")];
    const pages = ["banana", "Apple", "cherry", "apple pie"].map((value) => withValue(value, "t", { kind: "text", value }));
    expect(order(pages, { by: "t", dir: "asc" }, schema)).toEqual(["Apple", "apple pie", "banana", "cherry"]);
    const urls = [def("u", "url")];
    const links = ["https://b.example", "https://a.example"].map((value) => withValue(value, "u", { kind: "url", value }));
    expect(order(links, { by: "u", dir: "asc" }, urls)).toEqual(["https://a.example", "https://b.example"]);
  });

  it("sorts options by their name (not their id or their place in the list), and a list of options by its names", () => {
    const options = [
      { id: "o1", name: "Review", color: "orange" as const },
      { id: "o2", name: "Draft", color: "gray" as const },
      { id: "o3", name: "archive", color: "red" as const },
    ];
    const select = [def("s", "select", options)];
    const pages = ["o1", "o2", "o3"].map((value) => withValue(value, "s", { kind: "select", value }));
    expect(order(pages, { by: "s", dir: "asc" }, select)).toEqual(["o3", "o2", "o1"]);
    expect(order(pages, { by: "s", dir: "desc" }, select)).toEqual(["o1", "o2", "o3"]);
    // An option the property no longer lists counts as no value: last.
    expect(order([withValue("gone", "s", { kind: "select", value: "o9" }), ...pages], { by: "s", dir: "asc" }, select).at(-1)).toBe("gone");

    const many = [def("m", "multi_select", options)];
    const lists = [
      withValue("two", "m", { kind: "multi_select", value: ["o1", "o2"] }),
      withValue("one", "m", { kind: "multi_select", value: ["o3"] }),
      withValue("none", "m", { kind: "multi_select", value: [] }),
    ];
    expect(order(lists, { by: "m", dir: "asc" }, many)).toEqual(["one", "two", "none"]);
  });

  it("sorts a checkbox false before true", () => {
    const schema = [def("c", "checkbox")];
    const pages = [true, false, true, false].map((value, index) => withValue(`${String(value)}${String(index)}`, "c", { kind: "checkbox", value }));
    expect(order(pages, { by: "c", dir: "asc" }, schema)).toEqual(["false1", "false3", "true0", "true2"]);
    expect(order(pages, { by: "c", dir: "desc" }, schema)).toEqual(["true0", "true2", "false1", "false3"]);
  });

  it("puts the pages without a value last, whichever the direction, in their own order", () => {
    const schema = [def("n", "number")];
    const pages = [info("x", "x"), withValue("two", "n", { kind: "number", value: 2 }), info("y", "y"), withValue("one", "n", { kind: "number", value: 1 })];
    expect(order(pages, { by: "n", dir: "asc" }, schema)).toEqual(["one", "two", "x", "y"]);
    expect(order(pages, { by: "n", dir: "desc" }, schema)).toEqual(["two", "one", "x", "y"]);
    // An empty text is no value either.
    const texts = [def("t", "text")];
    expect(order([withValue("blank", "t", { kind: "text", value: "" }), withValue("a", "t", { kind: "text", value: "a" })], { by: "t", dir: "asc" }, texts)).toEqual(["a", "blank"]);
  });

  it("sorts values of a kind it does not know by their text, and a property the schema lacks like the title", () => {
    const schema = [def("f", "hologram" as never)];
    const files = (title: string, names: string[]): PageInfo =>
      withValue(title, "f", { kind: "hologram", value: names.map((name) => ({ name })) });
    expect(order([files("b", ["b.pdf"]), files("a", ["a.pdf", "z.pdf"])], { by: "f", dir: "asc" }, schema)).toEqual(["a", "b"]);
    const pages = [info("2", "b"), info("1", "a")];
    expect(order(pages, { by: "deleted-property", dir: "asc" }, schema)).toEqual(["a", "b"]);
    expect(isKnownSort("deleted-property", schema)).toBe(false);
    expect(isKnownSort("f", schema)).toBe(true);
    expect(isKnownSort("updated", [])).toBe(true);
  });
});

describe("the text of a value", () => {
  it("reads anything as text: strings, numbers, lists, objects with a value or a name", () => {
    expect(genericText("a")).toBe("a");
    expect(genericText(3)).toBe("3");
    expect(genericText(true)).toBe("true");
    expect(genericText(["a", 2, ""])).toBe("a, 2");
    expect(genericText({ value: ["x", "y"] })).toBe("x, y");
    expect(genericText([{ name: "a.pdf" }, { text: "task" }, { title: "T" }, { label: "L" }, { other: 1 }])).toBe("a.pdf, task, T, L");
    expect(genericText(null)).toBe("");
    expect(genericText({})).toBe("");
  });

  it("cuts a long text to one line with an ellipsis, white space flattened", () => {
    expect(oneLine("  a\n\n b\t c ")).toEqual({ shown: "a b c", cut: false });
    const cut = oneLine("x".repeat(200));
    expect(Array.from(cut.shown)).toHaveLength(MAX_CELL_CHARS);
    expect(cut.shown.endsWith("…")).toBe(true);
    expect(cut.cut).toBe(true);
    expect(oneLine("x".repeat(MAX_CELL_CHARS))).toEqual({ shown: "x".repeat(MAX_CELL_CHARS), cut: false });
  });

  it("shows a value of a kind it does not know as its text", () => {
    // A kind from a newer version (here an invented one): its value is shown as the text it holds.
    const def: PropertyDef = { id: "f", name: "Hologram", kind: "hologram" as never, options: [], createdAt: 0 };
    const view = render(<PageListValue def={def} value={{ kind: "hologram", value: [{ name: "a.pdf" }, { name: "b.pdf" }] } as never} today="2026-10-03" />);
    expect(view.container.textContent).toBe("a.pdf, b.pdf");
    view.unmount();
    const empty = render(<PageListValue def={def} value={{ kind: "hologram", value: [] } as never} today="2026-10-03" />);
    expect(empty.container.textContent).toBe("");
  });
});

// ----- the / menu --------------------------------------------------------------------------

describe("the / menu", () => {
  it("has the entry in the advanced section, found by its name and by list, view, database, pages and children", () => {
    const entry = SLASH_COMMANDS.find((command) => command.id === "pageList");
    expect(entry).toBeDefined();
    expect(entry === undefined ? null : t(entry.label)).toBe("List view");
    expect(entry?.section).toBe("advanced");
    expect(entry?.keywords).toEqual(["list", "view", "database", "pages", "children"]);
    for (const word of ["list view", "view", "list", "database", "pages", "children"]) {
      expect(filterSlashCommands(word).map((command) => command.id), word).toContain("pageList");
    }
    expect(filterSlashCommands("list view")[0]?.id).toBe("pageList");
  });

  it("inserts the block where /list view was typed, with an id, the defaults and a paragraph below it", async () => {
    const { user, memory, editor, ids } = await openHub(hubContent(), "Notes");
    editor.view.dispatch(
      editor.state.tr.setSelection(TextSelection.create(editor.state.doc, editor.state.doc.child(0).nodeSize + 1)),
    );
    editor.commands.insertContent("/list"); // a space would end the command: the label is found by its first word

    const menu = await screen.findByRole("listbox", { name: t("slash.label") });
    await waitFor(() => {
      expect(within(menu).getAllByRole("option").map((option) => option.querySelector(".suggestion-title")?.textContent)[0]).toBe("List view");
    });
    await user.click(within(menu).getByText("List view"));

    expect(editor.state.doc.childCount).toBe(3);
    const node = editor.state.doc.child(1);
    const blockId: unknown = node.attrs.id;
    expect(node.type.name).toBe("pageList");
    expect(blockId).toMatch(UUID);
    expect({ ...node.attrs, id: undefined }).toEqual({ ...EDITOR_DEFAULTS, id: undefined });
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
    expect(editor.state.selection.$from.parent).toBe(editor.state.doc.child(2));

    // It is drawn at once (Notes has no children), and what reaches the core is the block with its settings.
    expect(await screen.findByRole("group", { name: t("pageList.label") })).toBeInTheDocument();
    expect(emptyText()).toBe(t("pageList.empty.children"));
    await waitFor(async () => {
      expect(JSON.stringify(await contentOf(memory, ids.page("Notes")))).toContain('"pageList"');
    });
    const stored = (await contentOf(memory, ids.page("Notes"))) as { content: { type: string }[] };
    expect(stored.content.find((entry) => entry.type === "pageList")).toEqual({
      type: "pageList",
      attrs: { id: blockId, ...DEFAULTS },
    });
  });

  it.each([
    ["tableView", "table"],
    ["boardView", "board"],
  ])("/%s inserts the block with the layout %s and the other defaults", async (id, layout) => {
    const { editor } = await openHub(hubContent(), "Notes");
    const at = editor.state.doc.child(0).nodeSize + 1;
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
    const entry = SLASH_COMMANDS.find((command) => command.id === id);
    expect(entry?.keywords).toContain("database");
    expect(filterSlashCommands("database").map((command) => command.id)).toContain(id);

    act(() => {
      void entry?.run(editor, { from: at, to: at });
    });

    const node = editor.state.doc.child(1);
    expect(node.type.name).toBe("pageList");
    expect({ ...node.attrs, id: undefined }).toEqual({ ...EDITOR_DEFAULTS, layout, id: undefined });
    expect(editor.state.doc.child(2).type.name).toBe("paragraph");
  });
});

// ----- rows from the children --------------------------------------------------------------

describe("a children source", () => {
  it("lists the pages under the page that holds the block, sorted by title, not in tree order", async () => {
    await openHub();
    // The tree order is Alpha, beta, Gamma, delta; the title order, ASCII case-insensitive, is this one.
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(emptyText()).toBe("");
    // The heading says where the pages come from; nothing of the page's own other text is in it.
    expect(within(block()).getByText(t("pageList.source.children"), { selector: ".page-list-heading" })).toBeInTheDocument();
  });

  it("shows the page's icon (a glyph when it has none) and title, an untitled page as Untitled", async () => {
    const { client, ids } = await openHub();
    expect(rowOf("Alpha").querySelector(".page-list-icon")?.textContent).toBe(ALPHA_ICON);
    expect(rowOf("beta").querySelector(".page-list-icon svg")).not.toBeNull();
    await client.pageRename(ids.page("Gamma"), "");
    await waitFor(() => {
      expect(titles()).toContain(t("page.untitled"));
    });
  });

  it("opens the page when its title is clicked", async () => {
    const { user } = await openHub(hubContent(asPages()));
    await user.click(within(block()).getByRole("button", { name: "beta" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("beta");
    });
    expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
  });

  it("does not list the page itself, its grandchildren or pages elsewhere", async () => {
    const { client, ids, spaceId } = await openHub();
    await client.pageCreate({ spaceId, parentId: ids.page("Alpha"), title: "Grandchild" });
    // A page made after it, under Hub: once it is listed the tree has been read again.
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "Zed" });
    await waitFor(() => {
      expect(titles()).toContain("Zed");
    });
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma", "Zed"]);
    expect(titles()).not.toContain("Hub");
    expect(titles()).not.toContain("Elsewhere");
  });
});

// ----- rows from a tag ---------------------------------------------------------------------

describe("a tag source", () => {
  const tagged = (optionId: string, extra: { [key: string]: JsonValue } = {}): JsonValue =>
    hubContent({ source: { kind: "tag", optionId }, ...extra });

  it("lists the pages of the space that carry the tag, wherever they are, sorted, asked through pagesByProperty", async () => {
    const app = await renderPages([space(tagged(TAG.idea))], { now: ticking() });
    const dispatch = vi.spyOn(app.memory, "dispatch");
    await openPage(app.user, "Hub");
    await screen.findByRole("group", { name: t("pageList.label") });
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "delta", "Elsewhere"]);
    });
    const ids = await idsOf(app.memory);
    expect(dispatch).toHaveBeenCalledWith({
      type: "PagesByProperty",
      data: { spaceId: ids.space("Personal"), propertyId: TAGS_PROPERTY_ID, filter: { type: "ContainsOption", data: TAG.idea } },
    });
    // The heading names the tag.
    await waitFor(() => {
      expect(block().querySelector(".page-list-heading")?.textContent).toBe(t("pageList.source.tag", { name: "idea" }));
    });
  });

  it("follows the tags of the pages: a page that gets the tag joins the list, one that loses it leaves", async () => {
    const { client, ids } = await openHub(tagged(TAG.project));
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "delta"]);
    });
    await client.pagePropertySet(ids.page("Gamma"), TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.project] });
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "delta", "Gamma"]);
    });
    await client.pagePropertySet(ids.page("beta"), TAGS_PROPERTY_ID, null);
    await waitFor(() => {
      expect(titles()).toEqual(["delta", "Gamma"]);
    });
  });

  it("says No pages with this tag when none carries it (a tag the space does not have)", async () => {
    const { user } = await openHub(tagged("de000000-0000-4000-8000-00000000ffff"));
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.empty.tag"));
    });
    expect(emptyText()).toBe("No pages with this tag");
    expect(titles()).toEqual([]);
    expect(block().querySelector(".page-list-heading")?.textContent).toBe(t("pageList.source.tagGone"));
    expect((await select(user, t("pageList.source.label"))).selectedOptions[0]?.textContent).toBe(t("pageList.source.tagGone"));
    // New page is offered all the same: it makes a page that has the tag.
    expect(within(block()).getByRole("button", { name: t("pageList.newPage") })).toBeEnabled();
  });

  it("empties the list when the tag it shows is deleted from the space", async () => {
    const { client, spaceId } = await openHub(tagged(TAG.idea));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "delta", "Elsewhere"]);
    });
    await client.propertyOptionDelete(spaceId, TAGS_PROPERTY_ID, TAG.idea);
    await waitFor(() => {
      expect(emptyText()).toBe(t("pageList.empty.tag"));
    });
    expect(block().querySelector(".page-list-heading")?.textContent).toBe(t("pageList.source.tagGone"));
  });

  it("follows a rename of the tag in its heading", async () => {
    const { client, spaceId } = await openHub(tagged(TAG.idea));
    await waitFor(() => {
      expect(titles()).toHaveLength(3);
    });
    await client.propertyOptionUpdate(spaceId, TAGS_PROPERTY_ID, TAG.idea, { name: "notion" });
    await waitFor(() => {
      expect(block().querySelector(".page-list-heading")?.textContent).toBe(t("pageList.source.tag", { name: "notion" }));
    });
  });
});

// ----- the property columns ----------------------------------------------------------------

describe("the properties of a row", () => {
  it("shows the chosen properties, in the order of the list, and nothing when none is chosen", async () => {
    fakeToday();
    await openHub(hubContent({ properties: [PROP.priority, PROP.status] }));
    expect(within(rowOf("Alpha")).getAllByRole("group").map((cell) => cell.getAttribute("aria-label"))).toEqual(["Priority", "Status"]);
    // A page without a value has an empty, hidden cell, so that the columns stay aligned.
    expect(cellOf("Gamma", "Status")).toBeNull();
    expect(rowOf("Gamma").querySelectorAll(".page-list-cell")).toHaveLength(2);
    expect(rowOf("Gamma").querySelectorAll(".page-list-cell[data-empty]")).toHaveLength(2);
  });

  it("shows options as coloured pills, a status, a select, a list of options and the tags", async () => {
    fakeToday();
    await openHub(hubContent({ properties: ALL_PROPERTIES }));
    const pill = (title: string, property: string): HTMLElement[] =>
      Array.from(cellOf(title, property)?.querySelectorAll<HTMLElement>(".option-pill") ?? []);
    // Status: the colour of the option (In progress is blue, Done green).
    expect(pill("Alpha", "Status").map((element) => [element.textContent, element.dataset.color])).toEqual([["In progress", "blue"]]);
    expect(pill("beta", "Status").map((element) => [element.textContent, element.dataset.color])).toEqual([["Done", "green"]]);
    expect(pill("Alpha", "Stage").map((element) => [element.textContent, element.dataset.color])).toEqual([["Draft", "gray"]]);
    expect(pill("beta", "Stage").map((element) => [element.textContent, element.dataset.color])).toEqual([["Review", "orange"]]);
    // A list of options: one pill each, in the order of the value.
    expect(pill("Alpha", "Areas").map((element) => [element.textContent, element.dataset.color])).toEqual([
      ["Home", "pink"],
      ["Work", "purple"],
    ]);
    // Tags are options too.
    expect(pill("Alpha", "Tags").map((element) => [element.textContent, element.dataset.color])).toEqual([["idea", "green"]]);
    expect(pill("delta", "Tags").map((element) => element.textContent)).toEqual(["idea", "project"]);
    expect(cellOf("Gamma", "Tags")).toBeNull();
  });

  it("shows dates as the property panel writes them, a date with a time too, and Tomorrow", async () => {
    fakeToday();
    await openHub(hubContent({ properties: [PROP.due] }));
    expect(cellOf("Alpha", "Due")?.textContent).toBe(formatDateMention("2026-10-15", null, "2026-10-03"));
    expect(cellOf("beta", "Due")?.textContent).toBe(formatDateMention("2026-10-09", "09:30", "2026-10-03"));
    expect(cellOf("beta", "Due")?.textContent).toMatch(/9:30/u);
    expect(cellOf("delta", "Due")?.textContent).toBe(t("date.tomorrow"));
    expect(cellOf("Gamma", "Due")).toBeNull();
  });

  it("shows a checkbox as a box, checked or not, and nothing for a page without a value", async () => {
    fakeToday();
    await openHub(hubContent({ properties: [PROP.done] }));
    const box = (title: string): HTMLElement | undefined => {
      const cell = cellOf(title, "Done");
      return cell === null ? undefined : within(cell).getByRole("img");
    };
    expect(box("Alpha")).toHaveAccessibleName(t("pageList.checked"));
    expect(box("Alpha")).toHaveAttribute("data-checked", "true");
    expect(box("beta")).toHaveAccessibleName(t("pageList.unchecked"));
    expect(box("beta")).toHaveAttribute("data-checked", "false");
    expect(box("Gamma")).toBeUndefined();
  });

  it("shows numbers as the locale writes them, and text on one line, cut when it is long", async () => {
    fakeToday();
    await openHub(hubContent({ properties: [PROP.priority, PROP.summary] }));
    expect(cellOf("beta", "Priority")?.textContent).toBe("10");
    expect(cellOf("Alpha", "Priority")?.textContent).toBe("3");
    expect(cellOf("Alpha", "Summary")?.textContent).toBe("A short note");
    expect(cellOf("Alpha", "Summary")?.querySelector("[title]")).toBeNull(); // nothing cut: no tooltip
    const long = cellOf("beta", "Summary")?.querySelector<HTMLElement>(".page-list-text");
    expect(Array.from(long?.textContent ?? "")).toHaveLength(MAX_CELL_CHARS);
    expect(long?.textContent?.endsWith("…")).toBe(true);
    expect(long).toHaveAttribute("title", LONG_TEXT);
  });

  it("skips a property the space does not have (deleted), and shows it again if the schema has it", async () => {
    fakeToday();
    await openHub(hubContent({ properties: ["de000000-0000-4000-8000-00000000eeee", PROP.priority] }));
    expect(within(rowOf("Alpha")).getAllByRole("group").map((cell) => cell.getAttribute("aria-label"))).toEqual(["Priority"]);
  });

  it("follows a change of a value and of an option's colour", async () => {
    fakeToday();
    const { client, ids, spaceId } = await openHub(hubContent({ properties: [PROP.priority, PROP.status] }));
    await client.pagePropertySet(ids.page("Gamma"), PROP.priority, { kind: "number", value: 7 });
    await waitFor(() => {
      expect(cellOf("Gamma", "Priority")?.textContent).toBe("7");
    });
    const status = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.status);
    const option = status?.options.find((candidate) => candidate.name === "In progress");
    await client.propertyOptionUpdate(spaceId, PROP.status, option?.id ?? "", { color: "red" });
    await waitFor(() => {
      expect(cellOf("Alpha", "Status")?.querySelector<HTMLElement>(".option-pill")?.dataset.color).toBe("red");
    });
  });
});

// ----- sorting -----------------------------------------------------------------------------

describe("the sort of the block", () => {
  it("is read from the document: by a property, descending, missing values last", async () => {
    await openHub(hubContent({ sort: { by: PROP.priority, dir: "desc" } }));
    expect(titles()).toEqual(["beta", "Alpha", "delta", "Gamma"]);
  });

  it("sorts by each kind of property through the toolbar, and by title, last change and creation", async () => {
    const { user, client, ids } = await openHub(hubContent({ properties: ALL_PROPERTIES }));
    // The pages change one after another in this order (the tree and the creation order are Alpha, beta, Gamma, delta).
    for (const title of ["Gamma", "Alpha", "delta", "beta"]) {
      await client.pageSetIcon(ids.page(title), "\u{1F4CC}");
    }
    await waitFor(() => {
      expect(rowOf("beta").querySelector(".page-list-icon")?.textContent).toBe("\u{1F4CC}");
    });
    const sortBy = async (label: string): Promise<string[]> => {
      await user.selectOptions(await select(user, t("pageList.sort.label")), label);
      await waitFor(() => {
        expect(openSelect(t("pageList.sort.label")).selectedOptions[0]?.textContent).toBe(label);
      });
      return titles();
    };
    expect(await sortBy("Priority")).toEqual(["delta", "Alpha", "beta", "Gamma"]);
    expect(await sortBy("Due")).toEqual(["delta", "beta", "Alpha", "Gamma"]);
    expect(await sortBy("Done")).toEqual(["beta", "Alpha", "Gamma", "delta"]);
    expect(await sortBy("Status")).toEqual(["beta", "Alpha", "Gamma", "delta"]); // Done, In progress, then none
    expect(await sortBy("Stage")).toEqual(["Alpha", "beta", "Gamma", "delta"]); // Draft, Review
    expect(await sortBy("Summary")).toEqual(["Alpha", "beta", "delta", "Gamma"]); // "A short note", "The quick...", "zzz"
    expect(await sortBy("Areas")).toEqual(["Alpha", "delta", "beta", "Gamma"]); // "Home, Work", "Work", then none
    expect(await sortBy("Tags")).toEqual(["Alpha", "delta", "beta", "Gamma"]); // "idea", "idea, project", "project", then none
    expect(await sortBy(t("pageList.sort.created"))).toEqual(["Alpha", "beta", "Gamma", "delta"]);
    expect(await sortBy(t("pageList.sort.updated"))).toEqual(["Gamma", "Alpha", "delta", "beta"]);
    expect(await sortBy(t("pageList.sort.title"))).toEqual(["Alpha", "beta", "delta", "Gamma"]);
  });

  it("reverses with the direction, keeping the pages without a value last", async () => {
    const { user } = await openHub(hubContent({ sort: { by: PROP.priority, dir: "asc" } }));
    expect(titles()).toEqual(["delta", "Alpha", "beta", "Gamma"]);
    await user.selectOptions(await select(user, t("pageList.sort.dir")), t("pageList.sort.desc"));
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "Alpha", "delta", "Gamma"]);
    });
    await user.selectOptions(openSelect(t("pageList.sort.label")), t("pageList.sort.created"));
    await waitFor(() => {
      expect(titles()).toEqual(["delta", "Gamma", "beta", "Alpha"]);
    });
  });

  it("sorts by several keys from the Sort panel: a second key upgrades the block, the first key stays the legacy sort (ADR-0058)", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.stage] }));
    const panel = await openPanel(user, block(), "sort");
    const byOf = (n: number): HTMLSelectElement =>
      within(within(panel).getByRole("group", { name: t("pageList.sort.row", { n }) })).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.sort.label") });
    // One key: the block keeps writing its legacy `sort`, no `views`.
    await user.selectOptions(byOf(1), "Stage");
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "Gamma", "delta"]); // Draft, Review, then the two without a stage as they came
    });
    expect(attrsOf(editor).sort).toEqual({ by: PROP.stage, dir: "asc" });
    expect(attrsOf(editor).views ?? null).toBeNull();

    // A second key (the first one not used yet: the title) breaks the tie, and writes `views` with the mirror.
    await user.click(within(panel).getByRole("button", { name: t("pageList.sort.add") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    });
    const [view] = attrsOf(editor).views as Record<string, unknown>[];
    expect(view?.sorts).toEqual([
      { by: PROP.stage, dir: "asc" },
      { by: "title", dir: "asc" },
    ]);
    expect(view?.sort).toEqual({ by: PROP.stage, dir: "asc" });
    expect(attrsOf(editor).sort).toEqual({ by: PROP.stage, dir: "asc" });
    expect(within(panel).getAllByRole("group")).toHaveLength(2);

    // The title first: the mirror follows the first key.
    await user.selectOptions(within(within(panel).getByRole("group", { name: t("pageList.sort.row", { n: 2 }) })).getByRole("combobox", { name: t("pageList.sort.dir") }), t("pageList.sort.desc"));
    await user.click(within(within(panel).getByRole("group", { name: t("pageList.sort.row", { n: 2 }) })).getByRole("button", { name: t("pageList.sort.up") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    });
    expect(attrsOf(editor).sort).toEqual({ by: "title", dir: "desc" });

    // Taking the first key away leaves the other, with no `sorts`.
    await user.click(within(within(panel).getByRole("group", { name: t("pageList.sort.row", { n: 1 }) })).getByRole("button", { name: t("pageList.sort.remove") }));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "Gamma", "delta"]);
    });
    const [last] = attrsOf(editor).views as Record<string, unknown>[];
    expect(last).not.toHaveProperty("sorts");
    expect(last?.sort).toEqual({ by: PROP.stage, dir: "asc" });
    expect(attrsOf(editor).sort).toEqual({ by: PROP.stage, dir: "asc" });
  });

  it("sorts like the title when the property it names is gone, and says it is removed in its picker", async () => {
    const { user } = await openHub(hubContent({ sort: { by: "de000000-0000-4000-8000-00000000eeee", dir: "desc" } }));
    expect(titles()).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    expect((await select(user, t("pageList.sort.label"))).selectedOptions[0]?.textContent).toBe(t("pageList.sort.gone"));
  });
});

// ----- empty states ------------------------------------------------------------------------

describe("the empty states", () => {
  it("says No pages yet when the page has no children, still offering a new page", async () => {
    // The list view of Elsewhere, a page with no children.
    await openHub(hubContent(), "Elsewhere", hubContent());
    expect(titles()).toEqual([]);
    expect(emptyText()).toBe("No pages yet");
    expect(emptyText()).toBe(t("pageList.empty.children"));
    expect(within(block()).queryByRole("list")).not.toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("pageList.newPage") })).toBeEnabled();
  });

  it("gives way to the rows as soon as there is a page, and back when the last one is trashed", async () => {
    const { client, ids, spaceId } = await openHub(hubContent(), "Elsewhere", hubContent());
    expect(emptyText()).toBe("No pages yet");
    const first = await client.pageCreate({ spaceId, parentId: ids.page("Elsewhere"), title: "First" });
    await waitFor(() => {
      expect(titles()).toEqual(["First"]);
    });
    expect(emptyText()).toBe("");
    await client.pageTrash(first.id);
    await waitFor(() => {
      expect(emptyText()).toBe("No pages yet");
    });
    expect(titles()).toEqual([]);
  });
});

// ----- New page ----------------------------------------------------------------------------

describe("New page", () => {
  it("makes a page under the page that holds the block and opens it (a children source)", async () => {
    const { user, client, ids, spaceId } = await openHub(hubContent(asPages()));
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
    });
    const hub = (await client.treeGet(spaceId)).roots.find((node) => node.page.id === ids.page("Hub"));
    expect(hub?.children.map((node) => node.page.title)).toEqual(["Alpha", "beta", "Gamma", "delta", ""]);
    const created = hub?.children.at(-1)?.page;
    expect(created?.parentId).toBe(ids.page("Hub"));
    expect(pageTitle()).toBe(t("page.untitled"));
    // The new page is the open one: selected in the sidebar.
    expect(screen.getByRole("treeitem", { selected: true })).toBeInTheDocument();
  });

  it("makes a page that carries the tag of a tag source, under the page, and opens it", async () => {
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
    expect(hub?.children).toHaveLength(5);
    expect(created?.title).toBe("");
    const values = await client.pagePropertiesGet(created?.id ?? "");
    expect(values).toEqual([[TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.project] }]]);
    expect(pageTitle()).toBe(t("page.untitled"));
  });

  it("says why when the page cannot be made, and stays where it is", async () => {
    const { user, memory } = await openHub();
    const original = memory.dispatch.bind(memory);
    vi.spyOn(memory, "dispatch").mockImplementation((request) =>
      request.type === "PageCreate" ? Promise.reject(new Error("disk full")) : original(request),
    );
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    const alert = await within(block()).findByRole("alert");
    expect(alert).toHaveTextContent(t("pageList.error", { message: "disk full" }));
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(within(block()).getByRole("button", { name: t("pageList.newPage") })).toBeEnabled();
  });
});

// ----- the toolbar -------------------------------------------------------------------------

describe("the toolbar", () => {
  it("shows the settings of the block: the source, the number of properties and the sort", async () => {
    const { user } = await openHub(hubContent({ properties: [PROP.status, PROP.due], sort: { by: PROP.due, dir: "desc" } }));
    expect((await select(user, t("pageList.source.label"))).value).toBe("children");
    const sources = Array.from(openSelect(t("pageList.source.label")).options).map((option) => option.textContent);
    expect(sources).toEqual([
      t("pageList.source.children"),
      "Pages tagged: idea",
      "Pages tagged: project",
      t("pageList.source.pickPage"),
      t("pageList.source.space"),
    ]);
    expect(propertiesButton()).toHaveTextContent("Properties 2");
    expect((await select(user, t("pageList.sort.label"))).value).toBe(PROP.due);
    expect(openSelect(t("pageList.sort.dir")).value).toBe("desc");
    // The sort picker lists the built-in sorts and the properties the rows show, in the order they are shown.
    const sorts = Array.from(openSelect(t("pageList.sort.label")).options).map((option) => option.textContent);
    expect(sorts).toEqual(["Title", "Last edited", "Created", "Status", "Due"]);
  });

  it("lists in the sort picker the property the block sorts by even when the rows do not show it", async () => {
    const { user } = await openHub(hubContent({ properties: [PROP.status], sort: { by: PROP.priority, dir: "asc" } }));
    const sorts = Array.from((await select(user, t("pageList.sort.label"))).options).map((option) => option.textContent);
    expect(sorts).toEqual(["Title", "Last edited", "Created", "Status", "Priority"]);
    expect(openSelect(t("pageList.sort.label")).selectedOptions[0]?.textContent).toBe("Priority");
    expect(titles()).toEqual(["delta", "Alpha", "beta", "Gamma"]);
  });

  it("changes the source with one change that writes the source alone, the block keeping its id", async () => {
    const { user, editor, memory, ids } = await openHub(hubContent({ properties: [PROP.status], sort: { by: "title", dir: "desc" } }));
    expect(titles()).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageList.source.label")), "Pages tagged: project");
    await waitFor(() => {
      expect(titles()).toEqual(["delta", "beta"]);
    });
    expect(attrsOf(editor)).toEqual({
      id: pid(2),
      source: { kind: "tag", optionId: TAG.project },
      layout: "list",
      properties: [PROP.status],
      sort: { by: "title", dir: "desc" },
      filters: null,
      columnWidths: null,
      groupBy: null,
      hiddenGroups: null,
      calculations: null,
      views: null,
    });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"source"');
    // And back to the children.
    await user.selectOptions(openSelect(t("pageList.source.label")), t("pageList.source.children"));
    await waitFor(() => {
      expect(titles()).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    });
    expect(attrsOf(editor).source).toEqual({ kind: "children" });
    // It reaches the core.
    await waitFor(async () => {
      const stored = (await contentOf(memory, ids.page("Hub"))) as { content: { attrs?: Record<string, unknown> }[] };
      expect(stored.content[1]?.attrs?.source).toEqual({ kind: "children" });
    });
  });

  it("toggles properties in a popover, one change each, writing the list alone, in the order they were turned on", async () => {
    const { user, editor } = await openHub();
    const doc = docOf(editor);
    await user.click(propertiesButton());
    const dialog = await screen.findByRole("dialog", { name: t("pageList.properties.label") });
    expect(within(dialog).getAllByRole("checkbox").map((box) => box.closest("label")?.textContent)).toEqual([
      "Tags",
      "Status",
      "Due",
      "Done",
      "Priority",
      "Summary",
      "Stage",
      "Areas",
    ]);
    for (const box of within(dialog).getAllByRole("checkbox")) {
      expect(box).not.toBeChecked();
    }

    let before = doc.oplogVersion();
    await user.click(within(dialog).getByRole("checkbox", { name: "Priority" }));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.priority]);
    });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"properties"');
    expect(attrsOf(editor).id).toBe(pid(2));

    before = doc.oplogVersion();
    await user.click(within(dialog).getByRole("checkbox", { name: "Status" }));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.priority, PROP.status]);
    });
    expect(changesSince(doc, before)).toHaveLength(1);
    // The columns appear in the rows, and the popover stays open.
    expect(within(rowOf("Alpha")).getAllByRole("group").map((cell) => cell.getAttribute("aria-label"))).toEqual(["Priority", "Status"]);
    expect(screen.getByRole("dialog", { name: t("pageList.properties.label") })).toBeInTheDocument();
    expect(propertiesButton()).toHaveTextContent("Properties 2");

    await user.click(within(dialog).getByRole("checkbox", { name: "Priority" }));
    await waitFor(() => {
      expect(attrsOf(editor).properties).toEqual([PROP.status]);
    });
    expect(within(dialog).getByRole("checkbox", { name: "Priority" })).not.toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "Status" })).toBeChecked();

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog", { name: t("pageList.properties.label") })).not.toBeInTheDocument();
    });
    expect(attrsOf(editor)).toEqual({ id: pid(2), ...EDITOR_DEFAULTS, properties: [PROP.status] });
  });

  it("changes the sort field and the direction, each writing the sort alone and keeping the other half", async () => {
    const { user, editor } = await openHub(hubContent({ properties: [PROP.due] }));
    const doc = docOf(editor);
    let before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageList.sort.label")), "Due");
    await waitFor(() => {
      expect(attrsOf(editor).sort).toEqual({ by: PROP.due, dir: "asc" });
    });
    let changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"sort"');

    before = doc.oplogVersion();
    await user.selectOptions(openSelect(t("pageList.sort.dir")), t("pageList.sort.desc"));
    await waitFor(() => {
      expect(attrsOf(editor).sort).toEqual({ by: PROP.due, dir: "desc" });
    });
    changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(attrsOf(editor)).toEqual({ id: pid(2), ...EDITOR_DEFAULTS, properties: [PROP.due], sort: { by: PROP.due, dir: "desc" } });
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
  });

  it("lists a source of a deleted tag in its picker, so that it never shows another source", async () => {
    const { user } = await openHub(hubContent({ source: { kind: "tag", optionId: "de000000-0000-4000-8000-00000000ffff" } }));
    await select(user, t("pageList.source.label"));
    await waitFor(() => {
      expect(openSelect(t("pageList.source.label")).selectedOptions[0]?.textContent).toBe(t("pageList.source.tagGone"));
    });
  });

  it("is not shown in a read-only editor, and neither is New page", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent();
    (content as { content: JsonValue[] }).content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
    const { editor, memory, ids } = await openHub(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    expect(within(block()).queryByRole("button", { name: t("pageList.newPage") })).not.toBeInTheDocument();
    expect(within(block()).queryByRole("combobox")).not.toBeInTheDocument();
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(content);
  });

  it("still opens a page from a row in a read-only editor", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent(asPages());
    (content as { content: JsonValue[] }).content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
    const { user } = await openHub(content);
    await screen.findByText(t("editor.readOnly"));
    await user.click(within(block()).getByRole("button", { name: "Gamma" }));
    await waitFor(() => {
      expect(pageTitle()).toBe("Gamma");
    });
  });
});

// ----- a source this version does not know -------------------------------------------------

describe("a source it does not know", () => {
  const odd = (): JsonValue => hubContent({ source: { kind: "query", text: "status = done" }, layout: "timeline" });

  it("says so and shows no rows, no New page, and writes nothing", async () => {
    const { user, editor, memory, ids } = await openHub(odd());
    expect(emptyText()).toBe(t("pageList.unsupported"));
    expect(titles()).toEqual([]);
    expect(within(block()).queryByRole("button", { name: t("pageList.newPage") })).not.toBeInTheDocument();
    // The toolbar is there to pick a source; the one stored is shown as a placeholder.
    expect((await select(user, t("pageList.source.label"))).value).toBe("");
    const doc = docOf(editor);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(odd());
  });

  it("is replaced by the source picked, the layout it holds kept as it is", async () => {
    const { user, editor } = await openHub(odd());
    await user.selectOptions(await select(user, t("pageList.source.label")), t("pageList.source.children"));
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    });
    expect(attrsOf(editor)).toMatchObject({ source: { kind: "children" }, layout: "timeline" });
  });

  it("shows a layout it does not know as the list", async () => {
    await openHub(hubContent({ layout: "timeline" }));
    expect(titles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
  });
});

// ----- following the tree ------------------------------------------------------------------

describe("following the page tree", () => {
  it("adds a page that is made under the page, sorted in, and drops one that is trashed", async () => {
    const { client, ids, spaceId } = await openHub();
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "Beta two" });
    await waitFor(() => {
      expect(titles()).toEqual(["Alpha", "beta", "Beta two", "delta", "Gamma"]);
    });
    await client.pageTrash(ids.page("Alpha"));
    await waitFor(() => {
      expect(titles()).toEqual(["beta", "Beta two", "delta", "Gamma"]);
    });
  });

  it("follows a rename (and re-sorts), an icon, and a move into or out of the page", async () => {
    const { client, ids } = await openHub();
    await client.pageRename(ids.page("Gamma"), "Aardvark");
    await waitFor(() => {
      expect(titles()).toEqual(["Aardvark", "Alpha", "beta", "delta"]);
    });
    await client.pageSetIcon(ids.page("beta"), "\u{1F680}");
    await waitFor(() => {
      expect(rowOf("beta").querySelector(".page-list-icon")?.textContent).toBe("\u{1F680}");
    });
    await client.pageMove(ids.page("delta"), null, 0);
    await waitFor(() => {
      expect(titles()).toEqual(["Aardvark", "Alpha", "beta"]);
    });
    await client.pageMove(ids.page("Elsewhere"), ids.page("Hub"), 0);
    await waitFor(() => {
      expect(titles()).toEqual(["Aardvark", "Alpha", "beta", "Elsewhere"]);
    });
  });

  it("shows a value that changes elsewhere in its column", async () => {
    fakeToday();
    const { client, ids } = await openHub(hubContent({ properties: [PROP.done] }));
    expect(cellOf("beta", "Done")).not.toBeNull();
    await client.pagePropertySet(ids.page("beta"), PROP.done, { kind: "checkbox", value: true });
    await waitFor(() => {
      expect(within(cellOf("beta", "Done") ?? document.body).getByRole("img")).toHaveAttribute("data-checked", "true");
    });
    await client.pagePropertySet(ids.page("beta"), PROP.done, null);
    await waitFor(() => {
      expect(cellOf("beta", "Done")).toBeNull();
    });
  });
});

// ----- the block ---------------------------------------------------------------------------

describe("the block", () => {
  function select0(editor: Editor): void {
    change(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, posOf(editor))));
    });
  }

  it("is selected as a whole and outlined, its rows are no text, and Backspace or Delete deletes it", async () => {
    const { editor } = await openHub();
    select0(editor);
    const dom = editor.view.nodeDOM(posOf(editor));
    expect(dom).toBeInstanceOf(HTMLElement);
    expect(dom).toHaveClass("ProseMirror-selectednode");
    expect(dom).toHaveAttribute("data-type", "pageList");
    // The rows are not text of the page.
    expect(editor.getText()).not.toContain("Alpha");
    expect(editor.getText()).toContain("Intro");

    change(() => {
      expect(editor.commands.keyboardShortcut("Backspace")).toBe(true);
    });
    expect(JSON.stringify(editor.getJSON())).not.toContain("pageList");
    expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();

    change(() => {
      editor.commands.insertContentAt(0, { type: "pageList" });
    });
    expect(editor.state.doc.child(0).type.name).toBe("pageList");
    expect(editor.state.doc.child(0).attrs.id).toMatch(UUID);
    change(() => {
      editor.view.dispatch(editor.state.tr.setSelection(NodeSelection.create(editor.state.doc, 0)));
    });
    change(() => {
      expect(editor.commands.keyboardShortcut("Delete")).toBe(true);
    });
    expect(JSON.stringify(editor.getJSON())).not.toContain("pageList");
  });

  it("keeps the text of the page when a control of it is used", async () => {
    const { user, editor } = await openHub();
    await user.selectOptions(await select(user, t("pageList.sort.dir")), t("pageList.sort.desc"));
    await waitFor(() => {
      expect(attrsOf(editor).sort).toEqual({ by: "title", dir: "desc" });
    });
    // What was typed in the page is not what changed.
    expect(editor.state.doc.child(0).textContent).toBe("Intro");
    expect(editor.state.doc.childCount).toBe(3);
  });

  it("can be put twice on a page, each with its own settings", async () => {
    const content: JsonValue = {
      type: "doc",
      content: [
        { type: "pageList", attrs: { id: pid(1), ...DEFAULTS } },
        paragraph(2, "between"),
        { type: "pageList", attrs: { id: pid(3), ...DEFAULTS, sort: { by: "title", dir: "desc" } } },
      ],
    };
    await openHub(content);
    const lists = screen.getAllByRole("group", { name: t("pageList.label") });
    expect(lists).toHaveLength(2);
    const order = (list: HTMLElement): string[] =>
      Array.from(list.querySelectorAll(".page-list-row .page-list-name")).map((name) => name.textContent ?? "");
    expect(order(lists[0] as HTMLElement)).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(order(lists[1] as HTMLElement)).toEqual(["Gamma", "delta", "beta", "Alpha"]);
  });

  it("offers a quote or a callout around it, never text that would replace it, and duplicates with an id of its own", async () => {
    const { editor } = await openHub(hubContent({ properties: [PROP.status], sort: { by: "created", dir: "desc" } }));
    const pos = posOf(editor);
    expect(turnIntoKinds(editor.state.doc.child(1))).toEqual(["blockquote", "callout"]);
    expect(turnInto(editor.state, pos, "heading2")).toBeNull();

    const copy = duplicateBlock(editor.state, pos);
    expect(copy).not.toBeNull();
    change(() => {
      if (copy !== null) {
        editor.view.dispatch(copy);
      }
    });
    const first = attrsOf(editor, 0);
    const second = attrsOf(editor, 1);
    expect(first.id).toBe(pid(2));
    expect(second.id).toMatch(UUID);
    expect(second.id).not.toBe(first.id);
    expect({ ...second, id: undefined }).toEqual({ ...first, id: undefined });
    await waitFor(() => {
      expect(screen.getAllByRole("group", { name: t("pageList.label") })).toHaveLength(2);
    });

    const wrap = turnInto(editor.state, posOf(editor, 1), "blockquote");
    expect(wrap).not.toBeNull();
    change(() => {
      if (wrap !== null) {
        editor.view.dispatch(wrap);
      }
    });
    expect(JSON.stringify(editor.getJSON())).toContain("blockquote");
    await waitFor(() => {
      expect(screen.getAllByRole("group", { name: t("pageList.label") })).toHaveLength(2);
    });
  });
});

// ----- nothing is written ------------------------------------------------------------------

describe("the document", () => {
  it("records no change when the page is shown, the rows drawn and the tree and the schema change", async () => {
    fakeToday();
    const { editor, client, ids, spaceId, memory } = await openHub(hubContent({ properties: [PROP.status, PROP.due] }));
    const doc = docOf(editor);
    // This replica wrote nothing: it only imported what the core held.
    const own = (): number => doc.oplogVersion().get(doc.peerIdStr) ?? 0;
    expect(own()).toBe(0);

    await client.pageRename(ids.page("Gamma"), "Aardvark");
    await waitFor(() => {
      expect(titles()[0]).toBe("Aardvark");
    });
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "New one" });
    await client.propertyCreate(spaceId, "Extra", "text");
    await client.pagePropertySet(ids.page("Alpha"), PROP.priority, { kind: "number", value: 5 });
    await waitFor(() => {
      expect(titles()).toContain("New one");
    });
    fireEvent.focus(editor.view.dom);
    expect(own()).toBe(0);
    expect(JSON.stringify(await contentOf(memory, ids.page("Hub")))).toContain(PROP.due);
  });
});

// ----- the kinds of ADR-0026 (amendment of 2026-10-08) and ADR-0027 -----------------------------

describe("the new property kinds in a list view", () => {
  const def = (id: string, kind: PropertyDef["kind"], config?: PropertyDef["config"]): PropertyDef => ({
    id,
    name: id,
    kind,
    options: [],
    createdAt: 0,
    ...(config === undefined ? {} : { config }),
  });
  const page = (title: string, values: [string, unknown][]): PageInfo => ({
    id: title,
    spaceId: "s",
    title,
    createdAt: 0,
    updatedAt: 0,
    properties: values.map(([id, value]) => [id, value as never]),
  });
  const order = (pages: PageInfo[], by: string, dir: "asc" | "desc" = "asc", defs: PropertyDef[] = [], lookups = {}): string[] =>
    sortPages(pages, [{ by, dir }], defs, lookups).map((entry) => entry.title);

  it("sorts a place by its name, the times numerically, a device by its display name; missing values last", () => {
    const places = [
      page("a", [["p", { kind: "place", value: { name: "berlin" } }]]),
      page("b", []),
      page("c", [["p", { kind: "place", value: { name: "Attic", lat: 1, lon: 2 } }]]),
    ];
    expect(order(places, "p", "asc", [def("p", "place")])).toEqual(["c", "a", "b"]);
    expect(order(places, "p", "desc", [def("p", "place")])).toEqual(["a", "c", "b"]);

    const times = [
      page("late", [["t", { kind: "created_time", value: 900 }]]),
      page("early", [["t", { kind: "created_time", value: 80 }]]),
      page("none", []),
    ];
    // Numerically (80 before 900, which a text order would reverse).
    expect(order(times, "t", "asc", [def("t", "created_time")])).toEqual(["early", "late", "none"]);
    expect(order(times, "t", "desc", [def("t", "created_time")])).toEqual(["late", "early", "none"]);

    const devices = [
      page("x", [["d", { kind: "created_by", value: "id-1" }]]),
      page("y", [["d", { kind: "created_by", value: "id-2" }]]),
    ];
    const deviceName = (id: string): string => (id === "id-1" ? "Phone" : "Desk");
    expect(order(devices, "d", "asc", [def("d", "created_by")], { deviceName })).toEqual(["y", "x"]);
  });

  it("sorts a relation by the joined titles of its pages", () => {
    const titlesById: Record<string, string> = { r1: "Zeta", r2: "Alpha", r3: "Beta" };
    const pageTitle = (id: string): string | null => titlesById[id] ?? null;
    const pages = [
      page("one", [["r", { kind: "relation", value: ["r1"] }]]),
      page("two", [["r", { kind: "relation", value: ["r2", "r1"] }]]),
      page("three", [["r", { kind: "relation", value: ["r3"] }]]),
      page("four", []),
    ];
    expect(order(pages, "r", "asc", [def("r", "relation")], { pageTitle })).toEqual(["two", "three", "one", "four"]);
  });

  it("sorts a rollup by its number, day, progress ratio or the text of its values; no result last", () => {
    const config = (fn: string): PropertyDef["config"] => ({ kind: "rollup", relationPropertyId: "rel", target: "title", function: fn }) as never;
    const rollup = (fn: string, result: unknown): unknown => ({ kind: "rollup", value: { function: fn, result } });
    const numbers = [
      page("ten", [["u", rollup("sum", { type: "number", data: 10 })]]),
      page("two", [["u", rollup("sum", { type: "number", data: 2 })]]),
      page("zip", []),
    ];
    expect(order(numbers, "u", "asc", [def("u", "rollup", config("sum"))])).toEqual(["two", "ten", "zip"]);

    const progress = [
      page("half", [["u", rollup("checklist_progress", { type: "progress", data: { done: 1, total: 2 } })]]),
      page("most", [["u", rollup("checklist_progress", { type: "progress", data: { done: 9, total: 10 } })]]),
      page("empty", [["u", rollup("checklist_progress", { type: "progress", data: { done: 0, total: 0 } })]]),
    ];
    expect(order(progress, "u", "asc", [def("u", "rollup", config("checklist_progress"))])).toEqual(["half", "most", "empty"]);

    const days = [
      page("later", [["u", rollup("latest", { type: "date", data: { date: "2026-11-01" } })]]),
      page("sooner", [["u", rollup("latest", { type: "date", data: { date: "2026-10-01", time: "08:00" } })]]),
    ];
    expect(order(days, "u", "asc", [def("u", "rollup", config("latest"))])).toEqual(["sooner", "later"]);

    const lists = [
      page("b", [["u", rollup("show_original", { type: "values", data: [{ kind: "text", value: "Zed" }] })]]),
      page("a", [["u", rollup("show_original", { type: "values", data: [{ kind: "text", value: "Abe" }, { kind: "text", value: "Cy" }] })]]),
    ];
    expect(order(lists, "u", "asc", [def("u", "rollup", config("show_original"))])).toEqual(["a", "b"]);
  });

  async function openKinds(attrs: { [key: string]: JsonValue } = {}) {
    const app = await renderPages([kindsSpace(kindsHub({ layout: "list", ...attrs }))], { now: ticking() });
    await openPage(app.user, "Hub");
    await screen.findAllByRole("group", { name: t("pageList.label") });
    const client = new CoreClient(app.memory);
    const ids = await idsOf(app.memory);
    return { ...app, client, ids };
  }

  it("shows a place by its name with the coordinates as its tooltip", async () => {
    await openKinds();
    const cafe = cellOf("Cafe", "Where");
    expect(cafe).toHaveTextContent("Café de Flore");
    expect(cafe?.querySelector(".page-list-text")).toHaveAttribute("title", "48.854, 2.333");
    expect(cellOf("Zoo", "Where")?.querySelector(".page-list-text")).not.toHaveAttribute("title");
    expect(cellOf("Plain", "Where")).toBeNull();
  });

  it("shows the times as the locale writes them and the devices by name", async () => {
    const { client, ids } = await openKinds();
    const created = (await client.pagePropertiesGet(ids.page("Cafe"))).find(([id]) => id === K.created)?.[1];
    expect(created?.kind).toBe("created_time");
    if (created?.kind === "created_time") {
      expect(cellOf("Cafe", "Created")).toHaveTextContent(formatInstant(created.value));
    }
    expect(cellOf("Cafe", "Edited")?.textContent).not.toBe("");
    const madeBy = cellOf("Cafe", "Made by")?.textContent ?? "";
    expect(madeBy).not.toBe("");
    expect(madeBy).not.toMatch(/^[0-9a-f]{8}-/u);
  });

  it("shows a relation as chips with the icon and title of each page, never the ids", async () => {
    await openKinds();
    const chips = (title: string): string[] =>
      Array.from(cellOf(title, "Tasks")?.querySelectorAll(".relation-chip-title") ?? []).map((node) => node.textContent ?? "");
    expect(chips("Cafe")).toEqual(["Write"]);
    expect(chips("Zoo")).toEqual(["Ship", "Write"]);
    expect(cellOf("Zoo", "Tasks")?.textContent).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}/u);
  });

  it("shows a rollup: a number as the locale writes it, a list of values as items", async () => {
    await openKinds();
    expect(cellOf("Zoo", "Total")).toHaveTextContent("7.5");
    expect(cellOf("Cafe", "Total")).toHaveTextContent("3");
    const names = Array.from(cellOf("Zoo", "Names")?.querySelectorAll(".rollup-item") ?? []).map((node) => node.textContent);
    expect(names).toEqual(["Ship", "Write"]);
  });

  it("sorts the rows by a rollup", async () => {
    await openKinds({ sort: { by: K.total, dir: "desc" } });
    expect(titles().slice(0, 2)).toEqual(["Zoo", "Cafe"]);
  });

  it("sorts the rows by a relation through the titles of the related pages", async () => {
    await openKinds({ sort: { by: K.tasks, dir: "asc" } });
    // "Ship, Write" (Zoo) comes before "Write" (Cafe); pages without a relation come last.
    expect(titles().slice(0, 2)).toEqual(["Zoo", "Cafe"]);
  });
});
