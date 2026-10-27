// The footer of the table layout (ADR-0040 item 5, ADR-0016 amendment of 2026-10-14): a row group of its own
// after the windowed body, one cell per shown column; the "Calculate" button of an empty cell and the menu of
// the functions the column's kind offers; the choice written as the block's `calculations` attribute alone, in
// one step; the value computed over the rows the view shows and kept up to date; computed columns; a read-only
// editor; the keyboard; a long (windowed) table; what a list, a board, an unknown function and an unknown
// column do; the schema and the HTML form; an older editor.

import { CoreClient, loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue, PropertyDef, SeedSpace } from "@atomik-kb/core-client";
import { Editor, getSchema } from "@tiptap/core";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import blockSchemaFixture from "../../../../fixtures/doc/block-schema-v1.json";
import { t } from "../i18n";
import { formatRollupNumber } from "../property-format";
import { bindView, loroDocFrom, pid, tick } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { viewSelect } from "../test/page-list";
import type { User } from "../test/render";
import { kindsHub, kindsSpace } from "../test/view-kinds";
import { describeSchema } from "../test/schema-fixture";
import type { BlockSchemaFixture } from "../test/schema-fixture";
import { formatDateMention, todayIso } from "./dates";
import { loroSyncPluginKey } from "./loro-binding";
import { PageList } from "./page-list";
import { CALCULATION_LABELS, calculationFunctionsFor } from "./page-table-calculations";
import type { CalculationFunction } from "./page-table-calculations";
import { ROW_HEIGHT, scrollToReveal } from "./page-table-model";
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
  areas: "de000000-0000-4000-8000-00000000c006",
};

const DEFAULTS = {
  source: { kind: "children" },
  layout: "table",
  properties: [] as string[],
  sort: { by: "title", dir: "asc" },
};

function paragraph(n: number, value: string): JsonValue {
  return value === ""
    ? { type: "paragraph", attrs: { id: pid(n) } }
    : { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

/** An intro, the view (id `pid(2)`, a table with the Priority, Due, Done and Summary columns unless told otherwise) and an empty paragraph. */
function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      paragraph(1, "Intro"),
      { type: "pageList", attrs: { id: pid(2), ...DEFAULTS, properties: [PROP.priority, PROP.due, PROP.done, PROP.summary], ...list } },
      paragraph(3, ""),
    ],
  };
}

/**
 * Hub (holds the view) with the children Alpha (Priority 3, due 2026-10-15, done), beta (10, due 2026-10-09 09:30, not done),
 * Gamma (no value) and delta (2, due 2026-10-04); `extra` more pages with no value when given.
 */
function space(content: JsonValue = hubContent(), extra = 0): SeedSpace {
  const more = Array.from({ length: extra }, (_, index) => ({ title: `Row ${String(index).padStart(3, "0")}` }));
  return {
    name: "Personal",
    properties: [
      { id: PROP.status, name: "Status", kind: "status" },
      { id: PROP.due, name: "Due", kind: "date" },
      { id: PROP.done, name: "Done", kind: "checkbox" },
      { id: PROP.priority, name: "Priority", kind: "number" },
      { id: PROP.summary, name: "Summary", kind: "text" },
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
        content,
        children: [
          { title: "Alpha", properties: { Due: { date: "2026-10-15" }, Done: true, Priority: 3, Summary: "A short note", Areas: ["Home", "Work"] } },
          { title: "beta", properties: { Due: { date: "2026-10-09", time: "09:30" }, Done: false, Priority: 10, Summary: "Second", Areas: ["Work"] } },
          { title: "Gamma" },
          { title: "delta", properties: { Due: { date: "2026-10-04" }, Priority: 2, Summary: "Second" } },
          ...more,
        ],
      },
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

async function openKinds(attrs: { [key: string]: JsonValue } = {}) {
  const app = await renderPages([kindsSpace(kindsHub(attrs))], { now: ticking() });
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  return { ...app, editor };
}

// ----- looking at the view -----------------------------------------------------------------

const TITLE = t("pageList.sort.title");

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

function titles(): string[] {
  return bodyRows().map((row) => row.querySelector(".page-table-title")?.textContent ?? "");
}

/** The three row groups of the grid: the header, the body and the footer. */
function groups(): HTMLElement[] {
  return within(grid()).getAllByRole("rowgroup");
}

function footerRow(): HTMLElement {
  const [, , foot] = groups();
  if (foot === undefined) {
    throw new Error("no footer");
  }
  return within(foot).getByRole("row");
}

function footerCell(column: string): HTMLElement {
  return within(footerRow()).getByRole("gridcell", { name: t("pageTable.calc.cell", { name: column }) });
}

/** What a footer cell shows: the name of the function and its value (`null` for an empty cell). */
function shown(column: string): { label: string; value: string } | null {
  const cell = footerCell(column);
  const label = cell.querySelector(".page-table-calc-label")?.textContent;
  const value = cell.querySelector(".page-table-calc-value")?.textContent;
  return label === undefined || value === undefined ? null : { label: label ?? "", value: value ?? "" };
}

function labelOf(fn: CalculationFunction): string {
  return t(CALCULATION_LABELS[fn]);
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

/** The `calculations` of the document's view, as the Loro document stores it (`undefined` when it has none). */
function storedCalculations(editor: Editor): unknown {
  const root = loroToPortable(docOf(editor)) as { content: { type: string; attrs?: { calculations?: unknown } }[] };
  return root.content.find((node) => node.type === "pageList")?.attrs?.calculations;
}

/** Opens the menu of a footer cell. */
async function openCalculationMenu(user: User, column: string): Promise<HTMLElement> {
  await user.click(within(footerCell(column)).getByRole("button"));
  return screen.findByRole("menu", { name: t("pageTable.calc.menu", { name: column }) });
}

/** Chooses a function (or "None") in the menu of a footer cell. */
async function choose(user: User, column: string, item: CalculationFunction | "none") {
  const menu = await openCalculationMenu(user, column);
  await user.click(within(menu).getByRole("menuitem", { name: item === "none" ? t("pageTable.calc.none") : labelOf(item) }));
}

/** The position of the view in the editor's document. */
function listPosition(editor: Editor): number {
  let at = -1;
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name === "pageList") {
      at = pos;
    }
    return at < 0;
  });
  return at;
}

