// The selection of rows, the action bar and the row limit of a database block (ADR-0058, section 5 of
// `docs/architecture/database-power-ups.md`), in the whole app over the in-memory core. The core's `PageBulk` is the other
// agent's: here it is a stand-in that records the request and does what it asks with the single requests, so the tests look at what the
// UI sends, what it says and what it does with the selection, never at what the core does with the pages.

import { CoreClient } from "@atomik-kb/core-client";
import type { JsonValue, MemoryTransport, PropertyDef, Request, Response, SeedSpace } from "@atomik-kb/core-client";
import type { Editor } from "@tiptap/core";
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { t, tp } from "../i18n";
import { pid } from "../test/editor";
import { idsOf, openPage, renderPages } from "../test/page";
import type { User } from "../test/render";
import { ROW_LIMIT_STEP } from "./page-list-row-limit";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

// ----- the space ---------------------------------------------------------------------------

const PROP = {
  priority: "de000000-0000-4000-8000-00000000e001",
  stage: "de000000-0000-4000-8000-00000000e002",
  summary: "de000000-0000-4000-8000-00000000e003",
  done: "de000000-0000-4000-8000-00000000e004",
};

function paragraph(n: number, value: string): JsonValue {
  return value === ""
    ? { type: "paragraph", attrs: { id: pid(n) } }
    : { type: "paragraph", attrs: { id: pid(n) }, content: [{ type: "text", text: value }] };
}

/** The block: a table unless `list` says another layout, sorted by title; a board groups by Stage. */
function hubContent(list: { [key: string]: JsonValue } = {}): JsonValue {
  return {
    type: "doc",
    content: [
      paragraph(1, "Intro"),
      {
        type: "pageList",
        attrs: { id: pid(2), source: { kind: "children" }, layout: "table", properties: [], sort: { by: "title", dir: "asc" }, groupBy: PROP.stage, ...list },
      },
      paragraph(3, ""),
    ],
  };
}

/** A view that opens a page as a page (the side peek is the default), for the tests that watch for a row being opened. */
function asPages(layout: string): { [key: string]: JsonValue } {
  return { views: [{ id: "v1", name: "", layout, properties: [], sort: { by: "title", dir: "asc" }, groupBy: PROP.stage, openIn: "page" }] };
}

/**
 * Hub holds the block; its children, in title order: Alpha (Priority 3, Draft), beta (10, Review), delta (2, Draft), Gamma (nothing), then
 * `extra` pages Row 000 ... Archive and Elsewhere are other pages of the space: places to move rows to.
 */
