// "Export CSV" of a list, table or board view (ADR-0036, section 2.8): the rows and columns sent to the core are
// exactly the ones the view shows (after its filters and sort; a board's cards column by column, hidden columns
// left out; the title first, then the properties it shows), saved as `<page title>.csv`. The menu is hidden
// without the shell's save dialog, disabled without rows; a cancelled dialog says nothing, a failure shows in
// the block.

import { CoreError } from "@atomik-kb/core-client";
import type { CsvColumn, JsonValue, PageInfo, PropertyDef, Request, SeedSpace, Transport } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { boardSelection, csvColumns, csvFileName, listSelection } from "./editor/csv-export";
import { NO_GROUP, resolveGroup } from "./editor/page-board-model";
import { t } from "./i18n";
import { pid } from "./test/editor";
import { renderInterop } from "./test/interop";
import { idsOf, openPage } from "./test/page";
import { openPanel } from "./test/page-list";
import type { User } from "./test/render";

afterEach(() => {
  vi.restoreAllMocks();
});

const PROP = {
  cost: "de000000-0000-4000-8000-00000000c001",
  status: "de000000-0000-4000-8000-00000000c002",
};

const CSV_TEXT = "\uFEFFName,Cost\r\nbeta,10\r\n";

const COST: PropertyDef = { id: PROP.cost, name: "Cost", kind: "number", options: [], createdAt: 0 };
const STATUS: PropertyDef = {
  id: PROP.status,
  name: "Status",
  kind: "status",
  options: [
    { id: "o1", name: "To do", color: "gray", group: "todo" },
    { id: "o2", name: "Done", color: "green", group: "done" },
  ],
  createdAt: 0,
};

function page(id: string, title: string): PageInfo {
  return { id, spaceId: "s", title, createdAt: 0, updatedAt: 0, properties: [] };
}

function hub(attrs: { [key: string]: JsonValue }): JsonValue {
  return {
    type: "doc",
    content: [
      {
        type: "pageList",
        attrs: {
          id: pid(1),
          source: { kind: "children" },
          layout: "table",
          properties: [PROP.cost, PROP.status],
          sort: { by: "title", dir: "asc" },
          ...attrs,
        },
      },
      { type: "paragraph", attrs: { id: pid(2) } },
    ],
  };
}

/** Hub holds the view; its children: Alpha (3, To do), beta (10, Done), Gamma (nothing), delta (2, To do). */
function space(content: JsonValue, withChildren = true): SeedSpace {
  return {
    name: "Personal",
    properties: [
      { id: PROP.cost, name: "Cost", kind: "number" },
      { id: PROP.status, name: "Status", kind: "status" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: withChildren
          ? [
              { title: "Alpha", properties: { Cost: 3, Status: "Not started" } },
              { title: "beta", properties: { Cost: 10, Status: "Done" } },
              { title: "Gamma" },
              { title: "delta", properties: { Cost: 2, Status: "Not started" } },
            ]
          : [],
      },
    ],
  };
}

type SaveTextFile = NonNullable<Transport["saveTextFile"]>;

interface Options {
  save?: SaveTextFile | null;
  withChildren?: boolean;
}

