// The tab strip of a database block's views (ADR-0054, `docs/architecture/database-views.md` section 7): one tab per view with
// its layout's icon and name, a "+" that adds a view, a menu per tab (rename, duplicate, change layout, move, delete), the
// limits, the keyboard of a tab list, read-only editors, the active view being this device's choice (nothing written to the page
// by a tab switch), and an older editor that does not know `views`. The strip is tested on its own with props first, then
// through the block in an editor.

import { CoreClient, loroToPortable } from "@atomik-kb/core-client";
import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import { Editor, getSchema } from "@tiptap/core";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { LoroDoc } from "loro-crdt";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t } from "../i18n";
import { bindView, loroDocFrom, pid, tick } from "../test/editor";
import { idsOf, openPage, renderPages } from "../test/page";
import { viewSelect } from "../test/page-list";
import { ACTIVE_VIEWS_KEY } from "../store-views";
import { loroSyncPluginKey } from "./loro-binding";
import { PageList } from "./page-list";
import { MAX_VIEWS, parseViews } from "./page-list-views";
import type { PageListViewSpec } from "./page-list-views";
import { PageListViewTabs } from "./PageListViewTabs";
import { createSchemaExtensions } from "./schema";

afterEach(() => {
  vi.restoreAllMocks();
});

// ----- the strip on its own ----------------------------------------------------------------

function view(id: string, layout: PageListViewSpec["layout"], name = ""): PageListViewSpec {
  return { id, name, layout, properties: [], sort: { by: "title", dir: "asc" }, sorts: [{ by: "title", dir: "asc" }], filter: { op: "and", items: [] } };
}

function handlers() {
  return {
    onSelect: vi.fn(),
    onAdd: vi.fn(),
    onRename: vi.fn(),
    onDuplicate: vi.fn(),
    onChangeLayout: vi.fn(),
    onMove: vi.fn(),
    onDelete: vi.fn(),
  };
}

function strip(views: readonly PageListViewSpec[], options: { activeId?: string; editable?: boolean } = {}) {
  const actions = handlers();
  const user = userEvent.setup();
  const utils = render(
    <PageListViewTabs views={views} activeId={options.activeId ?? views[0]?.id ?? ""} editable={options.editable ?? true} {...actions} />,
  );
  return { ...utils, ...actions, user };
}

const tabNamed = (name: string): HTMLElement => screen.getByRole("tab", { name });
const menuButton = (name: string): HTMLElement => screen.getByRole("button", { name: t("pageList.view.menu", { name }) });

describe("the strip", () => {
  it("is a tab list with one tab per view: the layout's name for an unnamed one, the view's own name otherwise, the active one selected", () => {
    strip([view("a", "table"), view("b", "board", "By status"), view("c", "calendar")], { activeId: "b" });
    expect(screen.getByRole("tablist", { name: t("pageList.views.label") })).toBeInTheDocument();
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([t("pageList.layout.table"), "By status", t("pageList.layout.calendar")]);
    expect(tabNamed("By status")).toHaveAttribute("aria-selected", "true");
    expect(tabNamed(t("pageList.layout.table"))).toHaveAttribute("aria-selected", "false");
    // Each tab shows the icon of its layout.
    for (const tab of screen.getAllByRole("tab")) {
      expect(tab.querySelector("svg")).not.toBeNull();
    }
  });

  it("has one tab stop, the active tab, and a click selects a tab", async () => {
    const { user, onSelect } = strip([view("a", "list"), view("b", "table")], { activeId: "a" });
    expect(tabNamed(t("pageList.layout.list"))).toHaveAttribute("tabindex", "0");
    expect(tabNamed(t("pageList.layout.table"))).toHaveAttribute("tabindex", "-1");
    await user.click(tabNamed(t("pageList.layout.table")));
    expect(onSelect).toHaveBeenCalledWith("b");
  });

  it("moves focus with Left, Right, Home and End (wrapping), and Enter or Space select the focused tab", async () => {
    const { user, onSelect } = strip([view("a", "list"), view("b", "table"), view("c", "board")], { activeId: "a" });
    tabNamed(t("pageList.layout.list")).focus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed(t("pageList.layout.table"))).toHaveFocus();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(tabNamed(t("pageList.layout.list"))).toHaveFocus(); // wrapped
    await user.keyboard("{ArrowLeft}");
    expect(tabNamed(t("pageList.layout.board"))).toHaveFocus();
    await user.keyboard("{Home}");
    expect(tabNamed(t("pageList.layout.list"))).toHaveFocus();
    await user.keyboard("{End}");
    expect(tabNamed(t("pageList.layout.board"))).toHaveFocus();
    expect(onSelect).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenLastCalledWith("c");
    await user.keyboard("{ArrowLeft}");
    await user.keyboard(" ");
    expect(onSelect).toHaveBeenLastCalledWith("b");
  });

  it("keeps the tab stop where focus went while the same view stays active", async () => {
    const { user } = strip([view("a", "list"), view("b", "table")], { activeId: "a" });
    tabNamed(t("pageList.layout.list")).focus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed(t("pageList.layout.table"))).toHaveAttribute("tabindex", "0");
    expect(tabNamed(t("pageList.layout.list"))).toHaveAttribute("tabindex", "-1");
  });
});