function space(content: JsonValue, extra = 0): SeedSpace {
  const more = Array.from({ length: extra }, (_, index) => ({ title: `Row ${String(index).padStart(3, "0")}` }));
  return {
    name: "Personal",
    properties: [
      { id: PROP.priority, name: "Priority", kind: "number" },
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
      { id: PROP.summary, name: "Summary", kind: "text" },
      { id: PROP.done, name: "Done", kind: "checkbox" },
    ],
    pages: [
      {
        title: "Hub",
        content,
        children: [
          { title: "Alpha", properties: { Priority: 3, Stage: "Draft" } },
          { title: "beta", properties: { Priority: 10, Stage: "Review" } },
          { title: "Gamma" },
          { title: "delta", properties: { Priority: 2, Stage: "Draft" } },
          ...more,
        ],
      },
      { title: "Archive", children: [{ title: "Old" }] },
      { title: "Elsewhere" },
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

async function openHub(list: { [key: string]: JsonValue } = {}, extra = 0) {
  const app = await renderPages([space(hubContent(list), extra)], { now: ticking() });
  const editor = await openPage(app.user, "Hub");
  await screen.findAllByRole("group", { name: t("pageList.label") });
  const client = new CoreClient(app.memory);
  const ids = await idsOf(app.memory);
  const spaceId = ids.space("Personal");
  const defs = await client.propertySchemaGet(spaceId);
  return { ...app, editor, client, ids, spaceId, defs };
}

// ----- the stand-in for PageBulk ------------------------------------------------------------

type BulkRequest = Extract<Request, { type: "PageBulk" }>["data"];

/**
 * Answers `PageBulk` by recording it and doing what the action says with the single requests (the core's own is another agent's work),
 * and `StructureUndo` by recording it. `fail` makes the next `PageBulk` refuse.
 */
function standInForBulk(memory: MemoryTransport) {
  const client = new CoreClient(memory);
  const original = memory.dispatch.bind(memory);
  const bulk: BulkRequest[] = [];
  const undone: string[] = [];
  const control: { fail: string | null } = { fail: null };
  vi.spyOn(memory, "dispatch").mockImplementation(async (request: Request): Promise<Response> => {
    if (request.type === "StructureUndo") {
      undone.push(request.data.spaceId);
      return { type: "Tree", data: await client.treeGet(request.data.spaceId) };
    }
    if (request.type !== "PageBulk") {
      return original(request);
    }
    const { spaceId, pageIds, action } = request.data;
    bulk.push(request.data);
    if (control.fail !== null) {
      const message = control.fail;
      control.fail = null;
      throw new Error(message);
    }
    const done = [...pageIds];
    switch (action.type) {
      case "SetValue":
        for (const id of pageIds) {
          await client.pagePropertySet(id, action.data.propertyId, action.data.value);
        }
        break;
      case "SetValues":
        for (const [at, id] of pageIds.entries()) {
          await client.pagePropertySet(id, action.data.propertyId, action.data.values[at] ?? null);
        }
        break;
      case "Move":
        for (const id of pageIds) {
          await client.pageMove(id, action.data.parentId ?? null, 0);
        }
        break;
      case "Duplicate": {
        done.length = 0;
        for (const id of pageIds) {
          done.push((await client.pageDuplicate(id)).id);
        }
        break;
      }
      case "Trash":
        for (const id of pageIds) {
          await client.pageTrash(id);
        }
        break;
    }
    void spaceId;
    return { type: "PageBulk", data: done };
  });
  return { bulk, undone, control };
}

// ----- looking at the view -----------------------------------------------------------------

function block(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.label") });
}

function grid(): HTMLElement {
  return within(block()).getByRole("grid", { name: t("pageTable.grid") });
}

function tableRows(): HTMLElement[] {
  return within(grid())
    .getAllByRole("row")
    .filter((row) => row.hasAttribute("data-page"));
}

function tableTitles(): string[] {
  return tableRows().map((row) => row.querySelector(".page-table-title")?.textContent ?? "");
}

function listRows(): HTMLElement[] {
  return Array.from(block().querySelectorAll<HTMLElement>(".page-list-row"));
}

function boardCards(): HTMLElement[] {
  return Array.from(block().querySelectorAll<HTMLElement>(".page-board-card"));
}

/** The checkbox of the row, item or card of a page. */
function box(title: string): HTMLInputElement {
  return within(block()).getByRole<HTMLInputElement>("checkbox", { name: t("pageList.bulk.select", { title }) });
}

function selectAllBox(): HTMLInputElement {
  return within(grid()).getByRole<HTMLInputElement>("checkbox", { name: t("pageList.bulk.selectAll") });
}

/** The action bar, `null` while no row is selected. */
function bar(): HTMLElement | null {
  return screen.queryByRole("group", { name: t("pageList.bulk.label") });
}

function barOf(): HTMLElement {
  return screen.getByRole("group", { name: t("pageList.bulk.label") });
}

function count(): string {
  return within(barOf()).getByRole("status").textContent ?? "";
}

function shiftClick(element: HTMLElement): void {
  fireEvent.click(element, { shiftKey: true });
}

/** The titles of the pages ticked, in the order given. */
function ticked(titles: readonly string[]): string[] {
  return titles.filter((title) => box(title).checked);
}

async function toastText(text: string): Promise<HTMLElement> {
  return within(await screen.findByRole("log", { name: t("toast.label") })).findByText(text);
}

function undoButton(): HTMLElement {
  return within(screen.getByRole("log", { name: t("toast.label") })).getByRole("button", { name: t("pageList.bulk.toast.undo") });
}

/** Sets an attribute of the block, as a change made elsewhere (another device, the Markdown view) would. */
function setBlockAttribute(editor: Editor, name: string, value: JsonValue): void {
  editor.state.doc.descendants((node, pos) => {
    if (node.type.name !== "pageList") {
      return true;
    }
    editor.view.dispatch(editor.state.tr.setNodeAttribute(pos, name, value));
    return false;
  });
}

function optionOf(defs: readonly PropertyDef[], propertyId: string, name: string): string {
  const id = defs.find((def) => def.id === propertyId)?.options.find((option) => option.name === name)?.id;
  if (id === undefined) {
    throw new Error(`no option ${name}`);
  }
  return id;
}

// ----- the checkboxes -----------------------------------------------------------------------

describe("the table's checkbox column", () => {
  it("has a checkbox before the page column of every row, named for its page, and a header 'Select all'", async () => {
    await openHub();
    expect(tableTitles()).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    const header = within(grid()).getAllByRole("columnheader")[0];
    expect(header).toHaveAttribute("aria-colindex", "1");
    expect(within(header as HTMLElement).getByRole("checkbox", { name: t("pageList.bulk.selectAll") })).toBe(selectAllBox());
    for (const row of tableRows()) {
      const title = row.querySelector(".page-table-title")?.textContent ?? "";
      const cell = within(row).getByRole("checkbox", { name: t("pageList.bulk.select", { title }) }).closest('[role="gridcell"]');
      expect(cell).toHaveAttribute("aria-colindex", "1");
      expect(row.querySelector('[role="gridcell"][aria-label]')?.getAttribute("aria-colindex")).toBe("2"); // the page column moved over
    }
    expect(grid()).toHaveAttribute("aria-colcount", "2");
    expect(bar()).toBeNull();
  });

  it("is a tri-state: unchecked, mixed while some rows are ticked, checked when all are; a click ticks all, or clears", async () => {
    const { user } = await openHub();
    expect(selectAllBox().checked).toBe(false);
    expect(selectAllBox().indeterminate).toBe(false);

    await user.click(box("beta"));
    expect(selectAllBox().checked).toBe(false);
    expect(selectAllBox().indeterminate).toBe(true);
    expect(count()).toBe(tp("pageList.bulk.count", 1));

    await user.click(selectAllBox());
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual(["Alpha", "beta", "delta", "Gamma"]);
    expect(selectAllBox().checked).toBe(true);
    expect(selectAllBox().indeterminate).toBe(false);
    expect(count()).toBe(tp("pageList.bulk.count", 4));

    await user.click(selectAllBox());
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual([]);
    expect(bar()).toBeNull();
  });

  it("ticks a row without opening it, and Shift+click ticks the range in display order", async () => {
    const { user } = await openHub(asPages("table"));
    await user.click(box("Alpha"));
    expect(within(tableRows()[0] as HTMLElement).getByRole("checkbox")).toBeChecked();
    expect(tableRows()[0]).toHaveAttribute("data-selected");
    expect(tableRows()[1]).not.toHaveAttribute("data-selected");
    // The view opens pages as pages: a row that was opened would have left this one.
    expect(block()).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: /Alpha/u })).not.toBeInTheDocument();

    shiftClick(box("delta"));
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual(["Alpha", "beta", "delta"]);
    expect(count()).toBe(tp("pageList.bulk.count", 3));
    // The range goes on from the row that was clicked last, and a plain click unticks one.
    shiftClick(box("Gamma"));
    expect(count()).toBe(tp("pageList.bulk.count", 4));
    await user.click(box("beta"));
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual(["Alpha", "delta", "Gamma"]);
  });

  it("is reached from the keyboard: Space ticks the focused checkbox, and the arrows still move between the cells", async () => {
    const { user } = await openHub();
    box("beta").focus();
    await user.keyboard(" ");
    expect(box("beta")).toBeChecked();
    expect(count()).toBe(tp("pageList.bulk.count", 1));
    // Only the first row's checkbox is a tab stop (roving), the others are reached with the arrows of the cells.
    expect(box("Alpha").tabIndex).toBe(0);
    expect(box("beta").tabIndex).toBe(-1);

    const alpha = within(tableRows()[0] as HTMLElement).getByRole("gridcell", { name: t("pageList.sort.title") });
    await user.click(alpha);
    await user.keyboard("{ArrowDown}");
    expect(within(tableRows()[1] as HTMLElement).getByRole("gridcell", { name: t("pageList.sort.title") })).toHaveFocus();
    expect(box("beta").tabIndex).toBe(0);
  });

  it("is not there in a read-only editor, and goes when the editor turns read-only", async () => {
    const { user, editor } = await openHub();
    await user.click(box("Alpha"));
    expect(bar()).not.toBeNull();
    act(() => {
      editor.setEditable(false);
    });
    await waitFor(() => {
      expect(within(block()).queryByRole("checkbox")).not.toBeInTheDocument();
    });
    expect(bar()).toBeNull();
    expect(grid()).toHaveAttribute("aria-colcount", "1");
    act(() => {
      editor.setEditable(true);
    });
    await waitFor(() => {
      expect(within(block()).getAllByRole("checkbox").length).toBeGreaterThan(0);
    });
    expect(bar()).toBeNull(); // the selection did not come back
  });
});

