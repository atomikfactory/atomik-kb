// The chart widget drawn with props (ADR-0055, `docs/architecture/dashboards.md` section 4): bar, line and donut with the marks the data
// says, the tooltip on hover and on the keys, the popover of a mark's pages and how a page opens, the accessible name and table, the
// messages of a chart with nothing to draw, names from the app's store, and the settings that call `onChange`.

import type { PageInfo, PropertyDef, PropertyKind, PropertyValue } from "@atomik-kb/core-client";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { t } from "../../i18n";
import type { AppStore } from "../../store";
import { StoreContext } from "../../store-context";
import { ChartWidgetBody, ChartWidgetSettings } from "./ChartWidget";
import type { ChartSpec, ChartWidgetBodyProps } from "./dashboard-types";

const SPACE = "11111111-1111-4111-8111-111111111111";

function def(id: string, name: string, kind: PropertyKind, extra: Partial<PropertyDef> = {}): PropertyDef {
  return { id, name, kind, options: [], createdAt: 1, ...extra };
}

const STATUS = def("p-status", "Status", "status", {
  options: [
    { id: "todo", name: "To do", color: "gray", group: "todo" },
    { id: "doing", name: "Doing", color: "blue", group: "in_progress" },
    { id: "done", name: "Done", color: "green", group: "done" },
  ],
});
const NOTE = def("p-note", "Note", "text");
const DUE = def("p-due", "Due", "date");
const HOURS = def("p-hours", "Hours", "number");
const OWNER = def("p-owner", "Owner", "person");
const PROJECT = def("p-project", "Project", "relation", { config: { kind: "relation", twoWay: false } });
const TOTAL = def("f-total", "Total", "formula", { config: { kind: "formula", expression: "", resultType: { base: "number", list: false } } });
const SCHEMA = [STATUS, NOTE, DUE, HOURS, OWNER, PROJECT, TOTAL];

let counter = 0;
function page(values: [string, PropertyValue][] = [], extra: Partial<PageInfo> = {}): PageInfo {
  counter += 1;
  return {
    id: `22222222-2222-4222-8222-${String(counter).padStart(12, "0")}`,
    spaceId: SPACE,
    title: `Page ${String(counter)}`,
    createdAt: new Date(2026, 9, 5, 12).getTime(),
    updatedAt: new Date(2026, 9, 6, 12).getTime(),
    properties: values,
    ...extra,
  };
}

const status = (value: string): [string, PropertyValue] => ["p-status", { kind: "status", value }];
const hours = (value: number): [string, PropertyValue] => ["p-hours", { kind: "number", value }];
const due = (date: string): [string, PropertyValue] => ["p-due", { kind: "date", value: { date } }];
const note = (value: string): [string, PropertyValue] => ["p-note", { kind: "text", value }];

const BAR: ChartSpec = { type: "bar", groupBy: "p-status", measure: { fn: "count" } };
const ROWS = [page([status("todo")]), page([status("done")]), page([status("todo")])];

function renderBody(props: Partial<ChartWidgetBodyProps> = {}, store: AppStore | null = null) {
  const onOpen = vi.fn();
  const all: ChartWidgetBodyProps = {
    chart: BAR,
    rows: ROWS,
    schema: SCHEMA,
    spaceId: SPACE,
    width: 400,
    height: 240,
    today: "2026-10-18",
    title: "Count by Status",
    onOpen,
    ...props,
  };
  const view = render(
    <StoreContext.Provider value={store}>
      <ChartWidgetBody {...all} />
    </StoreContext.Provider>,
  );
  return { onOpen, ...view };
}

const plot = (): HTMLElement => screen.getByRole("img", { name: /Count by Status|:/u });
const marks = (container: HTMLElement, selector: string): HTMLElement[] => Array.from(container.querySelectorAll<HTMLElement>(selector));

