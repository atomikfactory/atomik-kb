// The layout registry and how the block uses it (ADR-0054): the seven layouts in order (the feed of ADR-0058 and the dashboard of ADR-0055 are the last two), the layout select, the calendar and the
// gallery drawn by the component the registry names (agents C and D's components: their own texts tell they are there), and how a change of layout is
// written: one attribute for a block that has no `views` and a layout the older attributes can hold, `views` and its mirror
// otherwise. The contract commit's wiring; B owns the block afterwards and keeps these true.

import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { TextSelection } from "@tiptap/pm/state";
import { act, screen, waitFor, within } from "@testing-library/react";
import type { LoroDoc } from "loro-crdt";
import { describe, expect, it } from "vitest";

import { t } from "../i18n";
import { pid } from "../test/editor";
import { contentOf, idsOf, openPage, renderPages } from "../test/page";
import { viewSelect } from "../test/page-list";
import type { User } from "../test/render";
import { isRawEntry } from "./dashboard/dashboard-model";
import { loroSyncPluginKey } from "./loro-binding";
import { PAGE_LIST_LAYOUT_DEFS, hasDefaultSort, layoutChangePatch, layoutDef, withInitialPatch } from "./page-list-layouts";
import { PAGE_LIST_LAYOUTS, readLayout } from "./page-list-model";
import { addView, emptyView, parseViews, patchView } from "./page-list-views";
import { filterSlashCommands, SLASH_COMMANDS } from "./slash-commands";

const LEGACY = { source: { kind: "children" }, layout: "list", properties: [] as string[], sort: { by: "title", dir: "asc" } };

function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      { type: "pageList", attrs: { id: pid(2), ...LEGACY, ...list } },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

function space(content: JsonValue): SeedSpace {
  return { name: "Personal", pages: [{ title: "Hub", content, children: [{ title: "Alpha" }, { title: "Beta" }] }] };
}

async function openHub(content: JsonValue = hubContent()) {
  const app = await renderPages([space(content)]);
  const editor = await openPage(app.user, "Hub");
  await screen.findByRole("group", { name: t("pageList.label") });
  return { ...app, editor, ids: await idsOf(app.memory) };
}

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

