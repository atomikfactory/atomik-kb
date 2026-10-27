// Relations and rollups in the properties panel (ADR-0027): the page picker, the chips, the paired
// property on the other page, the new property form (two-way or not, the rollup choices), the
// rollup's result and its refresh when a related page changes, and the settings of a rollup.

import { CoreClient } from "@atomik-kb/core-client";
import type { SeedSpace } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { t } from "./i18n";
import {
  formatRollupNumber,
  pairedPropertyOf,
  rollupItemText,
  rollupTexts,
} from "./property-format";
import { idsOf } from "./test/page";
import { openProperties, propertyRow, renderProperties, rowNames, schemaOf, valueOf } from "./test/properties";

const SPACE: SeedSpace = {
  name: "Personal",
  properties: [
    { name: "Tasks", kind: "relation", twoWay: true, backName: "Project" },
    { name: "Links", kind: "relation" },
    { name: "Cost", kind: "number" },
    { name: "Due", kind: "date" },
    { name: "Total", kind: "rollup", rollup: { relation: "Tasks", target: "Cost", function: "sum" } },
    { name: "Count", kind: "rollup", rollup: { relation: "Tasks", target: "title", function: "count_all" } },
  ],
  pages: [
    { title: "Launch", properties: { Tasks: ["Write", "Ship"] } },
    { title: "Write", properties: { Cost: 3 } },
    { title: "Ship", properties: { Cost: 4.5 } },
    { title: "Loose" },
  ],
};

const chipNames = (row: HTMLElement): string[] =>
  Array.from(row.querySelectorAll(".relation-chip-title")).map((node) => node.textContent ?? "");

describe("rollup texts", () => {
  const day = (date: string, time: string | null): string => (time === null ? date : `${date} ${time}`);
  it("writes numbers, percents, days, progress and lists of values", () => {
    expect(formatRollupNumber(7.5, "sum")).toBe("7.5");
    expect(formatRollupNumber(100 / 3, "percent_checked")).toBe(t("properties.rollup.percent", { value: "33.33" }));
    expect(rollupTexts("sum", { type: "number", data: 12 }, undefined, day)).toEqual(["12"]);
    expect(rollupTexts("earliest", { type: "date", data: { date: "2026-10-07", time: "09:00" } }, undefined, day)).toEqual([
      "2026-10-07 09:00",
    ]);
    expect(rollupTexts("checklist_progress", { type: "progress", data: { done: 2, total: 3 } }, undefined, day)).toEqual([
      t("properties.rollup.progress", { done: 2, total: 3 }),
    ]);
    const stage = {
      id: "p",
      name: "Stage",
      kind: "select" as const,
      options: [{ id: "o1", name: "Draft", color: "gray" as const }],
      createdAt: 0,
    };
    expect(
      rollupTexts(
        "show_original",
        {
          type: "values",
          data: [
            { kind: "select", value: "o1" },
            { kind: "select", value: "gone" },
            { kind: "checkbox", value: true },
          ],
        },
        stage,
        day,
      ),
    ).toEqual(["Draft", t("properties.rollup.yes")]);
    expect(rollupItemText({ kind: "number", value: 1.5 }, undefined, day)).toBe("1.5");
  });

  it("finds the paired property of a two-way relation", () => {
    const forward = {
      id: "a",
      name: "A",
      kind: "relation" as const,
      options: [],
      createdAt: 0,
      config: { kind: "relation" as const, twoWay: true, pairedPropertyId: "b" },
    };
    const back = { ...forward, id: "b", name: "B", config: { kind: "relation" as const, twoWay: true, pairedPropertyId: "a" } };
    const alone = { ...forward, id: "c", config: { kind: "relation" as const, twoWay: false } };
    expect(pairedPropertyOf(forward, [forward, back])?.name).toBe("B");
    expect(pairedPropertyOf(alone, [forward, back, alone])).toBeUndefined();
  });
});