/** The layout select, in the view settings menu of the toolbar (opened when it is not). */
function layoutSelect(user: User): Promise<HTMLSelectElement> {
  return viewSelect(user, block(), t("pageList.layout.label"));
}

/** The cell of a row of the body. */
function bodyCell(title: string, column: string): HTMLElement {
  const row = bodyRows().find((candidate) => candidate.querySelector(".page-table-title")?.textContent === title);
  if (row === undefined) {
    throw new Error(`no row for ${title}`);
  }
  return within(row).getByRole("gridcell", { name: column });
}

// ----- the footer ---------------------------------------------------------------------------

describe("the footer row", () => {
  it("is a row group of its own after the body, with one cell per shown column, and the grid counts its row", async () => {
    await openHub();
    const [head, body, foot, ...more] = groups();
    expect(more).toHaveLength(0);
    expect(head?.compareDocumentPosition(body as Node) ?? 0).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(body?.compareDocumentPosition(foot as Node) ?? 0).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    // Outside the windowed body: not a child of it, and not in the scroll that the body's padding stands for.
    expect(body?.contains(foot as Node)).toBe(false);
    expect(foot).toHaveClass("page-table-foot");

    const cells = within(footerRow()).getAllByRole("gridcell");
    expect(cells.map((cell) => cell.getAttribute("aria-label"))).toEqual(
      [TITLE, "Priority", "Due", "Done", "Summary"].map((name) => t("pageTable.calc.cell", { name })),
    );
    // The first column of the grid is the checkbox column of the selection (ADR-0058): the footer's cells start at the second.
    expect(cells.map((cell) => cell.getAttribute("aria-colindex"))).toEqual(["2", "3", "4", "5", "6"]);
    // 4 rows, the header and the footer.
    expect(grid()).toHaveAttribute("aria-rowcount", "6");
    expect(footerRow()).toHaveAttribute("aria-rowindex", "6");
    expect(grid()).toHaveAttribute("aria-colcount", "6");
  });

  it("is a row of the same grid as the header and the rows, so that it has their columns and scrolls with them", async () => {
    await openHub(hubContent({ columnWidths: { title: 300, [PROP.priority]: 120 } }));
    const gridElement = grid();
    expect(gridElement.style.getPropertyValue("--page-table-columns")).toBe("32px minmax(300px, 1fr) 120px 170px 170px 170px");
    const [head, body, foot] = groups();
    for (const group of [head, body, foot]) {
      expect(group?.parentElement).toBe(gridElement);
    }
    // The rows of the three groups are the same kind of row (the grid template is theirs).
    expect(footerRow()).toHaveClass("page-table-row");
    expect(within(head as HTMLElement).getByRole("row")).toHaveClass("page-table-row");
    expect(bodyRows()[0]).toHaveClass("page-table-row");
    // It sits inside the scroll area, so a sideways scroll moves it with the header and the rows.
    expect(footerRow().closest(".page-table-scroll")).toBe(gridElement.closest(".page-table-scroll"));
  });

  it("starts empty: every cell offers Calculate, and nothing is written", async () => {
    const { editor } = await openHub();
    const doc = docOf(editor);
    for (const column of [TITLE, "Priority", "Due", "Done", "Summary"]) {
      expect(within(footerCell(column)).getByRole("button")).toHaveTextContent(t("pageTable.calc.calculate"));
      expect(shown(column)).toBeNull();
    }
    expect(attrsOf(editor).calculations).toBeNull();
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
  });

  it("keeps the one cell of the grid in the tab order: the cells of the footer are not it", async () => {
    await openHub();
    const tabbable = within(grid())
      .getAllByRole("gridcell")
      .filter((cell) => cell.tabIndex === 0);
    expect(tabbable).toHaveLength(1);
    expect(tabbable[0]?.closest(".page-table-foot")).toBeNull();
  });

  it("follows a column that is hidden, moved or added: it has a cell per shown column, in their order", async () => {
    const { editor } = await openHub();
    act(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(listPosition(editor), "properties", [PROP.summary, PROP.priority]));
    });
    await waitFor(() => {
      expect(within(footerRow()).getAllByRole("gridcell")).toHaveLength(3);
    });
    expect(within(footerRow()).getAllByRole("gridcell").map((cell) => cell.getAttribute("aria-label"))).toEqual(
      [TITLE, "Summary", "Priority"].map((name) => t("pageTable.calc.cell", { name })),
    );
  });
});

