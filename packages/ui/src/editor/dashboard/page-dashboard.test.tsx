// The dashboard layout drawn with props (ADR-0055, `docs/architecture/dashboards.md` section 3): the grid and its frames, the "…" menu
// (width, height, duplicate, move, delete), the "Add widget" tile, the settings panel (title, column and function, the widget's own
// filter, the view), the values of the numbers, the chart bodies (mounted with the size of their body and the rows after the filters),
// an embedded table that edits its own view, an embedded list that opens pages as its own view says, a deleted view, a widget of a
// newer version kept, the drag of a header, the empty dashboard and the read-only editor.
//
// The harness plays the part of `PageListView`: it holds the block's views, patches them as the block does (`updateView`), hands
// the dashboard its rows (after the dashboard's own filter) and the props of section 3, and provides the host the registry's
// components come through. The charts are agent F's and are replaced by stand-ins that show what they were given.

import { CoreClient, MemoryTransport } from "@atomik-kb/core-client";
import type { PageId, PageInfo, PropertyDef, SpaceId } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useEffect, useMemo, useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import { t } from "../../i18n";
import { createAppStore } from "../../store";
import { StoreContext } from "../../store-context";
import type { OpenDisposition } from "../../store-tabs";
import { NOW } from "../../test/render";
import { applyFilterTree } from "../page-list-filter-tree";
import type { PageListLayoutProps } from "../page-list-layout-types";
import { layoutDef } from "../page-list-layouts";
import { parseView, updateView, viewToJson } from "../page-list-views";
import type { PageListViewSpec } from "../page-list-views";
import { isRawEntry } from "./dashboard-model";
import { DashboardHostContext } from "./dashboard-host";
import type { DashboardHost } from "./dashboard-host";
import type { ChartWidgetBodyProps, ChartWidgetSettingsProps, DashboardWidget } from "./dashboard-types";
import { WIDGET_HEIGHT_PX } from "./dashboard-types";
import { PageDashboardView } from "./PageDashboardView";

// ----- stand-ins for the charts (agent F's, tested in `chart-widget.test.tsx`) ---------------------------

const charts = vi.hoisted(() => ({ bodies: [] as unknown[] }));

vi.mock("./ChartWidget", () => ({
  ChartWidgetBody: (props: ChartWidgetBodyProps) => {
    charts.bodies.push(props);
    return (
      <div
        role="img"
        aria-label={props.title}
        data-testid="chart-body"
        data-width={props.width}
        data-height={props.height}
        data-rows={props.rows.length}
        data-type={props.chart.type}
      />
    );
  },
  ChartWidgetSettings: ({ chart, onChange }: ChartWidgetSettingsProps) => (
    <button
      type="button"
      onClick={() => {
        onChange({ ...chart, type: "donut" });
      }}
    >
      Make it a donut
    </button>
  ),
}));

function lastChart(): ChartWidgetBodyProps {
  const last = charts.bodies[charts.bodies.length - 1];
  if (last === undefined) {
    throw new Error("no chart was drawn");
  }
  return last as ChartWidgetBodyProps;
}

// ----- the space ----------------------------------------------------------------------------------

const SPACE: SpaceId = "11111111-1111-4111-8111-111111111111";
const pageId = (n: number): PageId => `22222222-2222-4222-8222-${String(n).padStart(12, "0")}`;

const STATUS: PropertyDef = {
  id: "de000000-0000-4000-8000-00000000d001",
  name: "Status",
  kind: "status",
  options: [
    { id: "todo", name: "To do", color: "gray", group: "todo" },
    { id: "done", name: "Done", color: "green", group: "done" },
  ],
  createdAt: 1,
};
const HOURS: PropertyDef = { id: "de000000-0000-4000-8000-00000000d002", name: "Hours", kind: "number", options: [], createdAt: 1 };
const SCHEMA: readonly PropertyDef[] = [STATUS, HOURS];

function page(n: number, title: string, hours: number | null, status: "todo" | "done" | null): PageInfo {
  const properties: NonNullable<PageInfo["properties"]> = [];
  if (hours !== null) {
    properties.push([HOURS.id, { kind: "number", value: hours }]);
  }
  if (status !== null) {
    properties.push([STATUS.id, { kind: "status", value: status }]);
  }
  return { id: pageId(n), spaceId: SPACE, title, createdAt: n, updatedAt: n, properties };
}

/** Alpha (3 h, to do), Beta (10 h, done), Gamma (no values) and Delta (2 h, to do): 15 hours in all. */
const PAGES: PageInfo[] = [page(1, "Alpha", 3, "todo"), page(2, "Beta", 10, "done"), page(3, "Gamma", null, null), page(4, "Delta", 2, "todo")];

const TODO_ONLY = { op: "and", items: [{ propertyId: STATUS.id, op: "hasOption", value: "todo" }] };

// ----- the views ----------------------------------------------------------------------------------

function dashboardView(widgets: unknown[], extra: Record<string, unknown> = {}): PageListViewSpec {
  const view = parseView({ id: "dash", name: "", layout: "dashboard", properties: [], sort: { by: "title", dir: "asc" }, widgets, ...extra });
  if (view === null) {
    throw new Error("the dashboard view did not read");
  }
  return view;
}

function otherView(extra: Record<string, unknown>): PageListViewSpec {
  const view = parseView({ properties: [], sort: { by: "title", dir: "asc" }, ...extra });
  if (view === null) {
    throw new Error("the view did not read");
  }
  return view;
}

const TABLE = otherView({ id: "tasks", name: "All tasks", layout: "table", properties: [HOURS.id] });
const LIST = otherView({ id: "list", name: "Pages", layout: "list", openIn: "center" });

const number = (id: string, title: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  id,
  kind: "number",
  title,
  w: 1,
  h: "s",
  calc: { by: "title", fn: "count_all" },
  ...extra,
});
const chart = (id: string, title: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  id,
  kind: "chart",
  title,
  w: 2,
  h: "m",
  chart: { type: "bar", groupBy: STATUS.id, measure: { fn: "count" } },
  ...extra,
});
const embedded = (id: string, title: string, viewId: string, extra: Record<string, unknown> = {}): Record<string, unknown> => ({
  id,
  kind: "view",
  title,
  w: 4,
  h: "l",
  viewId,
  ...extra,
});

