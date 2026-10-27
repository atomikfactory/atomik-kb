// The toolbar of a database block (ADR-0054): a slim bar of Filter, Sort, Properties and a view settings button, each
// opening a popover. The tests mount `PageListToolbar` over a view they hold in state (as `PageListView` does, with the
// registry's settings section of the layout) and watch the changes it reports: the badges of Filter and Sort, the Sort
// popover, the view settings menu with the layout, the source, the layout's own section, "Open pages in" and the CSV
// export, and the focus the popovers take and give back. The block, its tabs and the writes into the document are
// `page-list.test.tsx`'s.

import { CoreClient, MemoryTransport, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import type { PropertyDef } from "@atomik-kb/core-client";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import { t, tp } from "../i18n";
import { createAppStore } from "../store";
import { StoreContext } from "../store-context";
import { PAGE_LIST_LAYOUTS } from "./page-list-model";
import { layoutDef } from "./page-list-layouts";
import { emptyView, patchView } from "./page-list-views";
import type { PageListViewSpec } from "./page-list-views";
import { PageListToolbar } from "./PageListToolbar";
import type { PageListChange, PageListCsvExport } from "./PageListToolbar";

const def = (id: string, name: string, kind: PropertyDef["kind"], options: PropertyDef["options"] = []): PropertyDef => ({
  id,
  name,
  kind,
  options,
  createdAt: 0,
});

const DEFS: readonly PropertyDef[] = [
  def(TAGS_PROPERTY_ID, "Tags", "multi_select", [
    { id: "o-idea", name: "idea", color: "green" },
    { id: "o-project", name: "project", color: "blue" },
  ]),
  def("p-priority", "Priority", "number"),
  def("p-due", "Due", "date"),
  def("p-end", "End", "date"),
  def("p-stage", "Stage", "select", [
    { id: "o-draft", name: "Draft", color: "gray" },
    { id: "o-review", name: "Review", color: "orange" },
  ]),
];

const FILTERS = {
  op: "and",
  items: [
    { propertyId: "p-priority", op: "isSet" },
    { propertyId: "p-due", op: "isSet" },
    { op: "or", items: [{ propertyId: "p-stage", op: "isSet" }] },
  ],
} as const;

interface HostProps {
  view?: PageListViewSpec;
  csv?: PageListCsvExport | null;
  onChange?: (change: PageListChange) => void;
}

/** The toolbar over a view held in state, with the layout's settings section the way `PageListView` builds it. */
function Host({ view: initial = emptyView("table", "v1"), csv = null, onChange }: HostProps) {
  const [view, setView] = useState(initial);
  const [source, setSource] = useState<PageListChange["source"]>({ kind: "children" });
  const { Settings } = layoutDef(view.layout);
  function change(next: PageListChange): void {
    onChange?.(next);
    const { source: nextSource, ...patch } = next;
    if (nextSource !== undefined) {
      setSource(nextSource);
    }
    setView((current) => patchView(current, patch));
  }
  return (
    <PageListToolbar
      source={source ?? null}
      view={view}
      defs={DEFS}
      onChange={change}
      layoutSettings={Settings === null ? null : <Settings view={view} schema={DEFS} onUpdateView={change} />}
      csv={csv}
    />
  );
}

/** Mounts the toolbar with a store (the filter editor reads the people and the pages from it). */
function mount(props: HostProps = {}) {
  const user = userEvent.setup();
  const memory = new MemoryTransport({ now: () => 0, workspaceName: "W", spaces: [{ name: "S", pages: [] }] });
  const store = createAppStore(new CoreClient(memory));
  const stop = store.getState().start();
  const result = render(
    <StoreContext value={store}>
      <Host {...props} />
    </StoreContext>,
  );
  return { user, stop, ...result };
}

const bar = (): HTMLElement => screen.getByRole("toolbar", { name: t("pageList.toolbar") });
const button = (name: string | RegExp): HTMLElement => within(bar()).getByRole("button", { name });
const settingsButton = (): HTMLElement => button(t("pageList.viewSettings"));

async function openSettings(user: ReturnType<typeof userEvent.setup>): Promise<HTMLElement> {
  await user.click(settingsButton());
  return screen.findByRole("dialog", { name: t("pageList.viewSettings") });
}

describe("the bar", () => {
  it("is Filter, Sort, Properties and the view settings button in one toolbar, with no select in it", () => {
    mount();
    expect(within(bar()).getAllByRole("button").map((entry) => entry.getAttribute("aria-label") ?? entry.textContent)).toEqual([
      t("pageList.filter"),
      t("pageList.sort"),
      t("pageList.properties"),
      t("pageList.viewSettings"),
    ]);
    expect(within(bar()).queryAllByRole("combobox")).toHaveLength(0);
    for (const entry of within(bar()).getAllByRole("button")) {
      expect(entry).toHaveAttribute("aria-haspopup", "dialog");
      expect(entry).toHaveAttribute("aria-expanded", "false");
    }
  });

  it("is Filter and the view settings button only for a dashboard, which has no use for Sort and Properties (ADR-0055)", async () => {
    const sorted = patchView(emptyView("dashboard", "v1"), { sort: { by: "p-due", dir: "desc" }, properties: ["p-due"] });
    const { user } = mount({ view: sorted });
    expect(within(bar()).getAllByRole("button").map((entry) => entry.getAttribute("aria-label") ?? entry.textContent)).toEqual([
      t("pageList.filter"),
      t("pageList.viewSettings"),
    ]);
    // A sort that is kept in the view but has no use does not light the bar up.
    expect(bar()).not.toHaveAttribute("data-badged");
    // The filter and the settings work as for any layout, and the layout select offers the way out.
    await user.click(button(new RegExp(`^${t("pageList.filter")}`, "u")));
    expect(await screen.findByRole("dialog", { name: t("pageList.filter.label") })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    const menu = await openSettings(user);
    const select = within(menu).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.layout.label") });
    expect(select.value).toBe("dashboard");
  });

  it("has a view settings button named by its label and its tooltip, not by its icon", () => {
    mount();
    expect(settingsButton()).toHaveAttribute("title", t("pageList.viewSettings"));
    expect(settingsButton().querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(settingsButton()).toHaveTextContent("");
  });

  it("opens one popover at a time, each button saying whether its popover is open", async () => {
    const { user } = mount();
    await user.click(button(t("pageList.sort")));
    expect(await screen.findByRole("dialog", { name: t("pageList.sort") })).toBeInTheDocument();
    expect(button(t("pageList.sort"))).toHaveAttribute("aria-expanded", "true");
    await user.click(button(t("pageList.properties")));
    expect(await screen.findByRole("dialog", { name: t("pageList.properties.label") })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: t("pageList.sort") })).not.toBeInTheDocument();
    expect(button(t("pageList.sort"))).toHaveAttribute("aria-expanded", "false");
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    // Pressing the button of the open popover closes it.
    await user.click(button(/^Properties/u));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("styles the badges in the accent colour", () => {
    const css = readFileSync("src/styles/database-views.css", "utf8");
    const rule = /\.page-list-badge\s*\{[^}]*\}/u.exec(css)?.[0] ?? "";
    expect(rule).toContain("background: var(--accent)");
    expect(rule).toContain("color: var(--accent-text)");
  });
});

describe("the Filter button", () => {
  it("has no badge while the view has no filter", () => {
    mount();
    expect(button(t("pageList.filter"))).toHaveTextContent(/^Filter$/u);
    expect(within(bar()).queryByRole("img")).not.toBeInTheDocument();
  });

  it("counts the filters of the view, the ones inside its groups too, in a badge that says it in words", () => {
    mount({ view: patchView(emptyView("table", "v1"), { filter: FILTERS }) });
    const badge = within(button(/^Filter/u)).getByRole("img", { name: tp("pageList.filter.badge", 3) });
    expect(badge).toHaveTextContent("3");
    expect(badge).toHaveClass("page-list-badge");
    expect(badge).toHaveAccessibleName("3 active filters");
    expect(button(/^Filter/u)).toHaveAccessibleName("Filter 3 active filters");
    expect(button(/^Filter/u)).toHaveTextContent("Filter 3");
  });

  it("says one filter in the singular", () => {
    mount({ view: patchView(emptyView("table", "v1"), { filter: { op: "and", items: [{ propertyId: "p-priority", op: "isSet" }] } }) });
    expect(within(button(/^Filter/u)).getByRole("img")).toHaveAccessibleName("1 active filter");
  });

  it("opens the filter editor, whose changes are the filter tree of the view", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange });
    await user.click(button(t("pageList.filter")));
    const dialog = await screen.findByRole("dialog", { name: t("pageList.filter.label") });
    await user.click(within(dialog).getByRole("button", { name: t("pageList.filter.add") }));
    expect(onChange).toHaveBeenCalledTimes(1);
    const first = onChange.mock.calls[0]?.[0] as PageListChange;
    expect(Object.keys(first)).toEqual(["filter"]);
    expect(first.filter?.op).toBe("and");
    expect(first.filter?.items).toHaveLength(1);
    // The button follows the view: one filter now.
    expect(within(button(/^Filter/u)).getByRole("img")).toHaveAccessibleName("1 active filter");
  });
});