// ----- the menu -----------------------------------------------------------------------------

describe("the menu of a footer cell", () => {
  it("lists None and only the functions the column's kind offers, in order", async () => {
    const { user } = await openHub(hubContent({ properties: [PROP.priority, PROP.due, PROP.done, PROP.summary, PROP.areas] }));
    const items = async (column: string): Promise<string[]> => {
      const menu = await openCalculationMenu(user, column);
      const names = within(menu)
        .getAllByRole("menuitem")
        .map((item) => item.textContent ?? "");
      await user.keyboard("{Escape}");
      await waitFor(() => {
        expect(screen.queryByRole("menu")).not.toBeInTheDocument();
      });
      return names;
    };
    const expected = (column: Parameters<typeof calculationFunctionsFor>[0]): string[] => [t("pageTable.calc.none"), ...calculationFunctionsFor(column).map(labelOf)];
    const defs: PropertyDef[] = [
      { id: PROP.priority, name: "Priority", kind: "number", options: [], createdAt: 0 },
      { id: PROP.due, name: "Due", kind: "date", options: [], createdAt: 0 },
      { id: PROP.done, name: "Done", kind: "checkbox", options: [], createdAt: 0 },
      { id: PROP.summary, name: "Summary", kind: "text", options: [], createdAt: 0 },
      { id: PROP.areas, name: "Areas", kind: "multi_select", options: [], createdAt: 0 },
    ];

    expect(await items("Priority")).toEqual(expected(defs[0] ?? null));
    expect(await items("Due")).toEqual(expected(defs[1] ?? null));
    expect(await items("Done")).toEqual(expected(defs[2] ?? null));
    expect(await items("Summary")).toEqual(expected(defs[3] ?? null));
    expect(await items("Areas")).toEqual(expected(defs[4] ?? null));
    expect(await items(TITLE)).toEqual(expected(null));

    // What a number has that a text does not, and the reverse.
    expect(await items("Priority")).toEqual(expect.arrayContaining(["Sum", "Average", "Median", "Minimum", "Maximum", "Range", "Count all", "Count unique values"]));
    expect(await items("Summary")).not.toContain("Sum");
    expect(await items(TITLE)).toEqual(
      ["None", "Count all", "Count values", "Count unique values", "Count empty", "Count not empty", "Percent empty", "Percent not empty"],
    );
    expect(await items("Due")).toEqual(expect.arrayContaining(["Earliest date", "Latest date", "Date range"]));
    expect(await items("Done")).toEqual(expect.arrayContaining(["Count checked", "Count unchecked", "Percent checked"]));
  });

  it("writes the choice as the block's calculations attribute, and shows the function and its value", async () => {
    const { user, editor } = await openHub();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await choose(user, "Priority", "sum");

    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum" });
    });
    expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    // One change of the document, of that attribute alone; the rest of the block is as it was.
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"calculations"');
    expect(JSON.stringify(changes[0])).not.toMatch(/"(columnWidths|filters|sort|properties|layout)"/u);
    expect(storedCalculations(editor)).toEqual({ [PROP.priority]: "sum" });
    expect(attrsOf(editor)).toMatchObject({ layout: "table", sort: { by: "title", dir: "asc" }, properties: [PROP.priority, PROP.due, PROP.done, PROP.summary] });
  });

  it("calculates the page column with the counts, and each column on its own", async () => {
    const { user, editor } = await openHub();
    await choose(user, TITLE, "count_all");
    await choose(user, "Summary", "count_unique");
    await choose(user, "Due", "date_range");
    await choose(user, "Done", "percent_checked");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ title: "count_all", [PROP.summary]: "count_unique", [PROP.due]: "date_range", [PROP.done]: "percent_checked" });
    });
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "4" });
    expect(shown("Summary")).toEqual({ label: "Count unique values", value: "2" }); // "A short note", "Second"
    expect(shown("Due")).toEqual({ label: "Date range", value: "11 days" });
    expect(shown("Done")).toEqual({ label: "Percent checked", value: formatRollupNumber(25, "percent_checked") });
    expect(shown("Priority")).toBeNull();
  });

  it("changes a function for another, and None removes it (the last one removes the attribute)", async () => {
    const { user, editor } = await openHub();
    await choose(user, "Priority", "sum");
    await choose(user, "Priority", "max");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "max" });
    });
    expect(shown("Priority")).toEqual({ label: "Maximum", value: "10" });
    await choose(user, "Summary", "count_all");
    await choose(user, "Priority", "none");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.summary]: "count_all" });
    });
    expect(shown("Priority")).toBeNull();
    await choose(user, "Summary", "none");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toBeNull();
    });
    expect(storedCalculations(editor)).toBeUndefined(); // the key is gone from the document, as if never set
    expect(within(footerCell("Summary")).getByRole("button")).toHaveTextContent(t("pageTable.calc.calculate"));
  });

  it("writes nothing for the choice that is already made, or for None on a column that has none", async () => {
    const { user, editor } = await openHub(hubContent({ calculations: { [PROP.priority]: "sum" } }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await choose(user, "Priority", "sum");
    await choose(user, "Summary", "none");
    expect(changesSince(doc, before)).toEqual([]);
  });

  it("is one step of the editor's undo, and undo takes back that step alone", async () => {
    const { user, editor } = await openHub();
    await choose(user, "Priority", "sum");
    await tick(600); // past the time within which changes undo together
    await choose(user, "Summary", "count_all");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum", [PROP.summary]: "count_all" });
    });
    act(() => {
      expect(editor.commands.keyboardShortcut("Mod-z")).toBe(true);
    });
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum" });
    });
    expect(shown("Summary")).toBeNull();
    expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    act(() => {
      expect(editor.commands.keyboardShortcut("Mod-Shift-z")).toBe(true);
    });
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum", [PROP.summary]: "count_all" });
    });
  });

  it("is reached with the keyboard: Tab to the button, Enter opens the menu, the arrows move, Enter chooses, Escape closes", async () => {
    const { user, editor } = await openHub();
    const button = within(footerCell("Priority")).getByRole("button");
    act(() => {
      button.focus();
    });
    await user.keyboard("{Enter}");
    const menu = await screen.findByRole("menu", { name: t("pageTable.calc.menu", { name: "Priority" }) });
    expect(button).toHaveAttribute("aria-expanded", "true");
    expect(within(menu).getByRole("menuitem", { name: t("pageTable.calc.none") })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(within(menu).getByRole("menuitem", { name: "Sum" })).toHaveFocus();
    await user.keyboard("{ArrowDown}{Enter}");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "average" });
    });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(button).toHaveFocus();
    expect(shown("Priority")).toEqual({ label: "Average", value: "5" });

    await user.keyboard("{Enter}");
    await screen.findByRole("menu");
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "average" });
  });

  it("leaves the keys of the grid alone: the arrows still move the active cell, Tab still has one cell", async () => {
    const { user } = await openHub();
    await user.click(within(bodyRows()[0] as HTMLElement).getByRole("gridcell", { name: TITLE }));
    await user.keyboard("{ArrowRight}");
    expect(within(bodyRows()[0] as HTMLElement).getByRole("gridcell", { name: "Priority" })).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}");
    expect(within(bodyRows()[3] as HTMLElement).getByRole("gridcell", { name: "Priority" })).toHaveFocus(); // the last row: the footer is not a row of the keyboard grid
    const tabbable = within(grid())
      .getAllByRole("gridcell")
      .filter((cell) => cell.tabIndex === 0);
    expect(tabbable).toHaveLength(1);
  });
});