/** The layout select, in the view settings menu of the toolbar (opened when it is not). */
function layoutSelect(user: User): Promise<HTMLSelectElement> {
  return viewSelect(user, block(), t("pageList.layout.label"));
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

describe("the registry", () => {
  it("lists the seven layouts in the order the pickers show them, each with a name, an icon and the model's own list", () => {
    expect(PAGE_LIST_LAYOUT_DEFS.map((def) => def.layout)).toEqual(["list", "table", "board", "calendar", "gallery", "feed", "dashboard"]);
    expect(PAGE_LIST_LAYOUT_DEFS.map((def) => def.layout)).toEqual([...PAGE_LIST_LAYOUTS]);
    for (const def of PAGE_LIST_LAYOUT_DEFS) {
      expect(t(def.labelKey), def.layout).not.toBe(def.labelKey);
      expect(typeof def.Icon).toBe("function");
      expect(readLayout(def.layout)).toBe(def.layout);
    }
  });

  it("has a component for every layout, and a settings section for the board, the calendar and the gallery only (not the feed)", () => {
    for (const def of PAGE_LIST_LAYOUT_DEFS) {
      expect(def.Component, def.layout).toBeTruthy();
    }
    expect(layoutDef("board").Settings).not.toBeNull();
    expect(layoutDef("calendar").Settings).not.toBeNull();
    expect(layoutDef("gallery").Settings).not.toBeNull();
    for (const layout of ["list", "table", "feed", "dashboard"] as const) {
      expect(layoutDef(layout).Settings, layout).toBeNull();
    }
  });

  it("says that a dashboard has no use for Sort and Properties, and every other layout shows them", () => {
    expect(layoutDef("dashboard").toolbar).toEqual({ sort: false, properties: false });
    for (const layout of ["list", "table", "board", "calendar", "gallery", "feed"] as const) {
      expect(layoutDef(layout).toolbar, layout).toBeUndefined();
    }
  });
});

describe("the layout select", () => {
  it("offers the seven layouts", async () => {
    const { user } = await openHub();
    const select = await layoutSelect(user);
    expect(Array.from(select.options).map((option) => option.value)).toEqual(["list", "table", "board", "calendar", "gallery", "feed", "dashboard"]);
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      t("pageList.layout.list"),
      t("pageList.layout.table"),
      t("pageList.layout.board"),
      t("pageList.layout.calendar"),
      t("pageList.layout.gallery"),
      t("pageList.layout.feed"),
      t("pageList.layout.dashboard"),
    ]);
  });

  it("writes one attribute for a layout the older attributes hold, as before", async () => {
    const { user, editor } = await openHub();
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.table"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("table");
    });
    expect(attrsOf(editor).views).toBeNull();
    const changes = doc.exportJsonUpdates(before).changes;
    expect(changes).toHaveLength(1);
    expect(changes[0]?.ops).toHaveLength(1);
  });

  it("draws the calendar through the registry and writes `views` with the mirror, in one change, keeping the id and the source", async () => {
    const { user, editor, memory, ids } = await openHub(hubContent({ source: { kind: "children" }, sort: { by: "created", dir: "desc" }, properties: ["p-a"] }));
    const doc = docOf(editor);
    const before = doc.oplogVersion();
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.calendar"));
    await waitFor(() => {
      // The calendar's own section of the view settings menu is there (registry `Settings`): the select of the date source.
      expect(within(screen.getByRole("dialog", { name: t("pageList.viewSettings") })).getByRole("combobox", { name: t("pageCalendar.dateBy") })).toBeInTheDocument();
    });
    expect(block()).toHaveAttribute("data-layout", "calendar");
    const attrs = attrsOf(editor);
    expect(attrs).toMatchObject({ id: pid(2), source: { kind: "children" }, layout: "calendar", properties: ["p-a"], sort: { by: "created", dir: "desc" } });
    const [view, ...rest] = parseViews(attrs);
    expect(rest).toEqual([]);
    expect(view).toMatchObject({ layout: "calendar", properties: ["p-a"], sort: { by: "created", dir: "desc" } });
    expect(Array.isArray(attrs.views)).toBe(true);
    // One transaction.
    expect(doc.exportJsonUpdates(before).changes).toHaveLength(1);
    // And it is in the document the core holds.
    await waitFor(async () => {
      const content = (await contentOf(memory, ids.page("Hub"))) as { content: { attrs?: { layout?: string; views?: unknown[] } }[] };
      expect(content.content[1]?.attrs?.layout).toBe("calendar");
      expect(content.content[1]?.attrs?.views).toHaveLength(1);
    });
  });

  it("draws the gallery, and changes back to the table through the view once the block has `views`", async () => {
    const { user, editor } = await openHub();
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.gallery"));
    await waitFor(() => {
      expect(within(block()).getByRole("button", { name: t("pageGallery.newCard") })).toBeInTheDocument();
    });
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.table"));
    await waitFor(() => {
      expect(attrsOf(editor).layout).toBe("table");
    });
    const [view] = parseViews(attrsOf(editor));
    expect(view?.layout).toBe("table");
    expect(block()).toHaveAttribute("data-layout", "table");
    expect(within(block()).getByRole("grid")).toBeInTheDocument();
  });

  it("draws a block that already has views through the active one, the first", async () => {
    const views = [
      { id: "v1", name: "", layout: "gallery", properties: [], sort: { by: "title", dir: "asc" } },
      { id: "v2", name: "", layout: "table", properties: [], sort: { by: "title", dir: "asc" } },
    ];
    await openHub(hubContent({ layout: "gallery", views }));
    expect(block()).toHaveAttribute("data-layout", "gallery");
    expect(within(block()).getByRole("button", { name: t("pageGallery.newCard") })).toBeInTheDocument();
  });
});

// ----- the dashboard (ADR-0055) --------------------------------------------------------------

/** The dashboard's widgets of the block, as the view holds them. */
function widgetsInDocument(editor: Editor) {
  const [view] = parseViews(attrsOf(editor));
  return view?.widgets ?? [];
}