describe("the Sort button", () => {
  it("has no badge for the sort every view starts with: the title, ascending", () => {
    mount();
    expect(button(t("pageList.sort"))).toHaveTextContent(/^Sort$/u);
    expect(within(button(t("pageList.sort"))).queryByRole("img")).not.toBeInTheDocument();
  });

  it("shows a 1 badge, named after the sort, for a sort by a property, by another built-in field, or in the other direction", () => {
    const { unmount } = mount({ view: patchView(emptyView("table", "v1"), { sort: { by: "p-due", dir: "asc" } }) });
    const badge = within(button(/^Sort/u)).getByRole("img", { name: t("pageList.sort.badge", { name: "Due" }) });
    expect(badge).toHaveTextContent("1");
    expect(badge).toHaveClass("page-list-badge");
    expect(button(/^Sort/u)).toHaveTextContent("Sort 1");
    unmount();

    const created = mount({ view: patchView(emptyView("table", "v1"), { sort: { by: "created", dir: "asc" } }) });
    expect(within(button(/^Sort/u)).getByRole("img")).toHaveAccessibleName(t("pageList.sort.badge", { name: t("pageList.sort.created") }));
    created.unmount();

    mount({ view: patchView(emptyView("table", "v1"), { sort: { by: "title", dir: "desc" } }) });
    expect(within(button(/^Sort/u)).getByRole("img")).toHaveAccessibleName(t("pageList.sort.badge", { name: t("pageList.sort.title") }));
  });

  it("has no badge for a sort by a property the space no longer has (it sorts like the title)", () => {
    mount({ view: patchView(emptyView("table", "v1"), { sort: { by: "p-gone", dir: "asc" } }) });
    expect(within(button(t("pageList.sort"))).queryByRole("img")).not.toBeInTheDocument();
  });

  it("opens a popover with the property and the direction, each change keeping the other half of the sort", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: patchView(emptyView("table", "v1"), { properties: ["p-priority"] }) });
    await user.click(button(t("pageList.sort")));
    const dialog = await screen.findByRole("dialog", { name: t("pageList.sort") });
    const by = within(dialog).getByRole("combobox", { name: t("pageList.sort.label") });
    expect(Array.from((by as HTMLSelectElement).options).map((option) => option.textContent)).toEqual([
      "Title",
      "Last edited",
      "Created",
      "Priority",
    ]);
    expect(within(dialog).getByRole("combobox", { name: t("pageList.sort.dir") })).toHaveValue("asc");

    await user.selectOptions(by, "Priority");
    expect(onChange).toHaveBeenLastCalledWith({ sorts: [{ by: "p-priority", dir: "asc" }] });
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("pageList.sort.dir") }), t("pageList.sort.desc"));
    expect(onChange).toHaveBeenLastCalledWith({ sorts: [{ by: "p-priority", dir: "desc" }] });
    // The popover stays open, and the badge follows.
    expect(screen.getByRole("dialog", { name: t("pageList.sort") })).toBeInTheDocument();
    expect(within(button(/^Sort/u)).getByRole("img")).toHaveAccessibleName(t("pageList.sort.badge", { name: "Priority" }));
  });
});