describe("the list's checkboxes", () => {
  it("has one before the icon of every row, which ticks the row and does not open it", async () => {
    const { user } = await openHub({ layout: "list", ...asPages("list") });
    expect(listRows()).toHaveLength(4);
    const first = listRows()[0] as HTMLElement;
    expect(first.firstElementChild).toBe(box("Alpha"));
    expect(first.querySelector(".page-list-icon")).not.toBeNull();

    await user.click(box("Alpha"));
    expect(first).toHaveAttribute("data-selected");
    expect(block()).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: /Alpha/u })).not.toBeInTheDocument();
    shiftClick(box("delta"));
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual(["Alpha", "beta", "delta"]);
    expect(count()).toBe(tp("pageList.bulk.count", 3));
  });

  it("still opens a row from its title", async () => {
    const { user } = await openHub({ layout: "list", ...asPages("list") });
    await user.click(within(block()).getByRole("button", { name: "Gamma" }));
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 1 }).querySelector(".page-title-button")?.textContent).toBe("Gamma");
    });
  });
});

describe("the board's checkboxes", () => {
  it("has one in the corner of every card, and a Shift+click range goes column by column, then card by card", async () => {
    const { user } = await openHub({ layout: "board" });
    // The columns: no Stage (Gamma), Review (beta), Draft (Alpha, delta), Later (none).
    expect(boardCards()).toHaveLength(4);
    expect(boardCards()[0]?.firstElementChild).toBe(box("Gamma"));

    await user.click(box("Gamma"));
    shiftClick(box("Alpha"));
    expect(ticked(["Gamma", "beta", "Alpha", "delta"])).toEqual(["Gamma", "beta", "Alpha"]);
    expect(count()).toBe(tp("pageList.bulk.count", 3));
    expect(boardCards().filter((card) => card.hasAttribute("data-selected"))).toHaveLength(3);
  });

  it("does not open a card from its checkbox", async () => {
    const { user } = await openHub(asPages("board"));
    await user.click(box("Alpha"));
    expect(block()).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: /Alpha/u })).not.toBeInTheDocument();
  });
});

