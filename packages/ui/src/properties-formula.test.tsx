// The formula property in the app (ADR-0055): the result in the properties panel (read only, on every page), "Formula" in the
// kind picker of a new property (the editor opens once the property has a name, and the property exists only when the formula
// is saved), "Edit formula…" in the property's menu, the schema dialog's details and the menu of a table column, and a formula
// column in a table (cells, sort, the editor previewing on the first row). The editor itself is `formula-editor.test.tsx`'s.

import type { JsonValue, SeedSpace } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import { pid } from "./test/editor";
import { openPage, renderPages } from "./test/page";
import { PROPERTY_SPACE, openProperties, propertyRow, renderProperties, schemaOf } from "./test/properties";
import type { User } from "./test/render";

const SPACE: SeedSpace = {
  ...PROPERTY_SPACE,
  properties: [
    ...(PROPERTY_SPACE.properties ?? []),
    { name: "Score", kind: "formula", formula: 'prop("Priority") * 10' },
    { name: "Late", kind: "formula", formula: 'prop("Priority") > 2' },
    { name: "Label", kind: "formula", formula: '"P" + format(prop("Priority"))' },
  ],
};

const field = (): HTMLElement => screen.getByRole("combobox", { name: t("formula.editor.code") });

async function openMenu(user: User, panel: HTMLElement, name: string): Promise<HTMLElement> {
  await user.click(within(propertyRow(panel, name)).getByRole("button", { name }));
  return screen.findByRole("menu", { name: t("properties.menu.label", { name }) });
}

describe("the result in the properties panel", () => {
  it("is shown read only, on every page, as the kind of the result", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const score = propertyRow(panel, "Score");
    expect(await within(score).findByText("30")).toBeInTheDocument();
    expect(within(score).queryByRole("textbox")).not.toBeInTheDocument();
    expect(score).toHaveAttribute("data-kind", "formula");
    const late = propertyRow(panel, "Late");
    const box = await within(late).findByRole("checkbox", { name: t("pageList.checked") });
    expect(box).toBeChecked();
    expect(box).toBeDisabled();
    expect(await within(propertyRow(panel, "Label")).findByText("P3")).toBeInTheDocument();
  });

  it("is empty on a page where the formula has no value", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Plain");
    expect(await within(propertyRow(panel, "Score")).findByText(t("properties.empty"))).toBeInTheDocument();
  });

  it("cannot be hidden from the page (it shows on every page, like the other computed properties)", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Score");
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.hide") })).toBeDisabled();
  });
});

describe("a new formula", () => {
  async function openNewProperty(user: User, panel: HTMLElement): Promise<HTMLElement> {
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.new.menu") }));
    return screen.findByRole("dialog", { name: t("properties.new.title") });
  }

  it("offers Formula with its description, and writes the formula in the editor once it has a name", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const dialog = await openNewProperty(user, panel);
    expect(within(dialog).getByText(t("properties.kind.formula.hint"))).toBeInTheDocument();
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Double");
    await user.click(within(dialog).getByRole("radio", { name: "Formula" }));
    expect(within(dialog).getByText(t("formula.new.hint"))).toBeInTheDocument();
    // The button says what comes next, and nothing is created by it.
    await user.click(within(dialog).getByRole("button", { name: t("formula.new.next") }));
    const editor = await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Double" }) });
    expect((await schemaOf(memory)).map((def) => def.name)).not.toContain("Double");

    await user.type(within(editor).getByRole("combobox", { name: t("formula.editor.code") }), 'prop("Priority") * 2');
    await waitFor(() => {
      expect(within(editor).getByRole("button", { name: t("formula.editor.save") })).toBeEnabled();
    });
    await user.click(within(editor).getByRole("button", { name: t("formula.editor.save") }));

    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    const created = (await schemaOf(memory)).at(-1);
    expect(created).toMatchObject({ name: "Double", kind: "formula" });
    expect(created?.config).toMatchObject({ kind: "formula", resultType: { base: "number", list: false } });
    expect(await within(propertyRow(panel, "Double")).findByText("6")).toBeInTheDocument();
  });

  it("creates nothing when the editor is cancelled, and the form is still there", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const before = (await schemaOf(memory)).map((def) => def.name);
    const dialog = await openNewProperty(user, panel);
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Double");
    await user.click(within(dialog).getByRole("radio", { name: "Formula" }));
    await user.click(within(dialog).getByRole("button", { name: t("formula.new.next") }));
    const editor = await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Double" }) });
    await user.type(within(editor).getByRole("combobox"), "1 + 1");
    await user.click(within(editor).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog", { name: t("formula.editor.title", { name: "Double" }) })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: t("properties.new.title") })).toBeInTheDocument();
    expect((await schemaOf(memory)).map((def) => def.name)).toEqual(before);
  });

  it("asks for a name before the editor opens", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const dialog = await openNewProperty(user, panel);
    await user.click(within(dialog).getByRole("radio", { name: "Formula" }));
    await user.click(within(dialog).getByRole("button", { name: t("formula.new.next") }));
    expect(await within(dialog).findByText(t("properties.error.nameEmpty"))).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: /Formula:/u })).not.toBeInTheDocument();
  });
});