describe("the \"+\" button", () => {
  it("opens a menu of the seven layouts and adds a view of the one chosen", async () => {
    const { user, onAdd } = strip([view("a", "list")]);
    await user.click(screen.getByRole("button", { name: t("pageList.views.add") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.views.addMenu") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("pageList.layout.list"),
      t("pageList.layout.table"),
      t("pageList.layout.board"),
      t("pageList.layout.calendar"),
      t("pageList.layout.gallery"),
      t("pageList.layout.feed"),
      t("pageList.layout.dashboard"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: t("pageList.layout.board") }));
    expect(onAdd).toHaveBeenCalledWith("board");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });

  it("is disabled at 20 views, with the message", () => {
    const views = Array.from({ length: MAX_VIEWS }, (_, at) => view(`v${String(at)}`, "list"));
    strip(views);
    const add = screen.getByRole("button", { name: t("pageList.views.add") });
    expect(add).toBeDisabled();
    expect(add).toHaveAttribute("title", t("pageList.views.limit", { max: MAX_VIEWS }));
    expect(screen.getByRole("status")).toHaveTextContent(t("pageList.views.limit", { max: MAX_VIEWS }));
  });

  it("is enabled below the limit, with no message", () => {
    strip(Array.from({ length: MAX_VIEWS - 1 }, (_, at) => view(`v${String(at)}`, "list")));
    expect(screen.getByRole("button", { name: t("pageList.views.add") })).toBeEnabled();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
});

describe("the menu of a tab", () => {
  const three = (): PageListViewSpec[] => [view("a", "list"), view("b", "table", "Tasks"), view("c", "board")];

  it("lists rename, duplicate, change layout, move left, move right and delete, from the button", async () => {
    const { user } = strip(three());
    await user.click(menuButton("Tasks"));
    const menu = await screen.findByRole("menu", { name: t("pageList.view.menu", { name: "Tasks" }) });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("pageList.view.rename"),
      t("pageList.view.duplicate"),
      t("pageList.view.changeLayout"),
      t("pageList.view.moveLeft"),
      t("pageList.view.moveRight"),
      t("pageList.view.delete"),
    ]);
    for (const item of within(menu).getAllByRole("menuitem")) {
      expect(item).toBeEnabled();
    }
  });

  it("also opens from the context menu key, Shift+F10 and a right click", async () => {
    const { user } = strip(three());
    tabNamed("Tasks").focus();
    await user.keyboard("{ContextMenu}");
    expect(await screen.findByRole("menu", { name: t("pageList.view.menu", { name: "Tasks" }) })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    await user.keyboard("{Shift>}{F10}{/Shift}");
    expect(await screen.findByRole("menu", { name: t("pageList.view.menu", { name: "Tasks" }) })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    fireEvent.contextMenu(tabNamed(t("pageList.layout.board")), { clientX: 40, clientY: 40 });
    expect(await screen.findByRole("menu", { name: t("pageList.view.menu", { name: t("pageList.layout.board") }) })).toBeInTheDocument();
  });

  it("disables Move left on the first tab and Move right on the last", async () => {
    const { user } = strip(three());
    await user.click(menuButton(t("pageList.layout.list")));
    expect(await screen.findByRole("menuitem", { name: t("pageList.view.moveLeft") })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: t("pageList.view.moveRight") })).toBeEnabled();
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    });
    await user.click(menuButton(t("pageList.layout.board")));
    expect(await screen.findByRole("menuitem", { name: t("pageList.view.moveRight") })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: t("pageList.view.moveLeft") })).toBeEnabled();
  });

  it("moves, duplicates and deletes the view of the tab, not the active one", async () => {
    const { user, onMove, onDuplicate, onDelete } = strip(three(), { activeId: "a" });
    await user.click(menuButton("Tasks"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.moveRight") }));
    expect(onMove).toHaveBeenCalledWith("b", 1);
    await user.click(menuButton("Tasks"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.moveLeft") }));
    expect(onMove).toHaveBeenLastCalledWith("b", -1);
    await user.click(menuButton("Tasks"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.duplicate") }));
    expect(onDuplicate).toHaveBeenCalledWith("b");
    await user.click(menuButton("Tasks"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.delete") }));
    expect(onDelete).toHaveBeenCalledWith("b");
  });

  it("never deletes the last view: the entry is disabled and says why", async () => {
    const { user, onDelete } = strip([view("a", "list")]);
    await user.click(menuButton(t("pageList.layout.list")));
    const entry = await screen.findByRole("menuitem", { name: t("pageList.view.delete") });
    expect(entry).toBeDisabled();
    expect(entry).toHaveAttribute("title", t("pageList.view.deleteLast"));
    await user.click(entry);
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("disables Duplicate at 20 views, with the message", async () => {
    const views = Array.from({ length: MAX_VIEWS }, (_, at) => view(`v${String(at)}`, "list", `View ${String(at)}`));
    const { user, onDuplicate } = strip(views);
    await user.click(menuButton("View 3"));
    const entry = await screen.findByRole("menuitem", { name: t("pageList.view.duplicate") });
    expect(entry).toBeDisabled();
    expect(entry).toHaveAttribute("title", t("pageList.views.limit", { max: MAX_VIEWS }));
    await user.click(entry);
    expect(onDuplicate).not.toHaveBeenCalled();
  });

  it("changes the layout through a second menu that disables the layout the view has", async () => {
    const { user, onChangeLayout } = strip(three());
    await user.click(menuButton("Tasks"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.changeLayout") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.view.changeLayout") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("pageList.layout.list"),
      t("pageList.layout.table"),
      t("pageList.layout.board"),
      t("pageList.layout.calendar"),
      t("pageList.layout.gallery"),
      t("pageList.layout.feed"),
      t("pageList.layout.dashboard"),
    ]);
    expect(within(menu).getByRole("menuitem", { name: t("pageList.layout.table") })).toBeDisabled();
    await user.click(within(menu).getByRole("menuitem", { name: t("pageList.layout.gallery") }));
    expect(onChangeLayout).toHaveBeenCalledWith("b", "gallery");
  });
});

describe("renaming a tab", () => {
  const views = (): PageListViewSpec[] => [view("a", "list"), view("b", "table", "Tasks")];

  async function startRename(user: ReturnType<typeof userEvent.setup>, name: string): Promise<HTMLInputElement> {
    await user.click(menuButton(name));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.rename") }));
    return screen.findByRole<HTMLInputElement>("textbox", { name: t("pageList.view.renameLabel") });
  }

  it("is an inline field with the name in it, focused and selected; Enter writes the trimmed name", async () => {
    const { user, onRename } = strip(views());
    const field = await startRename(user, "Tasks");
    expect(field).toHaveFocus();
    expect(field).toHaveValue("Tasks");
    await user.keyboard("  Work  {Enter}");
    expect(onRename).toHaveBeenCalledWith("b", "Work");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(tabNamed("Tasks")).toHaveFocus(); // the (unchanged here) tab takes focus back
  });

  it("restores the layout's name when the field is emptied, which the placeholder says", async () => {
    const { user, onRename } = strip(views());
    const field = await startRename(user, "Tasks");
    expect(field).toHaveAttribute("placeholder", t("pageList.layout.table"));
    await user.clear(field);
    await user.keyboard("{Enter}");
    expect(onRename).toHaveBeenCalledWith("b", "");
  });

  it("writes nothing for Escape, for the same name, and writes once when the field is left", async () => {
    const { user, onRename } = strip(views());
    await user.type(await startRename(user, "Tasks"), "x{Escape}");
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    await startRename(user, "Tasks");
    await user.keyboard("{Enter}");
    expect(onRename).not.toHaveBeenCalled();

    const field = await startRename(user, "Tasks");
    await user.type(field, "!");
    await user.click(document.body);
    expect(onRename).toHaveBeenCalledTimes(1);
    expect(onRename).toHaveBeenCalledWith("b", "Tasks!");
  });

  it("starts with F2 and a double click, and cuts the name at 80 characters", async () => {
    const { user } = strip(views());
    tabNamed("Tasks").focus();
    await user.keyboard("{F2}");
    const field = await screen.findByRole<HTMLInputElement>("textbox", { name: t("pageList.view.renameLabel") });
    expect(field).toHaveAttribute("maxlength", "80");
    await user.keyboard("{Escape}");
    await user.dblClick(tabNamed("Tasks"));
    expect(await screen.findByRole("textbox", { name: t("pageList.view.renameLabel") })).toBeInTheDocument();
  });
});

describe("a read-only strip", () => {
  it("lets the tabs switch and offers nothing else: no \"+\", no menu button, no context menu, no rename", async () => {
    const { user, onSelect } = strip([view("a", "list"), view("b", "table", "Tasks")], { editable: false });
    expect(screen.queryByRole("button", { name: t("pageList.views.add") })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Menu of the view/u })).not.toBeInTheDocument();
    await user.click(tabNamed("Tasks"));
    expect(onSelect).toHaveBeenCalledWith("b");
    tabNamed("Tasks").focus();
    await user.keyboard("{ContextMenu}{F2}");
    await user.dblClick(tabNamed("Tasks"));
    fireEvent.contextMenu(tabNamed("Tasks"));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    // The arrow keys still move between the tabs.
    tabNamed(t("pageList.layout.list")).focus();
    await user.keyboard("{ArrowRight}");
    expect(tabNamed("Tasks")).toHaveFocus();
  });
});

// ----- through the block in an editor ----------------------------------------------------------

const PROP = { stage: "de000000-0000-4000-8000-00000000c006" };

const LEGACY = { source: { kind: "children" }, layout: "list", properties: [] as string[], sort: { by: "title", dir: "asc" } };

function stored(id: string, layout: string, extra: { [key: string]: JsonValue } = {}): JsonValue {
  return { id, name: "", layout, properties: [], sort: { by: "title", dir: "asc" }, ...extra };
}

function hubContent(list: { [key: string]: JsonValue } = {}, readOnly = false): JsonValue {
  const content: JsonValue[] = [
    { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
    { type: "pageList", attrs: { id: pid(2), ...LEGACY, ...list } },
    { type: "paragraph", attrs: { id: pid(3) } },
  ];
  if (readOnly) {
    // A block of a newer version makes the editor read-only (the binding refuses to edit what it cannot keep).
    content.push({ type: "kanban", attrs: { id: "k0", columns: 3 } });
  }
  return { type: "doc", content };
}

function space(content: JsonValue): SeedSpace {
  return {
    name: "Personal",
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
    pages: [{ title: "Hub", content, children: [{ title: "Alpha", properties: { Stage: "Draft" } }, { title: "Beta", properties: { Stage: "Review" } }] }],
  };
}

async function openHub(content: JsonValue = hubContent(), settings: Record<string, JsonValue> = {}) {
  const app = await renderPages([space(content)], { settings });
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  return { ...app, editor, client: new CoreClient(app.memory), ids: await idsOf(app.memory) };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

function tablist(): HTMLElement {
  return within(block()).getByRole("tablist", { name: t("pageList.views.label") });
}

function tabLabels(): string[] {
  return within(tablist()).getAllByRole("tab").map((tab) => tab.textContent ?? "");
}

function selectedTab(): string {
  return within(tablist()).getByRole("tab", { selected: true }).textContent ?? "";
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

const own = (doc: LoroDoc): number => doc.oplogVersion().get(doc.peerIdStr) ?? 0;

async function menuOf(user: ReturnType<typeof userEvent.setup>, name: string, entry: string): Promise<void> {
  await user.click(within(tablist()).getByRole("button", { name: t("pageList.view.menu", { name }) }));
  await user.click(await screen.findByRole("menuitem", { name: entry }));
}

const TWO_VIEWS = (): { [key: string]: JsonValue } => ({
  layout: "table",
  views: [stored("k1", "table"), stored("k2", "board", { name: "By stage", groupBy: PROP.stage })],
});

describe("the strip in a block", () => {
  it("shows one tab for a block written before views, with the layout's name, and writes nothing", async () => {
    const { editor } = await openHub();
    expect(tabLabels()).toEqual([t("pageList.layout.list")]);
    expect(selectedTab()).toBe(t("pageList.layout.list"));
    expect(within(block()).getByRole("button", { name: t("pageList.views.add") })).toBeEnabled();
    expect(own(docOf(editor))).toBe(0);
    expect(attrsOf(editor).views).toBeNull();
  });

  it("shows a tab per view and draws the first, the active one being this device's choice", async () => {
    const { editor } = await openHub(hubContent(TWO_VIEWS()));
    expect(tabLabels()).toEqual([t("pageList.layout.table"), "By stage"]);
    expect(selectedTab()).toBe(t("pageList.layout.table"));
    expect(within(block()).getByRole("grid")).toBeInTheDocument();
    expect(block()).toHaveAttribute("data-layout", "table");
    expect(own(docOf(editor))).toBe(0);
  });

  it("switches views without writing anything to the page, and remembers the choice in a device setting", async () => {
    const { user, editor, client } = await openHub(hubContent(TWO_VIEWS()));
    const doc = docOf(editor);
    await user.click(within(tablist()).getByRole("tab", { name: "By stage" }));
    expect(selectedTab()).toBe("By stage");
    expect(block()).toHaveAttribute("data-layout", "board");
    expect(within(block()).getByRole("group", { name: t("pageBoard.label") })).toBeInTheDocument();
    expect(within(block()).queryByRole("grid")).not.toBeInTheDocument();
    expect(own(doc)).toBe(0);
    await waitFor(async () => {
      expect(await client.settingsGet(ACTIVE_VIEWS_KEY)).toEqual({ [pid(2)]: "k2" });
    });
    // The toolbar is the active view's: the board's group select is in its view settings menu, the table's calculation footer is not.
    expect(await viewSelect(user, block(), t("pageBoard.groupBy"))).toHaveValue(PROP.stage);
    await user.click(within(tablist()).getByRole("tab", { name: t("pageList.layout.table") }));
    expect(within(block()).getByRole("grid")).toBeInTheDocument();
    expect(own(doc)).toBe(0);
  });

  it("opens on the view this device chose, and on the first when that view is gone", async () => {
    const chosen = await openHub(hubContent(TWO_VIEWS()), { [ACTIVE_VIEWS_KEY]: { [pid(2)]: "k2" } });
    expect(selectedTab()).toBe("By stage");
    expect(block()).toHaveAttribute("data-layout", "board");
    chosen.unmount();

    await openHub(hubContent(TWO_VIEWS()), { [ACTIVE_VIEWS_KEY]: { [pid(2)]: "deleted-elsewhere" } });
    expect(selectedTab()).toBe(t("pageList.layout.table"));
  });

  it("adds a view of the layout chosen, one change that writes `views` and the mirror of the first, and shows it", async () => {
    const { user, editor } = await openHub();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.click(within(block()).getByRole("button", { name: t("pageList.views.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.layout.table") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))).toHaveLength(2);
    });
    const [first, second] = parseViews(attrsOf(editor));
    expect(first).toMatchObject({ id: "default", layout: "list", name: "" });
    expect(second).toMatchObject({ layout: "table", name: "", properties: [], sort: { by: "title", dir: "asc" } });
    expect(second?.id).toMatch(/^[0-9a-f]{8}$/u);
    // The mirror is the first view, and `source` and the block's id are as they were.
    expect(attrsOf(editor)).toMatchObject({ id: pid(2), source: { kind: "children" }, layout: "list", properties: [], sort: { by: "title", dir: "asc" } });
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    // The new view is the one shown, on this device.
    expect(tabLabels()).toEqual([t("pageList.layout.list"), t("pageList.layout.table")]);
    expect(selectedTab()).toBe(t("pageList.layout.table"));
    expect(within(block()).getByRole("grid")).toBeInTheDocument();
  });

  it("adds a gallery or calendar view the same way, and keeps the first view in the mirror", async () => {
    const { user, editor } = await openHub(hubContent({ layout: "table" }));
    await user.click(within(block()).getByRole("button", { name: t("pageList.views.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.layout.calendar") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))).toHaveLength(2);
    });
    expect(parseViews(attrsOf(editor)).map((entry) => entry.layout)).toEqual(["table", "calendar"]);
    expect(attrsOf(editor).layout).toBe("table");
    expect(block()).toHaveAttribute("data-layout", "calendar");
  });

  it("renames a view, and an empty name gives the layout's back", async () => {
    const { user, editor } = await openHub();
    await menuOf(user, t("pageList.layout.list"), t("pageList.view.rename"));
    await user.keyboard("Tasks{Enter}");
    await waitFor(() => {
      expect(tabLabels()).toEqual(["Tasks"]);
    });
    expect(parseViews(attrsOf(editor))[0]).toMatchObject({ id: "default", name: "Tasks", layout: "list" });
    expect(Array.isArray(attrsOf(editor).views)).toBe(true);

    await menuOf(user, "Tasks", t("pageList.view.rename"));
    await user.clear(screen.getByRole("textbox", { name: t("pageList.view.renameLabel") }));
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(tabLabels()).toEqual([t("pageList.layout.list")]);
    });
    expect(parseViews(attrsOf(editor))[0]?.name).toBe("");
  });

  it("duplicates a view right after it, named with (copy), and shows the copy", async () => {
    const { user, editor } = await openHub(hubContent(TWO_VIEWS()));
    await menuOf(user, "By stage", t("pageList.view.duplicate"));
    await waitFor(() => {
      expect(tabLabels()).toEqual([t("pageList.layout.table"), "By stage", t("pageList.view.copyName", { name: "By stage" })]);
    });
    const views = parseViews(attrsOf(editor));
    expect(views.map((entry) => entry.layout)).toEqual(["table", "board", "board"]);
    expect(views[2]).toMatchObject({ layout: "board", groupBy: PROP.stage });
    expect(new Set(views.map((entry) => entry.id)).size).toBe(3);
    expect(selectedTab()).toBe(t("pageList.view.copyName", { name: "By stage" }));

    // The copy of an unnamed view says the layout's name in its name.
    await menuOf(user, t("pageList.layout.table"), t("pageList.view.duplicate"));
    await waitFor(() => {
      expect(tabLabels()).toContain(t("pageList.view.copyName", { name: t("pageList.layout.table") }));
    });
    expect(tabLabels()[1]).toBe(t("pageList.view.copyName", { name: t("pageList.layout.table") }));
  });

  it("changes the layout of a view that is not the active one, keeping its other settings", async () => {
    const { user, editor } = await openHub(hubContent(TWO_VIEWS()));
    await menuOf(user, "By stage", t("pageList.view.changeLayout"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.layout.gallery") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[1]?.layout).toBe("gallery");
    });
    expect(parseViews(attrsOf(editor))[1]).toMatchObject({ name: "By stage", groupBy: PROP.stage });
    expect(selectedTab()).toBe(t("pageList.layout.table")); // the active view did not change
    expect(block()).toHaveAttribute("data-layout", "table");
  });

  it("changes the layout of an unnamed view and its tab follows the new name of the layout", async () => {
    const { user } = await openHub(hubContent(TWO_VIEWS()));
    await menuOf(user, t("pageList.layout.table"), t("pageList.view.changeLayout"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.layout.board") }));
    await waitFor(() => {
      expect(tabLabels()).toEqual([t("pageList.layout.board"), "By stage"]);
    });
    expect(block()).toHaveAttribute("data-layout", "board");
  });

  it("moves a view, and the legacy attributes follow the first one", async () => {
    const { user, editor } = await openHub(hubContent(TWO_VIEWS()));
    await menuOf(user, "By stage", t("pageList.view.moveLeft"));
    await waitFor(() => {
      expect(tabLabels()).toEqual(["By stage", t("pageList.layout.table")]);
    });
    expect(attrsOf(editor)).toMatchObject({ layout: "board", groupBy: PROP.stage });
    await menuOf(user, "By stage", t("pageList.view.moveRight"));
    await waitFor(() => {
      expect(tabLabels()).toEqual([t("pageList.layout.table"), "By stage"]);
    });
    expect(attrsOf(editor)).toMatchObject({ layout: "table", groupBy: null });
  });

  it("deletes a view in one change, shows its neighbour when it was the active one, and never deletes the last", async () => {
    const { user, editor } = await openHub(hubContent(TWO_VIEWS()), { [ACTIVE_VIEWS_KEY]: { [pid(2)]: "k2" } });
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await menuOf(user, "By stage", t("pageList.view.delete"));
    await waitFor(() => {
      expect(tabLabels()).toEqual([t("pageList.layout.table")]);
    });
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    expect(parseViews(attrsOf(editor)).map((entry) => entry.id)).toEqual(["k1"]);
    expect(selectedTab()).toBe(t("pageList.layout.table"));
    expect(within(block()).getByRole("grid")).toBeInTheDocument();

    await user.click(within(tablist()).getByRole("button", { name: t("pageList.view.menu", { name: t("pageList.layout.table") }) }));
    expect(await screen.findByRole("menuitem", { name: t("pageList.view.delete") })).toBeDisabled();
  });

  it("stops at 20 views: the \"+\" and Duplicate are disabled with the message, and what is stored beyond 20 is not read", async () => {
    const views = Array.from({ length: MAX_VIEWS + 3 }, (_, at) => stored(`v${String(at)}`, "list", { name: `View ${String(at)}` }));
    const { user } = await openHub(hubContent({ views }));
    expect(tabLabels()).toHaveLength(MAX_VIEWS);
    expect(within(block()).getByRole("button", { name: t("pageList.views.add") })).toBeDisabled();
    expect(within(block()).getByText(t("pageList.views.limit", { max: MAX_VIEWS }))).toBeInTheDocument();
    await user.click(within(tablist()).getByRole("button", { name: t("pageList.view.menu", { name: "View 0" }) }));
    expect(await screen.findByRole("menuitem", { name: t("pageList.view.duplicate") })).toBeDisabled();
  });

  it("keeps the toolbar of the active view: a control of the second view writes its part of `views` and leaves the first alone", async () => {
    const { user, editor } = await openHub(hubContent(TWO_VIEWS()));
    await user.click(within(tablist()).getByRole("tab", { name: "By stage" }));
    await user.selectOptions(await viewSelect(user, block(), t("pageList.sort.dir")), t("pageList.sort.desc"));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[1]?.sort).toEqual({ by: "title", dir: "desc" });
    });
    expect(parseViews(attrsOf(editor))[0]?.sort).toEqual({ by: "title", dir: "asc" });
    expect(attrsOf(editor).sort).toEqual({ by: "title", dir: "asc" }); // the mirror follows the first view only
  });
});

describe("the strip in a read-only editor", () => {
  it("shows the tabs of several views and lets them switch, offering nothing else and writing nothing", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    const { user, editor } = await openHub(hubContent(TWO_VIEWS(), true));
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(editor.isEditable).toBe(false);
    expect(tabLabels()).toEqual([t("pageList.layout.table"), "By stage"]);
    expect(within(block()).queryByRole("button", { name: t("pageList.views.add") })).not.toBeInTheDocument();
    expect(within(block()).queryByRole("button", { name: /^Menu of the view/u })).not.toBeInTheDocument();
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    await user.click(within(tablist()).getByRole("tab", { name: "By stage" }));
    expect(selectedTab()).toBe("By stage");
    expect(block()).toHaveAttribute("data-layout", "board");
    expect(own(docOf(editor))).toBe(0);
  });

  it("shows no strip for a single view", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    await openHub(hubContent({}, true));
    expect(await screen.findByText(t("editor.readOnly"))).toBeInTheDocument();
    expect(within(block()).queryByRole("tablist")).not.toBeInTheDocument();
  });
});