describe("the Sort panel with several keys (ADR-0058)", () => {
  const SHOWN = ["p-priority", "p-due"];
  const keys = (...sorts: [string, "asc" | "desc"][]): { sorts: { by: string; dir: "asc" | "desc" }[] } => ({
    sorts: sorts.map(([by, dir]) => ({ by, dir })),
  });
  const withKeys = (...sorts: [string, "asc" | "desc"][]): PageListViewSpec =>
    patchView(emptyView("table", "v1"), { properties: SHOWN, ...keys(...sorts) });
  const openSort = async (user: ReturnType<typeof userEvent.setup>): Promise<HTMLElement> => {
    await user.click(button(/^Sort/u));
    return screen.findByRole("dialog", { name: t("pageList.sort") });
  };
  const row = (dialog: HTMLElement, n: number): HTMLElement => within(dialog).getByRole("group", { name: t("pageList.sort.row", { n }) });
  const byOf = (dialog: HTMLElement, n: number): HTMLSelectElement =>
    within(row(dialog, n)).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.sort.label") });
  const shownBy = (dialog: HTMLElement): string[] =>
    within(dialog)
      .getAllByRole<HTMLSelectElement>("combobox", { name: t("pageList.sort.label") })
      .map((select) => select.selectedOptions[0]?.textContent ?? "");

  it("has one row for the one key every view starts with, which cannot be removed, moved up or down", async () => {
    const { user } = mount({ view: withKeys(["title", "asc"]) });
    const dialog = await openSort(user);
    expect(within(dialog).getAllByRole("group")).toHaveLength(1);
    expect(shownBy(dialog)).toEqual(["Title"]);
    expect(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.remove") })).toBeDisabled();
    expect(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.up") })).toBeDisabled();
    expect(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.down") })).toBeDisabled();
  });

  it("adds a key with the first property that is not a key yet, up to 5, then says why it cannot", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: withKeys(["title", "asc"]) });
    const dialog = await openSort(user);
    const add = (): HTMLElement => within(dialog).getByRole("button", { name: t("pageList.sort.add") });
    await user.click(add());
    expect(onChange).toHaveBeenLastCalledWith({
      sorts: [
        { by: "title", dir: "asc" },
        { by: "updated", dir: "asc" },
      ],
    });
    expect(shownBy(dialog)).toEqual(["Title", "Last edited"]);
    await user.click(add());
    await user.click(add());
    await user.click(add());
    expect(shownBy(dialog)).toEqual(["Title", "Last edited", "Created", "Priority", "Due"]);
    expect(onChange).toHaveBeenLastCalledWith({
      sorts: ["title", "updated", "created", "p-priority", "p-due"].map((by) => ({ by, dir: "asc" })),
    });
    expect(add()).toBeDisabled();
    expect(within(dialog).getByRole("status")).toHaveTextContent(t("pageList.sort.limit", { max: 5 }));
  });

  it("cannot add a key when every property that can be sorted by is one already", async () => {
    const { user } = mount({ view: patchView(emptyView("table", "v1"), keys(["title", "asc"], ["updated", "asc"], ["created", "desc"])) });
    const dialog = await openSort(user);
    expect(within(dialog).getByRole("button", { name: t("pageList.sort.add") })).toBeDisabled();
    // Fewer than 5 keys, so no limit message.
    expect(within(dialog).queryByRole("status")).not.toBeInTheDocument();
  });

  it("offers a key once: a row lists what the other rows do not use, and its own", async () => {
    const { user } = mount({ view: withKeys(["p-due", "desc"], ["title", "asc"]) });
    const dialog = await openSort(user);
    const options = (n: number): string[] => Array.from(byOf(dialog, n).options).map((option) => option.textContent ?? "");
    expect(options(1)).toEqual(["Last edited", "Created", "Priority", "Due"]);
    expect(options(2)).toEqual(["Title", "Last edited", "Created", "Priority"]);
    expect(shownBy(dialog)).toEqual(["Due", "Title"]);
    expect(within(row(dialog, 1)).getByRole("combobox", { name: t("pageList.sort.dir") })).toHaveValue("desc");
    expect(within(row(dialog, 2)).getByRole("combobox", { name: t("pageList.sort.dir") })).toHaveValue("asc");
  });

  it("changes the property or the direction of one key and leaves the others", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: withKeys(["p-due", "desc"], ["title", "asc"]) });
    const dialog = await openSort(user);
    await user.selectOptions(byOf(dialog, 2), "Priority");
    expect(onChange).toHaveBeenLastCalledWith({
      sorts: [
        { by: "p-due", dir: "desc" },
        { by: "p-priority", dir: "asc" },
      ],
    });
    await user.selectOptions(within(row(dialog, 1)).getByRole("combobox", { name: t("pageList.sort.dir") }), t("pageList.sort.asc"));
    expect(onChange).toHaveBeenLastCalledWith({
      sorts: [
        { by: "p-due", dir: "asc" },
        { by: "p-priority", dir: "asc" },
      ],
    });
  });

  it("moves a key up or down, the disabled buttons being the ends, and keeps the focus on the button that moved it", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: withKeys(["p-due", "desc"], ["title", "asc"], ["created", "asc"]) });
    const dialog = await openSort(user);
    const control = (n: number, name: string): HTMLElement => within(row(dialog, n)).getByRole("button", { name });
    expect(control(1, t("pageList.sort.up"))).toBeDisabled();
    expect(control(3, t("pageList.sort.down"))).toBeDisabled();
    expect(control(2, t("pageList.sort.up"))).toBeEnabled();
    await user.click(control(2, t("pageList.sort.up")));
    expect(onChange).toHaveBeenLastCalledWith({
      sorts: [
        { by: "title", dir: "asc" },
        { by: "p-due", dir: "desc" },
        { by: "created", dir: "asc" },
      ],
    });
    expect(shownBy(dialog)).toEqual(["Title", "Due", "Created"]);
    // The key is in row 1, and the focus went there (the up button is disabled at the top: its neighbour has it).
    expect(control(1, t("pageList.sort.down"))).toHaveFocus();
    await user.click(control(2, t("pageList.sort.down")));
    expect(shownBy(dialog)).toEqual(["Title", "Created", "Due"]);
    expect(control(3, t("pageList.sort.up"))).toHaveFocus();
  });

  it("removes a key, and the last key goes back to the default sort", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: withKeys(["p-due", "desc"], ["title", "asc"]) });
    const dialog = await openSort(user);
    await user.click(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.remove") }));
    expect(onChange).toHaveBeenLastCalledWith({ sorts: [{ by: "title", dir: "asc" }] });
    expect(within(dialog).getAllByRole("group")).toHaveLength(1);
    // A single key that is not the default one can be removed: the view is sorted by the title again.
    await user.selectOptions(byOf(dialog, 1), "Due");
    expect(onChange).toHaveBeenLastCalledWith({ sorts: [{ by: "p-due", dir: "asc" }] });
    await user.click(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.remove") }));
    expect(onChange).toHaveBeenLastCalledWith({ sorts: [{ by: "title", dir: "asc" }] });
    expect(within(row(dialog, 1)).getByRole("button", { name: t("pageList.sort.remove") })).toBeDisabled();
  });

  it("shows a property the space no longer has as removed, to be replaced or taken away", async () => {
    const { user } = mount({ view: withKeys(["p-gone", "asc"], ["title", "asc"]) });
    const dialog = await openSort(user);
    expect(shownBy(dialog)).toEqual([t("pageList.sort.gone"), "Title"]);
    await user.selectOptions(byOf(dialog, 1), "Due");
    expect(shownBy(dialog)).toEqual(["Due", "Title"]);
  });

  it("lights the badge with the number of keys and their names, but not for the default key alone", () => {
    const { unmount } = mount({ view: withKeys(["title", "asc"]) });
    expect(within(button(t("pageList.sort"))).queryByRole("img")).not.toBeInTheDocument();
    unmount();
    mount({ view: withKeys(["title", "asc"], ["p-due", "desc"]) });
    const badge = within(button(/^Sort/u)).getByRole("img");
    expect(badge).toHaveTextContent("2");
    expect(badge).toHaveAccessibleName(t("pageList.sort.badge", { name: "Title, Due" }));
    expect(bar()).toHaveAttribute("data-badged");
  });
});