describe("editing a formula", () => {
  it("is in the menu of the property: the editor shows the names, and the result follows the change", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Score");
    await user.click(within(menu).getByRole("menuitem", { name: t("formula.menu.edit") }));
    const editor = await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Score" }) });
    expect(field()).toHaveValue('prop("Priority") * 10');
    // It previews on the page the panel shows.
    expect(within(editor).getByText(t("formula.preview.on", { title: "Notes" }))).toBeInTheDocument();
    await user.clear(field());
    await user.type(field(), 'prop("Priority") * 20');
    await waitFor(() => {
      expect(within(editor).getByRole("button", { name: t("formula.editor.save") })).toBeEnabled();
    });
    await user.click(within(editor).getByRole("button", { name: t("formula.editor.save") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(await within(propertyRow(panel, "Score")).findByText("60")).toBeInTheDocument();
    const score = (await schemaOf(memory)).find((def) => def.name === "Score");
    const priority = (await schemaOf(memory)).find((def) => def.name === "Priority");
    expect(score?.config).toMatchObject({ kind: "formula", expression: `prop("${priority?.id ?? ""}") * 20` });
  });

  it("gives focus back to the property's name when the editor is cancelled", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Score");
    await user.click(within(menu).getByRole("menuitem", { name: t("formula.menu.edit") }));
    await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Score" }) });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(within(propertyRow(panel, "Score")).getByRole("button", { name: "Score" })).toHaveFocus();
  });

  it("has no \"Edit formula\" in the menu of a property that is not one", async () => {
    const { user } = await renderProperties([SPACE]);
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Priority");
    expect(within(menu).queryByRole("menuitem", { name: t("formula.menu.edit") })).not.toBeInTheDocument();
  });

  it("is in the details of the property in the schema dialog, with its expression and the type of its result", async () => {
    const { user } = await renderProperties([SPACE]);
    await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.menu.space") }));
    const dialog = await screen.findByRole("dialog", { name: t("properties.dialog.title") });
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.details", { name: "Score" }) }));
    expect(await within(dialog).findByText('prop("Priority") * 10')).toBeInTheDocument();
    expect(within(dialog).getByText(t("formula.result", { type: t("formula.type.number") }))).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("formula.menu.edit") }));
    await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Score" }) });
    expect(field()).toHaveValue('prop("Priority") * 10');
    await user.keyboard("{Escape}");
    // Only the editor closed.
    expect(screen.getByRole("dialog", { name: t("properties.dialog.title") })).toBeInTheDocument();
  });
});

// ----- a formula column in a table ---------------------------------------------------------------------------------------------

const PRIORITY = "de000000-0000-4000-8000-00000000e001";
const SCORE = "de000000-0000-4000-8000-00000000e002";
const LATE = "de000000-0000-4000-8000-00000000e003";

function hub(): JsonValue {
  return {
    type: "doc",
    content: [
      { type: "paragraph", attrs: { id: pid(1) }, content: [{ type: "text", text: "Intro" }] },
      {
        type: "pageList",
        attrs: {
          id: pid(2),
          source: { kind: "children" },
          layout: "table",
          properties: [PRIORITY, SCORE, LATE],
          sort: { by: "title", dir: "asc" },
        },
      },
      { type: "paragraph", attrs: { id: pid(3) } },
    ],
  };
}

const TABLE_SPACE: SeedSpace = {
  name: "Personal",
  properties: [
    { id: PRIORITY, name: "Priority", kind: "number" },
    { id: SCORE, name: "Score", kind: "formula", formula: 'prop("Priority") * 10' },
    { id: LATE, name: "Late", kind: "formula", formula: 'prop("Priority") > 2' },
  ],
  pages: [
    {
      title: "Hub",
      content: hub(),
      children: [
        { title: "Alpha", properties: { Priority: 3 } },
        { title: "beta", properties: { Priority: 10 } },
        { title: "Gamma" },
        { title: "delta", properties: { Priority: 2 } },
      ],
    },
  ],
};

