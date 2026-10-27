// The properties dialog of the space menu (ADR-0026): the schema of a space with rename, reorder and
// delete, the number of pages that use a property (asked when its details open), and the options
// of a select, multi-select, status or Tags (edit, add, delete with its cascade).

import { CoreClient, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { t, tp } from "./i18n";
import { idsOf } from "./test/page";
import { renderProperties, schemaOf, valueOf } from "./test/properties";
import type { User } from "./test/render";

/** Opens the dialog from the space menu. */
async function openDialog(user: User): Promise<HTMLElement> {
  await user.click(screen.getByRole("button", { name: t("spaces.optionsLabel") }));
  await user.click(await screen.findByRole("menuitem", { name: t("properties.menu.space") }));
  return screen.findByRole("dialog", { name: t("properties.dialog.title") });
}

/** The row of a property in the dialog (its name must not be in the middle of a rename). */
function manageRow(dialog: HTMLElement, name: string): HTMLElement {
  const row = within(dialog).getByRole("button", { name: t("properties.manage.rename", { name }) }).closest("li");
  if (row === null) {
    throw new Error(`no row for ${name}`);
  }
  return row;
}

async function names(memory: Parameters<typeof schemaOf>[0]): Promise<string[]> {
  return (await schemaOf(memory)).map((def) => def.name);
}

describe("opening the dialog", () => {
  it("lists the properties of the space with their kinds, Tags first", async () => {
    const { user } = await renderProperties();
    const dialog = await openDialog(user);
    expect(within(dialog).getByText(t("properties.dialog.intro", { space: "Personal" }))).toBeInTheDocument();
    const list = within(dialog).getByRole("list", { name: t("properties.dialog.list") });
    const rows = within(list).getAllByRole("button", { name: /^Rename / }).map((button) => button.textContent);
    expect(rows).toEqual(["Tags", "Status", "Due", "Done", "Link", "Owner email", "Priority", "Summary", "Phone", "Stage", "Areas"]);
    expect(within(manageRow(dialog, "Status")).getByText("Status", { selector: ".manage-kind" })).toBeInTheDocument();
    expect(within(manageRow(dialog, "Priority")).getByText("Number", { selector: ".manage-kind" })).toBeInTheDocument();
  });

  it("closes with the button or Escape and gives focus back to the space menu button", async () => {
    const { user } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.queryByRole("dialog", { name: t("properties.dialog.title") })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("spaces.optionsLabel") })).toHaveFocus();

    await openDialog(user);
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: t("properties.dialog.title") })).not.toBeInTheDocument();
  });

  it("asks for the page counts only when a property's details are opened", async () => {
    const { user, memory } = await renderProperties();
    const dispatch = vi.spyOn(memory, "dispatch");
    const dialog = await openDialog(user);
    expect(dispatch.mock.calls.filter(([request]) => request.type === "PagesByProperty")).toHaveLength(0);
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.details", { name: "Priority" }) }));
    expect(await within(dialog).findByText(tp("properties.manage.usage", 1))).toBeInTheDocument();
    expect(dispatch.mock.calls.filter(([request]) => request.type === "PagesByProperty")).toHaveLength(1);

    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.details", { name: "Tags" }) }));
    expect(await within(dialog).findByText(tp("properties.manage.usage", 2))).toBeInTheDocument();
    // One details region at a time.
    expect(within(dialog).queryByText(tp("properties.manage.usage", 1))).not.toBeInTheDocument();
  });
});

