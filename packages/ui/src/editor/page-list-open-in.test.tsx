// "Open pages in" of a view (ADR-0054, `docs/architecture/database-views.md` sections 4 and 8): the view settings menu's select (side peek, center
// peek, full page) for every layout, written as a part of the view, and every click on a row, card or title of the list, the
// table and the board, and every "New page", going through `openRow`: a plain click follows the view's `openIn`, the gestures that
// ask for another tab keep their tabs, and the page that holds the database opens as a page. The tests replace the store's
// `openPeek`, `closePeek` and `navigateTo` with spies once the page is open, so they hold whatever the peek and tabs slices do.

import { CoreClient, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import { t } from "../i18n";
import type { AppState, AppStore } from "../store";
import type * as StoreModule from "../store";
import { pid } from "../test/editor";
import { idsOf, openPage, renderPages } from "../test/page";
import { viewSelect } from "../test/page-list";
import type { User } from "../test/render";
import { loroSyncPluginKey } from "./loro-binding";
import { parseViews } from "./page-list-views";

// The app creates its store inside `<App>`; the tests reach it through this record.
const created = vi.hoisted((): { stores: AppStore[] } => ({ stores: [] }));

vi.mock("../store", async (importOriginal) => {
  const actual = await importOriginal<typeof StoreModule>();
  return {
    ...actual,
    createAppStore: (...args: Parameters<typeof actual.createAppStore>) => {
      const store = actual.createAppStore(...args);
      created.stores.push(store);
      return store;
    },
  };
});

afterEach(() => {
  created.stores.length = 0;
  vi.restoreAllMocks();
});

function appStore(): AppStore {
  const store = created.stores.at(-1);
  if (store === undefined) {
    throw new Error("the app has not created its store");
  }
  return store;
}

interface Spies {
  navigateTo: Mock<AppState["navigateTo"]>;
  openPeek: Mock<AppState["openPeek"]>;
  closePeek: Mock<AppState["closePeek"]>;
}

/** Replaces the actions a click on a row can end in with spies; call it after the page the test needs is open. */
function spy(): Spies {
  const spies: Spies = { navigateTo: vi.fn(), openPeek: vi.fn(), closePeek: vi.fn() };
  appStore().setState({ ...spies });
  return spies;
}

// ----- the space -----------------------------------------------------------------------------

const PROP = { stage: "de000000-0000-4000-8000-00000000c006" };
const TAG = { idea: "de000000-0000-4000-8000-00000000d001" };

const LEGACY = { source: { kind: "children" }, layout: "list", properties: [] as string[], sort: { by: "title", dir: "asc" } };

function stored(layout: string, openIn?: string, extra: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    id: "k1",
    name: "",
    layout,
    properties: [],
    sort: { by: "title", dir: "asc" },
    ...(layout === "board" ? { groupBy: PROP.stage } : {}),
    ...(openIn === undefined ? {} : { openIn }),
    ...extra,
  };
}

/** The block of Hub with one view of `layout` that opens pages as `openIn` (the default when left out). */
function hubContent(layout: string, openIn?: string, list: { [key: string]: JsonValue } = {}, readOnly = false): JsonValue {
  const content: JsonValue[] = [
    { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
    {
      type: "pageList",
      attrs: { id: pid(2), ...LEGACY, layout, ...(layout === "board" ? { groupBy: PROP.stage } : {}), views: [stored(layout, openIn)], ...list },
    },
    { type: "paragraph", attrs: { id: pid(3) } },
  ];
  if (readOnly) {
    content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
  }
  return { type: "doc", content };
}

function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
    tagOptions: [{ id: TAG.idea, name: "idea", color: "green" }],
    properties: [
      {
        id: PROP.stage,
        name: "Stage",
        kind: "select",
        options: [
          { name: "Draft", color: "gray" },
          { name: "Review", color: "orange" },
        ],
      },
    ],
    pages: [
      {
        title: "Hub",
        content,
        tags: ["idea"],
        children: [
          { title: "Alpha", properties: { Stage: "Draft" }, tags: ["idea"] },
          { title: "Beta", properties: { Stage: "Review" } },
        ],
      },
    ],
  };
}