// ----- the harness --------------------------------------------------------------------------------

interface Harness {
  /** The views of the block as they are now (after the writes the dashboard asked for). */
  views: () => readonly PageListViewSpec[];
  /** The id of the view a click on "Open view" made active, `null` when none did. */
  activated: () => string | null;
  openInView: Mock<DashboardHost["openInView"]>;
  onOpen: Mock<OpenPage>;
}

type OpenPage = (page: PageInfo, disposition: OpenDisposition) => void;

interface Options {
  views: PageListViewSpec[];
  editable?: boolean;
  /** The pages and the schema of the space, when the test needs others than the default ones. */
  pages?: PageInfo[];
  schema?: readonly PropertyDef[];
}

/** What the block tells the test: its views after each change, the view "Open view" asked for, and the pages opened. */
interface Callbacks {
  readonly onViews: (views: readonly PageListViewSpec[]) => void;
  readonly onActivate: (id: string) => void;
  readonly onOpen: Mock<OpenPage>;
}

function Block({ options, callbacks }: { options: Options; callbacks: Callbacks }) {
  const [views, setViews] = useState<readonly PageListViewSpec[]>(options.views);
  const dashboard = views[0];
  if (dashboard === undefined) {
    throw new Error("no view");
  }
  const { onViews } = callbacks;
  useEffect(() => {
    onViews(views);
  }, [views, onViews]);
  const pages = options.pages ?? PAGES;
  const schema = options.schema ?? SCHEMA;
  const rows = useMemo(() => applyFilterTree(pages, dashboard.filter, { defs: schema }), [pages, schema, dashboard.filter]);
  const props: PageListLayoutProps = {
    rows,
    view: dashboard,
    schema,
    columns: [],
    spaceId: SPACE,
    editable: options.editable ?? true,
    today: "2026-10-03",
    onOpen: callbacks.onOpen,
    onUpdateView: (patch) => {
      setViews((current) => updateView(current, dashboard.id, patch));
    },
    onCreatePage: null,
    creating: false,
    views,
    sourcePages: pages,
    onUpdateOtherView: (id, patch) => {
      setViews((current) => updateView(current, id, patch));
    },
    onActivateView: callbacks.onActivate,
  };
  return <PageDashboardView {...props} />;
}

function setup(options: Options) {
  const memory = new MemoryTransport({ now: () => NOW, workspaceName: "Research", spaces: [{ name: "Personal", pages: [{ title: "Hub" }] }] });
  const store = createAppStore(new CoreClient(memory), { corePollMs: 5 });
  // The people of the space are known (nobody): the dashboard does not ask a core that has no such space.
  store.setState({ people: { [SPACE]: [] } });
  let current: readonly PageListViewSpec[] = options.views;
  let activated: string | null = null;
  const callbacks: Callbacks = {
    onViews: (views) => {
      current = views;
    },
    onActivate: (id) => {
      activated = id;
    },
    onOpen: vi.fn<OpenPage>(),
  };
  const openInView = vi.fn<DashboardHost["openInView"]>();
  const host: DashboardHost = { layoutDef, openInView };
  const user = userEvent.setup();
  const view = render(
    <StoreContext value={store}>
      <DashboardHostContext value={host}>
        <Block options={options} callbacks={callbacks} />
      </DashboardHostContext>
    </StoreContext>,
  );
  const harness: Harness = {
    views: () => current,
    activated: () => activated,
    openInView,
    onOpen: callbacks.onOpen,
  };
  return { user, harness, ...view };
}

/** The widgets of the dashboard (the first view) as the block holds them now. */
function widgetsOf(harness: Harness): DashboardWidget[] {
  const [dashboard] = harness.views();
  return (dashboard?.widgets ?? []).flatMap((entry) => (isRawEntry(entry) ? [] : [entry]));
}

const frame = (title: string): HTMLElement => screen.getByRole("group", { name: title });

/** The titles of the frames, in the order they are drawn. */
function drawn(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>("[data-widget-id]")).map((element) => element.getAttribute("aria-label") ?? "");
}

const menuName = (title: string): string => t("dashboard.widget.menu", { title });

async function openMenu(user: ReturnType<typeof userEvent.setup>, title: string): Promise<HTMLElement> {
  await user.click(within(frame(title)).getByRole("button", { name: menuName(title) }));
  return screen.findByRole("menu", { name: menuName(title) });
}

/** The panel of a widget's settings, opened from its menu. */
async function openSettings(user: ReturnType<typeof userEvent.setup>, title: string): Promise<HTMLElement> {
  const menu = await openMenu(user, title);
  await user.click(within(menu).getByRole("menuitem", { name: t("dashboard.widget.edit") }));
  return screen.findByRole("dialog", { name: t("dashboard.settings") });
}

async function choose(user: ReturnType<typeof userEvent.setup>, title: string, item: string | RegExp, role: "menuitem" | "menuitemradio" = "menuitem") {
  const menu = await openMenu(user, title);
  await user.click(within(menu).getByRole(role, { name: item }));
}