describe("the bar chart", () => {
  it("draws a bar for each option, in the options' order, in the option's colour, and no bar for a group of nothing", () => {
    const { container } = renderBody();
    const bars = marks(container, ".chart-bar");
    expect(bars.map((bar) => bar.dataset.key)).toEqual(["todo", "doing", "done"]);
    expect(bars.map((bar) => bar.dataset.value)).toEqual(["2", "0", "1"]);
    expect(bars.map((bar) => bar.querySelector(".chart-mark")?.getAttribute("data-color") ?? null)).toEqual(["gray", null, "green"]);
    // The value axis starts at 0 and has a tick label for each of its ticks.
    expect(Array.from(container.querySelectorAll(".chart-tick")).map((tick) => tick.textContent)).toEqual(["0", "1", "2"]);
    expect(Array.from(container.querySelectorAll(".chart-category")).map((label) => label.textContent)).toEqual(["To do", "Doing", "Done"]);
  });

  it("is a picture named by the chart with its groups and values, and has a table of the same data", () => {
    renderBody();
    expect(screen.getByRole("img", { name: "Count by Status: To do: 2; Doing: 0; Done: 1" })).toBeInTheDocument();
    const table = screen.getByRole("table", { name: "Count by Status" });
    const rows = within(table).getAllByRole("row");
    expect(rows.map((row) => row.textContent)).toEqual(["GroupCount", "To do2", "Doing0", "Done1"]);
  });

  it("is horizontal when the spec says so", () => {
    const { container } = renderBody({ chart: { ...BAR, horizontal: true } });
    expect(container.querySelector(".chart-plot")).toHaveAttribute("data-orientation", "horizontal");
    expect(marks(container, ".chart-bar")).toHaveLength(3);
    // The labels sit on the left of the bars, each on its own row.
    expect(Array.from(container.querySelectorAll(".chart-category")).map((label) => label.getAttribute("text-anchor"))).toEqual(["end", "end", "end"]);
  });

  it("cuts a long label with an ellipsis and keeps the whole text in a tooltip", () => {
    const long = "A very long category name that cannot fit";
    const { container } = renderBody({ chart: { ...BAR, groupBy: "p-note" }, rows: [page([note(long)]), page([note("Short")])], width: 200 });
    const labels = Array.from(container.querySelectorAll(".chart-category"));
    const cut = labels.find((label) => label.textContent?.includes("…") === true);
    expect(cut).toBeDefined();
    expect(cut?.querySelector("title")?.textContent).toBe(long);
    expect(labels.find((label) => label.textContent === "Short")?.querySelector("title")).toBeNull();
  });

  it("draws a negative value below the baseline", () => {
    const rows = [page([status("todo"), hours(-4)]), page([status("done"), hours(6)])];
    const { container } = renderBody({ chart: { ...BAR, measure: { fn: "sum", propertyId: "p-hours" } }, rows });
    const todo = container.querySelector('.chart-bar[data-key="todo"] .chart-mark')?.getAttribute("d") ?? "";
    const done = container.querySelector('.chart-bar[data-key="done"] .chart-mark')?.getAttribute("d") ?? "";
    expect(todo).not.toBe("");
    expect(done).not.toBe("");
    const start = (path: string): number => Number(path.match(/^M[\d.-]+ ([\d.-]+)/u)?.[1]);
    const corner = (path: string): number => Number(path.match(/Q[\d.-]+ ([\d.-]+)/u)?.[1]);
    // Both bars start on the baseline; the rounded end of the positive one is above it, the negative one's below.
    expect(start(todo)).toBeCloseTo(start(done));
    expect(corner(done)).toBeLessThan(start(done));
    expect(corner(todo)).toBeGreaterThan(start(todo));
  });

  it("has the axis of a count on whole numbers", () => {
    const { container } = renderBody({ rows: [page([status("done")])] });
    expect(Array.from(container.querySelectorAll(".chart-tick")).map((tick) => tick.textContent)).toEqual(["0", "1"]);
  });

  it("gives each bar a colour of the palette up to eight, and one colour past that, which are not cycled", () => {
    const few = Array.from({ length: 3 }, (_, index) => page([note(`item ${String(index)}`)]));
    const { container, unmount } = renderBody({ chart: { ...BAR, groupBy: "p-note" }, rows: few });
    expect(marks(container, ".chart-bar .chart-mark").map((mark) => mark.getAttribute("data-series"))).toEqual(["1", "2", "3"]);
    unmount();
    const many = Array.from({ length: 9 }, (_, index) => page([note(`item ${String(index)}`)]));
    const nine = renderBody({ chart: { ...BAR, groupBy: "p-note" }, rows: many });
    expect(marks(nine.container, ".chart-bar .chart-mark").map((mark) => mark.getAttribute("data-series"))).toEqual(Array<string>(9).fill("1"));
  });

  it("says how many bars it left out", () => {
    const rows = Array.from({ length: 60 }, (_, index) => page([note(`item ${String(index).padStart(2, "0")}`)]));
    const { container } = renderBody({ chart: { ...BAR, groupBy: "p-note" }, rows });
    expect(marks(container, ".chart-bar")).toHaveLength(50);
    expect(screen.getByText("+10 more")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /; \+10 more$/u })).toBeInTheDocument();
  });
});