describe("renaming and reordering", () => {
  it("renames a property in place", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.rename", { name: "Priority" }) }));
    const input = within(dialog).getByRole("textbox", { name: t("properties.rename.label") });
    await user.clear(input);
    await user.type(input, "Score{Enter}");
    await waitFor(async () => {
      expect(await names(memory)).toContain("Score");
    });
    expect(await within(dialog).findByRole("button", { name: t("properties.manage.rename", { name: "Score" }) })).toBeInTheDocument();
  });

  it("Escape cancels the rename and keeps the dialog open", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.rename", { name: "Priority" }) }));
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.rename.label") }), "Zzz{Escape}");
    expect(screen.getByRole("dialog", { name: t("properties.dialog.title") })).toBeInTheDocument();
    expect(within(dialog).queryByRole("textbox", { name: t("properties.rename.label") })).not.toBeInTheDocument();
    expect(await names(memory)).toContain("Priority");
  });

  it("shows the core's refusal of a name", async () => {
    const { user } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.rename", { name: "Priority" }) }));
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.rename.label") }), "x".repeat(101));
    await user.keyboard("{Enter}");
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/could not be renamed/);
  });

  it("moves a property up and down, never Tags and never in front of it", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    // Tags: nothing. The property next to Tags cannot go up; the last cannot go down.
    expect(within(dialog).getByRole("button", { name: t("properties.manage.up", { name: "Tags" }) })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("properties.manage.down", { name: "Tags" }) })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("properties.manage.delete", { name: "Tags" }) })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("properties.manage.up", { name: "Status" }) })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: t("properties.manage.down", { name: "Areas" }) })).toBeDisabled();

    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.up", { name: "Due" }) }));
    await waitFor(async () => {
      expect((await names(memory)).slice(0, 4)).toEqual(["Tags", "Due", "Status", "Done"]);
    });
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.down", { name: "Due" }) }));
    await waitFor(async () => {
      expect((await names(memory)).slice(0, 4)).toEqual(["Tags", "Status", "Due", "Done"]);
    });
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.down", { name: "Stage" }) }));
    await waitFor(async () => {
      expect((await names(memory)).slice(-2)).toEqual(["Areas", "Stage"]);
    });
    // The dialog shows the new order.
    const order = within(dialog).getAllByRole("button", { name: /^Rename / }).map((button) => button.textContent);
    expect(order.slice(-2)).toEqual(["Areas", "Stage"]);
  });
});

describe("deleting a property", () => {
  it("asks first, says how many pages lose a value, and deletes it", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.delete", { name: "Summary" }) }));
    const question = await screen.findByRole("alertdialog", { name: t("properties.delete.title", { name: "Summary" }) });
    expect(await within(question).findByText(tp("properties.delete.bodyCount", 1, { name: "Summary" }))).toBeInTheDocument();

    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    expect(await names(memory)).toContain("Summary");

    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.delete", { name: "Summary" }) }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("properties.delete.confirm") }));
    await waitFor(async () => {
      expect(await names(memory)).not.toContain("Summary");
    });
    expect(within(dialog).queryByRole("button", { name: t("properties.manage.rename", { name: "Summary" }) })).not.toBeInTheDocument();
    // The dialog is still there.
    expect(screen.getByRole("dialog", { name: t("properties.dialog.title") })).toBeInTheDocument();
  });
});

describe("new property", () => {
  it("creates a property from the dialog and opens its details", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.button") }));
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Mood");
    await user.click(within(dialog).getByRole("radio", { name: "Select" }));
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).at(-1)).toMatchObject({ name: "Mood", kind: "select" });
    });
    expect(await within(dialog).findByRole("button", { name: t("properties.manage.rename", { name: "Mood" }) })).toBeInTheDocument();
    // Its details are open: no page has a value, and there are no options yet.
    expect(await within(dialog).findByText(t("properties.manage.usage.none"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("properties.option.none"))).toBeInTheDocument();
  });
});