beforeEach(() => {
  charts.bodies.length = 0;
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// ----- the grid ----------------------------------------------------------------------------------

describe("the grid", () => {
  it("draws a frame for each widget in the order of the list, with its title, width and height", () => {
    setup({
      views: [
        dashboardView([
          number("a0000001", "Pages"),
          chart("a0000002", "By status", { w: 3, h: "l" }),
          number("a0000003", "Hours", { w: 4, h: "m", calc: { by: HOURS.id, fn: "sum" } }),
        ]),
      ],
    });
    expect(screen.getByRole("group", { name: t("dashboard.label") })).toBeInTheDocument();
    expect(drawn()).toEqual(["Pages", "By status", "Hours"]);
    expect(frame("Pages")).toHaveAttribute("data-w", "1");
    expect(frame("Pages")).toHaveStyle({ height: `${String(WIDGET_HEIGHT_PX.s)}px` });
    expect(frame("By status")).toHaveAttribute("data-w", "3");
    expect(frame("By status")).toHaveStyle({ height: `${String(WIDGET_HEIGHT_PX.l)}px` });
    expect(frame("Hours")).toHaveAttribute("data-w", "4");
    expect(frame("Hours")).toHaveAttribute("data-h", "m");
    // Each frame's title is a heading, and the tile that adds a widget ends the grid.
    expect(within(frame("Pages")).getByRole("heading", { name: "Pages" })).toBeInTheDocument();
    const tile = screen.getByRole("button", { name: t("dashboard.add") });
    expect(tile.parentElement?.lastElementChild).toBe(tile);
  });

  it("names a widget without a title after what it shows", () => {
    setup({
      views: [
        dashboardView(
          [
            number("a0000001", ""),
            number("a0000002", "", { calc: { by: HOURS.id, fn: "sum" } }),
            chart("a0000003", ""),
            embedded("a0000004", "", "tasks"),
          ],
        ),
        TABLE,
      ],
    });
    expect(drawn()).toEqual(["Count all", "Sum · Hours", "Count by Status", "All tasks"]);
  });

  it("shows a badge with the number of filters a widget has of its own", () => {
    setup({ views: [dashboardView([number("a0000001", "To do", { filter: TODO_ONLY }), number("a0000002", "All")])] });
    expect(within(frame("To do")).getByRole("img", { name: "1 filter on this widget" })).toBeInTheDocument();
    expect(within(frame("All")).queryByRole("img")).toBeNull();
  });
});

// ----- the numbers -------------------------------------------------------------------------------

describe("a number", () => {
  it("counts the pages, sums a column and applies the widget's own filter on top of the dashboard's", () => {
    setup({
      views: [
        dashboardView(
          [
            number("a0000001", "Pages"),
            number("a0000002", "Hours", { calc: { by: HOURS.id, fn: "sum" } }),
            number("a0000003", "To do", { filter: TODO_ONLY }),
            number("a0000004", "To do hours", { filter: TODO_ONLY, calc: { by: HOURS.id, fn: "sum" } }),
            number("a0000005", "Mean", { calc: { by: HOURS.id, fn: "average" } }),
          ],
          // The dashboard's own filter: the pages with a number of hours at all.
          { filter: { op: "and", items: [{ propertyId: HOURS.id, op: "isSet" }] } },
        ),
      ],
    });
    // Gamma has no hours: the dashboard's filter leaves three pages to every widget.
    expect(within(frame("Pages")).getByText("3")).toBeInTheDocument();
    expect(within(frame("Hours")).getByText("15")).toBeInTheDocument();
    expect(within(frame("To do")).getByText("2")).toBeInTheDocument();
    expect(within(frame("To do hours")).getByText("5")).toBeInTheDocument();
    expect(within(frame("Mean")).getByText("5")).toBeInTheDocument();
  });

  it("shows a dash for a column that was deleted, and for a calculation with nothing to say", () => {
    setup({
      views: [
        dashboardView([
          number("a0000001", "Gone", { calc: { by: "de000000-0000-4000-8000-0000000000ff", fn: "sum" } }),
          number("a0000002", "Nothing", { calc: { by: HOURS.id, fn: "average" }, filter: { op: "and", items: [{ propertyId: STATUS.id, op: "hasOption", value: "nobody" }] } }),
        ]),
      ],
    });
    expect(within(frame("Gone")).getByText("—")).toBeInTheDocument();
    expect(within(frame("Nothing")).getByText("—")).toBeInTheDocument();
  });
});

// ----- relative days in the filters of a widget (ADR-0058) ----------------------------------------

describe("the filter of a widget", () => {
  const DUE: PropertyDef = { id: "de000000-0000-4000-8000-00000000d003", name: "Due", kind: "date", options: [], createdAt: 1 };
  const dated = (n: number, title: string, day: string | null): PageInfo => ({
    id: pageId(n),
    spaceId: SPACE,
    title,
    createdAt: n,
    updatedAt: n,
    properties: day === null ? [] : [[DUE.id, { kind: "date", value: { date: day } }]],
  });
  // The dashboard is drawn on Saturday 2026-10-03 (the harness's `today`).
  const pages = [dated(1, "Today", "2026-10-03"), dated(2, "Yesterday", "2026-10-02"), dated(3, "Last week", "2026-09-27"), dated(4, "Next week", "2026-10-10"), dated(5, "Never", null)];
  const schema = [STATUS, HOURS, DUE];
  const filterOn = (op: string, value: unknown): Record<string, unknown> => ({ filter: { op: "and", items: [{ propertyId: DUE.id, op, value }] } });
  /** The value a number widget shows (its frame also carries the number of filters in a badge). */
  const valueOf = (title: string): string | null => frame(title).querySelector(".page-dashboard-number-value")?.textContent ?? null;

  it("resolves a relative day at the day the dashboard is drawn on", () => {
    setup({
      views: [
        dashboardView([
          number("a0000001", "Today", filterOn("equals", { relative: "today" })),
          number("a0000002", "Before today", filterOn("before", { relative: "today" })),
          number("a0000003", "From tomorrow", filterOn("onOrAfter", { relative: "tomorrow" })),
        ]),
      ],
      pages,
      schema,
    });
    expect(["Today", "Before today", "From tomorrow"].map(valueOf)).toEqual(["1", "2", "1"]);
  });

  it("resolves a period at that day, the days of a period counted from it, for a number and for the rows a chart is given", () => {
    setup({
      views: [
        dashboardView([
          number("a0000001", "Past week", filterOn("within", { relative: "pastDays", n: 7 })),
          number("a0000002", "Next 7 days", filterOn("within", { relative: "nextDays", n: 7 })),
          chart("a0000003", "This month", filterOn("within", { relative: "thisMonth" })),
        ]),
      ],
      pages,
      schema,
    });
    // 2026-09-26 to 2026-10-03: Today, Yesterday and the page of 27 September; 2026-10-03 to 2026-10-10: Today and Next week.
    expect(["Past week", "Next 7 days"].map(valueOf)).toEqual(["3", "2"]);
    // October: Today, Yesterday and Next week.
    expect(within(frame("This month")).getByTestId("chart-body")).toHaveAttribute("data-rows", "3");
  });

  it("is kept and hides nothing while its relative value is not complete", () => {
    setup({ views: [dashboardView([number("a0000001", "All", filterOn("within", { relative: "pastDays" }))])], pages, schema });
    expect(valueOf("All")).toBe("5");
  });
});

// ----- the charts --------------------------------------------------------------------------------

describe("a chart", () => {
  it("is given the rows after the widget's filter, the title and the size of its body", () => {
    const observers: { callback: ResizeObserverCallback; target: Element | null }[] = [];
    class FakeObserver {
      target: Element | null = null;
      constructor(readonly callback: ResizeObserverCallback) {
        observers.push(this);
      }
      observe(target: Element): void {
        this.target = target;
      }
      unobserve(): void {}
      disconnect(): void {}
    }
    vi.stubGlobal("ResizeObserver", FakeObserver);
    setup({ views: [dashboardView([chart("a0000001", "By status", { filter: TODO_ONLY }), chart("a0000002", "Everything")])] });
    const filtered = within(frame("By status")).getByTestId("chart-body");
    expect(filtered).toHaveAttribute("data-rows", "2");
    expect(within(frame("Everything")).getByTestId("chart-body")).toHaveAttribute("data-rows", "4");
    // Before it is measured a chart has a plausible size: the height of the widget less its header.
    expect(filtered).toHaveAttribute("data-height", String(WIDGET_HEIGHT_PX.m - 36));
    expect(lastChart()).toMatchObject({ title: "Everything", spaceId: SPACE, today: "2026-10-03" });
    // Then the body's own.
    act(() => {
      for (const observer of observers) {
        observer.callback(
          [{ target: observer.target, contentRect: { width: 411, height: 197 } } as unknown as ResizeObserverEntry],
          observer as unknown as ResizeObserver,
        );
      }
    });
    expect(within(frame("By status")).getByTestId("chart-body")).toHaveAttribute("data-width", "411");
    expect(within(frame("By status")).getByTestId("chart-body")).toHaveAttribute("data-height", "197");
  });

  it("opens a page of a mark the way the dashboard's view says", () => {
    const { harness } = setup({ views: [dashboardView([chart("a0000001", "By status")])] });
    const [first] = PAGES;
    if (first === undefined) {
      throw new Error("no page");
    }
    lastChart().onOpen(first, "background");
    expect(harness.onOpen).toHaveBeenCalledWith(first, "background");
  });
});

// ----- the menu ----------------------------------------------------------------------------------

describe("the menu of a widget", () => {
  const three = (): PageListViewSpec[] => [dashboardView([number("a0000001", "A"), number("a0000002", "B"), number("a0000003", "C")])];

  it("lists Edit, the widths and heights, Duplicate, the two moves and Delete, and marks the width and height the widget has", async () => {
    const { user } = setup({ views: three() });
    const menu = await openMenu(user, "B");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("dashboard.widget.edit"),
      t("dashboard.widget.duplicate"),
      t("dashboard.widget.moveEarlier"),
      t("dashboard.widget.moveLater"),
      t("dashboard.widget.delete"),
    ]);
    const widths = within(within(menu).getByRole("group", { name: t("dashboard.widget.width") })).getAllByRole("menuitemradio");
    expect(widths.map((item) => item.textContent)).toEqual(["¼", "½", "¾", t("dashboard.width.4")]);
    expect(widths.map((item) => item.getAttribute("aria-checked"))).toEqual(["true", "false", "false", "false"]);
    const heights = within(within(menu).getByRole("group", { name: t("dashboard.widget.height") })).getAllByRole("menuitemradio");
    expect(heights.map((item) => item.textContent)).toEqual([t("dashboard.height.s"), t("dashboard.height.m"), t("dashboard.height.l")]);
    expect(heights.map((item) => item.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
  });

  it("changes the width and the height, one write each", async () => {
    const { user, harness } = setup({ views: three() });
    await choose(user, "B", "½", "menuitemradio");
    expect(frame("B")).toHaveAttribute("data-w", "2");
    await choose(user, "B", t("dashboard.height.l"), "menuitemradio");
    expect(frame("B")).toHaveAttribute("data-h", "l");
    expect(widgetsOf(harness).map((widget) => [widget.title, widget.w, widget.h])).toEqual([
      ["A", 1, "s"],
      ["B", 2, "l"],
      ["C", 1, "s"],
    ]);
  });

  it("moves a widget earlier and later, and disables the move that has nowhere to go", async () => {
    const { user } = setup({ views: three() });
    await choose(user, "B", t("dashboard.widget.moveEarlier"));
    expect(drawn()).toEqual(["B", "A", "C"]);
    const first = await openMenu(user, "B");
    expect(within(first).getByRole("menuitem", { name: t("dashboard.widget.moveEarlier") })).toBeDisabled();
    await user.click(within(first).getByRole("menuitem", { name: t("dashboard.widget.moveLater") }));
    expect(drawn()).toEqual(["A", "B", "C"]);
    const last = await openMenu(user, "C");
    expect(within(last).getByRole("menuitem", { name: t("dashboard.widget.moveLater") })).toBeDisabled();
    expect(within(last).getByRole("menuitem", { name: t("dashboard.widget.moveEarlier") })).toBeEnabled();
  });

  it("duplicates a widget right after itself, with a new id and the same settings", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "A", { calc: { by: HOURS.id, fn: "sum" } }), number("a0000002", "B")])] });
    await choose(user, "A", t("dashboard.widget.duplicate"));
    expect(drawn()).toEqual(["A", "A", "B"]);
    const [first, copy] = widgetsOf(harness);
    expect(copy?.id).not.toBe(first?.id);
    expect(copy?.id).toMatch(/^[0-9a-f]{8}$/u);
    expect(copy).toEqual({ ...first, id: copy?.id });
  });

  it("deletes a widget", async () => {
    const { user, harness } = setup({ views: three() });
    await choose(user, "B", t("dashboard.widget.delete"));
    expect(drawn()).toEqual(["A", "C"]);
    expect(widgetsOf(harness).map((widget) => widget.id)).toEqual(["a0000001", "a0000003"]);
  });

  it("disables Duplicate at 12 widgets", async () => {
    const twelve = Array.from({ length: 12 }, (_, n) => number(`a00000${String(n).padStart(2, "0")}`, `W${String(n)}`));
    const { user } = setup({ views: [dashboardView(twelve)] });
    const menu = await openMenu(user, "W0");
    expect(within(menu).getByRole("menuitem", { name: t("dashboard.widget.duplicate") })).toBeDisabled();
  });

  it("closes with Escape", async () => {
    const { user } = setup({ views: three() });
    await openMenu(user, "A");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
  });
});