async function openHub() {
  const app = await renderPages([TABLE_SPACE]);
  await openPage(app.user, "Hub");
  const block = await screen.findByRole("group", { name: t("pageList.label") });
  return { ...app, block, grid: within(block).getByRole("grid", { name: t("pageTable.grid") }) };
}

function rowOf(grid: HTMLElement, title: string): HTMLElement {
  const row = within(grid)
    .getAllByRole("row")
    .find((candidate) => candidate.querySelector(".page-table-title")?.textContent === title);
  if (row === undefined) {
    throw new Error(`no row for ${title}`);
  }
  return row;
}

const titles = (grid: HTMLElement): string[] =>
  within(grid)
    .getAllByRole("row")
    .filter((row) => row.hasAttribute("data-page"))
    .map((row) => row.querySelector(".page-table-title")?.textContent ?? "");

describe("a formula column in a table", () => {
  it("shows the result in each cell, nothing where there is none, and the cell is not edited", async () => {
    const { user, grid } = await openHub();
    expect(await within(within(rowOf(grid, "Alpha")).getByRole("gridcell", { name: "Score" })).findByText("30")).toBeInTheDocument();
    expect(within(within(rowOf(grid, "beta")).getByRole("gridcell", { name: "Score" })).getByText("100")).toBeInTheDocument();
    expect(within(rowOf(grid, "Gamma")).getByRole("gridcell", { name: "Score" })).toHaveTextContent("");
    const late = within(rowOf(grid, "Alpha")).getByRole("gridcell", { name: "Late" });
    expect(within(late).getByRole("img", { name: t("pageList.checked") })).toBeInTheDocument();
    expect(within(within(rowOf(grid, "delta")).getByRole("gridcell", { name: "Late" })).getByRole("img", { name: t("pageList.unchecked") })).toBeInTheDocument();
    // A double click does not open an editor on a computed cell.
    await user.dblClick(within(rowOf(grid, "Alpha")).getByRole("gridcell", { name: "Score" }));
    expect(within(rowOf(grid, "Alpha")).queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("sorts by the result, the rows without one last", async () => {
    const { user, grid } = await openHub();
    await within(rowOf(grid, "Alpha")).findByText("30");
    await user.click(within(within(grid).getByRole("columnheader", { name: "Score" })).getByRole("button", { name: /^Score/u }));
    const menu = await screen.findByRole("menu", { name: t("pageTable.menu", { name: "Score" }) });
    await user.click(within(menu).getByRole("menuitem", { name: t("pageTable.sortDesc") }));
    expect(titles(grid)).toEqual(["beta", "Alpha", "delta", "Gamma"]);
  });

  it("has \"Edit formula…\" in the menu of its header, and only there", async () => {
    const { user, grid } = await openHub();
    await user.click(within(within(grid).getByRole("columnheader", { name: "Priority" })).getByRole("button", { name: /^Priority/u }));
    const plain = await screen.findByRole("menu", { name: t("pageTable.menu", { name: "Priority" }) });
    expect(within(plain).queryByRole("menuitem", { name: t("formula.menu.edit") })).not.toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.click(within(within(grid).getByRole("columnheader", { name: "Score" })).getByRole("button", { name: /^Score/u }));
    const menu = await screen.findByRole("menu", { name: t("pageTable.menu", { name: "Score" }) });
    await user.click(within(menu).getByRole("menuitem", { name: t("formula.menu.edit") }));
    const editor = await screen.findByRole("dialog", { name: t("formula.editor.title", { name: "Score" }) });
    // The editor previews on the first row of the table, not on the page that holds it.
    expect(within(editor).getByText(t("formula.preview.on", { title: "Alpha" }))).toBeInTheDocument();
    expect(field()).toHaveValue('prop("Priority") * 10');
    await user.clear(field());
    await user.type(field(), 'prop("Priority") + 1');
    expect(await within(editor).findByText("4")).toBeInTheDocument();
    await user.click(within(editor).getByRole("button", { name: t("formula.editor.save") }));
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
    expect(await within(within(rowOf(grid, "Alpha")).getByRole("gridcell", { name: "Score" })).findByText("4")).toBeInTheDocument();
  });
});