// ----- the keys ------------------------------------------------------------------------------

describe("Ctrl+A and Escape in the view", () => {
  it("selects every row of the view, and Escape clears the selection", async () => {
    const { user } = await openHub();
    await user.click(box("Alpha"));
    box("Alpha").focus();
    await user.keyboard("{Control>}a{/Control}");
    expect(count()).toBe(tp("pageList.bulk.count", 4));
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toHaveLength(4);
    await user.keyboard("{Escape}");
    expect(bar()).toBeNull();
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual([]);
  });

  it("selects every row, those beyond the row limit included, with Cmd+A from a cell of the grid", async () => {
    const { user } = await openHub({}, 230);
    await user.click(within(tableRows()[0] as HTMLElement).getByRole("gridcell", { name: t("pageList.sort.title") }));
    await user.keyboard("{Meta>}a{/Meta}");
    expect(count()).toBe(tp("pageList.bulk.count", 234));
    expect(selectAllBox().checked).toBe(true);
    await user.keyboard("{Escape}");
    expect(bar()).toBeNull();
  });

  it("leaves the keys to a text field: Ctrl+A selects its text, Escape does not clear the selection", async () => {
    const { user } = await openHub();
    await user.click(box("Alpha"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.move") }));
    const filter = await screen.findByRole("textbox", { name: t("pageList.bulk.move.filter") });
    await user.type(filter, "arch");
    await user.keyboard("{Control>}a{/Control}");
    expect(count()).toBe(tp("pageList.bulk.count", 1));
    await user.keyboard("{Escape}"); // closes the picker, as any popover
    expect(screen.queryByRole("dialog", { name: t("pageList.bulk.move.label") })).not.toBeInTheDocument();
    expect(count()).toBe(tp("pageList.bulk.count", 1));
  });

  it("does nothing in a view without rows, and Escape with nothing selected is left for the editor", async () => {
    const { user } = await openHub({ filters: [{ propertyId: PROP.summary, op: "contains", value: "nothing like it" }] });
    expect(within(block()).queryByRole("checkbox", { name: t("pageList.bulk.select", { title: "Alpha" }) })).not.toBeInTheDocument();
    await user.click(within(block()).getByRole("grid"));
    await user.keyboard("{Control>}a{/Control}{Escape}");
    expect(bar()).toBeNull();
  });
});

// ----- the bar ------------------------------------------------------------------------------

describe("the action bar", () => {
  it("takes the place of the tabs and the toolbar while rows are selected, counts them, and its close button clears the selection", async () => {
    const { user } = await openHub();
    expect(within(block()).getByRole("toolbar")).toBeInTheDocument();
    await user.click(box("Alpha"));
    expect(within(block()).queryByRole("toolbar")).not.toBeInTheDocument();
    expect(count()).toBe(tp("pageList.bulk.count", 1));
    for (const name of ["editProperty", "move", "duplicate", "trash"] as const) {
      expect(within(barOf()).getByRole("button", { name: t(`pageList.bulk.${name}`) })).toBeEnabled();
    }
    await user.click(box("beta"));
    expect(count()).toBe(tp("pageList.bulk.count", 2));

    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.close") }));
    expect(bar()).toBeNull();
    expect(within(block()).getByRole("toolbar")).toBeInTheDocument();
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual([]);
    // The bar went with the selection: focus is on the first checkbox of the block, not nowhere.
    await waitFor(() => {
      expect(selectAllBox()).toHaveFocus();
    });
  });

  it("waits for fewer than 1,001 pages: it says so, and keeps only the button that clears the selection", async () => {
    const { user } = await openHub({}, 997);
    await user.click(selectAllBox());
    expect(count()).toBe(tp("pageList.bulk.count", 1001));
    expect(within(barOf()).getByText(t("pageList.bulk.tooMany", { max: "1,000" }))).toBeInTheDocument();
    for (const name of ["editProperty", "move", "duplicate", "trash"] as const) {
      expect(within(barOf()).getByRole("button", { name: t(`pageList.bulk.${name}`) })).toBeDisabled();
    }
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.close") }));
    expect(bar()).toBeNull();
    // One page fewer and the actions are there.
    await user.click(selectAllBox());
    await user.click(box("Alpha"));
    expect(count()).toBe(tp("pageList.bulk.count", 1000));
    expect(within(barOf()).queryByText(/at a time/u)).not.toBeInTheDocument();
    expect(within(barOf()).getByRole("button", { name: t("pageList.bulk.duplicate") })).toBeEnabled();
  }, 60_000);

  it("drops a row that left the view from the selection", async () => {
    const { user, client, ids } = await openHub();
    await user.click(box("Alpha"));
    await user.click(box("beta"));
    expect(count()).toBe(tp("pageList.bulk.count", 2));
    await client.pageTrash(ids.page("beta"));
    await waitFor(() => {
      expect(count()).toBe(tp("pageList.bulk.count", 1));
    });
    expect(within(block()).queryByRole("checkbox", { name: t("pageList.bulk.select", { title: "beta" }) })).not.toBeInTheDocument();
    expect(box("Alpha")).toBeChecked();
  });

  it("is reset by a change of the layout of the view", async () => {
    const { user, editor } = await openHub();
    await user.click(box("Alpha"));
    expect(bar()).not.toBeNull();
    act(() => {
      setBlockAttribute(editor, "layout", "list");
    });
    await waitFor(() => {
      expect(listRows()).toHaveLength(4);
    });
    expect(bar()).toBeNull();
    expect(ticked(["Alpha", "beta", "delta", "Gamma"])).toEqual([]);
  });
});

// ----- the actions ---------------------------------------------------------------------------

/** The titles of the rows of the table whose checkbox is ticked. */
function tickedRows(): string[] {
  return tableRows()
    .filter((row) => within(row).getByRole<HTMLInputElement>("checkbox").checked)
    .map((row) => row.querySelector(".page-table-title")?.textContent ?? "");
}

describe("Duplicate", () => {
  it("sends one request for the selected pages in display order, selects the copies, and its toast has an Undo that is a structural undo", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("beta"));
    await user.click(box("Alpha"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.duplicate") }));
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(1);
    });
    expect(stand.bulk[0]).toEqual({ spaceId, pageIds: [ids.page("Alpha"), ids.page("beta")], action: { type: "Duplicate" } });
    await toastText(tp("pageList.bulk.toast.duplicated", 2));

    // The copies come into the view; they, and only they, are selected.
    await waitFor(() => {
      expect(tableTitles()).toHaveLength(6);
      expect(tickedRows()).toEqual(["Alpha (copy)", "beta (copy)"]);
    });
    expect(count()).toBe(tp("pageList.bulk.count", 2));

    await user.click(undoButton());
    await waitFor(() => {
      expect(stand.undone).toEqual([spaceId]);
    });
    await toastText(t("pageList.bulk.toast.undone"));
  });
});