describe("the Properties button", () => {
  it("opens the shown-properties popover, one checkbox per property, each change writing the list alone", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: patchView(emptyView("table", "v1"), { properties: ["p-due"] }) });
    expect(button(/^Properties/u)).toHaveTextContent("Properties 1");
    await user.click(button(/^Properties/u));
    const dialog = await screen.findByRole("dialog", { name: t("pageList.properties.label") });
    expect(within(dialog).getAllByRole("checkbox").map((box) => box.closest("label")?.textContent)).toEqual([
      "Tags",
      "Priority",
      "Due",
      "End",
      "Stage",
    ]);
    await user.click(within(dialog).getByRole("checkbox", { name: "Priority" }));
    expect(onChange).toHaveBeenLastCalledWith({ properties: ["p-due", "p-priority"] });
    await user.click(within(dialog).getByRole("checkbox", { name: "Due" }));
    expect(onChange).toHaveBeenLastCalledWith({ properties: ["p-priority"] });
  });
});

describe("the view settings menu", () => {
  it("holds the layout, the source and Open pages in, as labelled selects, and nothing else for a list or a table", async () => {
    const { user } = mount();
    const menu = await openSettings(user);
    const labelOf = (select: HTMLElement): string | undefined => select.closest("label")?.querySelector(".page-list-field-label")?.textContent ?? undefined;
    expect(within(menu).getAllByRole("combobox").map(labelOf)).toEqual([
      t("pageList.layout.label"),
      t("pageList.source.label"),
      t("pageList.openIn.label"),
    ]);
    expect(within(menu).getByRole("combobox", { name: t("pageList.layout.label") })).toHaveValue("table");
    expect(within(menu).getByRole("combobox", { name: t("pageList.source.label") })).toHaveValue("children");
    expect(within(menu).getByRole("combobox", { name: t("pageList.openIn.label") })).toHaveValue("side");
    // The labels are visible text.
    expect(within(menu).getByText(t("pageList.layout.label"))).toBeVisible();
    expect(within(menu).queryByRole("group", { name: t("pageList.viewSettings.layout") })).not.toBeInTheDocument();
    // No CSV where the shell has no file to save.
    expect(within(menu).queryByRole("button", { name: t("pageList.exportCsv") })).not.toBeInTheDocument();
  });

  it("lists the five layouts and each choice writes the layout alone", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange });
    const menu = await openSettings(user);
    const select = within(menu).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.layout.label") });
    expect(Array.from(select.options).map((option) => option.value)).toEqual([...PAGE_LIST_LAYOUTS]);
    await user.selectOptions(select, t("pageList.layout.board"));
    expect(onChange).toHaveBeenLastCalledWith({ layout: "board" });
    await user.selectOptions(select, t("pageList.layout.gallery"));
    expect(onChange).toHaveBeenLastCalledWith({ layout: "gallery" });
    expect(onChange).toHaveBeenCalledTimes(2);
  });

  it("lists the children of the page and the tags, and a choice writes the block's source alone", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange });
    const menu = await openSettings(user);
    const select = within(menu).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.source.label") });
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual([
      t("pageList.source.children"),
      "Pages tagged: idea",
      "Pages tagged: project",
      t("pageList.source.pickPage"),
      t("pageList.source.space"),
    ]);
    await user.selectOptions(select, "Pages tagged: project");
    expect(onChange).toHaveBeenLastCalledWith({ source: { kind: "tag", optionId: "o-project" } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("offers Open pages in with the three places, a choice writing openIn alone", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange });
    const menu = await openSettings(user);
    const select = within(menu).getByRole<HTMLSelectElement>("combobox", { name: t("pageList.openIn.label") });
    expect(Array.from(select.options).map((option) => [option.value, option.textContent])).toEqual([
      ["side", t("pageList.openIn.side")],
      ["center", t("pageList.openIn.center")],
      ["page", t("pageList.openIn.page")],
    ]);
    await user.selectOptions(select, t("pageList.openIn.center"));
    expect(onChange).toHaveBeenLastCalledWith({ openIn: "center" });
  });

  it("hosts the calendar's settings: the date source, then the end date once a date property is chosen", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: emptyView("calendar", "v1") });
    const menu = await openSettings(user);
    const section = within(menu).getByRole("group", { name: t("pageList.viewSettings.layout") });
    const date = within(section).getByRole<HTMLSelectElement>("combobox", { name: t("pageCalendar.dateBy") });
    expect(within(section).queryByRole("combobox", { name: t("pageCalendar.endBy") })).not.toBeInTheDocument();
    expect(Array.from(date.options).map((option) => option.textContent)).toEqual([
      t("pageCalendar.dateBy.choose"),
      "Due",
      "End",
      t("pageCalendar.dateBy.created"),
      t("pageCalendar.dateBy.updated"),
    ]);
    await user.selectOptions(date, "Due");
    expect(onChange).toHaveBeenLastCalledWith({ dateBy: "p-due" });
    const end = within(section).getByRole("combobox", { name: t("pageCalendar.endBy") });
    await user.selectOptions(end, "End");
    expect(onChange).toHaveBeenLastCalledWith({ endBy: "p-end" });
    // The section sits between the common settings: after the source, before Open pages in.
    const source = within(menu).getByRole("combobox", { name: t("pageList.source.label") });
    const openIn = within(menu).getByRole("combobox", { name: t("pageList.openIn.label") });
    expect(source.compareDocumentPosition(date) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(date.compareDocumentPosition(openIn) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("hosts the gallery's card size, the default being the absence of the setting", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: emptyView("gallery", "v1") });
    const menu = await openSettings(user);
    const section = within(menu).getByRole("group", { name: t("pageList.viewSettings.layout") });
    const size = within(section).getByRole<HTMLSelectElement>("combobox", { name: t("pageGallery.size") });
    expect(size).toHaveValue("m");
    await user.selectOptions(size, t("pageGallery.size.l"));
    expect(onChange).toHaveBeenLastCalledWith({ cardSize: "l" });
    await user.selectOptions(size, t("pageGallery.size.m"));
    expect(onChange).toHaveBeenLastCalledWith({ cardSize: null });
  });

  it("hosts the board's group by and its columns, which write groupBy and hiddenGroups", async () => {
    const onChange = vi.fn();
    const { user } = mount({ onChange, view: emptyView("board", "v1") });
    const menu = await openSettings(user);
    const section = within(menu).getByRole("group", { name: t("pageList.viewSettings.layout") });
    const group = within(section).getByRole<HTMLSelectElement>("combobox", { name: t("pageBoard.groupBy") });
    expect(group).toHaveValue("");
    // The select and status properties only.
    expect(Array.from(group.options).map((option) => option.textContent)).toEqual([t("pageBoard.groupBy.choose"), "Stage"]);
    // No columns to show until there is a property to group by.
    expect(within(section).queryByRole("group", { name: t("pageBoard.groups.label") })).not.toBeInTheDocument();
    await user.selectOptions(group, "Stage");
    expect(onChange).toHaveBeenLastCalledWith({ groupBy: "p-stage" });
    const columns = within(section).getByRole("group", { name: t("pageBoard.groups.label") });
    expect(within(columns).getAllByRole("checkbox").map((box) => box.closest("label")?.textContent)).toEqual([
      t("pageBoard.none", { name: "Stage" }),
      "Draft",
      "Review",
    ]);
    await user.click(within(columns).getByRole("checkbox", { name: "Draft" }));
    expect(onChange).toHaveBeenLastCalledWith({ hiddenGroups: ["o-draft"] });
    await user.click(within(columns).getByRole("checkbox", { name: "Draft" }));
    expect(onChange).toHaveBeenLastCalledWith({ hiddenGroups: [] });
  });

  it("shows the section of the layout that is active, and follows a change of layout", async () => {
    const { user } = mount({ view: emptyView("gallery", "v1") });
    const menu = await openSettings(user);
    expect(within(menu).getByRole("combobox", { name: t("pageGallery.size") })).toBeInTheDocument();
    await user.selectOptions(within(menu).getByRole("combobox", { name: t("pageList.layout.label") }), t("pageList.layout.calendar"));
    expect(within(menu).queryByRole("combobox", { name: t("pageGallery.size") })).not.toBeInTheDocument();
    expect(within(menu).getByRole("combobox", { name: t("pageCalendar.dateBy") })).toBeInTheDocument();
    await user.selectOptions(within(menu).getByRole("combobox", { name: t("pageList.layout.label") }), t("pageList.layout.table"));
    expect(within(menu).queryByRole("group", { name: t("pageList.viewSettings.layout") })).not.toBeInTheDocument();
  });

  it("holds Export CSV at the bottom: it closes the menu and exports, or says why it cannot", async () => {
    const onExport = vi.fn();
    const { user, unmount } = mount({ csv: { onExport, disabledReason: null } });
    let menu = await openSettings(user);
    const group = within(menu).getByRole("group", { name: t("pageList.actions") });
    expect(menu.lastElementChild).toBe(group);
    await user.click(within(group).getByRole("button", { name: t("pageList.exportCsv") }));
    expect(onExport).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    unmount();

    const disabled = mount({ csv: { onExport, disabledReason: t("pageList.exportCsv.empty") } });
    menu = await openSettings(disabled.user);
    const entry = within(menu).getByRole("button", { name: t("pageList.exportCsv") });
    expect(entry).toBeDisabled();
    expect(entry).toHaveAttribute("title", t("pageList.exportCsv.empty"));
    expect(onExport).toHaveBeenCalledTimes(1);
  });
});

describe("the popovers and the keyboard", () => {
  it.each([
    ["Filter", () => button(t("pageList.filter")), () => t("pageList.filter.label")],
    ["Sort", () => button(t("pageList.sort")), () => t("pageList.sort")],
    ["Properties", () => button(/^Properties/u), () => t("pageList.properties.label")],
    ["the view settings button", settingsButton, () => t("pageList.viewSettings")],
  ] as const)("take focus when %s opens one and give it back to the button on Escape", async (_name, opener, dialogName) => {
    const { user } = mount();
    opener().focus();
    await user.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: dialogName() });
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(opener()).toHaveFocus();
  });

  it("are opened by the keys of a button, and their controls are reached with Tab", async () => {
    const { user } = mount();
    settingsButton().focus();
    await user.keyboard(" ");
    const menu = await screen.findByRole("dialog", { name: t("pageList.viewSettings") });
    expect(within(menu).getByRole("combobox", { name: t("pageList.layout.label") })).toHaveFocus();
    await user.tab();
    expect(within(menu).getByRole("combobox", { name: t("pageList.source.label") })).toHaveFocus();
    await user.tab();
    expect(within(menu).getByRole("combobox", { name: t("pageList.openIn.label") })).toHaveFocus();
  });
});