describe("the relation editor", () => {
  it("shows the linked pages as chips and the back links on the other page", async () => {
    const { user } = await renderProperties([SPACE]);
    const launch = await openProperties(user, "Launch");
    expect(rowNames(launch)).toEqual(["Tags", "Tasks", "Total", "Count"]);
    expect(chipNames(propertyRow(launch, "Tasks"))).toEqual(["Write", "Ship"]);
    const write = await openProperties(user, "Write");
    expect(rowNames(write)).toEqual(["Tags", "Project", "Cost", "Total", "Count"]);
    expect(chipNames(propertyRow(write, "Project"))).toEqual(["Launch"]);
  });

  it("adds a page through the picker and removes one, and the other side follows", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const launch = await openProperties(user, "Launch");
    const row = propertyRow(launch, "Tasks");
    await user.click(within(row).getByRole("button", { name: t("properties.relation.addTo", { name: "Tasks" }) }));
    const search = await screen.findByRole("combobox", { name: t("properties.relation.search") });
    await user.type(search, "Lo");
    const option = await screen.findByRole("option", { name: /Loose/u });
    // The pages already linked are not offered again.
    expect(screen.queryByRole("option", { name: /Write/u })).not.toBeInTheDocument();
    await user.click(option);
    await waitFor(() => {
      expect(chipNames(row)).toEqual(["Write", "Ship", "Loose"]);
    });
    const { page } = await idsOf(memory);
    expect(await valueOf(memory, "Launch", "Tasks")).toEqual({
      kind: "relation",
      value: [page("Write"), page("Ship"), page("Loose")],
    });
    expect(await valueOf(memory, "Loose", "Project")).toEqual({ kind: "relation", value: [page("Launch")] });

    await user.click(within(row).getByRole("button", { name: t("properties.relation.remove", { title: "Write", name: "Tasks" }) }));
    await waitFor(() => {
      expect(chipNames(row)).toEqual(["Ship", "Loose"]);
    });
    expect(await valueOf(memory, "Write", "Project")).toBeUndefined();
  });

  it("starts empty with an add button, offers the pages of the space only, and opens a page from its chip", async () => {
    const { user, memory } = await renderProperties([SPACE, { name: "Work", pages: [{ title: "Elsewhere" }] }]);
    const loose = await openProperties(user, "Loose");
    await user.click(within(loose).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: /^Links/u }));
    const row = propertyRow(loose, "Links");
    await user.click(await within(row).findByRole("button", { name: t("properties.relation.add") }));
    const search = await screen.findByRole("combobox", { name: t("properties.relation.search") });
    await user.type(search, "Elsewhere");
    await screen.findByText(t("properties.relation.none"));
    await user.clear(search);
    await user.type(search, "Wri");
    await user.keyboard("{ArrowDown}{ArrowUp}{Enter}");
    await waitFor(() => {
      expect(chipNames(row)).toEqual(["Write"]);
    });
    expect(await valueOf(memory, "Loose", "Links")).toMatchObject({ kind: "relation" });
    // A click on the chip opens the page.
    await user.click(within(row).getByRole("button", { name: t("properties.relation.open", { title: "Write" }) }));
    const write = await screen.findByRole("region", { name: t("properties.title") });
    await waitFor(() => {
      expect(rowNames(write)).toContain("Cost");
    });
  });

  it("dims a page in the trash and shows a page that is gone as unknown", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    const launch = await openProperties(user, "Launch");
    await client.pageTrash(page("Ship"));
    await waitFor(() => {
      const chip = within(propertyRow(launch, "Tasks")).getByRole("button", { name: t("properties.relation.open", { title: "Ship" }) });
      expect(chip.closest(".relation-chip")).toHaveClass("is-trashed");
    });
    await client.pagePurge(page("Ship"));
    await waitFor(() => {
      expect(chipNames(propertyRow(launch, "Tasks"))).toEqual(["Write"]);
    });
  });
});