describe("the dashboard", () => {
  it("is drawn through the registry once the layout is chosen: an empty dashboard with its tile, and a bar with Filter and the settings only", async () => {
    const { user, editor } = await openHub();
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.dashboard"));
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "dashboard");
    });
    expect(within(block()).getByRole("group", { name: t("dashboard.label") })).toBeInTheDocument();
    expect(within(block()).getByText(t("dashboard.empty"))).toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("dashboard.add") })).toBeEnabled();
    // A dashboard never fits the legacy attributes: the block has `views` and the mirror says the layout.
    expect(Array.isArray(attrsOf(editor).views)).toBe(true);
    expect(attrsOf(editor).layout).toBe("dashboard");
    // Close the settings menu the layout was chosen in; the bar has no Sort and no Properties.
    await user.keyboard("{Escape}");
    const bar = within(block()).getByRole("toolbar", { name: t("pageList.toolbar") });
    expect(within(bar).getByRole("button", { name: new RegExp(`^${t("pageList.filter")}`, "u") })).toBeInTheDocument();
    expect(within(bar).getByRole("button", { name: t("pageList.viewSettings") })).toBeInTheDocument();
    expect(within(bar).queryByRole("button", { name: new RegExp(`^${t("pageList.sort")}`, "u") })).toBeNull();
    expect(within(bar).queryByRole("button", { name: new RegExp(`^${t("pageList.properties")}`, "u") })).toBeNull();
  });

  it("adds a number from the tile: it counts the pages, and the widget is written in the view's `widgets`", async () => {
    const dashboard = { id: "v1", name: "", layout: "dashboard", properties: [], sort: { by: "title", dir: "asc" } };
    const { user, editor } = await openHub(hubContent({ layout: "dashboard", views: [dashboard] }));
    await user.click(within(block()).getByRole("button", { name: t("dashboard.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("dashboard.add.number") }));
    await waitFor(() => {
      expect(widgetsInDocument(editor)).toHaveLength(1);
    });
    const [widget] = widgetsInDocument(editor);
    expect(widget !== undefined && !isRawEntry(widget) ? widget.kind : null).toBe("number");
    // Hub has two children, Alpha and Beta.
    const frame = await within(block()).findByRole("group", { name: "Count all" });
    expect(within(frame).getByText("2")).toBeInTheDocument();
    expect(await screen.findByRole("dialog", { name: t("dashboard.settings") })).toBeInTheDocument();
  });

  it("draws another view of the block in a widget, and a change of that view's own settings is written to that view", async () => {
    const views: JsonValue[] = [
      {
        id: "v1",
        name: "",
        layout: "dashboard",
        properties: [],
        sort: { by: "title", dir: "asc" },
        widgets: [{ id: "a0000001", kind: "view", title: "Pages", w: 4, h: "l", viewId: "v2" }],
      },
      { id: "v2", name: "Tasks", layout: "table", properties: [], sort: { by: "title", dir: "asc" } },
    ];
    const { user, editor } = await openHub(hubContent({ layout: "dashboard", views }));
    const frame = within(block()).getByRole("group", { name: "Pages" });
    expect(within(frame).getByRole("grid")).toBeInTheDocument();
    expect(within(frame).getByRole("button", { name: t("pageTable.open", { title: "Alpha" }) })).toBeInTheDocument();
    await user.click(within(within(frame).getByRole("columnheader", { name: t("pageList.sort.title") })).getByRole("button"));
    await user.click(await screen.findByRole("menuitem", { name: t("pageTable.sortDesc") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))[1]?.sort).toEqual({ by: "title", dir: "desc" });
    });
    const [dashboardView, tableView] = parseViews(attrsOf(editor));
    expect(dashboardView?.sort).toEqual({ by: "title", dir: "asc" });
    expect(dashboardView?.widgets).toHaveLength(1);
    expect(tableView?.layout).toBe("table");
    // "Open view" makes the table the view on show (this device's choice: nothing is written).
    const written = JSON.stringify(attrsOf(editor));
    await user.click(within(frame).getByRole("button", { name: t("dashboard.view.openNamed", { name: "Pages" }) }));
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "table");
    });
    expect(JSON.stringify(attrsOf(editor))).toBe(written);
  });

  it("is made by /dashboard view: a block whose only view is a dashboard with a count and a chart", async () => {
    const { editor } = await openHub();
    const entry = SLASH_COMMANDS.find((command) => command.id === "dashboardView");
    expect(entry?.keywords).toContain("database");
    expect(filterSlashCommands("dashboard").map((command) => command.id)).toContain("dashboardView");
    const at = editor.state.doc.content.size - 1;
    editor.view.dispatch(editor.state.tr.setSelection(TextSelection.create(editor.state.doc, at)));
    act(() => {
      void entry?.run(editor, { from: at, to: at });
    });
    const inserted: Record<string, unknown>[] = [];
    editor.state.doc.descendants((node) => {
      if (node.type.name === "pageList" && node.attrs.layout === "dashboard") {
        inserted.push(node.attrs);
      }
      return true;
    });
    expect(inserted).toHaveLength(1);
    const views = parseViews(inserted[0] ?? {});
    expect(views).toHaveLength(1);
    const [view] = views;
    expect(view?.layout).toBe("dashboard");
    const kinds = (view?.widgets ?? []).map((widget) => (isRawEntry(widget) ? "raw" : widget.kind));
    expect(kinds).toEqual(["number", "chart"]);
    const [count, chart] = view?.widgets ?? [];
    expect(count !== undefined && !isRawEntry(count) ? count.calc : null).toEqual({ by: "title", fn: "count_all" });
    expect(chart !== undefined && !isRawEntry(chart) ? chart.chart : null).toMatchObject({
      type: "bar",
      groupBy: "created",
      bucket: "month",
      measure: { fn: "count" },
    });
    expect(Array.isArray(inserted[0]?.views)).toBe(true);
  });
});