// ----- adding a widget ---------------------------------------------------------------------------

describe("adding a widget", () => {
  it("offers a number, a chart and a view, and adds the one chosen at the end with its defaults and opens its settings", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "A")]), TABLE] });
    await user.click(screen.getByRole("button", { name: t("dashboard.add") }));
    const menu = await screen.findByRole("menu", { name: t("dashboard.add.menu") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      t("dashboard.add.number"),
      t("dashboard.add.chart"),
      t("dashboard.add.view"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: t("dashboard.add.chart") }));
    expect(drawn()).toEqual(["A", "Count by Status"]);
    const added = widgetsOf(harness)[1];
    expect(added).toMatchObject({ kind: "chart", w: 2, h: "m", title: "" });
    expect(added?.chart).toMatchObject({ type: "bar", groupBy: STATUS.id, measure: { fn: "count" } });
    expect(await screen.findByRole("dialog", { name: t("dashboard.settings") })).toBeInTheDocument();
  });

  it("adds a view widget that shows the first view that is not a dashboard", async () => {
    const { user, harness } = setup({ views: [dashboardView([]), TABLE, LIST] });
    await user.click(screen.getByRole("button", { name: t("dashboard.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("dashboard.add.view") }));
    expect(widgetsOf(harness)[0]).toMatchObject({ kind: "view", w: 4, h: "l", viewId: "tasks" });
    expect(drawn()).toEqual(["All tasks"]);
  });

  it("disables the view entry, with a hint, while the block has no view that is not a dashboard", async () => {
    const { user } = setup({ views: [dashboardView([])] });
    await user.click(screen.getByRole("button", { name: t("dashboard.add") }));
    const entry = await screen.findByRole("menuitem", { name: t("dashboard.add.view") });
    expect(entry).toBeDisabled();
    expect(entry).toHaveAttribute("title", t("dashboard.add.viewNone"));
  });

  it("disables the tile at 12 widgets", () => {
    const twelve = Array.from({ length: 12 }, (_, n) => number(`a00000${String(n).padStart(2, "0")}`, `W${String(n)}`));
    setup({ views: [dashboardView(twelve)] });
    const tile = screen.getByRole("button", { name: t("dashboard.add") });
    expect(tile).toBeDisabled();
    expect(tile).toHaveAttribute("title", t("dashboard.add.limit", { max: 12 }));
  });

  it("says what a dashboard is when it has no widget, and keeps the tile", () => {
    setup({ views: [dashboardView([])] });
    expect(screen.getByText(t("dashboard.empty"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("dashboard.add") })).toBeEnabled();
  });
});

// ----- the settings ------------------------------------------------------------------------------

describe("the settings of a widget", () => {
  it("edits the title (written when the field is left or on Enter), and an empty title is the automatic one", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "Pages")])] });
    const panel = await openSettings(user, "Pages");
    const field = within(panel).getByRole("textbox", { name: t("dashboard.settings.title") });
    expect(field).toHaveValue("Pages");
    await user.clear(field);
    await user.type(field, "Open pages{Enter}");
    expect(drawn()).toEqual(["Open pages"]);
    expect(widgetsOf(harness)[0]?.title).toBe("Open pages");
    await user.clear(field);
    // Left with the field empty: the automatic title shows (and is the hint of the field).
    await user.tab();
    expect(drawn()).toEqual(["Count all"]);
    expect(field).toHaveAttribute("placeholder", "Count all");
    expect(widgetsOf(harness)[0]?.title).toBe("");
  });

  it("writes a title that was typed when the panel is closed by a press outside it", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "Pages")])] });
    const panel = await openSettings(user, "Pages");
    const field = within(panel).getByRole("textbox", { name: t("dashboard.settings.title") });
    await user.clear(field);
    await user.type(field, "Typed");
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog", { name: t("dashboard.settings") })).toBeNull();
    expect(widgetsOf(harness)[0]?.title).toBe("Typed");
  });

  it("picks the column and the function of a number: the functions of the column's kind, and the count when the old one does not fit", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "")])] });
    const panel = await openSettings(user, "Count all");
    const column = within(panel).getByRole("combobox", { name: t("dashboard.settings.column") });
    expect(Array.from((column as HTMLSelectElement).options).map((option) => option.textContent)).toEqual([t("pageList.sort.title"), "Status", "Hours"]);
    await user.selectOptions(column, "Hours");
    expect(widgetsOf(harness)[0]?.calc).toEqual({ by: HOURS.id, fn: "count_all" });
    const fn = within(panel).getByRole("combobox", { name: t("dashboard.settings.function") });
    // A number column offers the sums first.
    expect(Array.from((fn as HTMLSelectElement).options)[0]?.textContent).toBe(t("properties.rollup.fn.sum"));
    await user.selectOptions(fn, t("properties.rollup.fn.sum"));
    expect(widgetsOf(harness)[0]?.calc).toEqual({ by: HOURS.id, fn: "sum" });
    expect(drawn()).toEqual(["Sum · Hours"]);
    expect(within(frame("Sum · Hours")).getByText("15")).toBeInTheDocument();
    // Back to a column that has no sum: the count of the pages.
    await user.selectOptions(column, "Status");
    expect(widgetsOf(harness)[0]?.calc).toEqual({ by: STATUS.id, fn: "count_all" });
  });

  it("asks for another column when the number's column was deleted", async () => {
    const { user } = setup({
      views: [dashboardView([number("a0000001", "Gone", { calc: { by: "de000000-0000-4000-8000-0000000000ff", fn: "sum" } })])],
    });
    const panel = await openSettings(user, "Gone");
    const column = within(panel).getByRole<HTMLSelectElement>("combobox", { name: t("dashboard.settings.column") });
    expect(column.value).toBe("");
    expect(within(column).getByRole("option", { name: t("dashboard.settings.columnChoose") })).toBeDisabled();
  });

  it("hands the chart's part of the settings to the chart and writes what it answers", async () => {
    const { user, harness } = setup({ views: [dashboardView([chart("a0000001", "By status")])] });
    const panel = await openSettings(user, "By status");
    await user.click(within(panel).getByRole("button", { name: "Make it a donut" }));
    expect(widgetsOf(harness)[0]?.chart?.type).toBe("donut");
    expect(within(frame("By status")).getByTestId("chart-body")).toHaveAttribute("data-type", "donut");
  });

  it("picks the view a view widget shows, among the views that are not dashboards", async () => {
    const { user, harness } = setup({ views: [dashboardView([embedded("a0000001", "", "tasks")]), TABLE, LIST] });
    const panel = await openSettings(user, "All tasks");
    const select = within(panel).getByRole<HTMLSelectElement>("combobox", { name: t("dashboard.settings.view") });
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual(["All tasks", "Pages"]);
    await user.selectOptions(select, "Pages");
    expect(widgetsOf(harness)[0]?.viewId).toBe("list");
    expect(drawn()).toEqual(["Pages"]);
  });

  it("has no filter for a view widget, and a filter button with the number of filters for a number and a chart", async () => {
    const { user } = setup({ views: [dashboardView([embedded("a0000001", "", "tasks"), number("a0000002", "To do", { filter: TODO_ONLY })]), TABLE] });
    const viewPanel = await openSettings(user, "All tasks");
    expect(within(viewPanel).queryByRole("button", { name: new RegExp(t("dashboard.settings.filterEdit")) })).toBeNull();
    fireEvent.pointerDown(document.body);
    const numberPanel = await openSettings(user, "To do");
    const button = within(numberPanel).getByRole("button", { name: new RegExp(t("dashboard.settings.filterEdit")) });
    expect(within(button).getByRole("img", { name: "1 active filter" })).toBeInTheDocument();
  });

  it("edits the widget's own filter in the filter editor, which leaves the settings open", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "Pages")])] });
    const panel = await openSettings(user, "Pages");
    await user.click(within(panel).getByRole("button", { name: new RegExp(t("dashboard.settings.filterEdit")) }));
    const editor = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    await user.click(within(editor).getByRole("button", { name: t("pageList.filter.add") }));
    // The filter is written, the badge on the frame counts it, and the settings are still there.
    expect(widgetsOf(harness)[0]?.filter?.items).toHaveLength(1);
    expect(within(frame("Pages")).getByRole("img", { name: "1 filter on this widget" })).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: t("dashboard.settings") })).toBeInTheDocument();
    // Removing the filter writes the empty filter; Escape closes the editor first, and the settings only the next time.
    await user.click(within(editor).getByRole("button", { name: t("pageList.filter.remove") }));
    expect(widgetsOf(harness)[0]?.filter).toBeUndefined();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("pageList.filter.label") })).toBeNull();
    expect(screen.getByRole("dialog", { name: t("dashboard.settings") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("dashboard.settings") })).toBeNull();
  });
});

