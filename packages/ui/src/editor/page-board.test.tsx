// The board layout of the list view block (ADR-0016, amendment of 2026-10-08, the board): the pure
// rules (reading `groupBy` and `hiddenGroups`, the columns of a property, which column a page is
// in), the HTML form, the layout switch, the group picker, the columns and their counts, filters and
// sort inside a column, moving cards by drag and drop and by the "Move to…" menu, "New" in a column,
// "Add group", hidden columns, a read-only editor, a group property that is gone or of the wrong
// kind, and that nothing is written on render.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, PageInfo, PropertyDef, SeedSpace } from "@atomik-kb/core-client";
import { Editor } from "@tiptap/core";
import type { Content } from "@tiptap/core";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { openPanel, viewSelect } from "../test/page-list";
import type { User } from "../test/render";
import { loroSyncPluginKey } from "./loro-binding";
import { boardColumns, columnOf, groupRows, NO_GROUP, readGroupBy, readHiddenGroups, resolveGroup, valueForColumn } from "./page-board-model";
import { readLayout } from "./page-list-model";
import { K, kindsHub, kindsSpace } from "../test/view-kinds";
import { createEditorSchema, createSchemaExtensions } from "./schema";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const PROP = {
  status: "de000000-0000-4000-8000-00000000b001",
  stage: "de000000-0000-4000-8000-00000000b002",
  priority: "de000000-0000-4000-8000-00000000b003",
  areas: "de000000-0000-4000-8000-00000000b004",
};

const DEFAULTS = {
  source: { kind: "children" },
  layout: "board",
  properties: [] as string[],
  sort: { by: "title", dir: "asc" },
};

/**
 * A view that opens a page as a page: the side peek is the default (ADR-0054) and `openIn` is a part of `views`, which the older
 * attributes cannot say. For the tests that follow a click on a row to the page it opens.
 */
function asPages(extra: { [key: string]: JsonValue } = {}): { [key: string]: JsonValue } {
  return { views: [{ id: "v1", name: "", layout: "board", properties: [], sort: { by: "title", dir: "asc" }, groupBy: PROP.stage, openIn: "page", ...extra }] };
}

function paragraph(n: number, value: string): JsonValue {
  return value === ""
    ? { type: "paragraph", attrs: { id: pid(n) } }
    : { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [paragraph(1, "Intro"), { type: "pageList", attrs: { id: pid(2), ...DEFAULTS, ...list } }, paragraph(3, "")],
  };
}