describe("options", () => {
  async function openDetails(user: User, dialog: HTMLElement, name: string): Promise<HTMLElement> {
    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.details", { name }) }));
    return manageRow(dialog, name);
  }

  it("adds an option with the next colour, and a status option in the group that is chosen", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const stage = await openDetails(user, dialog, "Stage");
    await user.type(within(stage).getByRole("textbox", { name: t("properties.manage.newOption") }), "Shipped");
    await user.click(within(stage).getByRole("button", { name: t("properties.manage.addOptionButton") }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.name === "Stage");
      expect(def?.options.map((option) => [option.name, option.color])).toContainEqual(["Shipped", "yellow"]);
    });
    expect(await within(stage).findByText("Shipped")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: t("properties.manage.details", { name: "Stage" }) })); // closes it
    const status = await openDetails(user, dialog, "Status");
    // The default options are listed under their groups.
    for (const [group, option] of [
      ["To-do", "Not started"],
      ["In progress", "In progress"],
      ["Done", "Done"],
    ] as const) {
      expect(within(within(status).getByRole("group", { name: group })).getByText(option, { selector: ".option-pill" })).toBeInTheDocument();
    }
    await user.type(within(status).getByRole("textbox", { name: t("properties.manage.newOption") }), "Blocked");
    await user.selectOptions(within(status).getByRole("combobox", { name: t("properties.option.group") }), "in_progress");
    await user.click(within(status).getByRole("button", { name: t("properties.manage.addOptionButton") }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.name === "Status");
      expect(def?.options.at(-1)).toMatchObject({ name: "Blocked", group: "in_progress" });
    });
  });

  it("refuses an empty option name", async () => {
    const { user } = await renderProperties();
    const dialog = await openDialog(user);
    const stage = await openDetails(user, dialog, "Stage");
    await user.click(within(stage).getByRole("button", { name: t("properties.manage.addOptionButton") }));
    expect(await within(stage).findByText(t("properties.error.optionEmpty"))).toBeInTheDocument();
  });

  it("edits an option's name, colour and group", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const status = await openDetails(user, dialog, "Status");
    await user.click(within(status).getByRole("button", { name: t("properties.option.edit", { name: "Not started" }) }));
    const form = within(status).getByRole("form", { name: t("properties.option.editTitle", { name: "Not started" }) });
    const name = within(form).getByRole("textbox", { name: t("properties.option.name") });
    await user.clear(name);
    await user.type(name, "Backlog");
    await user.click(within(form).getByRole("button", { name: t("callout.color.purple") }));
    await user.selectOptions(within(form).getByRole("combobox", { name: t("properties.option.group") }), "in_progress");
    await user.click(within(form).getByRole("button", { name: t("properties.option.save") }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.name === "Status");
      expect(def?.options[0]).toMatchObject({ name: "Backlog", color: "purple", group: "in_progress" });
    });
    expect(within(status).queryByRole("form", { name: t("properties.option.editTitle", { name: "Backlog" }) })).not.toBeInTheDocument();
    expect(within(within(status).getByRole("group", { name: "In progress" })).getByText("Backlog", { selector: ".option-pill" })).toBeInTheDocument();
  });

  it("cancels an edit with Escape without closing the dialog", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const stage = await openDetails(user, dialog, "Stage");
    await user.click(within(stage).getByRole("button", { name: t("properties.option.edit", { name: "Draft" }) }));
    await user.type(within(stage).getByRole("textbox", { name: t("properties.option.name") }), "ZZZ{Escape}");
    expect(within(stage).queryByRole("form", { name: t("properties.option.editTitle", { name: "Draft" }) })).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: t("properties.dialog.title") })).toBeInTheDocument();
    expect((await schemaOf(memory)).find((def) => def.name === "Stage")?.options[0]?.name).toBe("Draft");
  });

  it("deleting an option asks first and takes it out of every page that has it", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const tags = await openDetails(user, dialog, "Tags");
    await user.click(within(tags).getByRole("button", { name: t("properties.option.deleteLabel", { name: "idea" }) }));
    const question = await screen.findByRole("alertdialog", { name: t("properties.option.deleteTitle", { name: "idea" }) });
    expect(
      await within(question).findByText(tp("properties.option.deleteBodyCount", 1, { name: "idea", property: "Tags" })),
    ).toBeInTheDocument();
    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    expect(await valueOf(memory, "Notes", "Tags")).toBeDefined();

    await user.click(within(tags).getByRole("button", { name: t("properties.option.deleteLabel", { name: "idea" }) }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("properties.option.deleteConfirm") }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.id === TAGS_PROPERTY_ID);
      expect(def?.options.map((option) => option.name)).toEqual(["project"]);
    });
    // The cascade: "Notes" had only that tag, so it has no Tags value now.
    expect(await valueOf(memory, "Notes", "Tags")).toBeUndefined();
    expect(await valueOf(memory, "Other", "Tags")).toBeDefined();
  });

  it("deletes an option from its edit form too", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const stage = await openDetails(user, dialog, "Stage");
    await user.click(within(stage).getByRole("button", { name: t("properties.option.edit", { name: "Review" }) }));
    await user.click(within(stage).getByRole("button", { name: t("properties.option.delete") }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("properties.option.deleteConfirm") }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.name === "Stage");
      expect(def?.options.map((option) => option.name)).toEqual(["Draft"]);
    });
  });

  it("follows what another writer changes while it is open", async () => {
    const { user, memory } = await renderProperties();
    const dialog = await openDialog(user);
    const { space } = await idsOf(memory);
    const client = new CoreClient(memory);
    const stage = (await schemaOf(memory)).find((def) => def.name === "Stage");
    await openDetails(user, dialog, "Stage");
    await client.propertyOptionAdd(space("Personal"), stage?.id ?? "", "Done", "green");
    await client.propertyRename(space("Personal"), stage?.id ?? "", "Phase");
    expect(await within(dialog).findByRole("button", { name: t("properties.manage.rename", { name: "Phase" }) })).toBeInTheDocument();
    expect(await within(dialog).findByRole("button", { name: t("properties.option.edit", { name: "Done" }) })).toBeInTheDocument();
  });
});