async function openHub(content: JsonValue) {
  const app = await renderPages([space(content)]);
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const ids = await idsOf(app.memory);
  const target = (title: string) => ({ pageId: ids.page(title), spaceId: ids.space("Personal") });
  return { ...app, editor, ids, target };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

/** The "Open pages in" select, in the view settings menu of the toolbar (opened when it is not). */
function openInSelect(user: User): Promise<HTMLSelectElement> {
  return viewSelect(user, block(), t("pageList.openIn.label"));
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

/** The rows' open controls: the list's title button, the table's open button, the board's card button. */
const OPENERS: Record<string, (title: string) => HTMLElement> = {
  list: (title) => within(within(block()).getByRole("list", { name: t("pageList.rows") })).getByRole("button", { name: title }),
  table: (title) => within(block()).getByRole("button", { name: t("pageTable.open", { title }) }),
  board: (title) => within(within(block()).getByRole("group", { name: t("pageBoard.label") })).getByRole("button", { name: title }),
};

// ----- the select ------------------------------------------------------------------------------

describe("the \"Open pages in\" select", () => {
  it("offers the side peek, the center peek and the full page, the side peek for a view that has not chosen", async () => {
    const { user } = await openHub(hubContent("list"));
    expect(Array.from((await openInSelect(user)).options).map((option) => [option.value, option.textContent])).toEqual([
      ["side", t("pageList.openIn.side")],
      ["center", t("pageList.openIn.center")],
      ["page", t("pageList.openIn.page")],
    ]);
    expect(await openInSelect(user)).toHaveValue("side");
  });

  it("shows what the view stores", async () => {
    const { user } = await openHub(hubContent("table", "center"));
    expect(await openInSelect(user)).toHaveValue("center");
  });

  it.each(["list", "table", "board", "calendar", "gallery"])("is in the view settings menu of the %s layout", async (layout) => {
    const { user } = await openHub(hubContent(layout));
    expect(await openInSelect(user)).toBeInTheDocument();
  });

  it("writes `openIn` as a part of the view, with the mirror, in one change: a block without views is upgraded by it", async () => {
    const legacy = hubContent("list");
    const attrs = (legacy as { content: { attrs: Record<string, unknown> }[] }).content[1]?.attrs ?? {};
    delete attrs.views;
    const { user, editor } = await openHub(legacy);
    expect(attrsOf(editor).views).toBeNull();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await openInSelect(user), t("pageList.openIn.center"));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[0]?.openIn).toBe("center");
    });
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    expect(Array.isArray(attrsOf(editor).views)).toBe(true);
    expect(attrsOf(editor)).toMatchObject({ layout: "list", source: { kind: "children" } });
    expect(await openInSelect(user)).toHaveValue("center");
    await user.selectOptions(await openInSelect(user), t("pageList.openIn.page"));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[0]?.openIn).toBe("page");
    });
  });

  it("changes the active view's setting only", async () => {
    const content = hubContent("list");
    const attrs = (content as { content: { attrs: Record<string, unknown> }[] }).content[1]?.attrs ?? {};
    attrs.views = [stored("list", "page"), { ...(stored("table") as object), id: "k2" }];
    const { user, editor } = await openHub(content);
    await user.click(within(block()).getByRole("tab", { name: t("pageList.layout.table") }));
    expect(await openInSelect(user)).toHaveValue("side");
    await user.selectOptions(await openInSelect(user), t("pageList.openIn.center"));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor)).map((view) => view.openIn)).toEqual(["page", "center"]);
    });
  });

  it("is not offered in a read-only editor (the toolbar is not)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openHub(hubContent("list", "center", {}, true));
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(within(block()).queryByRole("combobox", { name: t("pageList.openIn.label") })).not.toBeInTheDocument();
  });
});

// ----- a click on a row ------------------------------------------------------------------------

describe.each(["list", "table", "board"])("a click on a row of the %s layout", (layout) => {
  it("opens the side peek by default, and nothing in the tabs", async () => {
    const { user, target } = await openHub(hubContent(layout));
    const spies = spy();
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    expect(spies.openPeek).toHaveBeenCalledTimes(1);
    expect(spies.openPeek).toHaveBeenCalledWith({ ...target("Beta"), mode: "side" });
    expect(spies.navigateTo).not.toHaveBeenCalled();
  });

  it("opens the center peek when the view says so", async () => {
    const { user, target } = await openHub(hubContent(layout, "center"));
    const spies = spy();
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    expect(spies.openPeek).toHaveBeenCalledWith({ ...target("Beta"), mode: "center" });
    expect(spies.navigateTo).not.toHaveBeenCalled();
  });

  it("opens the page in the tab in front for \"Full page\", closing the peek", async () => {
    const { user, target } = await openHub(hubContent(layout, "page"));
    const spies = spy();
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    expect(spies.openPeek).not.toHaveBeenCalled();
    expect(spies.closePeek).toHaveBeenCalled();
    expect(spies.navigateTo).toHaveBeenCalledWith({ ...target("Beta"), disposition: "current" });
  });

  it("keeps the tab gestures whatever the view says: Ctrl or Cmd, a middle click and Ctrl+Shift", async () => {
    const { user, target } = await openHub(hubContent(layout, "side"));
    const spies = spy();
    await user.keyboard("{Control>}");
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    await user.keyboard("{/Control}");
    expect(spies.navigateTo).toHaveBeenLastCalledWith({ ...target("Beta"), disposition: "background" });
    fireEvent(OPENERS[layout]?.("Beta") ?? document.body, new MouseEvent("auxclick", { bubbles: true, cancelable: true, button: 1 }));
    expect(spies.navigateTo).toHaveBeenLastCalledWith({ ...target("Beta"), disposition: "background" });
    await user.keyboard("{Control>}{Shift>}");
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    await user.keyboard("{/Shift}{/Control}");
    expect(spies.navigateTo).toHaveBeenLastCalledWith({ ...target("Beta"), disposition: "foreground" });
    expect(spies.openPeek).not.toHaveBeenCalled();
  });

  it("still opens a page in a read-only editor", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user, target } = await openHub(hubContent(layout, undefined, {}, true));
    await screen.findByText(t("editor.readOnly"));
    const spies = spy();
    await user.click(OPENERS[layout]?.("Beta") ?? document.body);
    expect(spies.openPeek).toHaveBeenCalledWith({ ...target("Beta"), mode: "side" });
  });
});