// ----- the feed (ADR-0058) -------------------------------------------------------------------

describe("the feed layout in the registry", () => {
  it("is the entry after the gallery, with its name and icon, no settings section, and a newest-first sort to start with", () => {
    const feed = layoutDef("feed");
    expect(feed.layout).toBe("feed");
    expect(feed.labelKey).toBe("pageList.layout.feed");
    expect(t(feed.labelKey)).toBe("Feed");
    expect(typeof feed.Icon).toBe("function");
    expect(feed.Settings).toBeNull();
    expect(feed.initialPatch).toEqual({ sorts: [{ by: "created", dir: "desc" }] });
    expect(readLayout("feed")).toBe("feed");
  });

  it("is the only layout with an initial patch", () => {
    expect(PAGE_LIST_LAYOUT_DEFS.filter((def) => def.initialPatch !== undefined).map((def) => def.layout)).toEqual(["feed"]);
  });

  it("is read from a stored view as the feed, not as an unknown layout", () => {
    const [view] = parseViews({ views: [{ id: "f", name: "", layout: "feed", properties: [], sort: { by: "created", dir: "desc" } }] });
    expect(view?.layout).toBe("feed");
    expect(view?.layoutRaw).toBeUndefined();
  });
});

describe("what a change to the feed starts the view with", () => {
  const defaultView = emptyView("list", "a");
  const sortedView = patchView(defaultView, { sorts: [{ by: "title", dir: "desc" }] });
  const twoKeys = patchView(defaultView, {
    sorts: [
      { by: "title", dir: "asc" },
      { by: "created", dir: "asc" },
    ],
  });

  it("knows a view that still has the default sort: the title, A to Z, alone", () => {
    expect(hasDefaultSort(defaultView)).toBe(true);
    expect(hasDefaultSort(sortedView)).toBe(false);
    expect(hasDefaultSort(patchView(defaultView, { sorts: [{ by: "updated", dir: "asc" }] }))).toBe(false);
    // The title, A to Z, as the first of two keys is a chosen sort too.
    expect(hasDefaultSort(twoKeys)).toBe(false);
  });

  it("changes a view with the default sort to the feed with the feed's sort", () => {
    expect(layoutChangePatch(defaultView, "feed")).toEqual({ layout: "feed", sorts: [{ by: "created", dir: "desc" }] });
    const next = patchView(defaultView, layoutChangePatch(defaultView, "feed"));
    expect(next.layout).toBe("feed");
    expect(next.sort).toEqual({ by: "created", dir: "desc" });
    expect(next.sorts).toEqual([{ by: "created", dir: "desc" }]);
  });

  it("leaves the sort a person chose alone", () => {
    expect(layoutChangePatch(sortedView, "feed")).toEqual({ layout: "feed" });
    expect(layoutChangePatch(twoKeys, "feed")).toEqual({ layout: "feed" });
    expect(patchView(sortedView, layoutChangePatch(sortedView, "feed")).sorts).toEqual([{ by: "title", dir: "desc" }]);
  });

  it("changes to any other layout with the layout alone", () => {
    for (const layout of ["list", "table", "board", "calendar", "gallery", "dashboard"] as const) {
      expect(layoutChangePatch(defaultView, layout), layout).toEqual({ layout });
    }
  });

  it("gives a view that was added with the feed layout its sort, and leaves the views of other layouts as they are", () => {
    const views = [emptyView("list", "a")];
    const withFeed = withInitialPatch(addView(views, "feed", "f"), "f");
    expect(withFeed).toHaveLength(2);
    expect(withFeed[0]).toBe(views[0]);
    expect(withFeed[1]?.layout).toBe("feed");
    expect(withFeed[1]?.sort).toEqual({ by: "created", dir: "desc" });
    expect(withFeed[1]?.sorts).toEqual([{ by: "created", dir: "desc" }]);
    const withTable = addView(views, "table", "t");
    const kept = withInitialPatch(withTable, "t");
    expect(kept).toEqual(withTable);
    expect(kept[1]).toBe(withTable[1]);
    expect(withInitialPatch(withTable, "nobody")).toEqual(withTable);
  });
});