// ----- an embedded view --------------------------------------------------------------------------

describe("an embedded view", () => {
  it("draws the view with its layout's component over the pages after the dashboard's filter, the view's filter and the view's sort", () => {
    const sorted = otherView({
      id: "tasks",
      name: "All tasks",
      layout: "list",
      sort: { by: "title", dir: "desc" },
      filter: { op: "and", items: [{ propertyId: STATUS.id, op: "hasOption", value: "todo" }] },
    });
    setup({
      views: [
        // The dashboard leaves out Gamma (no status); the view keeps the pages to do (not Beta), by title, last first.
        dashboardView([embedded("a0000001", "Tasks", "tasks")], { filter: { op: "and", items: [{ propertyId: STATUS.id, op: "isSet" }] } }),
        sorted,
      ],
    });
    const names = Array.from(frame("Tasks").querySelectorAll(".page-list-name")).map((name) => name.textContent);
    expect(names).toEqual(["Delta", "Alpha"]);
  });

  it("edits its own view, not the dashboard: the table's sort goes to the table view", async () => {
    const { user, harness } = setup({ views: [dashboardView([embedded("a0000001", "Tasks", "tasks")]), TABLE] });
    const body = within(frame("Tasks"));
    expect(body.getByRole("grid")).toBeInTheDocument();
    expect(body.getByRole("button", { name: t("pageTable.open", { title: "Alpha" }) })).toBeInTheDocument();
    await user.click(body.getByRole("button", { name: HOURS.name }));
    await user.click(await screen.findByRole("menuitem", { name: t("pageTable.sortDesc") }));
    const [dashboard, table] = harness.views();
    expect(table?.sort).toEqual({ by: HOURS.id, dir: "desc" });
    expect(dashboard?.sort).toEqual({ by: "title", dir: "asc" });
    // The dashboard's widgets did not change.
    expect(dashboard?.widgets).toHaveLength(1);
    // The table now lists the pages by hours, most first.
    const titles = within(frame("Tasks")).getAllByRole("row").slice(1).map((row) => row.textContent ?? "");
    expect(titles[0]).toContain("Beta");
  });

  it("opens a page as the embedded view's own `openIn` says, and the dashboard's chart the way the dashboard's view says", async () => {
    const { user, harness } = setup({ views: [dashboardView([embedded("a0000001", "Pages", "list")]), LIST] });
    await user.click(within(frame("Pages")).getByRole("button", { name: "Alpha" }));
    expect(harness.openInView).toHaveBeenCalledTimes(1);
    const [target, opened, disposition] = harness.openInView.mock.calls[0] ?? [];
    expect(target?.id).toBe("list");
    expect(target?.openIn).toBe("center");
    expect(opened?.title).toBe("Alpha");
    expect(disposition).toBe("current");
    expect(harness.onOpen).not.toHaveBeenCalled();
  });

  it("has an Open view button that makes that view the active one", async () => {
    const { user, harness } = setup({ views: [dashboardView([embedded("a0000001", "Tasks", "tasks"), number("a0000002", "N")]), TABLE] });
    expect(within(frame("N")).queryByRole("button", { name: /Open view/u })).toBeNull();
    await user.click(within(frame("Tasks")).getByRole("button", { name: t("dashboard.view.openNamed", { name: "Tasks" }) }));
    expect(harness.activated()).toBe("tasks");
  });

  it("says the view was deleted, with Remove, for a view that is gone and for a dashboard", async () => {
    const { user, harness } = setup({
      views: [
        dashboardView([embedded("a0000001", "Gone", "nope"), embedded("a0000002", "Itself", "dash"), embedded("a0000003", "Nothing", "")]),
        TABLE,
      ],
    });
    for (const title of ["Gone", "Itself", "Nothing"]) {
      expect(within(frame(title)).getByText(t("dashboard.view.deleted"))).toBeInTheDocument();
      expect(within(frame(title)).queryByRole("button", { name: /Open view/u })).toBeNull();
    }
    await user.click(within(frame("Itself")).getByRole("button", { name: t("dashboard.widget.remove") }));
    expect(drawn()).toEqual(["Gone", "Nothing"]);
    expect(widgetsOf(harness).map((widget) => widget.id)).toEqual(["a0000001", "a0000003"]);
  });

  it("turns into the message once the view it showed is deleted from the block", () => {
    const dashboard = dashboardView([embedded("a0000001", "Tasks", "tasks")]);
    const { unmount } = setup({ views: [dashboard, TABLE] });
    expect(within(frame("Tasks")).getByRole("grid")).toBeInTheDocument();
    unmount();
    // The same dashboard in a block that no longer has the table.
    setup({ views: [dashboard] });
    expect(within(frame("Tasks")).getByText(t("dashboard.view.deleted"))).toBeInTheDocument();
  });
});