describe("Move to trash", () => {
  it("sends Trash, clears the selection, and its toast has an Undo that is a structural undo", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    shiftClick(box("beta"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.trash") }));
    await waitFor(() => {
      expect(stand.bulk).toEqual([{ spaceId, pageIds: [ids.page("Alpha"), ids.page("beta")], action: { type: "Trash" } }]);
    });
    await toastText(tp("pageList.bulk.toast.trashed", 2));
    expect(bar()).toBeNull();
    await waitFor(() => {
      expect(tableTitles()).toEqual(["delta", "Gamma"]);
    });
    expect(within(block()).getByRole("toolbar")).toBeInTheDocument();

    await user.click(undoButton());
    await waitFor(() => {
      expect(stand.undone).toEqual([spaceId]);
    });
  });
});

describe("Move to…", () => {
  it("lists the pages of the space but not the selected ones, filters them, and sends Move under the one picked", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    await user.click(box("beta"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.move") }));
    const picker = await screen.findByRole("dialog", { name: t("pageList.bulk.move.label") });
    const names = within(picker)
      .getAllByRole("button")
      .map((button) => button.textContent ?? "");
    expect(names).toContain(t("pageList.bulk.move.topLevel"));
    expect(names).toEqual(expect.arrayContaining(["Hub", "delta", "Gamma", "Archive", "Old", "Elsewhere"]));
    expect(names).not.toContain("Alpha");
    expect(names).not.toContain("beta");

    await user.type(within(picker).getByRole("textbox", { name: t("pageList.bulk.move.filter") }), "arch");
    expect(within(picker).getAllByRole("button").map((button) => button.textContent)).toEqual([t("pageList.bulk.move.topLevel"), "Archive"]);
    await user.click(within(picker).getByRole("button", { name: "Archive" }));

    await waitFor(() => {
      expect(stand.bulk).toEqual([
        { spaceId, pageIds: [ids.page("Alpha"), ids.page("beta")], action: { type: "Move", data: { parentId: ids.page("Archive") } } },
      ]);
    });
    await toastText(tp("pageList.bulk.toast.moved", 2));
    expect(bar()).toBeNull();
    await waitFor(() => {
      expect(tableTitles()).toEqual(["delta", "Gamma"]);
    });
    await user.click(undoButton());
    await waitFor(() => {
      expect(stand.undone).toEqual([spaceId]);
    });
  });

  it("sends Move with no parent for Top level", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Gamma"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.move") }));
    await user.click(await screen.findByRole("button", { name: t("pageList.bulk.move.topLevel") }));
    await waitFor(() => {
      expect(stand.bulk).toEqual([{ spaceId, pageIds: [ids.page("Gamma")], action: { type: "Move", data: {} } }]);
    });
    await toastText(tp("pageList.bulk.toast.moved", 1));
  });
});