// ----- the values ---------------------------------------------------------------------------

describe("the value", () => {
  it("is computed over the rows the view shows, after its filters", async () => {
    const { user } = await openHub(hubContent({ filters: [{ propertyId: PROP.priority, op: "isSet" }], calculations: { title: "count_all", [PROP.priority]: "sum", [PROP.summary]: "count_empty" } }));
    expect(titles()).toEqual(["Alpha", "beta", "delta"]); // Gamma has no priority
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "3" });
    expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    expect(shown("Summary")).toEqual({ label: "Count empty", value: "0" }); // Gamma's empty summary is filtered out
    await choose(user, "Done", "count_empty");
    expect(shown("Done")).toEqual({ label: "Count empty", value: "1" }); // delta has no box value
  });

  it("changes with the filters: a filter set on the block, or taken off it, recomputes it", async () => {
    const { editor } = await openHub(hubContent({ calculations: { title: "count_all", [PROP.priority]: "average" } }));
    expect(shown(TITLE)?.value).toBe("4");
    expect(shown("Priority")?.value).toBe("5");
    const position = listPosition(editor);
    act(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(position, "filters", [{ propertyId: PROP.priority, op: "equals", value: 10 }]));
    });
    await waitFor(() => {
      expect(shown(TITLE)?.value).toBe("1");
    });
    expect(shown("Priority")?.value).toBe("10");
    expect(titles()).toEqual(["beta"]);
    act(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(position, "filters", null));
    });
    await waitFor(() => {
      expect(shown(TITLE)?.value).toBe("4");
    });
    expect(shown("Priority")?.value).toBe("5");
  });

  it("changes when a value of a row changes, and when a row appears or goes", async () => {
    const { client, ids, spaceId } = await openHub(hubContent({ calculations: { title: "count_all", [PROP.priority]: "sum", [PROP.done]: "count_checked" } }));
    expect(shown("Priority")?.value).toBe("15");
    await client.pagePropertySet(ids.page("Gamma"), PROP.priority, { kind: "number", value: 5 });
    await waitFor(() => {
      expect(shown("Priority")?.value).toBe("20");
    });
    await client.pagePropertySet(ids.page("beta"), PROP.done, { kind: "checkbox", value: true });
    await waitFor(() => {
      expect(shown("Done")?.value).toBe("2");
    });
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "Epsilon" });
    await waitFor(() => {
      expect(shown(TITLE)?.value).toBe("5");
    });
    await client.pagePropertySet(ids.page("Alpha"), PROP.priority, null);
    await waitFor(() => {
      expect(shown("Priority")?.value).toBe("17");
    });
  });

  it("has a dash for a function with nothing to say (the average of no numbers)", async () => {
    await openHub(hubContent({ filters: [{ propertyId: PROP.summary, op: "equals", value: "nothing like it" }], calculations: { [PROP.priority]: "average", [PROP.summary]: "percent_empty", title: "count_all" } }));
    expect(titles()).toEqual([]);
    expect(shown("Priority")).toEqual({ label: "Average", value: t("pageTable.calc.noResult") });
    expect(shown("Summary")?.value).toBe(t("pageTable.calc.noResult")); // a percent of no rows
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "0" });
  });

  it("shows a number as a rollup does: at most two decimals, a percent with its sign, and a day as a date mention", async () => {
    await openHub(hubContent({ calculations: { [PROP.priority]: "average", [PROP.summary]: "percent_not_empty", [PROP.due]: "earliest", [PROP.done]: "percent_empty" } }));
    expect(shown("Priority")?.value).toBe("5");
    expect(shown("Summary")?.value).toBe(formatRollupNumber(75, "percent_not_empty"));
    expect(shown("Summary")?.value).toContain("%");
    expect(shown("Due")?.value).toBe(formatDateMention("2026-10-04", null, todayIso()));
    expect(shown("Done")?.value).toBe(formatRollupNumber(50, "percent_empty"));
  });

  it("counts the values of a multi-select once each, and the rows that have one once each", async () => {
    const { user } = await openHub(hubContent({ properties: [PROP.areas], calculations: { [PROP.areas]: "count_values" } }));
    expect(shown("Areas")).toEqual({ label: "Count values", value: "3" }); // Home, Work, Work
    await choose(user, "Areas", "count_not_empty");
    expect(shown("Areas")).toEqual({ label: "Count not empty", value: "2" }); // Alpha and beta
    await choose(user, "Areas", "count_unique");
    expect(shown("Areas")?.value).toBe("2"); // Home and Work
  });
});