// ----- a widget of a newer version ----------------------------------------------------------------

describe("a widget this version cannot read", () => {
  const NEWER = { id: "a0000009", kind: "gauge", title: "Speed", value: 7 };

  it("is drawn as needing a newer version, can be moved and removed, and is written back as it was by every other change", async () => {
    const { user, harness } = setup({ views: [dashboardView([number("a0000001", "A"), NEWER, number("a0000002", "B")])] });
    expect(drawn()).toEqual(["A", t("dashboard.widget.unknownTitle"), "B"]);
    expect(screen.getByText(t("dashboard.widget.unknown"))).toBeInTheDocument();
    // Another widget changes: the unreadable one is in the written list untouched.
    await choose(user, "A", t("dashboard.widget.duplicate"));
    const written = harness.views()[0] === undefined ? [] : (viewToJson(harness.views()[0] as PageListViewSpec).widgets as unknown[]);
    expect(written).toHaveLength(4);
    expect(written[2]).toEqual(NEWER);
    // Its own menu has no Edit, no sizes and no Duplicate.
    const menu = await openMenu(user, t("dashboard.widget.unknownTitle"));
    expect(within(menu).queryByRole("menuitem", { name: t("dashboard.widget.edit") })).toBeNull();
    expect(within(menu).queryByRole("menuitemradio")).toBeNull();
    expect(within(menu).queryByRole("menuitem", { name: t("dashboard.widget.duplicate") })).toBeNull();
    await user.click(within(menu).getByRole("menuitem", { name: t("dashboard.widget.delete") }));
    expect(drawn()).toEqual(["A", "A", "B"]);
  });

  it("can be removed with the button of its message", async () => {
    const { user } = setup({ views: [dashboardView([NEWER])] });
    await user.click(screen.getByRole("button", { name: t("dashboard.widget.remove") }));
    expect(drawn()).toEqual([]);
  });
});