describe("the tooltip", () => {
  it("shows the group's name and value under the mouse", async () => {
    const user = userEvent.setup();
    const { container } = renderBody();
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.hover(container.querySelector('.chart-bar[data-key="todo"]') as HTMLElement);
    const tooltip = screen.getByRole("tooltip");
    expect(tooltip).toHaveTextContent("To do");
    expect(tooltip).toHaveTextContent("Count: 2");
    await user.hover(container.querySelector('.chart-bar[data-key="done"]') as HTMLElement);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Count: 1");
    await user.unhover(container.querySelector('.chart-bar[data-key="done"]') as HTMLElement);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("names the measure and the number of pages for a sum, and shows a group without a value as a dash", async () => {
    const user = userEvent.setup();
    const rows = [page([status("todo"), hours(2)]), page([status("todo"), hours(4.256)]), page([status("done")])];
    const { container } = renderBody({ chart: { ...BAR, measure: { fn: "sum", propertyId: "p-hours" } }, rows });
    await user.hover(container.querySelector('.chart-bar[data-key="todo"]') as HTMLElement);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Sum of Hours: 6.26");
    expect(screen.getByRole("tooltip")).toHaveTextContent("2 pages");
    await user.hover(container.querySelector('.chart-bar[data-key="done"]') as HTMLElement);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Sum of Hours: —");
    expect(screen.getByRole("tooltip")).toHaveTextContent("1 page");
  });

  it("follows the keys: the chart is one tab stop and the arrows move between marks", async () => {
    const user = userEvent.setup();
    renderBody();
    expect(plot()).toHaveAttribute("tabindex", "0");
    expect(document.querySelectorAll(".chart-body [tabindex]")).toHaveLength(1);
    await user.tab();
    expect(plot()).toHaveFocus();
    expect(screen.getByRole("tooltip")).toHaveTextContent("To do");
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Doing");
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Done");
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Done");
    await user.keyboard("{Home}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("To do");
    await user.keyboard("{End}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Done");
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Doing");
    expect(plot()).toHaveAttribute("aria-describedby", screen.getByRole("tooltip").id);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).toBeNull();
    await user.tab();
    expect(plot()).not.toHaveFocus();
  });
});

describe("the pages of a mark", () => {
  it("opens on Enter with the group's pages, and a page opens as the gesture says", async () => {
    const user = userEvent.setup();
    const { onOpen } = renderBody();
    await user.tab();
    await user.keyboard("{ArrowRight}{ArrowRight}{Enter}");
    const dialog = screen.getByRole("dialog", { name: "Pages of Done" });
    expect(within(dialog).getByText("Count: 1")).toBeInTheDocument();
    const buttons = within(dialog).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([ROWS[1]?.title]);
    await user.click(buttons[0] as HTMLElement);
    expect(onOpen).toHaveBeenCalledExactlyOnceWith(ROWS[1], "current");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens a tab on Ctrl+click, a tab in front on Ctrl+Enter, and stays open for a tab in the background", async () => {
    const user = userEvent.setup();
    const { onOpen, container } = renderBody();
    await user.click(container.querySelector('.chart-bar[data-key="todo"]') as HTMLElement);
    const dialog = screen.getByRole("dialog", { name: "Pages of To do" });
    const [first, second] = within(dialog).getAllByRole("button");
    fireEvent.click(first as HTMLElement, { ctrlKey: true });
    expect(onOpen).toHaveBeenLastCalledWith(ROWS[0], "background");
    expect(screen.getByRole("dialog", { name: "Pages of To do" })).toBeInTheDocument();
    (second as HTMLElement).focus();
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onOpen).toHaveBeenLastCalledWith(ROWS[2], "foreground");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("opens on a click on a mark, shows another group's pages on a click on another, and closes on the same mark or Escape", async () => {
    const user = userEvent.setup();
    const { container } = renderBody();
    const todo = container.querySelector('.chart-bar[data-key="todo"]') as HTMLElement;
    const done = container.querySelector('.chart-bar[data-key="done"]') as HTMLElement;
    await user.click(todo);
    expect(screen.getByRole("dialog", { name: "Pages of To do" })).toBeInTheDocument();
    await user.click(done);
    expect(screen.getByRole("dialog", { name: "Pages of Done" })).toBeInTheDocument();
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    await user.click(done);
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(todo);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("lists at most 50 pages and says how many more there are, and says when there are none", async () => {
    const user = userEvent.setup();
    const rows = Array.from({ length: 53 }, () => page([status("todo")]));
    const { container } = renderBody({ rows });
    await user.click(container.querySelector('.chart-bar[data-key="todo"]') as HTMLElement);
    const dialog = screen.getByRole("dialog", { name: "Pages of To do" });
    expect(within(dialog).getAllByRole("button")).toHaveLength(50);
    expect(within(dialog).getByText("and 3 more")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.click(container.querySelector('.chart-bar[data-key="doing"]') as HTMLElement);
    expect(screen.getByText("No pages")).toBeInTheDocument();
  });
});

describe("the line chart", () => {
  const LINE: ChartSpec = { type: "line", groupBy: "p-due", bucket: "month", measure: { fn: "count" } };
  const rows = [page([due("2026-08-10"), hours(2)]), page([due("2026-08-20"), hours(3)]), page([due("2026-10-01"), hours(5)])];

  it("joins the points in order with a line and an area, and has a point for every bucket, the empty ones too", () => {
    const { container } = renderBody({ chart: LINE, rows });
    expect(marks(container, ".chart-point").map((point) => point.dataset.value)).toEqual(["2", "0", "1"]);
    expect(container.querySelectorAll(".chart-line")).toHaveLength(1);
    expect(container.querySelectorAll(".chart-area")).toHaveLength(1);
    expect(Array.from(container.querySelectorAll(".chart-category")).map((label) => label.textContent)).toEqual(["Aug 2026", "Sep 2026", "Oct 2026"]);
    // No dot until a point is under the mouse or the keys.
    expect(container.querySelectorAll(".chart-dot")).toHaveLength(0);
  });

  it("shows a dot and a tooltip for the point under the mouse", async () => {
    const user = userEvent.setup();
    const { container } = renderBody({ chart: LINE, rows });
    await user.hover(marks(container, ".chart-point")[2] as HTMLElement);
    expect(container.querySelectorAll(".chart-dot")).toHaveLength(1);
    expect(screen.getByRole("tooltip")).toHaveTextContent("October 2026");
    expect(screen.getByRole("tooltip")).toHaveTextContent("Count: 1");
  });

  it("breaks the line at a group without a value and draws a point alone as a dot", () => {
    const { container } = renderBody({ chart: { ...LINE, measure: { fn: "sum", propertyId: "p-hours" } }, rows });
    expect(marks(container, ".chart-point").map((point) => point.dataset.value)).toEqual(["5", "", "5"]);
    // Two runs of one point each: two dots and no line between them.
    expect(container.querySelectorAll(".chart-line")).toHaveLength(0);
    expect(container.querySelectorAll(".chart-dot")).toHaveLength(2);
  });

  it("adds up when it is cumulative", () => {
    const { container } = renderBody({ chart: { ...LINE, cumulative: true }, rows });
    expect(marks(container, ".chart-point").map((point) => point.dataset.value)).toEqual(["2", "2", "3"]);
  });
});

describe("the donut chart", () => {
  const DONUT: ChartSpec = { type: "donut", groupBy: "p-status", measure: { fn: "count" } };
  const rows = [page([status("todo")]), page([status("done")]), page([status("todo")]), page([status("todo")])];

  it("draws a slice for each group above 0, with the total in the centre and a legend", () => {
    const { container } = renderBody({ chart: DONUT, rows });
    expect(marks(container, ".chart-slice").map((slice) => slice.dataset.key)).toEqual(["todo", "done"]);
    expect(container.querySelector(".chart-donut-value")?.textContent).toBe("4");
    expect(container.querySelector(".chart-donut-caption")?.textContent).toBe("Count");
    const legend = marks(container, ".chart-legend-item");
    expect(legend.map((item) => item.querySelector(".chart-legend-label")?.textContent)).toEqual(["To do", "Done"]);
    expect(legend.map((item) => item.querySelector(".chart-legend-value")?.textContent)).toEqual(["375%", "125%"]);
    expect(legend.map((item) => item.querySelector(".chart-legend-share")?.textContent)).toEqual(["75%", "25%"]);
    expect(container.querySelector(".chart-donut")).toHaveAttribute("data-legend", "side");
  });

  it("gives the slices of a date source each a colour of their own, not the one series colour of bars and lines", () => {
    const dated = [page([due("2026-08-10")]), page([due("2026-09-10")]), page([due("2026-10-01")])];
    const { container } = renderBody({ chart: { ...DONUT, groupBy: "p-due" }, rows: dated });
    expect(marks(container, ".chart-slice .chart-mark").map((mark) => mark.getAttribute("data-series"))).toEqual(["1", "2", "3"]);
    const bars = renderBody({ chart: { ...BAR, groupBy: "p-due" }, rows: dated });
    expect(marks(bars.container, ".chart-bar .chart-mark").map((mark) => mark.getAttribute("data-series"))).toEqual(["1", "1", "1"]);
  });

  it("puts the legend under the ring in a narrow widget", () => {
    const { container } = renderBody({ chart: DONUT, rows, width: 220, height: 280 });
    expect(container.querySelector(".chart-donut")).toHaveAttribute("data-legend", "below");
  });

  it("shows the share of a slice in its tooltip and its value in the centre", async () => {
    const user = userEvent.setup();
    const { container } = renderBody({ chart: DONUT, rows });
    await user.hover(container.querySelector('.chart-slice[data-key="todo"]') as HTMLElement);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Count: 3 (75%)");
    expect(container.querySelector(".chart-donut-value")?.textContent).toBe("3");
    await user.unhover(container.querySelector('.chart-slice[data-key="todo"]') as HTMLElement);
    expect(container.querySelector(".chart-donut-value")?.textContent).toBe("4");
  });

  it("groups the slices past the eighth in Other, in a neutral colour", () => {
    const many = Array.from({ length: 10 }, (_, index) => Array.from({ length: 10 - index }, () => page([note(`t${String(index + 1)}`)]))).flat();
    const { container } = renderBody({ chart: { ...DONUT, groupBy: "p-note" }, rows: many });
    const slices = marks(container, ".chart-slice");
    expect(slices).toHaveLength(9);
    expect(slices.at(-1)?.dataset.key).toBe("~other");
    expect(slices.at(-1)?.querySelector(".chart-mark")).toHaveAttribute("data-series", "other");
    expect(container.querySelector(".chart-donut-value")?.textContent).toBe("55");
    expect(within(container.querySelector(".chart-legend") as HTMLElement).getByText("Other")).toBeInTheDocument();
  });

  it("opens the pages of a slice on a click on its legend entry", async () => {
    const user = userEvent.setup();
    const { container } = renderBody({ chart: DONUT, rows });
    await user.click(marks(container, ".chart-legend-item")[1] as HTMLElement);
    expect(screen.getByRole("dialog", { name: "Pages of Done" })).toBeInTheDocument();
  });

  it("does not call an average a share", async () => {
    const user = userEvent.setup();
    const withHours = [page([status("todo"), hours(2)]), page([status("done"), hours(6)])];
    const { container } = renderBody({ chart: { ...DONUT, measure: { fn: "average", propertyId: "p-hours" } }, rows: withHours });
    expect(container.querySelector(".chart-donut-value")?.textContent).toBe("4");
    expect(container.querySelector(".chart-legend-share")).toBeNull();
    await user.hover(container.querySelector('.chart-slice[data-key="done"]') as HTMLElement);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Average of Hours: 6");
    expect(screen.getByRole("tooltip")).not.toHaveTextContent("%");
  });
});

describe("a chart with nothing to draw", () => {
  it("asks what to group by when none is chosen or the property is gone", () => {
    const { unmount } = renderBody({ chart: { ...BAR, groupBy: null } });
    expect(screen.getByText("Choose what to group by")).toBeInTheDocument();
    unmount();
    renderBody({ chart: { ...BAR, groupBy: "p-gone" } });
    expect(screen.getByText("Choose what to group by")).toBeInTheDocument();
  });

  it("says there is no data when no page is left", () => {
    renderBody({ rows: [] });
    expect(screen.getByText("No data")).toBeInTheDocument();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("asks for a number when the measure's property is gone", () => {
    renderBody({ chart: { ...BAR, measure: { fn: "sum", propertyId: "p-gone" } } });
    expect(screen.getByText("Choose a number to measure")).toBeInTheDocument();
  });

  it("draws nothing before it has a size", () => {
    const { container } = renderBody({ width: 0, height: 0 });
    expect(container.querySelector(".chart-plot")).toBeNull();
    expect(screen.queryByText("No data")).toBeNull();
  });
});

describe("names from the app", () => {
  /** A store that answers what the chart reads: the tree (titles of related pages) and the people of the space. */
  function fakeStore(linked: PageInfo[]): AppStore {
    const state = {
      tree: { spaceId: SPACE, roots: linked.map((related) => ({ page: related, children: [] })) },
      people: { [SPACE]: [{ accountId: "acc-1", name: "Ada", verified: false, isMe: true }] },
      ensurePeople: vi.fn(() => Promise.resolve()),
    };
    return { subscribe: () => () => undefined, getState: () => state } as unknown as AppStore;
  }

  it("names the groups of a relation by the titles of the related pages", () => {
    const website = page([], { title: "Website" });
    const app = page([], { title: "App" });
    const rows = [page([["p-project", { kind: "relation", value: [website.id, app.id] }]]), page([["p-project", { kind: "relation", value: [website.id] }]])];
    const { container } = renderBody({ chart: { ...BAR, groupBy: "p-project" }, rows }, fakeStore([website, app]));
    expect(Array.from(container.querySelectorAll(".chart-category")).map((label) => label.textContent)).toEqual(["App", "Website"]);
  });

  it("names the groups of a person property by the members' names, and asks for the people", () => {
    const store = fakeStore([]);
    const rows = [page([["p-owner", { kind: "person", value: ["acc-1"] }]]), page([["p-owner", { kind: "person", value: ["acc-gone"] }]])];
    const { container } = renderBody({ chart: { ...BAR, groupBy: "p-owner" }, rows }, store);
    expect(Array.from(container.querySelectorAll(".chart-category")).map((label) => label.textContent)).toEqual(["Ada", "Former member"]);
    expect(store.getState().ensurePeople).not.toHaveBeenCalled(); // they were already read
  });

  it("reads the people when they are not read yet", () => {
    const state = { tree: null, people: {}, ensurePeople: vi.fn(() => Promise.resolve()) };
    const store = { subscribe: () => () => undefined, getState: () => state } as unknown as AppStore;
    renderBody({ chart: { ...BAR, groupBy: "p-owner" }, rows: [page([["p-owner", { kind: "person", value: ["acc-1"] }]])] }, store);
    expect(state.ensurePeople).toHaveBeenCalledExactlyOnceWith(SPACE);
  });
});

// ----- the settings ------------------------------------------------------------------------------------

describe("the chart settings", () => {
  function renderSettings(chart: ChartSpec = BAR, schema: readonly PropertyDef[] = SCHEMA) {
    const onChange = vi.fn();
    const view = render(<ChartWidgetSettings chart={chart} schema={schema} onChange={onChange} />);
    return { onChange, ...view };
  }

  it("has the type as three buttons, the current one pressed", async () => {
    const user = userEvent.setup();
    const { onChange } = renderSettings();
    const group = screen.getByRole("group", { name: "Chart type" });
    const buttons = within(group).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual(["Bar", "Line", "Donut"]);
    expect(buttons.map((button) => button.getAttribute("aria-pressed"))).toEqual(["true", "false", "false"]);
    await user.click(within(group).getByRole("button", { name: "Line" }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith({ ...BAR, type: "line" });
  });

  it("groups by the properties that can group and by the times", async () => {
    const user = userEvent.setup();
    const { onChange } = renderSettings();
    const select = screen.getByLabelText("Group by");
    expect(select).toHaveValue("p-status");
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual([
      "Group by…",
      "Status",
      "Note",
      "Due",
      "Owner",
      "Project",
      "Created time",
      "Last edited time",
    ]);
    await user.selectOptions(select, "p-due");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, groupBy: "p-due" });
    await user.selectOptions(select, "created");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, groupBy: "created" });
  });

  it("asks to choose when the group is not chosen or gone", () => {
    const { unmount } = renderSettings({ ...BAR, groupBy: null });
    expect(screen.getByLabelText("Group by")).toHaveValue("");
    unmount();
    renderSettings({ ...BAR, groupBy: "p-gone" });
    expect(screen.getByLabelText("Group by")).toHaveValue("");
  });

  it("has the bucket for a date source, and no order for it", async () => {
    const user = userEvent.setup();
    const dated: ChartSpec = { ...BAR, groupBy: "p-due", order: "value_desc" };
    const { onChange } = renderSettings(dated);
    const bucket = screen.getByLabelText("Group dates by");
    expect(bucket).toHaveValue("month");
    expect(within(bucket).getAllByRole("option").map((option) => option.textContent)).toEqual(["Day", "Week", "Month", "Quarter", "Year"]);
    await user.selectOptions(bucket, "week");
    expect(onChange).toHaveBeenLastCalledWith({ ...dated, bucket: "week" });
    expect(screen.queryByLabelText("Order")).toBeNull();
  });

  it("has neither bucket nor a date source's other part for options, and has the order", async () => {
    const user = userEvent.setup();
    const { onChange } = renderSettings();
    expect(screen.queryByLabelText("Group dates by")).toBeNull();
    const order = screen.getByLabelText("Order");
    expect(within(order).getAllByRole("option").map((option) => option.textContent)).toEqual(["Default", "Highest first", "Lowest first"]);
    await user.selectOptions(order, "value_desc");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, order: "value_desc" });
  });

  it("measures a count, or a function of a number with its property", async () => {
    const user = userEvent.setup();
    const { onChange, rerender } = renderSettings();
    const measure = screen.getByLabelText("Measure");
    expect(measure).toHaveValue("count");
    expect(within(measure).getAllByRole("option").map((option) => option.textContent)).toEqual(["Count", "Sum", "Average", "Median", "Minimum", "Maximum"]);
    expect(screen.queryByLabelText("Property")).toBeNull();
    await user.selectOptions(measure, "sum");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, measure: { fn: "sum", propertyId: "p-hours" } });

    const summed: ChartSpec = { ...BAR, measure: { fn: "sum", propertyId: "p-hours" } };
    rerender(<ChartWidgetSettings chart={summed} schema={SCHEMA} onChange={onChange} />);
    const property = screen.getByLabelText("Property");
    expect(property).toHaveValue("p-hours");
    expect(within(property).getAllByRole("option").map((option) => option.textContent)).toEqual(["Choose a property…", "Hours", "Total"]);
    await user.selectOptions(property, "f-total");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, measure: { fn: "sum", propertyId: "f-total" } });
    await user.selectOptions(screen.getByLabelText("Measure"), "count");
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, measure: { fn: "count" } });
  });

  it("offers no function of a number when there is no number, and asks for one when the property is gone", () => {
    const { unmount } = renderSettings(BAR, [STATUS, NOTE]);
    const options = within(screen.getByLabelText("Measure")).getAllByRole<HTMLOptionElement>("option");
    expect(options.map((option) => option.disabled)).toEqual([false, true, true, true, true, true]);
    unmount();
    renderSettings({ ...BAR, measure: { fn: "max", propertyId: "p-gone" } });
    expect(screen.getByLabelText("Property")).toHaveValue("");
  });

  it("has horizontal for a bar, cumulative for a line, and neither for a donut", async () => {
    const user = userEvent.setup();
    const { onChange, rerender } = renderSettings();
    await user.click(screen.getByRole("checkbox", { name: "Horizontal bars" }));
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, horizontal: true });
    expect(screen.queryByRole("checkbox", { name: "Cumulative" })).toBeNull();

    const line: ChartSpec = { ...BAR, type: "line", cumulative: true };
    rerender(<ChartWidgetSettings chart={line} schema={SCHEMA} onChange={onChange} />);
    expect(screen.queryByRole("checkbox", { name: "Horizontal bars" })).toBeNull();
    const cumulative = screen.getByRole("checkbox", { name: "Cumulative" });
    expect(cumulative).toBeChecked();
    await user.click(cumulative);
    expect(onChange).toHaveBeenLastCalledWith({ ...line, cumulative: undefined });

    rerender(<ChartWidgetSettings chart={{ ...BAR, type: "donut" }} schema={SCHEMA} onChange={onChange} />);
    expect(screen.queryByRole("checkbox", { name: "Horizontal bars" })).toBeNull();
    expect(screen.queryByRole("checkbox", { name: "Cumulative" })).toBeNull();
  });

  it("shows pages without a value, except on a line over dates, where they have no place", async () => {
    const user = userEvent.setup();
    const { onChange, rerender } = renderSettings();
    const empty = screen.getByRole("checkbox", { name: "Show pages without a value" });
    expect(empty).not.toBeChecked();
    await user.click(empty);
    expect(onChange).toHaveBeenLastCalledWith({ ...BAR, showEmpty: true });
    rerender(<ChartWidgetSettings chart={{ ...BAR, type: "line", groupBy: "p-due" }} schema={SCHEMA} onChange={onChange} />);
    expect(screen.queryByRole("checkbox", { name: "Show pages without a value" })).toBeNull();
    rerender(<ChartWidgetSettings chart={{ ...BAR, type: "line" }} schema={SCHEMA} onChange={onChange} />);
    expect(screen.getByRole("checkbox", { name: "Show pages without a value" })).toBeInTheDocument();
  });

  it("is translated: its labels come from the dictionary", () => {
    renderSettings();
    expect(screen.getByLabelText("Group by")).toBeInTheDocument();
    expect(t("chart.settings.groupBy")).toBe("Group by");
  });
});