async function openHub(content: JsonValue, { save, withChildren }: Options = {}) {
  const saveTextFile =
    save === null
      ? undefined
      : (save ?? vi.fn<SaveTextFile>((name) => Promise.resolve(`C:\\Users\\Ada\\${name}`)));
  const ctx = await renderInterop(
    { spaces: [space(content, withChildren)] },
    {
      methods: saveTextFile === undefined ? {} : { saveTextFile },
      intercept: (request) => (request.type === "ExportCsv" ? { type: "Csv", data: CSV_TEXT } : undefined),
    },
  );
  const editor = await openPage(ctx.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const ids = await idsOf(ctx.memory);
  return { ...ctx, editor, ids, saveTextFile };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

/** Opens the view settings menu of the block, where the CSV export is, and returns its Export CSV entry. */
async function exportEntry(user: User): Promise<HTMLElement> {
  const menu = await openPanel(user, block(), "settings");
  return within(menu).getByRole("button", { name: t("pageList.exportCsv") });
}

async function exportCsv(user: User): Promise<void> {
  await user.click(await exportEntry(user));
}

/** The request the core got for the CSV. */
function asked(requests: readonly Request[]): { spaceId: string; pageIds: string[]; columns: CsvColumn[] } {
  const request = requests.find((candidate) => candidate.type === "ExportCsv");
  if (request?.type !== "ExportCsv") {
    throw new Error("no ExportCsv request");
  }
  return request.data;
}

/** The pages of the table in the order it draws its rows. */
function tableOrder(editor: Editor): string[] {
  return Array.from(editor.view.dom.querySelectorAll<HTMLElement>(".page-table-body [role='row']")).map((row) => row.dataset.page ?? "");
}

describe("the rows and columns", () => {
  it("of a table are the rows after its filter and sort, and its columns in order", async () => {
    const { user, editor, ids, requests, saveTextFile } = await openHub(
      hub({
        sort: { by: PROP.cost, dir: "desc" },
        filters: [{ propertyId: PROP.cost, op: "isSet" }],
        properties: [PROP.status, PROP.cost],
      }),
    );
    // Gamma has no Cost, so the filter hides it; the rest go from the dearest to the cheapest.
    await waitFor(() => {
      expect(tableOrder(editor)).toEqual([ids.page("beta"), ids.page("Alpha"), ids.page("delta")]);
    });
    await exportCsv(user);

    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledTimes(1);
    });
    expect(asked(requests)).toEqual({
      spaceId: ids.space("Personal"),
      pageIds: [ids.page("beta"), ids.page("Alpha"), ids.page("delta")],
      columns: [
        { type: "Title" },
        { type: "Property", data: { propertyId: PROP.status } },
        { type: "Property", data: { propertyId: PROP.cost } },
      ],
    });
    expect(saveTextFile).toHaveBeenCalledWith("Hub.csv", CSV_TEXT);
    expect(await screen.findByRole("log", { name: t("toast.label") })).toHaveTextContent("Saved Hub.csv.");
  });

  it("of a table with calculations are the same: the footer (ADR-0040) is not a row and has no column of its own", async () => {
    const { user, editor, ids, requests, saveTextFile } = await openHub(
      hub({ calculations: { title: "count_all", [PROP.cost]: "sum", [PROP.status]: "count_unique" } }),
    );
    await waitFor(() => {
      expect(tableOrder(editor)).toHaveLength(4);
    });
    // The footer is there, and shows the values.
    expect(within(block()).getByRole("gridcell", { name: t("pageTable.calc.cell", { name: "Cost" }) })).toHaveTextContent("15");
    await exportCsv(user);
    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledTimes(1);
    });
    expect(asked(requests)).toEqual({
      spaceId: ids.space("Personal"),
      pageIds: [ids.page("Alpha"), ids.page("beta"), ids.page("delta"), ids.page("Gamma")], // by title, as the table draws them
      columns: [{ type: "Title" }, { type: "Property", data: { propertyId: PROP.cost } }, { type: "Property", data: { propertyId: PROP.status } }],
    });
    expect(saveTextFile).toHaveBeenCalledWith("Hub.csv", CSV_TEXT);
  });

  it("of a list are the rows it draws, with only the properties it shows", async () => {
    const { user, ids, requests, saveTextFile } = await openHub(
      hub({ layout: "list", sort: { by: "title", dir: "desc" }, properties: [PROP.cost] }),
    );
    const links = await within(block()).findAllByRole("button", { name: /^(Alpha|beta|Gamma|delta)/ });
    expect(links.map((link) => link.textContent)).toEqual(["Gamma", "delta", "beta", "Alpha"]);
    await exportCsv(user);
    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledTimes(1);
    });
    expect(asked(requests).pageIds).toEqual([ids.page("Gamma"), ids.page("delta"), ids.page("beta"), ids.page("Alpha")]);
    expect(asked(requests).columns).toEqual([{ type: "Title" }, { type: "Property", data: { propertyId: PROP.cost } }]);
  });

  it("of a board are its cards column by column, with the hidden columns left out", async () => {
    const { user, editor, ids, requests, saveTextFile } = await openHub(
      hub({ layout: "board", groupBy: PROP.status, hiddenGroups: [NO_GROUP], properties: [PROP.cost] }),
    );
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll(".page-board-card").length).toBeGreaterThan(0);
    });
    // Gamma has no status: its column is hidden, and so is the card.
    const cards = Array.from(editor.view.dom.querySelectorAll<HTMLElement>(".page-board-card")).map((card) => card.dataset.page);
    expect(cards).toHaveLength(3);
    expect(cards).not.toContain(ids.page("Gamma"));
    await exportCsv(user);
    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledTimes(1);
    });
    expect(asked(requests).pageIds).toEqual(cards);
    expect(asked(requests).columns).toEqual([{ type: "Title" }, { type: "Property", data: { propertyId: PROP.cost } }]);
  });

  it("of a board that shows the column of the pages without a value start with them", async () => {
    const { user, editor, ids, requests, saveTextFile } = await openHub(
      hub({ layout: "board", groupBy: PROP.status, properties: [] }),
    );
    await waitFor(() => {
      expect(editor.view.dom.querySelectorAll(".page-board-card")).toHaveLength(4);
    });
    const cards = Array.from(editor.view.dom.querySelectorAll<HTMLElement>(".page-board-card")).map((card) => card.dataset.page);
    expect(cards[0]).toBe(ids.page("Gamma"));
    await exportCsv(user);
    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledTimes(1);
    });
    expect(asked(requests).pageIds).toEqual(cards);
    expect(asked(requests).columns).toEqual([{ type: "Title" }]);
  });
});