// ----- dragging by the header ---------------------------------------------------------------------

describe("dragging a widget by its header", () => {
  /** The frames side by side, 100 px wide each, in the order they are drawn now. */
  function layOut(): void {
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
      const id = this.getAttribute("data-widget-id");
      const left = id === null ? 0 : Array.from(document.querySelectorAll("[data-widget-id]")).indexOf(this) * 100;
      const width = id === null ? 0 : 100;
      return { x: left, y: 0, left, top: 0, width, height: id === null ? 0 : 100, right: left + width, bottom: id === null ? 0 : 100, toJSON: () => ({}) };
    });
  }

  const header = (title: string): HTMLElement => {
    const found = frame(title).querySelector<HTMLElement>(".page-dashboard-widget-header");
    if (found === null) {
      throw new Error("no header");
    }
    return found;
  };

  const press = (title: string): void => {
    fireEvent.pointerDown(within(header(title)).getByRole("heading"), { clientX: 50, clientY: 10, button: 0, pointerId: 1, pointerType: "mouse" });
  };

  const three = (): PageListViewSpec[] => [dashboardView([number("a0000001", "A"), number("a0000002", "B"), number("a0000003", "C")])];

  it("shows a drop indicator while it moves and writes the new order, in one change, when it is dropped", () => {
    layOut();
    const { harness } = setup({ views: three() });
    const before = harness.views()[0];
    press("A");
    // Past the middle of C: after it.
    fireEvent.pointerMove(document, { clientX: 290, clientY: 20, pointerId: 1 });
    expect(frame("C")).toHaveAttribute("data-drop", "after");
    expect(frame("A")).toHaveAttribute("data-dragging");
    // In the left half of B: before it, which is where A is next to: nothing to show.
    fireEvent.pointerMove(document, { clientX: 120, clientY: 20, pointerId: 1 });
    expect(frame("B")).not.toHaveAttribute("data-drop");
    // In the left half of C: before C.
    fireEvent.pointerMove(document, { clientX: 210, clientY: 20, pointerId: 1 });
    expect(frame("C")).toHaveAttribute("data-drop", "before");
    fireEvent.pointerUp(document, { clientX: 210, clientY: 20, pointerId: 1 });
    expect(drawn()).toEqual(["B", "A", "C"]);
    expect(frame("A")).not.toHaveAttribute("data-dragging");
    expect(harness.views()[0]).not.toBe(before);
    expect(widgetsOf(harness).map((widget) => widget.title)).toEqual(["B", "A", "C"]);
  });

  it("drops after the last widget, and before the first", () => {
    layOut();
    setup({ views: three() });
    press("A");
    fireEvent.pointerMove(document, { clientX: 299, clientY: 20, pointerId: 1 });
    fireEvent.pointerUp(document, { clientX: 299, clientY: 20, pointerId: 1 });
    expect(drawn()).toEqual(["B", "C", "A"]);
    layOut();
    press("A");
    fireEvent.pointerMove(document, { clientX: 5, clientY: 20, pointerId: 1 });
    expect(frame("B")).toHaveAttribute("data-drop", "before");
    fireEvent.pointerUp(document, { clientX: 5, clientY: 20, pointerId: 1 });
    expect(drawn()).toEqual(["A", "B", "C"]);
  });

  it("is cancelled by Escape and writes nothing", () => {
    layOut();
    const { harness } = setup({ views: three() });
    const before = harness.views()[0];
    press("A");
    fireEvent.pointerMove(document, { clientX: 290, clientY: 20, pointerId: 1 });
    fireEvent.keyDown(document, { key: "Escape" });
    expect(frame("C")).not.toHaveAttribute("data-drop");
    fireEvent.pointerUp(document, { clientX: 290, clientY: 20, pointerId: 1 });
    expect(drawn()).toEqual(["A", "B", "C"]);
    expect(harness.views()[0]).toBe(before);
  });

  it("is not a drag until the pointer has moved a few pixels, so a press on the title writes nothing", () => {
    layOut();
    const { harness } = setup({ views: three() });
    const before = harness.views()[0];
    press("A");
    fireEvent.pointerMove(document, { clientX: 52, clientY: 11, pointerId: 1 });
    expect(frame("A")).not.toHaveAttribute("data-dragging");
    fireEvent.pointerUp(document, { clientX: 52, clientY: 11, pointerId: 1 });
    expect(harness.views()[0]).toBe(before);
  });

  it("does not begin from a button of the header", () => {
    layOut();
    setup({ views: three() });
    fireEvent.pointerDown(within(frame("A")).getByRole("button", { name: menuName("A") }), { clientX: 90, clientY: 10, button: 0, pointerId: 1 });
    fireEvent.pointerMove(document, { clientX: 290, clientY: 20, pointerId: 1 });
    expect(frame("C")).not.toHaveAttribute("data-drop");
  });
});