describe("Edit property", () => {
  async function chooseProperty(user: User, name: string): Promise<HTMLElement> {
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.editProperty") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.bulk.editProperty.menu") });
    await user.click(within(menu).getByRole("menuitem", { name }));
    return screen.findByRole("group", { name: t("pageList.bulk.editing", { property: name }) });
  }

  it("offers the properties that take a value, in the order of the schema, never a computed one", async () => {
    const { user } = await openHub();
    await user.click(box("Alpha"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.editProperty") }));
    const menu = await screen.findByRole("menu", { name: t("pageList.bulk.editProperty.menu") });
    const names = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);
    expect(names.slice(-4)).toEqual(["Priority", "Stage", "Summary", "Done"]);
  });

  it("sets an option on every selected page, then the Undo of its toast sends each page's previous value back", async () => {
    const { user, memory, ids, spaceId, defs } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    await user.click(box("beta"));
    const editor = await chooseProperty(user, "Stage");
    // The other actions give way to the editor; nothing is sent until Apply.
    expect(within(barOf()).queryByRole("button", { name: t("pageList.bulk.duplicate") })).not.toBeInTheDocument();
    expect(within(editor).getByRole("button", { name: t("pageList.bulk.apply") })).toBeDisabled();
    await user.click(within(editor).getByRole("button", { name: t("properties.value.label", { name: "Stage" }) }));
    await user.click(within(await screen.findByRole("listbox", { name: "Stage" })).getByRole("option", { name: "Later" }));
    expect(stand.bulk).toHaveLength(0);
    await user.click(within(editor).getByRole("button", { name: t("pageList.bulk.apply") }));

    await waitFor(() => {
      expect(stand.bulk).toHaveLength(1);
    });
    const later = optionOf(defs, PROP.stage, "Later");
    expect(stand.bulk[0]).toEqual({
      spaceId,
      pageIds: [ids.page("Alpha"), ids.page("beta")],
      action: { type: "SetValue", data: { propertyId: PROP.stage, value: { kind: "select", value: later } } },
    });
    await toastText(tp("pageList.bulk.toast.set", 2, { property: "Stage" }));
    // Back to the buttons, on the one that opened the editor; the selection stays.
    await waitFor(() => {
      expect(within(barOf()).getByRole("button", { name: t("pageList.bulk.editProperty") })).toHaveFocus();
    });
    expect(count()).toBe(tp("pageList.bulk.count", 2));

    await user.click(undoButton());
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(2);
    });
    expect(stand.bulk[1]).toEqual({
      spaceId,
      pageIds: [ids.page("Alpha"), ids.page("beta")],
      action: {
        type: "SetValues",
        data: {
          propertyId: PROP.stage,
          values: [
            { kind: "select", value: optionOf(defs, PROP.stage, "Draft") },
            { kind: "select", value: optionOf(defs, PROP.stage, "Review") },
          ],
        },
      },
    });
    await toastText(t("pageList.bulk.toast.undone"));
  });

  it("clears a property with Clear, and Undo restores the values, a page that had none getting none", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    await user.click(box("Gamma"));
    const editor = await chooseProperty(user, "Priority");
    await user.click(within(editor).getByRole("button", { name: t("pageList.bulk.clearValue") }));
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(1);
    });
    expect(stand.bulk[0]).toEqual({
      spaceId,
      pageIds: [ids.page("Alpha"), ids.page("Gamma")],
      action: { type: "SetValue", data: { propertyId: PROP.priority, value: null } },
    });
    await toastText(tp("pageList.bulk.toast.cleared", 2, { property: "Priority" }));

    await user.click(undoButton());
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(2);
    });
    expect(stand.bulk[1]?.action).toEqual({
      type: "SetValues",
      data: { propertyId: PROP.priority, values: [{ kind: "number", value: 3 }, null] },
    });
  });

  it("applies what is typed in a text field on Enter", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    await user.click(box("delta"));
    const editor = await chooseProperty(user, "Priority");
    await user.type(within(editor).getByRole("textbox", { name: "Priority" }), "7{Enter}");
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(1);
    });
    expect(stand.bulk[0]).toEqual({
      spaceId,
      pageIds: [ids.page("Alpha"), ids.page("delta")],
      action: { type: "SetValue", data: { propertyId: PROP.priority, value: { kind: "number", value: 7 } } },
    });
  });

  it("sets a checkbox, and Cancel goes back to the buttons without sending anything", async () => {
    const { user, memory, ids, spaceId } = await openHub();
    const stand = standInForBulk(memory);
    await user.click(box("Alpha"));
    let editor = await chooseProperty(user, "Done");
    await user.click(within(editor).getByRole("checkbox", { name: "Done" }));
    await user.click(within(editor).getByRole("button", { name: t("pageList.bulk.apply") }));
    await waitFor(() => {
      expect(stand.bulk).toHaveLength(1);
    });
    expect(stand.bulk[0]).toEqual({
      spaceId,
      pageIds: [ids.page("Alpha")],
      action: { type: "SetValue", data: { propertyId: PROP.done, value: { kind: "checkbox", value: true } } },
    });

    editor = await chooseProperty(user, "Summary");
    await user.click(within(editor).getByRole("button", { name: t("pageList.bulk.cancel") }));
    expect(within(barOf()).getByRole("button", { name: t("pageList.bulk.duplicate") })).toBeInTheDocument();
    expect(stand.bulk).toHaveLength(1);
  });
});