// ----- an older editor -----------------------------------------------------------------------

/** The attributes of the first list view of a Loro document, as the portable form holds them. */
function storedAttrsOf(doc: LoroDoc): Record<string, unknown> {
  const root = loroToPortable(doc) as { content: { type: string; attrs?: Record<string, unknown> }[] };
  return root.content.find((node) => node.type === "pageList")?.attrs ?? {};
}

describe("an editor from before views", () => {
  it("shows the first view through the mirror, keeps `views` while it only reads, and loses the other views when it changes the block", async () => {
    // The schema of an older editor: the list view without the attribute.
    const Older = PageList.extend({
      addAttributes() {
        const { views: ignored, ...rest } = (this.parent?.() ?? {}) as Record<string, unknown>;
        void ignored;
        return rest;
      },
    });
    const older = getSchema(createSchemaExtensions().map((extension) => (extension.name === "pageList" ? Older : extension)));
    expect(older.nodes.pageList?.spec.attrs).not.toHaveProperty("views");

    // Two views, with the mirror of the first (a table sorted by title with one column).
    const first = stored("k1", "table", { properties: ["p1"] });
    const second = stored("k2", "board", { name: "By stage", groupBy: PROP.stage });
    const doc = loroDocFrom(hubContent({ layout: "table", properties: ["p1"], views: [first, second] }));
    expect(parseViews(storedAttrsOf(doc)).map((entry) => entry.id)).toEqual(["k1", "k2"]);

    const bound = bindView(older, doc);
    await tick();
    let position = -1;
    bound.view.state.doc.descendants((node, pos) => {
      if (node.type.name === "pageList") {
        position = pos;
      }
      return position < 0;
    });
    expect(position).toBeGreaterThan(-1);
    // The older editor's node has the attributes it knows: what it draws is the first view.
    const attrs = bound.view.state.doc.nodeAt(position)?.attrs ?? {};
    expect(attrs).not.toHaveProperty("views");
    expect(parseViews(attrs)).toHaveLength(1);
    expect(parseViews(attrs)[0]).toMatchObject({ layout: "table", properties: ["p1"] });
    expect(storedAttrsOf(doc).views).toEqual([first, second]); // the document still has both

    // Reading, and editing other blocks, leave it alone.
    bound.view.dispatch(bound.view.state.tr.insertText("!", bound.view.state.doc.content.size - 1));
    expect(storedAttrsOf(doc).views).toEqual([first, second]);

    // A change of the block's own attributes by the older editor rewrites it without `views`: what is left is the mirror.
    bound.view.dispatch(bound.view.state.tr.setNodeAttribute(position, "sort", { by: "updated", dir: "desc" }));
    expect(storedAttrsOf(doc).views).toBeUndefined();
    const left = parseViews(storedAttrsOf(doc));
    expect(left).toHaveLength(1);
    expect(left[0]).toMatchObject({ id: "default", layout: "table", properties: ["p1"], sort: { by: "updated", dir: "desc" } });
    bound.destroy();
  });

  it("is what an editor of this version reads back as one view when `views` is gone", () => {
    const editor = new Editor({
      element: document.body.appendChild(document.createElement("div")),
      extensions: createSchemaExtensions(),
      content: hubContent({ layout: "board", groupBy: PROP.stage, properties: ["p1"] }) as never,
    });
    const [only, ...rest] = parseViews(attrsOf(editor));
    expect(rest).toEqual([]);
    expect(only).toMatchObject({ id: "default", name: "", layout: "board", groupBy: PROP.stage, properties: ["p1"] });
    editor.destroy();
  });
});