// ----- read-only ---------------------------------------------------------------------------------

describe("a read-only editor", () => {
  it("draws the widgets without menus, a handle or a tile, and its embedded views are read-only too", () => {
    setup({ views: [dashboardView([number("a0000001", "Pages"), chart("a0000002", "By status"), embedded("a0000003", "Tasks", "tasks")]), TABLE], editable: false });
    expect(drawn()).toEqual(["Pages", "By status", "Tasks"]);
    expect(within(frame("Pages")).getByText("4")).toBeInTheDocument();
    expect(within(frame("By status")).getByTestId("chart-body")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t("dashboard.add") })).toBeNull();
    for (const title of ["Pages", "By status", "Tasks"]) {
      expect(within(frame(title)).queryByRole("button", { name: menuName(title) })).toBeNull();
      expect(frame(title).querySelector(".page-dashboard-widget-header")).not.toHaveAttribute("data-draggable");
    }
    // The embedded table's header is not a menu: the view cannot be changed.
    const grid = within(frame("Tasks")).getByRole("grid");
    expect(within(grid).queryByRole("button", { name: HOURS.name })).toBeNull();
    expect(within(grid).queryByRole("separator")).toBeNull();
    // Opening the view in its own tab is a choice of this device, not a write: it stays.
    expect(within(frame("Tasks")).getByRole("button", { name: t("dashboard.view.openNamed", { name: "Tasks" }) })).toBeInTheDocument();
  });

  it("says so when there is nothing to show, without a tile, and gives a deleted view and a newer widget no Remove", () => {
    const { unmount } = setup({ views: [dashboardView([])], editable: false });
    expect(screen.getByText(t("dashboard.emptyReadOnly"))).toBeInTheDocument();
    expect(screen.queryByRole("button")).toBeNull();
    unmount();
    setup({ views: [dashboardView([embedded("a0000001", "Gone", "nope"), { id: "a0000002", kind: "gauge" }])], editable: false });
    expect(screen.getByText(t("dashboard.view.deleted"))).toBeInTheDocument();
    expect(screen.getByText(t("dashboard.widget.unknown"))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t("dashboard.widget.remove") })).toBeNull();
  });

  it("does not start a drag from a header", () => {
    setup({ views: [dashboardView([number("a0000001", "A"), number("a0000002", "B")])], editable: false });
    const header = frame("A").querySelector<HTMLElement>(".page-dashboard-widget-header");
    if (header === null) {
      throw new Error("no header");
    }
    fireEvent.pointerDown(header, { clientX: 50, clientY: 10, button: 0, pointerId: 1 });
    fireEvent.pointerMove(document, { clientX: 290, clientY: 20, pointerId: 1 });
    expect(frame("A")).not.toHaveAttribute("data-dragging");
    expect(header.closest("[data-dragging]")).toBeNull();
  });
});