describe("the new property form", () => {
  async function openForm(user: Awaited<ReturnType<typeof renderProperties>>["user"], title: string) {
    const panel = await openProperties(user, title);
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.new.menu") }));
    return { panel, dialog: await screen.findByRole("dialog", { name: t("properties.new.title") }) };
  }

  it("asks whether a relation is two-way and creates both properties", async () => {
    const { user, memory } = await renderProperties([{ name: "Personal", pages: [{ title: "A" }] }]);
    const { dialog } = await openForm(user, "A");
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Blocks");
    await user.click(within(dialog).getByRole("radio", { name: "Relation" }));
    const check = within(dialog).getByRole("checkbox", { name: t("properties.new.twoWay") });
    expect(check).toBeChecked();
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).map((def) => def.name)).toEqual(["Tags", "Blocks", "Blocks (back)"]);
    });
    const schema = await schemaOf(memory);
    expect(schema[1]?.config).toEqual({ kind: "relation", twoWay: true, pairedPropertyId: schema[2]?.id });
  });

  it("makes a one-way relation when the box is unchecked", async () => {
    const { user, memory } = await renderProperties([{ name: "Personal", pages: [{ title: "A" }] }]);
    const { dialog } = await openForm(user, "A");
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "See also");
    await user.click(within(dialog).getByRole("radio", { name: "Relation" }));
    await user.click(within(dialog).getByRole("checkbox", { name: t("properties.new.twoWay") }));
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).map((def) => def.name)).toEqual(["Tags", "See also"]);
    });
    expect((await schemaOf(memory))[1]?.config).toEqual({ kind: "relation", twoWay: false });
  });

  it("offers only the functions a target accepts and creates the rollup", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const { dialog } = await openForm(user, "Launch");
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Average cost");
    await user.click(within(dialog).getByRole("radio", { name: "Rollup" }));
    const target = within(dialog).getByRole("combobox", { name: t("properties.rollup.target") });
    const fn = within(dialog).getByRole("combobox", { name: t("properties.rollup.function") });
    const names = (select: HTMLElement) => within(select).getAllByRole("option").map((option) => option.textContent);
    // The titles take the counting functions only.
    expect(names(fn)).not.toContain(t("properties.rollup.fn.sum"));
    await user.selectOptions(target, "Cost");
    expect(names(fn)).toContain(t("properties.rollup.fn.sum"));
    expect(names(fn)).not.toContain(t("properties.rollup.fn.earliest"));
    await user.selectOptions(fn, t("properties.rollup.fn.average"));
    await user.selectOptions(within(dialog).getByRole("combobox", { name: t("properties.rollup.relation") }), "Tasks");
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).at(-1)).toMatchObject({ name: "Average cost", kind: "rollup" });
    });
    const created = (await schemaOf(memory)).at(-1);
    const cost = (await schemaOf(memory)).find((def) => def.name === "Cost");
    const tasks = (await schemaOf(memory)).find((def) => def.name === "Tasks");
    expect(created?.config).toEqual({ kind: "rollup", relationPropertyId: tasks?.id, target: cost?.id, function: "average" });
    expect(await valueOf(memory, "Launch", "Average cost")).toMatchObject({ kind: "rollup", value: { result: { data: 3.75 } } });
  });

  it("says a relation is needed first when the space has none", async () => {
    const { user } = await renderProperties([{ name: "Personal", pages: [{ title: "A" }] }]);
    const { dialog } = await openForm(user, "A");
    await user.click(within(dialog).getByRole("radio", { name: "Rollup" }));
    expect(within(dialog).getByText(t("properties.rollup.noRelation"))).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: t("properties.new.create") })).toBeDisabled();
  });
});

describe("the rollup value", () => {
  it("shows its result read only on every page and follows the related pages", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const launch = await openProperties(user, "Launch");
    const total = propertyRow(launch, "Total");
    expect(within(total).getByLabelText(/^Total: /u)).toHaveTextContent("7.5");
    expect(within(total).queryByRole("textbox")).not.toBeInTheDocument();
    expect(within(propertyRow(launch, "Count")).getByLabelText(/^Count: /u)).toHaveTextContent("2");
    // A page without links: the sum of nothing is 0.
    const loose = await openProperties(user, "Loose");
    expect(within(propertyRow(loose, "Total")).getByLabelText(/^Total: /u)).toHaveTextContent("0");

    // A related page changes elsewhere: the rollup of the open page is refreshed.
    const again = await openProperties(user, "Launch");
    const { page } = await idsOf(memory);
    const cost = (await schemaOf(memory)).find((def) => def.name === "Cost");
    await new CoreClient(memory).pagePropertySet(page("Write"), cost?.id ?? "", { kind: "number", value: 10 });
    await waitFor(() => {
      expect(within(propertyRow(again, "Total")).getByLabelText(/^Total: /u)).toHaveTextContent("14.5");
    });
  });

  it("is not offered by 'Add a property' and cannot be hidden", async () => {
    const { user } = await renderProperties([SPACE]);
    const loose = await openProperties(user, "Loose");
    await user.click(within(loose).getByRole("button", { name: t("properties.add") }));
    const names = Array.from(document.querySelectorAll(".properties-add-name")).map((node) => node.textContent);
    expect(names).not.toContain("Total");
    expect(names).toContain("Tasks");
  });

  it("is configured again from the settings of the property", async () => {
    const { user, memory } = await renderProperties([SPACE]);
    const launch = await openProperties(user, "Launch");
    await user.click(within(propertyRow(launch, "Total")).getByRole("button", { name: "Total" }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.menu.settings") }));
    const dialog = await screen.findByRole("dialog", { name: t("properties.dialog.title") });
    const fn = await within(dialog).findByRole("combobox", { name: t("properties.rollup.function") });
    await user.selectOptions(fn, t("properties.rollup.fn.max"));
    await user.click(within(dialog).getByRole("button", { name: t("properties.rollup.save") }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Launch", "Total")).toMatchObject({ value: { function: "max", result: { data: 4.5 } } });
    });
  });
});