describe("the computed columns", () => {
  it("add up a rollup, and take the earliest and latest of a created time", async () => {
    const { user } = await openKinds();
    await choose(user, "Total", "sum");
    expect(shown("Total")).toEqual({ label: "Sum", value: "10.5" }); // Cafe 3, Zoo 7.5
    await choose(user, "Total", "max");
    expect(shown("Total")).toEqual({ label: "Maximum", value: "7.5" });

    await choose(user, "Created", "earliest");
    expect(shown("Created")?.value).toBe(bodyCell("Cafe", "Created").textContent); // the first page made
    await choose(user, "Created", "date_range");
    expect(shown("Created")?.value).toBe("0 days");
  });

  it("offer a rollup of texts and a relation only the counts, and count the pages a relation holds", async () => {
    const { user } = await openKinds();
    for (const column of ["Names", "Tasks", "Where", "Made by"]) {
      const menu = await openCalculationMenu(user, column);
      expect(within(menu).getAllByRole("menuitem").length).toBe(1 + calculationFunctionsFor(null).length);
      await user.keyboard("{Escape}");
      await waitFor(() => {
        expect(screen.queryByRole("menu")).not.toBeInTheDocument();
      });
    }
    await choose(user, "Tasks", "count_values");
    expect(shown("Tasks")?.value).toBe("3"); // Write; Ship and Write
    await choose(user, "Tasks", "count_unique");
    expect(shown("Tasks")?.value).toBe("2");
    await choose(user, "Tasks", "count_not_empty");
    expect(shown("Tasks")?.value).toBe("2");
    await choose(user, "Where", "count_not_empty");
    expect(shown("Where")?.value).toBe("3");
  });
});