describe("a refusal of the core", () => {
  it("is said in the bar, nothing changes, the selection stays and the buttons work again", async () => {
    const { user, memory } = await openHub();
    const stand = standInForBulk(memory);
    stand.control.fail = "no room for that";
    await user.click(box("Alpha"));
    await user.click(within(barOf()).getByRole("button", { name: t("pageList.bulk.trash") }));
    const alert = await within(barOf()).findByRole("alert");
    expect(alert).toHaveTextContent("no room for that");
    expect(count()).toBe(tp("pageList.bulk.count", 1));
    expect(tableTitles()).toContain("Alpha");
    expect(within(barOf()).getByRole("button", { name: t("pageList.bulk.trash") })).toBeEnabled();
    expect(within(screen.getByRole("log", { name: t("toast.label") })).queryByRole("button")).not.toBeInTheDocument(); // no toast, so no Undo
  });
});

// ----- the row limit --------------------------------------------------------------------------

describe("the row limit", () => {
  it("draws 200 rows of a list, says so, and Show more adds up to 200 more", async () => {
    const { user } = await openHub({ layout: "list" }, 230);
    expect(listRows()).toHaveLength(ROW_LIMIT_STEP);
    expect(within(block()).getByText(t("pageList.rows.shown", { shown: 200, total: 234 }))).toBeInTheDocument();
    await user.click(within(block()).getByRole("button", { name: t("pageList.rows.showMore", { count: 34 }) }));
    expect(listRows()).toHaveLength(234);
    expect(within(block()).queryByText(/Showing/u)).not.toBeInTheDocument();
    expect(within(block()).queryByRole("button", { name: /Show \d+ more/u })).not.toBeInTheDocument();
  });

  it("draws 200 rows of a table (the window of rows in view inside them), and the footer still counts every row", async () => {
    const { user } = await openHub({ calculations: { title: "count_all" } }, 230);
    const scroller = grid().closest(".page-table-scroll") as HTMLElement;
    const scrollTo = (row: number): void => {
      fireEvent.scroll(scroller, { target: { scrollTop: row * 34 } });
    };
    scrollTo(199);
    // The 200th row is "Row 195" (Alpha, beta, delta, Gamma come first); the next rows are not drawn.
    expect(tableTitles().at(-1)).toBe("Row 195");
    expect(tableTitles()).not.toContain("Row 196");
    expect(within(block()).getByText(t("pageList.rows.shown", { shown: 200, total: 234 }))).toBeInTheDocument();
    expect(within(grid()).getByRole("gridcell", { name: t("pageTable.calc.cell", { name: t("pageList.sort.title") }) })).toHaveTextContent("234");

    await user.click(within(block()).getByRole("button", { name: t("pageList.rows.showMore", { count: 34 }) }));
    scrollTo(233);
    expect(tableTitles().at(-1)).toBe("Row 229");
    expect(within(block()).queryByRole("button", { name: /Show \d+ more/u })).not.toBeInTheDocument();
  });

  it("draws 200 cards per column of a board, the count of the column staying whole, and Show more belongs to its column", async () => {
    const { user } = await openHub({ layout: "board" }, 205);
    // Gamma and the 205 rows have no Stage: they are the first column.
    const noneName = t("pageBoard.none", { name: "Stage" });
    const none = within(block()).getByRole("group", { name: noneName });
    expect(none.querySelectorAll(".page-board-card")).toHaveLength(ROW_LIMIT_STEP);
    expect(within(none).getByLabelText(t("pageBoard.count", { count: 206 }))).toBeInTheDocument();
    // The other columns are short: no button there.
    const draft = within(block()).getByRole("group", { name: "Draft" });
    expect(within(draft).queryByRole("button", { name: /Show/u })).not.toBeInTheDocument();

    await user.click(within(none).getByRole("button", { name: t("pageList.rows.showMoreIn", { name: noneName, count: 6 }) }));
    expect(none.querySelectorAll(".page-board-card")).toHaveLength(206);
    expect(within(none).queryByRole("button", { name: /Show/u })).not.toBeInTheDocument();
  });

  it("starts again at 200 when the view changes", async () => {
    const views = [
      { id: "v1", name: "First", layout: "list", properties: [], sort: { by: "title", dir: "asc" } },
      { id: "v2", name: "Second", layout: "list", properties: [], sort: { by: "title", dir: "asc" } },
    ];
    const { user } = await openHub({ views }, 230);
    await user.click(within(block()).getByRole("button", { name: /Show \d+ more/u }));
    expect(listRows()).toHaveLength(234);
    await user.click(within(block()).getByRole("tab", { name: "Second" }));
    expect(listRows()).toHaveLength(ROW_LIMIT_STEP);
  });

  it("selects the rows beyond the limit with Select all, and the bar counts them", async () => {
    const { user } = await openHub({}, 230);
    await user.click(selectAllBox());
    expect(count()).toBe(tp("pageList.bulk.count", 234));
    expect(selectAllBox().checked).toBe(true);
    expect(selectAllBox().indeterminate).toBe(false);
  });
});