describe("the view settings menu", () => {
  it("does not offer Export CSV where the shell cannot save a file", async () => {
    const { user } = await openHub(hub({}), { save: null });
    const menu = await openPanel(user, block(), "settings");
    expect(within(menu).queryByRole("button", { name: t("pageList.exportCsv") })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("group", { name: t("pageList.actions") })).not.toBeInTheDocument();
    // The rest of the menu is there all the same.
    expect(within(menu).getByRole("combobox", { name: t("pageList.layout.label") })).toBeInTheDocument();
  });

  it("holds Export CSV in a group of its own, below the settings", async () => {
    const { user } = await openHub(hub({}));
    const menu = await openPanel(user, block(), "settings");
    const group = within(menu).getByRole("group", { name: t("pageList.actions") });
    expect(within(group).getByRole("button", { name: t("pageList.exportCsv") })).toBeEnabled();
    const openIn = within(menu).getByRole("combobox", { name: t("pageList.openIn.label") });
    expect(openIn.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("has Export CSV disabled, with the reason, when the view shows no rows", async () => {
    const { user, saveTextFile } = await openHub(hub({}), { withChildren: false });
    const entry = await exportEntry(user);
    expect(entry).toBeDisabled();
    expect(entry).toHaveAttribute("title", t("pageList.exportCsv.empty"));
    expect(saveTextFile).not.toHaveBeenCalled();
  });

  it("has Export CSV disabled for a board with no property to group by", async () => {
    const { user } = await openHub(hub({ layout: "board" }));
    expect(await exportEntry(user)).toBeDisabled();
  });

  it("says nothing when the save dialog is cancelled", async () => {
    const save = vi.fn<SaveTextFile>(() =>
      Promise.reject(new CoreError({ code: "InvalidInput", message: "no file was chosen", details: { field: "path" } })),
    );
    const { user } = await openHub(hub({}), { save });
    await exportCsv(user);
    await waitFor(() => {
      expect(save).toHaveBeenCalledTimes(1);
    });
    expect(within(block()).queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("log", { name: t("toast.label") })).toBeEmptyDOMElement();
  });

  it("shows why the file could not be saved, in the block", async () => {
    const save = vi.fn<SaveTextFile>(() => Promise.reject(new CoreError({ code: "Storage", message: "disk full" })));
    const { user } = await openHub(hub({}), { save });
    await exportCsv(user);
    expect(await within(block()).findByRole("alert")).toHaveTextContent(t("pageList.exportCsv.error", { message: "disk full" }));
  });
});

describe("the pure rules", () => {
  it("put the title first, then one column per property shown", () => {
    expect(csvColumns([])).toEqual([{ type: "Title" }]);
    expect(csvColumns([STATUS, COST])).toEqual([
      { type: "Title" },
      { type: "Property", data: { propertyId: PROP.status } },
      { type: "Property", data: { propertyId: PROP.cost } },
    ]);
    expect(listSelection([page("a", "A"), page("b", "B")], [COST])).toEqual({
      pageIds: ["a", "b"],
      columns: [{ type: "Title" }, { type: "Property", data: { propertyId: PROP.cost } }],
    });
  });

  it("list a board's cards by column, the pages without a value first, and none without a group", () => {
    const withStatus = (id: string, option: string): PageInfo => ({
      ...page(id, id),
      properties: [[PROP.status, { kind: "status", value: option }]],
    });
    const rows = [withStatus("a", "o2"), page("b", "b"), withStatus("c", "o1"), withStatus("d", "o2")];
    const group = resolveGroup(PROP.status, [STATUS]);
    expect(boardSelection(rows, group, [], [])?.pageIds).toEqual(["b", "c", "a", "d"]);
    expect(boardSelection(rows, group, ["o1"], [])?.pageIds).toEqual(["b", "a", "d"]);
    expect(boardSelection(rows, group, [NO_GROUP, "o2"], [COST])?.pageIds).toEqual(["c"]);
    expect(boardSelection(rows, resolveGroup(null, [STATUS]), [], [])).toBeNull();
    expect(boardSelection(rows, resolveGroup("gone", [STATUS]), [], [])).toBeNull();
  });

  it("name the file after the page, with what a file name cannot hold turned into spaces", () => {
    expect(csvFileName("Tasks")).toBe("Tasks.csv");
    expect(csvFileName("  Q3: plan / budget?  ")).toBe("Q3 plan budget.csv");
    expect(csvFileName('a<b>c"d|e*f\\g')).toBe("a b c d e f g.csv");
    expect(csvFileName("Trailing dots...")).toBe("Trailing dots.csv");
    expect(csvFileName("")).toBe(`${t("page.untitled")}.csv`);
    expect(csvFileName("???")).toBe(`${t("page.untitled")}.csv`);
    expect(csvFileName("x".repeat(300))).toBe(`${"x".repeat(100)}.csv`);
    expect(csvFileName("Café ☕ notes")).toBe("Café ☕ notes.csv");
  });
});