// ----- a read-only editor -------------------------------------------------------------------

describe("a read-only editor", () => {
  it("shows the values of the calculations, and offers no button, menu or empty cell to fill", async () => {
    const { user, editor, memory, ids } = await openHub(hubContent({ calculations: { title: "count_all", [PROP.priority]: "sum" } }));
    expect(within(footerCell("Priority")).getByRole("button")).toBeInTheDocument();
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(within(footerCell("Priority")).queryByRole("button")).not.toBeInTheDocument();
    });
    expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "4" });
    expect(shown("Summary")).toBeNull();
    expect(within(footerRow()).queryAllByRole("button")).toHaveLength(0);
    await user.click(footerCell("Priority"));
    await user.keyboard("{Enter}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(footerRow().textContent).not.toContain(t("pageTable.calc.calculate"));
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(hubContent({ calculations: { title: "count_all", [PROP.priority]: "sum" } }));
  });

  it("has no footer at all when it holds no calculation", async () => {
    const { editor } = await openHub();
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(within(grid()).getAllByRole("rowgroup")).toHaveLength(2);
    });
    expect(grid()).toHaveAttribute("aria-rowcount", "5");
  });
});

// ----- a long table -------------------------------------------------------------------------

describe("a table of many rows", () => {
  it("reveals a row above the footer: the area shows a row less of the rows, and no row less without a footer", () => {
    expect(scrollToReveal(300, 0)).toBe(ROW_HEIGHT * 301 - (480 - ROW_HEIGHT));
    expect(scrollToReveal(300, 0, ROW_HEIGHT)).toBe(ROW_HEIGHT * 301 - (480 - 2 * ROW_HEIGHT));
    expect(scrollToReveal(3, 0, ROW_HEIGHT)).toBe(0);
    expect(scrollToReveal(2, ROW_HEIGHT * 10, ROW_HEIGHT)).toBe(ROW_HEIGHT * 2);
    // A row that the footer would cover is brought up, one that it would not is left where it is.
    expect(scrollToReveal(12, 0)).toBe(0);
    expect(scrollToReveal(12, 0, ROW_HEIGHT)).toBe(ROW_HEIGHT * 13 - (480 - 2 * ROW_HEIGHT)); // 30
    expect(scrollToReveal(11, 0, ROW_HEIGHT)).toBe(0);
  });

  it("keeps the footer outside the window of rows, with the value over every row, however far it is scrolled", async () => {
    await openHub(hubContent({ calculations: { title: "count_all", [PROP.priority]: "sum", [PROP.summary]: "count_empty" } }), 200);
    expect(grid()).toHaveAttribute("aria-rowcount", "206");
    expect(bodyRows().length).toBeLessThan(40);
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "204" }); // the rows not drawn count
    expect(shown("Priority")?.value).toBe("15");
    expect(shown("Summary")?.value).toBe("201"); // Gamma and the 200 rows added

    const [head, body, foot] = groups();
    expect(foot).toHaveClass("page-table-foot");
    expect(body?.contains(foot as Node)).toBe(false);
    expect(head?.compareDocumentPosition(foot as Node)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(footerRow()).toHaveAttribute("aria-rowindex", "206");

    const scroller = grid().closest(".page-table-scroll");
    expect(scroller).not.toBeNull();
    fireEvent.scroll(scroller as Element, { target: { scrollTop: ROW_HEIGHT * 150 } });
    await waitFor(() => {
      expect(titles()).toContain("Row 150");
    });
    expect(titles()).not.toContain("Alpha");
    expect(groups()).toHaveLength(3);
    expect(footerRow()).toBeInTheDocument();
    expect(shown(TITLE)?.value).toBe("204");
    expect(shown("Priority")?.value).toBe("15");
  });

  it("can still choose a function in a long table, and the choice is written once", async () => {
    const { user, editor } = await openHub(hubContent(), 200);
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await choose(user, "Summary", "count_not_empty");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({ [PROP.summary]: "count_not_empty" });
    });
    expect(shown("Summary")?.value).toBe("3");
    expect(changesSince(doc, before)).toHaveLength(1);
  });

  it("brings a row into view above the footer when the keyboard moves to it", async () => {
    const { user } = await openHub(hubContent({ properties: [PROP.priority] }), 200);
    await user.click(within(bodyRows()[0] as HTMLElement).getByRole("gridcell", { name: TITLE }));
    await user.keyboard("{ArrowDown}".repeat(30));
    const scroller = grid().closest(".page-table-scroll") as HTMLElement;
    // Row 30 ends 31 rows down; the area shows 480 - 34 (the header) - 34 (the footer) = 412 pixels of the rows.
    expect(scroller.scrollTop).toBe(ROW_HEIGHT * 31 - 412);
  });
});