describe("the feed in the block", () => {
  it("is drawn through the registry once the layout is chosen, with the sort of a new feed, and writes `views` with the mirror", async () => {
    const { user, editor } = await openHub();
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.feed"));
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "feed");
    });
    expect(within(block()).getByRole("group", { name: t("pageList.feed.label") })).toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("pageList.feed.open", { title: "Alpha" }) })).toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("pageList.feed.open", { title: "Beta" }) })).toBeInTheDocument();
    expect(within(block()).getByRole("button", { name: t("pageList.feed.newCard") })).toBeInTheDocument();
    // The feed has no section of its own in the settings menu, only the layout, the source and where a click opens.
    const settings = screen.getByRole("dialog", { name: t("pageList.viewSettings") });
    expect(within(settings).queryByRole("combobox", { name: t("pageGallery.size") })).toBeNull();
    // A feed does not fit the legacy attributes: the block has `views`, and the mirror carries the layout and the newest-first sort.
    const attrs = attrsOf(editor);
    expect(Array.isArray(attrs.views)).toBe(true);
    expect(attrs).toMatchObject({ layout: "feed", sort: { by: "created", dir: "desc" } });
    const [view, ...rest] = parseViews(attrs);
    expect(rest).toEqual([]);
    expect(view).toMatchObject({ layout: "feed", sort: { by: "created", dir: "desc" } });
    expect(view?.sorts).toEqual([{ by: "created", dir: "desc" }]);
  });

  it("keeps the sort a person chose when the layout changes to the feed", async () => {
    const { user, editor } = await openHub(hubContent({ sort: { by: "title", dir: "desc" } }));
    await user.selectOptions(await layoutSelect(user), t("pageList.layout.feed"));
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "feed");
    });
    const [view] = parseViews(attrsOf(editor));
    expect(view?.sorts).toEqual([{ by: "title", dir: "desc" }]);
    expect(attrsOf(editor).sort).toEqual({ by: "title", dir: "desc" });
  });

  it("gives the sort to a view changed to the feed from its tab's menu", async () => {
    const { user, editor } = await openHub();
    await user.click(within(block()).getByRole("button", { name: t("pageList.view.menu", { name: t("pageList.layout.list") }) }));
    await user.click(await screen.findByRole("menuitem", { name: t("pageList.view.changeLayout") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.view.changeLayout") });
    await user.click(within(menu).getByRole("menuitem", { name: t("pageList.layout.feed") }));
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "feed");
    });
    const [view] = parseViews(attrsOf(editor));
    expect(view).toMatchObject({ layout: "feed", sort: { by: "created", dir: "desc" } });
  });

  it("adds a view of the feed from the \"+\" menu: it starts sorted by created time, newest first, and the first view is as it was", async () => {
    const { user, editor } = await openHub();
    await user.click(within(block()).getByRole("button", { name: t("pageList.views.add") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.views.addMenu") });
    await user.click(within(menu).getByRole("menuitem", { name: t("pageList.layout.feed") }));
    await waitFor(() => {
      expect(parseViews(attrsOf(editor))).toHaveLength(2);
    });
    const [first, added] = parseViews(attrsOf(editor));
    expect(first).toMatchObject({ layout: "list", sort: { by: "title", dir: "asc" } });
    expect(added).toMatchObject({ layout: "feed", sort: { by: "created", dir: "desc" } });
    expect(added?.sorts).toEqual([{ by: "created", dir: "desc" }]);
    // The new view is the one shown, and the mirror of the block is still the first view's.
    await waitFor(() => {
      expect(block()).toHaveAttribute("data-layout", "feed");
    });
    expect(attrsOf(editor)).toMatchObject({ layout: "list", sort: { by: "title", dir: "asc" } });
  });

  it("is drawn in a dashboard's view widget by the same component, the cards of its view", async () => {
    const views: JsonValue[] = [
      {
        id: "v1",
        name: "",
        layout: "dashboard",
        properties: [],
        sort: { by: "title", dir: "asc" },
        widgets: [{ id: "a0000001", kind: "view", title: "Latest", w: 4, h: "l", viewId: "v2" }],
      },
      { id: "v2", name: "News", layout: "feed", properties: [], sort: { by: "created", dir: "desc" } },
    ];
    await openHub(hubContent({ layout: "dashboard", views }));
    const frame = within(block()).getByRole("group", { name: "Latest" });
    expect(within(frame).getByRole("group", { name: t("pageList.feed.label") })).toBeInTheDocument();
    expect(within(frame).getByRole("button", { name: t("pageList.feed.open", { title: "Alpha" }) })).toBeInTheDocument();
    expect(within(frame).getByRole("button", { name: t("pageList.feed.open", { title: "Beta" }) })).toBeInTheDocument();
  });
});