/** Hub holds the board; its children: Alpha (In progress, Draft), beta (Done, Review), Gamma (no values), delta (Not started, Draft). */
function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    properties: [
      { id: PROP.status, name: "Status", kind: "status" },
      {
        id: PROP.stage,
        name: "Stage",
        kind: "select",
        options: [
          { name: "Review", color: "orange" },
          { name: "Draft", color: "gray" },
          { name: "Later", color: "pink" },
        ],
      },
      { id: PROP.priority, name: "Priority", kind: "number" },
      { id: PROP.areas, name: "Areas", kind: "multi_select", options: [{ name: "Home" }] },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", properties: { Status: "In progress", Stage: "Draft", Priority: 3 } },
          { title: "beta", properties: { Status: "Done", Stage: "Review", Priority: 10 } },
          { title: "Gamma" },
          { title: "delta", properties: { Status: "Not started", Stage: "Draft", Priority: 2 } },
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

async function openHub(content: JsonValue = hubContent()) {
  const app = await renderPages([space(content)], { now: ticking() });
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const client = new CoreClient(app.memory);
  const ids = await idsOf(app.memory);
  const spaceId = ids.space("Personal");
  const defs = await client.propertySchemaGet(spaceId);
  return { ...app, editor, client, ids, spaceId, defs };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

function board(): HTMLElement {
  return within(block()).getByRole("group", { name: t("pageBoard.label") });
}

function column(name: string): HTMLElement {
  return within(board()).getByRole("group", { name });
}

/** The column names, in order. */
function columnNames(): string[] {
  return Array.from(board().querySelectorAll(":scope > .page-board-columns > .page-board-column")).map(
    (element) => element.getAttribute("aria-label") ?? "",
  );
}

/** The titles of the cards of a column, in order. */
function cards(name: string): string[] {
  return Array.from(column(name).querySelectorAll(".page-board-card .page-list-name")).map((element) => element.textContent ?? "");
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

async function stored(client: CoreClient, pageId: string, propertyId: string) {
  return (await client.pagePropertiesGet(pageId)).find(([id]) => id === propertyId)?.[1];
}

function optionId(defs: readonly PropertyDef[], propertyId: string, name: string): string {
  const id = defs.find((def) => def.id === propertyId)?.options.find((option) => option.name === name)?.id;
  if (id === undefined) {
    throw new Error(`no option ${name}`);
  }
  return id;
}

/** A drag and drop of the card of `title` onto the column `name`, with a minimal DataTransfer. */
function dragCard(title: string, name: string): void {
  const data = new Map<string, string>();
  const dataTransfer = {
    setData: (type: string, value: string) => data.set(type, value),
    getData: (type: string) => data.get(type) ?? "",
    types: [] as string[],
    effectAllowed: "all",
    dropEffect: "none",
  };
  const card = Array.from(document.querySelectorAll(".page-board-card")).find(
    (element) => element.querySelector(".page-list-name")?.textContent === title,
  );
  if (card === undefined) {
    throw new Error(`no card ${title}`);
  }
  fireEvent.dragStart(card, { dataTransfer });
  fireEvent.dragOver(column(name), { dataTransfer });
  fireEvent.drop(column(name), { dataTransfer });
  fireEvent.dragEnd(card, { dataTransfer });
}

const NONE_STAGE = "No Stage";
const NONE_STATUS = "No Status";

// ----- the rules ---------------------------------------------------------------------------

describe("reading the attributes", () => {
  it("reads the layout board, and anything else as the list", () => {
    expect(readLayout("board")).toBe("board");
    for (const odd of ["timeline", "", null, undefined, 3]) {
      expect(readLayout(odd), JSON.stringify(odd)).toBe("list");
    }
  });

  it("reads groupBy as a non-empty text and hiddenGroups as a list of texts without repeats", () => {
    expect(readGroupBy("abc")).toBe("abc");
    for (const odd of ["", null, undefined, 3, {}, ["a"]]) {
      expect(readGroupBy(odd), JSON.stringify(odd)).toBeNull();
    }
    expect(readHiddenGroups(["a", "none", "a", "", 3, null])).toEqual(["a", "none"]);
    for (const odd of ["a", {}, null, undefined, 3]) {
      expect(readHiddenGroups(odd), JSON.stringify(odd)).toEqual([]);
    }
  });
});

describe("the columns", () => {
  const def = (kind: PropertyDef["kind"], options: PropertyDef["options"]): PropertyDef => ({ id: "p", name: "P", kind, options, createdAt: 0 });
  const page = (id: string, option?: string, kind: "select" | "status" = "select"): PageInfo =>
    ({ id, title: id, properties: option === undefined ? undefined : [["p", { kind, value: option }]] }) as unknown as PageInfo;

  it("are the pages without a value, then the options in order", () => {
    const select = def("select", [
      { id: "b", name: "B", color: "gray" },
      { id: "a", name: "A", color: "gray" },
    ]);
    expect(boardColumns(select).map((column) => column.key)).toEqual([NO_GROUP, "b", "a"]);
  });

  it("put a status's options in the order of their groups", () => {
    const status = def("status", [
      { id: "d", name: "D", color: "gray", group: "done" },
      { id: "t", name: "T", color: "gray", group: "todo" },
      { id: "i", name: "I", color: "gray", group: "in_progress" },
    ]);
    const columns = boardColumns(status);
    expect(columns.map((column) => column.key)).toEqual([NO_GROUP, "t", "i", "d"]);
    expect(columns.map((column) => column.group)).toEqual([null, "todo", "in_progress", "done"]);
  });

  it("place a page by its option, and one without a value or with an option the property lacks in the first column", () => {
    const select = def("select", [{ id: "a", name: "A", color: "gray" }]);
    expect(columnOf(page("x", "a"), select)).toBe("a");
    expect(columnOf(page("x"), select)).toBe(NO_GROUP);
    expect(columnOf(page("x", "gone"), select)).toBe(NO_GROUP);
    const groups = groupRows([page("x", "a"), page("y"), page("z", "a")], select);
    expect([...groups.entries()].map(([key, rows]) => [key, rows.map((row) => row.id)])).toEqual([
      [NO_GROUP, ["y"]],
      ["a", ["x", "z"]],
    ]);
  });

  it("give the value that puts a page in a column, null for the first", () => {
    const select = def("select", [{ id: "a", name: "A", color: "gray" }]);
    expect(valueForColumn(select, "a")).toEqual({ kind: "select", value: "a" });
    expect(valueForColumn(def("status", [{ id: "a", name: "A", color: "gray", group: "todo" }]), "a")).toEqual({ kind: "status", value: "a" });
    expect(valueForColumn(select, NO_GROUP)).toBeNull();
  });

  it("resolve the group property: none, gone, of a kind that cannot group, or fine", () => {
    const defs = [def("select", []), { ...def("number", []), id: "n" }];
    expect(resolveGroup(null, defs)).toEqual({ kind: "none" });
    expect(resolveGroup("zz", defs)).toEqual({ kind: "missing", id: "zz" });
    expect(resolveGroup("n", defs).kind).toBe("wrongKind");
    expect(resolveGroup("p", defs).kind).toBe("ok");
  });
});

describe("the schema and the HTML form", () => {
  it("lists groupBy and hiddenGroups after columnWidths, calculations (2026-10-14) and views (ADR-0054) last", () => {
    const schema = createEditorSchema();
    expect(Object.keys(schema.nodes.pageList?.spec.attrs ?? {})).toEqual([
      "id",
      "source",
      "layout",
      "properties",
      "sort",
      "filters",
      "columnWidths",
      "groupBy",
      "hiddenGroups",
      "calculations",
      "views",
    ]);
  });

  it("writes them as attributes when set, leaves them out when not, and reads them back", () => {
    const editor = new Editor({
      extensions: createSchemaExtensions(),
      content: hubContent({ groupBy: PROP.stage, hiddenGroups: ["none"] }) as Content,
    });
    expect(editor.getHTML()).toContain(`data-group-by="${PROP.stage}"`);
    expect(editor.getHTML()).toContain("data-hidden-groups");
    const copy = new Editor({ extensions: createSchemaExtensions(), content: editor.getHTML() });
    let attrs: Record<string, unknown> = {};
    copy.state.doc.descendants((node) => {
      if (node.type.name === "pageList") {
        attrs = node.attrs;
      }
      return true;
    });
    expect(attrs.groupBy).toBe(PROP.stage);
    expect(attrs.hiddenGroups).toEqual(["none"]);
    const bare = new Editor({ extensions: createSchemaExtensions(), content: hubContent() as Content });
    expect(bare.getHTML()).not.toContain("data-group-by");
    expect(bare.getHTML()).not.toContain("data-hidden-groups");
    editor.destroy();
    copy.destroy();
    bare.destroy();
  });
});

// ----- the view ----------------------------------------------------------------------------

describe("the layout and the group picker", () => {
  it("switch to the board writing the layout alone, which asks for a group property", async () => {
    const { user, editor } = await openHub(hubContent({ layout: "list" }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageList.layout.label")), t("pageList.layout.board"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("board");
    });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"layout"');
    expect(within(block()).getByText(t("pageBoard.choose"))).toBeInTheDocument();
    expect(within(block()).queryByRole("group", { name: t("pageBoard.label") })).not.toBeInTheDocument();
    // The picker lists the select and status properties only, and nothing is preselected in the document.
    const picker = await select(user, t("pageBoard.groupBy"));
    expect(Array.from(picker.options).map((option) => option.textContent)).toEqual([t("pageBoard.groupBy.choose"), "Status", "Stage"]);
    expect(attrsOf(editor).groupBy).toBeNull();
  });

  it("write the group property as one attribute and show the columns", async () => {
    const { user, editor } = await openHub();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await select(user, t("pageBoard.groupBy")), "Stage");
    await waitFor(() => {
      expect(attrsOf(editor).groupBy).toBe(PROP.stage);
    });
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"groupBy"');
    expect(columnNames()).toEqual([NONE_STAGE, "Review", "Draft", "Later"]);
  });

  it("show an unknown layout as the list", async () => {
    await openHub(hubContent({ layout: "timeline" }));
    expect(within(block()).getAllByRole("listitem")).toHaveLength(4);
  });
});

describe("the columns of a board", () => {
  it("are one per option in order after the No column, each with its pill and count, the cards sorted by title", async () => {
    await openHub(hubContent({ groupBy: PROP.stage }));
    expect(columnNames()).toEqual([NONE_STAGE, "Review", "Draft", "Later"]);
    expect(cards(NONE_STAGE)).toEqual(["Gamma"]);
    expect(cards("Review")).toEqual(["beta"]);
    expect(cards("Draft")).toEqual(["Alpha", "delta"]);
    expect(cards("Later")).toEqual([]);
    expect(within(column("Draft")).getByLabelText(t("pageBoard.count", { count: 2 }))).toHaveTextContent("2");
    expect(column("Draft").querySelector(".option-pill")).toHaveTextContent("Draft");
    expect(column(NONE_STAGE).querySelector(".option-pill")).toBeNull();
  });

  it("show a status in its groups, with the group named", async () => {
    await openHub(hubContent({ groupBy: PROP.status }));
    expect(columnNames()).toEqual([NONE_STATUS, "Not started", "In progress", "Done"]);
    expect(column("Not started").querySelector(".page-board-group")).toHaveTextContent(t("properties.group.todo"));
    expect(column("Done").querySelector(".page-board-group")).toHaveTextContent(t("properties.group.done"));
    expect(cards("In progress")).toEqual(["Alpha"]);
  });

  it("respect the filters and the sort of the block inside each column", async () => {
    await openHub(
      hubContent({
        groupBy: PROP.stage,
        sort: { by: PROP.priority, dir: "desc" },
        filters: [{ propertyId: PROP.priority, op: "isSet" }],
      }),
    );
    expect(cards("Draft")).toEqual(["Alpha", "delta"]);
    expect(cards(NONE_STAGE)).toEqual([]);
    expect(within(column("Draft")).getByLabelText(t("pageBoard.count", { count: 2 }))).toHaveTextContent("2");
  });

  it("show the properties of the block on the cards, read-only", async () => {
    await openHub(hubContent({ groupBy: PROP.stage, properties: [PROP.priority] }));
    const card = column("Review").querySelector(".page-board-card");
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByRole("group", { name: "Priority" })).toHaveTextContent("10");
  });

  it("open a page from its card", async () => {
    const { user } = await openHub(hubContent(asPages()));
    await user.click(within(column("Review")).getByRole("button", { name: "beta" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("beta");
    });
  });

  it("fold the No column without writing anything", async () => {
    const { user, editor } = await openHub(hubContent({ groupBy: PROP.stage }));
    const doc = docOf(editor);
    await user.click(within(column(NONE_STAGE)).getByRole("button", { name: t("pageBoard.collapse", { name: NONE_STAGE }) }));
    expect(cards(NONE_STAGE)).toEqual([]);
    await user.click(within(column(NONE_STAGE)).getByRole("button", { name: t("pageBoard.expand", { name: NONE_STAGE }) }));
    expect(cards(NONE_STAGE)).toEqual(["Gamma"]);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
  });
});

describe("moving cards", () => {
  it("by drag and drop sets the value through the core", async () => {
    const { client, ids, defs } = await openHub(hubContent({ groupBy: PROP.stage }));
    dragCard("Gamma", "Later");
    await waitFor(() => {
      expect(cards("Later")).toEqual(["Gamma"]);
    });
    expect(await stored(client, ids.page("Gamma"), PROP.stage)).toEqual({ kind: "select", value: optionId(defs, PROP.stage, "Later") });
    expect(cards(NONE_STAGE)).toEqual([]);
  });

  it("onto the No column clears the value", async () => {
    const { client, ids } = await openHub(hubContent({ groupBy: PROP.stage }));
    dragCard("beta", NONE_STAGE);
    await waitFor(() => {
      expect(cards(NONE_STAGE)).toEqual(["beta", "Gamma"]);
    });
    expect(await stored(client, ids.page("beta"), PROP.stage)).toBeUndefined();
  });

  it("writes a status value for a status board, and nothing for a drop where the card already is", async () => {
    const { client, ids, defs } = await openHub(hubContent({ groupBy: PROP.status }));
    dragCard("Alpha", "In progress");
    dragCard("Alpha", "Done");
    await waitFor(() => {
      expect(cards("Done")).toEqual(["Alpha", "beta"]);
    });
    expect(await stored(client, ids.page("Alpha"), PROP.status)).toEqual({ kind: "status", value: optionId(defs, PROP.status, "Done") });
  });

  it("with the Move to… menu of a card, which lists the other columns", async () => {
    const { user, client, ids, defs } = await openHub(hubContent({ groupBy: PROP.stage }));
    const card = Array.from(column("Draft").querySelectorAll(".page-board-card")).find((c) => c.textContent?.includes("Alpha")) as HTMLElement;
    await user.click(within(card).getByRole("button", { name: t("pageBoard.cardMenu", { title: "Alpha" }) }));
    const menu = await screen.findByRole("menu", { name: t("pageBoard.moveMenu", { title: "Alpha" }) });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([NONE_STAGE, "Review", "Later"]);
    await user.click(within(menu).getByRole("menuitem", { name: "Review" }));
    await waitFor(() => {
      expect(cards("Review")).toEqual(["Alpha", "beta"]);
    });
    expect(await stored(client, ids.page("Alpha"), PROP.stage)).toEqual({ kind: "select", value: optionId(defs, PROP.stage, "Review") });
  });

  it("writes no attribute of the block", async () => {
    const { editor } = await openHub(hubContent({ groupBy: PROP.stage }));
    const doc = docOf(editor);
    dragCard("Gamma", "Later");
    await waitFor(() => {
      expect(cards("Later")).toEqual(["Gamma"]);
    });
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
  });
});

describe("New in a column", () => {
  it("makes a page under the page with the column's value, and opens it", async () => {
    const { user, client, ids, spaceId, defs } = await openHub(hubContent(asPages()));
    await user.click(within(column("Review")).getByRole("button", { name: t("pageBoard.newIn", { name: "Review" }) }));
    await waitFor(() => {
      expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
    });
    const hub = (await client.treeGet(spaceId)).roots.find((node) => node.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(hub?.children).toHaveLength(5);
    expect(await stored(client, created?.id ?? "", PROP.stage)).toEqual({ kind: "select", value: optionId(defs, PROP.stage, "Review") });
  });

  it("in the No column sets no value", async () => {
    const { user, client, ids, spaceId } = await openHub(hubContent(asPages()));
    await user.click(within(column(NONE_STAGE)).getByRole("button", { name: t("pageBoard.newIn", { name: NONE_STAGE }) }));
    await waitFor(() => {
      expect(screen.queryByRole("group", { name: t("pageList.label") })).not.toBeInTheDocument();
    });
    const hub = (await client.treeGet(spaceId)).roots.find((node) => node.page.id === ids.page("Hub"));
    expect(await client.pagePropertiesGet(hub?.children.at(-1)?.page.id ?? "")).toEqual([]);
  });
});

describe("Add group", () => {
  it("creates an option of the property, which becomes a column", async () => {
    const { user, client, spaceId } = await openHub(hubContent({ groupBy: PROP.stage }));
    await user.click(within(board()).getByRole("button", { name: t("pageBoard.addGroup") }));
    await user.type(within(board()).getByRole("textbox", { name: t("pageBoard.addGroup.name") }), "Shipped{Enter}");
    await waitFor(() => {
      expect(columnNames()).toEqual([NONE_STAGE, "Review", "Draft", "Later", "Shipped"]);
    });
    const stage = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.stage);
    expect(stage?.options.map((option) => option.name)).toEqual(["Review", "Draft", "Later", "Shipped"]);
    expect(stage?.options[3]?.color).not.toBe(stage?.options[2]?.color);
  });

  it("refuses a name that is taken and creates nothing", async () => {
    const { user, client, spaceId } = await openHub(hubContent({ groupBy: PROP.stage }));
    await user.click(within(board()).getByRole("button", { name: t("pageBoard.addGroup") }));
    await user.type(within(board()).getByRole("textbox", { name: t("pageBoard.addGroup.name") }), "Draft{Enter}");
    expect(await within(block()).findByRole("alert")).toHaveTextContent("Draft");
    const stage = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.stage);
    expect(stage?.options).toHaveLength(3);
  });

  it("adds a status option in the to-do group", async () => {
    const { user, client, spaceId } = await openHub(hubContent({ groupBy: PROP.status }));
    await user.click(within(board()).getByRole("button", { name: t("pageBoard.addGroup") }));
    await user.type(within(board()).getByRole("textbox", { name: t("pageBoard.addGroup.name") }), "Blocked{Enter}");
    await waitFor(() => {
      expect(columnNames()).toContain("Blocked");
    });
    const status = (await client.propertySchemaGet(spaceId)).find((def) => def.id === PROP.status);
    expect(status?.options.find((option) => option.name === "Blocked")?.group).toBe("todo");
  });
});

describe("hidden columns", () => {
  it("are left out of the board and listed in the view settings menu, whose Columns checkboxes write hiddenGroups alone", async () => {
    const { user, editor, defs } = await openHub(hubContent({ groupBy: PROP.stage, hiddenGroups: ["none"] }));
    expect(columnNames()).toEqual(["Review", "Draft", "Later"]);
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    const menu = await openPanel(user, block(), "settings");
    const panel = within(menu).getByRole("group", { name: t("pageBoard.groups.label") });
    expect(within(panel).getByRole("checkbox", { name: NONE_STAGE })).not.toBeChecked();
    await user.click(within(panel).getByRole("checkbox", { name: "Later" }));
    await waitFor(() => {
      expect(attrsOf(editor).hiddenGroups).toEqual(["none", optionId(defs, PROP.stage, "Later")]);
    });
    expect(columnNames()).toEqual(["Review", "Draft"]);
    const changes = changesSince(doc, before);
    expect(changes).toHaveLength(1);
    expect(changes[0]).toHaveLength(1);
    expect(JSON.stringify(changes[0])).toContain('"hiddenGroups"');
    await user.click(within(panel).getByRole("checkbox", { name: NONE_STAGE }));
    await waitFor(() => {
      expect(columnNames()).toEqual([NONE_STAGE, "Review", "Draft"]);
    });
  });
});

describe("a group property the board cannot use", () => {
  it("that the space no longer has says so, shows no columns and writes nothing", async () => {
    const { editor, memory, ids } = await openHub(hubContent({ groupBy: "de000000-0000-4000-8000-00000000eeee" }));
    expect(within(block()).getByText(t("pageBoard.groupGone"))).toBeInTheDocument();
    expect(within(block()).queryByRole("group", { name: t("pageBoard.label") })).not.toBeInTheDocument();
    const doc = docOf(editor);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    expect(await contentOf(memory, ids.page("Hub"))).toEqual(hubContent({ groupBy: "de000000-0000-4000-8000-00000000eeee" }));
  });

  it("of the wrong kind says so and writes nothing", async () => {
    const { user, editor } = await openHub(hubContent({ groupBy: PROP.priority }));
    expect(within(block()).getByText(t("pageBoard.groupKind", { name: "Priority" }))).toBeInTheDocument();
    expect(within(block()).queryByRole("group", { name: t("pageBoard.label") })).not.toBeInTheDocument();
    const doc = docOf(editor);
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
    // The picker still lets the person choose another.
    expect((await select(user, t("pageBoard.groupBy"))).value).toBe("");
  });
});

describe("a read-only editor", () => {
  it("shows the board and offers no drag, menu, New, Add group or toolbar, but opens a page", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const content = hubContent(asPages());
    (content as { content: JsonValue[] }).content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
    const { user } = await openHub(content);
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(columnNames()).toEqual([NONE_STAGE, "Review", "Draft", "Later"]);
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    expect(within(board()).queryByRole("button", { name: t("pageBoard.addGroup") })).not.toBeInTheDocument();
    expect(within(board()).queryByRole("button", { name: /^New page in/u })).not.toBeInTheDocument();
    expect(within(board()).queryByRole("button", { name: /^Menu of/u })).not.toBeInTheDocument();
    for (const card of Array.from(board().querySelectorAll(".page-board-card"))) {
      expect(card.getAttribute("draggable")).not.toBe("true");
    }
    await user.click(within(column("Review")).getByRole("button", { name: "beta" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("beta");
    });
  });
});

describe("the document", () => {
  it("records no change when the board is shown and the tree changes", async () => {
    const { editor, client, ids, spaceId } = await openHub(hubContent({ groupBy: PROP.stage, hiddenGroups: ["x"] }));
    const doc = docOf(editor);
    await client.pageCreate({ spaceId, parentId: ids.page("Hub"), title: "New one" });
    await waitFor(() => {
      expect(cards(NONE_STAGE)).toContain("New one");
    });
    expect(doc.oplogVersion().get(doc.peerIdStr) ?? 0).toBe(0);
  });
});

// ----- the kinds of ADR-0026 (amendment of 2026-10-08) and ADR-0027 -----------------------------

describe("the new property kinds on a board", () => {
  async function openKinds(attrs: { [key: string]: JsonValue } = {}) {
    const app = await renderPages([kindsSpace(kindsHub({ layout: "board", groupBy: K.stage, ...attrs }))], { now: ticking() });
    await openPage(app.user, "Hub");
    await screen.findAllByRole("group", { name: t("pageList.label") });
    return app;
  }

  function prop(card: string, name: string): HTMLElement | null {
    const item = within(board()).getByRole("button", { name: card }).closest("li");
    return item === null ? null : within(item).queryByRole("group", { name });
  }

  it("shows the new kinds on the cards: a place, the times, a device, a relation and a rollup", async () => {
    await openKinds();
    expect(prop("Cafe", "Where")).toHaveTextContent("Café de Flore");
    expect(prop("Cafe", "Where")?.querySelector(".page-list-text")).toHaveAttribute("title", "48.854, 2.333");
    expect(prop("Cafe", "Created")?.textContent).not.toBe("");
    expect(prop("Cafe", "Made by")?.textContent).not.toMatch(/^[0-9a-f]{8}-/u);
    const chips = Array.from(prop("Zoo", "Tasks")?.querySelectorAll(".relation-chip-title") ?? []).map((node) => node.textContent);
    expect(chips).toEqual(["Ship", "Write"]);
    expect(prop("Zoo", "Total")).toHaveTextContent("7.5");
    expect(prop("Zoo", "Names")?.querySelectorAll(".rollup-item")).toHaveLength(2);
    // The cards stay in the column of their Stage.
    expect(cards("Draft")).toEqual(["Cafe"]);
    expect(cards("Review")).toEqual(["Zoo"]);
  });

  it("sorts the cards of a column by a place (the ones without a value last)", async () => {
    await openKinds({ sort: { by: K.where, dir: "asc" } });
    expect(cards("No Stage")).toEqual(["Write", "Plain", "Ship"]);
  });

  it("groups by a select or status only: a relation, a place or a rollup cannot", () => {
    for (const property of [K.tasks, K.where, K.total, K.created]) {
      const def = {
        id: property,
        name: "x",
        kind: property === K.tasks ? "relation" : property === K.where ? "place" : property === K.total ? "rollup" : "created_time",
        options: [],
        createdAt: 0,
      } as PropertyDef;
      expect(resolveGroup(property, [def]).kind).toBe("wrongKind");
    }
  });
});