// ----- the other layouts --------------------------------------------------------------------

describe("the other layouts", () => {
  it("have no footer: a list and a board draw no grid, and keep the calculations for the table", async () => {
    const { user, editor } = await openHub(hubContent({ layout: "list", calculations: { [PROP.priority]: "sum" } }));
    expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
    expect(block().querySelector(".page-table-foot")).toBeNull();
    expect(block().textContent).not.toContain(t("pageTable.calc.calculate"));
    expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum" });

    await user.selectOptions(await layoutSelect(user), t("pageList.layout.table"));
    await waitFor(() => {
      expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    });
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.board"));
    await waitFor(() => {
      expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
    });
    expect(block().querySelector(".page-table-foot")).toBeNull();
    expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum" });
  });
});

// ----- what is not understood ---------------------------------------------------------------

describe("a calculation this version does not understand", () => {
  const odd = {
    [PROP.priority]: "future_function", // a function of a newer version
    [PROP.summary]: "sum", // a function that the column's kind does not offer
    "de000000-0000-4000-8000-00000000dead": "sum", // a column the space does not have
    [PROP.due]: 7, // no function
    title: "count_all",
  };

  it("is ignored: the column offers Calculate again, and reading it writes nothing", async () => {
    const { editor } = await openHub(hubContent({ calculations: odd as never }));
    const doc = docOf(editor);
    expect(shown("Priority")).toBeNull();
    expect(shown("Summary")).toBeNull();
    expect(shown("Due")).toBeNull();
    expect(shown(TITLE)).toEqual({ label: "Count all", value: "4" });
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    expect(within(grid()).getAllByRole("gridcell").filter((cell) => cell.closest(".page-table-foot") !== null)).toHaveLength(5);
  });

  it("is kept when another column's calculation is written (an entry of a newer version is not lost)", async () => {
    const { user, editor } = await openHub(hubContent({ calculations: odd as never }));
    await choose(user, "Due", "latest");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toEqual({
        [PROP.priority]: "future_function",
        [PROP.summary]: "sum",
        "de000000-0000-4000-8000-00000000dead": "sum",
        title: "count_all",
        [PROP.due]: "latest",
      });
    });
    // Choosing for the column that holds the unknown one replaces it.
    await choose(user, "Priority", "max");
    await waitFor(() => {
      expect(attrsOf(editor).calculations).toMatchObject({ [PROP.priority]: "max" });
    });
  });

  it("is a calculation that comes back when its column does: hiding a column does not drop it", async () => {
    const { editor } = await openHub(hubContent({ calculations: { [PROP.priority]: "sum" } }));
    const position = listPosition(editor);
    act(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(position, "properties", [PROP.summary]));
    });
    await waitFor(() => {
      expect(within(footerRow()).getAllByRole("gridcell")).toHaveLength(2);
    });
    expect(attrsOf(editor).calculations).toEqual({ [PROP.priority]: "sum" });
    act(() => {
      editor.view.dispatch(editor.state.tr.setNodeAttribute(position, "properties", [PROP.summary, PROP.priority]));
    });
    await waitFor(() => {
      expect(shown("Priority")).toEqual({ label: "Sum", value: "15" });
    });
  });
});