describe("the page that holds the database", () => {
  it("opens as a page, not in a peek (a tag source can list it)", async () => {
    const { user, target } = await openHub(hubContent("list", "side", { source: { kind: "tag", optionId: TAG.idea } }));
    await within(block()).findByRole("button", { name: "Hub" });
    const spies = spy();
    await user.click(OPENERS.list?.("Hub") ?? document.body);
    expect(spies.openPeek).not.toHaveBeenCalled();
    expect(spies.navigateTo).toHaveBeenCalledWith({ ...target("Hub"), disposition: "current" });
    // Another page of the same list is peeked.
    await user.click(OPENERS.list?.("Alpha") ?? document.body);
    expect(spies.openPeek).toHaveBeenCalledWith({ ...target("Alpha"), mode: "side" });
  });
});

describe("the keyboard in a read-only table", () => {
  it("opens the page of the active title cell with Enter, in a tab in front with Ctrl+Enter", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user, target } = await openHub(hubContent("table", undefined, {}, true));
    await screen.findByText(t("editor.readOnly"));
    const spies = spy();
    const cell = within(within(block()).getByRole("grid")).getAllByRole("gridcell")[0];
    expect(cell).toBeDefined();
    cell?.focus();
    await user.keyboard("{Enter}");
    expect(spies.openPeek).toHaveBeenCalledWith({ ...target("Alpha"), mode: "side" });
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(spies.navigateTo).toHaveBeenLastCalledWith({ ...target("Alpha"), disposition: "foreground" });
  });
});

// ----- New page --------------------------------------------------------------------------------

describe("New page", () => {
  it("makes the page under the page and opens it like a row: in the side peek by default, in the tabs for Full page", async () => {
    const { user, memory, ids } = await openHub(hubContent("list"));
    const client = new CoreClient(memory);
    const spies = spy();
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(spies.openPeek).toHaveBeenCalledTimes(1);
    });
    const hub = (await client.treeGet(ids.space("Personal"))).roots.find((node) => node.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(hub?.children).toHaveLength(3);
    expect(spies.openPeek).toHaveBeenCalledWith({ pageId: created?.id, spaceId: ids.space("Personal"), mode: "side" });
    expect(spies.navigateTo).not.toHaveBeenCalled();
  });

  it("opens the new page as a page when the view says Full page, with the tag of a tag source", async () => {
    const { user, memory, ids } = await openHub(hubContent("table", "page", { source: { kind: "tag", optionId: TAG.idea } }));
    const client = new CoreClient(memory);
    await within(block()).findByRole("button", { name: t("pageTable.open", { title: "Alpha" }) });
    const spies = spy();
    await user.click(within(block()).getByRole("button", { name: t("pageList.newPage") }));
    await waitFor(() => {
      expect(spies.navigateTo).toHaveBeenCalledTimes(1);
    });
    const hub = (await client.treeGet(ids.space("Personal"))).roots.find((node) => node.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(spies.navigateTo).toHaveBeenCalledWith({ pageId: created?.id, spaceId: ids.space("Personal"), disposition: "current" });
    expect(await client.pagePropertiesGet(created?.id ?? "")).toEqual([[TAGS_PROPERTY_ID, { kind: "multi_select", value: [TAG.idea] }]]);
    expect(spies.openPeek).not.toHaveBeenCalled();
  });

  it("makes a board card in the column with the column's value and opens it in the peek", async () => {
    const { user, memory, ids } = await openHub(hubContent("board", "center"));
    const client = new CoreClient(memory);
    const spies = spy();
    await user.click(within(block()).getByRole("button", { name: t("pageBoard.newIn", { name: "Review" }) }));
    await waitFor(() => {
      expect(spies.openPeek).toHaveBeenCalledTimes(1);
    });
    const hub = (await client.treeGet(ids.space("Personal"))).roots.find((node) => node.page.id === ids.page("Hub"));
    const created = hub?.children.at(-1)?.page;
    expect(spies.openPeek).toHaveBeenCalledWith({ pageId: created?.id, spaceId: ids.space("Personal"), mode: "center" });
    const values = await client.pagePropertiesGet(created?.id ?? "");
    expect(values.find(([id]) => id === PROP.stage)?.[1]).toMatchObject({ kind: "select" });
  });
});