// ----- the schema ---------------------------------------------------------------------------

describe("the schema and the HTML form", () => {
  const fixture = blockSchemaFixture as unknown as BlockSchemaFixture;
  const live = describeSchema(createEditorSchema());

  function mount(content: JsonValue): Editor {
    const element = document.createElement("div");
    document.body.append(element);
    return new Editor({ element, extensions: createSchemaExtensions(), content: content as never });
  }

  it("lists calculations after hiddenGroups, in the fixture and in the editor", () => {
    expect(live.nodes.pageList?.attrs).toEqual(["id", "source", "layout", "properties", "sort", "filters", "columnWidths", "groupBy", "hiddenGroups", "calculations", "views"]);
    expect(fixture.nodes.pageList).toEqual(live.nodes.pageList);
  });

  it("is absent until a control sets it: the attribute is null, which the binding does not write", () => {
    const editor = mount(hubContent());
    expect(attrsOf(editor).calculations).toBeNull();
    expect(storedCalculationsOf(loroDocFrom(hubContent()))).toBeUndefined();
    editor.destroy();
  });

  it("is written to the HTML as JSON text when the block has it, read back, and left out when it has not", () => {
    const calculations = { title: "count_all", [PROP.priority]: "sum" };
    const editor = mount(hubContent({ calculations }));
    const element = editor.view.dom.querySelector('[data-type="pageList"]');
    expect(JSON.parse(element?.getAttribute("data-calculations") ?? "null")).toEqual(calculations);
    const copy = mount(editor.getHTML());
    expect(attrsOf(copy).calculations).toEqual(calculations);
    editor.destroy();
    copy.destroy();

    const plainEditor = mount(hubContent());
    expect(plainEditor.view.dom.querySelector('[data-type="pageList"]')?.hasAttribute("data-calculations")).toBe(false);
    expect(attrsOf(mount(plainEditor.getHTML())).calculations).toBeNull();
    plainEditor.destroy();
  });

  it("reads the HTML leniently: what is not an object is none, an unknown function is kept for a newer version", () => {
    const html = (text: string): string => `<div data-type="pageList" data-layout="table" data-calculations='${text}'></div>`;
    const bad = mount(html("not json"));
    expect(attrsOf(bad).calculations).toEqual({});
    bad.destroy();
    const list = mount(html("[1,2]"));
    expect(attrsOf(list).calculations).toEqual({});
    list.destroy();
    const newer = mount(html('{"a":"future_function","b":3}'));
    expect(attrsOf(newer).calculations).toEqual({ a: "future_function" });
    newer.destroy();
  });
});

/** The `calculations` the Loro document stores for its first view. */
function storedCalculationsOf(doc: LoroDoc): unknown {
  const root = loroToPortable(doc) as { content: { type: string; attrs?: { calculations?: unknown } }[] };
  return root.content.find((node) => node.type === "pageList")?.attrs?.calculations;
}

// ----- an older editor ----------------------------------------------------------------------

describe("an editor from before this amendment", () => {
  it("shows the table without calculations, keeps the attribute while it only reads, and drops it when it changes the block", async () => {
    // The schema of an older editor: the list view without the attribute.
    const Older = PageList.extend({
      addAttributes() {
        const { calculations: ignored, ...rest } = (this.parent?.() ?? {}) as Record<string, unknown>;
        void ignored;
        return rest;
      },
    });
    const older = getSchema(createSchemaExtensions().map((extension) => (extension.name === "pageList" ? Older : extension)));
    expect(older.nodes.pageList?.spec.attrs).not.toHaveProperty("calculations");

    const calculations = { title: "count_all", [PROP.priority]: "sum" };
    const doc = loroDocFrom(hubContent({ calculations }));
    const view = bindView(older, doc);
    await tick();
    let position = -1;
    view.view.state.doc.descendants((node, pos) => {
      if (node.type.name === "pageList") {
        position = pos;
      }
      return position < 0;
    });
    expect(position).toBeGreaterThan(-1);
    expect(view.view.state.doc.nodeAt(position)?.attrs).not.toHaveProperty("calculations");
    expect(storedCalculationsOf(doc)).toEqual(calculations); // the document still has it

    // Reading, and editing other blocks, leave it alone.
    view.view.dispatch(view.view.state.tr.insertText("!", view.view.state.doc.content.size - 1));
    expect(storedCalculationsOf(doc)).toEqual(calculations);

    // A change of the block's own attributes by the older editor rewrites it without what it does not hold.
    view.view.dispatch(view.view.state.tr.setNodeAttribute(position, "layout", "list"));
    expect(storedCalculationsOf(doc)).toBeUndefined();
    view.destroy();
  });
});
