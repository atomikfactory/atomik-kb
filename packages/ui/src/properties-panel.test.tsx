// The properties panel under the title of the open page (ADR-0026): which rows show and in what
// order, the editor of each kind (what it commits, how it reverts, what it refuses), adding a
// property to the page and creating one, hiding and deleting with their questions, moving and
// renaming, a page in the trash, and following changes made elsewhere.

import { CoreClient, MemoryTransport, TAGS_PROPERTY_ID } from "@atomik-kb/core-client";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PropertiesPanel } from "./components/PropertiesPanel";
import { fullDate } from "./editor/dates";
import { t, tp } from "./i18n";
import { formatNumber } from "./property-format";
import { createAppStore as createStore } from "./store";
import { StoreContext } from "./store-context";
import { idsOf } from "./test/page";
import {
  PROPERTY_SPACE,
  openProperties,
  optionIdOf,
  propertyRow,
  renderProperties,
  rowNames,
  schemaOf,
  valueOf,
} from "./test/properties";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("which rows show", () => {
  it("lists the properties that have a value in the order of the schema, Tags first", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    expect(rowNames(panel)).toEqual([
      "Tags",
      "Status",
      "Due",
      "Done",
      "Link",
      "Owner email",
      "Priority",
      "Summary",
      "Phone",
      "Stage",
      "Areas",
    ]);
    // Values are shown by their editors.
    expect(within(propertyRow(panel, "Tags")).getByRole("button", { name: t("properties.tag.search", { name: "idea" }) })).toBeInTheDocument();
    expect(within(propertyRow(panel, "Status")).getByText("In progress")).toBeInTheDocument();
    expect(within(propertyRow(panel, "Summary")).getByRole("textbox", { name: "Summary" })).toHaveValue("Draft");
    expect(within(propertyRow(panel, "Done")).getByRole("checkbox", { name: "Done" })).toBeChecked();
  });

  it("always shows Tags, with an invitation to add some, and hides the other properties that have no value", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    expect(rowNames(panel)).toEqual(["Tags"]);
    expect(within(propertyRow(panel, "Tags")).getByRole("button", { name: t("properties.tags.add") })).toBeInTheDocument();
    expect(within(panel).getByRole("button", { name: t("properties.add") })).toBeInTheDocument();
  });

  it("follows the pages: another page shows its own values", async () => {
    const { user } = await renderProperties();
    await openProperties(user, "Notes");
    const panel = await openProperties(user, "Other");
    expect(rowNames(panel)).toEqual(["Tags"]);
    expect(within(panel).getByRole("button", { name: t("properties.tag.search", { name: "project" }) })).toBeInTheDocument();
  });
});

describe("the editors", () => {
  it("text: Enter commits, leaving the field commits, Escape puts the old text back", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Summary" });

    await user.clear(field);
    await user.type(field, "Final{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Summary")).toEqual({ kind: "text", value: "Final" });
    });

    await user.clear(field);
    await user.type(field, "Changed my mind{Escape}");
    expect(field).toHaveValue("Final");
    expect(field).not.toHaveFocus();
    expect(await valueOf(memory, "Notes", "Summary")).toEqual({ kind: "text", value: "Final" });

    await user.clear(field);
    await user.type(field, "On blur");
    await user.tab();
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Summary")).toEqual({ kind: "text", value: "On blur" });
    });
  });

  it("text: Shift+Enter makes a new line and clearing it removes the value, and the row with it", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Summary" });
    await user.type(field, "{Shift>}{Enter}{/Shift}more{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Summary")).toEqual({ kind: "text", value: "Draft\nmore" });
    });
    await user.clear(field);
    await user.tab();
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Summary")).toBeUndefined();
    });
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Summary");
    });
  });

  it("number: reads any notation, shows the number formatted, refuses what is not a number", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Priority" });
    expect(field).toHaveValue("3");

    await user.clear(field);
    await user.type(field, "1.250,5{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Priority")).toEqual({ kind: "number", value: 1250.5 });
    });
    await user.tab();
    expect(field).toHaveValue(formatNumber(1250.5));
    await user.click(field);
    expect(field).toHaveValue("1250.5");

    await user.clear(field);
    await user.type(field, "twelve{Enter}");
    expect(await within(panel).findByText(t("properties.problem.number"))).toBeInTheDocument();
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(await valueOf(memory, "Notes", "Priority")).toEqual({ kind: "number", value: 1250.5 });

    await user.type(field, "{Escape}");
    expect(field).toHaveValue(formatNumber(1250.5));
    expect(within(panel).queryByText(t("properties.problem.number"))).not.toBeInTheDocument();
  });

  it("checkbox: a click commits at once", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    await user.click(within(panel).getByRole("checkbox", { name: "Done" }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Done")).toEqual({ kind: "checkbox", value: false });
    });
    await user.click(within(panel).getByRole("checkbox", { name: "Done" }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Done")).toEqual({ kind: "checkbox", value: true });
    });
  });

  it("date: picks a day in the calendar, Escape drops a change, Clear removes the value", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const chip = within(propertyRow(panel, "Due")).getByRole("button", { name: /^Due: / });

    await user.click(chip);
    const picker = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    await user.click(within(picker).getByRole("button", { name: fullDate("2026-10-20") }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Due")).toEqual({ kind: "date", value: { date: "2026-10-20" } });
    });
    expect(screen.queryByRole("dialog", { name: t("date.pickerTitle") })).not.toBeInTheDocument();

    // A typed day applies to the picker, and is dropped when the picker closes with Escape.
    await user.click(within(propertyRow(panel, "Due")).getByRole("button", { name: /^Due: / }));
    const again = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    const typed = within(again).getByRole("textbox", { name: t("date.field") });
    await user.clear(typed);
    await user.type(typed, "2026-11-01");
    await user.keyboard("{Escape}");
    expect(await valueOf(memory, "Notes", "Due")).toEqual({ kind: "date", value: { date: "2026-10-20" } });

    // Enter in the field keeps it, with a time.
    await user.click(within(propertyRow(panel, "Due")).getByRole("button", { name: /^Due: / }));
    const third = await screen.findByRole("dialog", { name: t("date.pickerTitle") });
    const field = within(third).getByRole("textbox", { name: t("date.field") });
    await user.clear(field);
    await user.type(field, "2026-11-01{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Due")).toEqual({ kind: "date", value: { date: "2026-11-01" } });
    });

    await user.click(within(propertyRow(panel, "Due")).getByRole("button", { name: t("properties.date.clear", { name: "Due" }) }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Due")).toBeUndefined();
    });
  });

  it("url: refuses a text that is not a link, commits a link and opens it through the shell", async () => {
    const openExternal = vi.fn(() => Promise.resolve());
    const { user, memory } = await renderProperties([PROPERTY_SPACE], { openExternal });
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Link" });

    await user.clear(field);
    await user.type(field, "not a link{Enter}");
    expect(await within(panel).findByText(t("properties.problem.url"))).toBeInTheDocument();
    expect(await valueOf(memory, "Notes", "Link")).toEqual({ kind: "url", value: "https://example.com" });

    await user.clear(field);
    await user.type(field, "https://atomik.dev/kb{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Link")).toEqual({ kind: "url", value: "https://atomik.dev/kb" });
    });
    await user.click(within(propertyRow(panel, "Link")).getByRole("button", { name: t("properties.open", { name: "Link" }) }));
    await waitFor(() => {
      expect(openExternal).toHaveBeenCalledWith("https://atomik.dev/kb");
    });
  });

  it("email: checks the address, commits it and opens a mailto link", async () => {
    const openExternal = vi.fn(() => Promise.resolve());
    const { user, memory } = await renderProperties([PROPERTY_SPACE], { openExternal });
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Owner email" });

    await user.clear(field);
    await user.type(field, "ada@{Enter}");
    expect(await within(panel).findByText(t("properties.problem.email"))).toBeInTheDocument();

    await user.clear(field);
    await user.type(field, "  grace@example.org {Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Owner email")).toEqual({ kind: "email", value: "grace@example.org" });
    });
    await user.click(within(propertyRow(panel, "Owner email")).getByRole("button", { name: t("properties.open", { name: "Owner email" }) }));
    await waitFor(() => {
      expect(openExternal).toHaveBeenCalledWith("mailto:grace@example.org");
    });
  });

  it("phone: checks the number and commits it as typed", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Phone" });
    await user.clear(field);
    await user.type(field, "12{Enter}");
    expect(await within(panel).findByText(t("properties.problem.phone"))).toBeInTheDocument();
    await user.clear(field);
    await user.type(field, "(020) 7946 0958{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Phone")).toEqual({ kind: "phone", value: "(020) 7946 0958" });
    });
    // A phone number has no "open" action: the shell hands only http, https and mailto to the system.
    expect(within(propertyRow(panel, "Phone")).queryByRole("button", { name: t("properties.open", { name: "Phone" }) })).not.toBeInTheDocument();
  });
});

describe("Escape", () => {
  it("puts back what every text field held, and nothing is sent", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    for (const [name, old, typed] of [
      ["Link", "https://example.com", "https://other.example"],
      ["Owner email", "ada@example.com", "grace@example.org"],
      ["Phone", "+1 555 0100", "020 7946 0958"],
      ["Summary", "Draft", "Something else"],
    ] as const) {
      const field = within(panel).getByRole("textbox", { name });
      await user.clear(field);
      await user.type(field, `${typed}{Escape}`);
      expect(field).toHaveValue(old);
      expect(await valueOf(memory, "Notes", name)).toMatchObject({ value: old });
    }
  });

  it("also clears a message and a failed attempt", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const field = within(panel).getByRole("textbox", { name: "Phone" });
    await user.clear(field);
    await user.type(field, "1{Enter}");
    expect(await within(panel).findByText(t("properties.problem.phone"))).toBeInTheDocument();
    await user.type(field, "{Escape}");
    expect(within(panel).queryByText(t("properties.problem.phone"))).not.toBeInTheDocument();
    expect(field).toHaveValue("+1 555 0100");
  });
});

describe("where the panel is", () => {
  it("sits between the title and the text of the page", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const title = screen.getByRole("heading", { level: 1, name: "Notes" });
    const editor = await screen.findByRole("textbox", { name: t("editor.label") });
    expect(title.compareDocumentPosition(panel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panel.compareDocumentPosition(editor) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("is not shown without a page, and a second space has its own schema", async () => {
    const { user } = await renderProperties([
      PROPERTY_SPACE,
      { name: "Work", properties: [{ name: "Client", kind: "text" }], pages: [{ title: "Meetings", properties: { Client: "ACME" } }] },
    ]);
    expect(screen.queryByRole("region", { name: t("properties.title") })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Personal/ }));
    await user.click(await screen.findByRole("button", { name: /Work/ }));
    const panel = await openProperties(user, "Meetings");
    expect(rowNames(panel)).toEqual(["Tags", "Client"]);
    expect(within(panel).getByRole("textbox", { name: "Client" })).toHaveValue("ACME");
  });
});

describe("options", () => {
  it("status: the options are grouped under To-do, In progress and Done, and a pick commits", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    await user.click(within(propertyRow(panel, "Status")).getByRole("button", { name: t("properties.value.label", { name: "Status" }) }));
    const list = await screen.findByRole("listbox", { name: "Status" });
    for (const [group, option] of [
      ["To-do", "Not started"],
      ["In progress", "In progress"],
      ["Done", "Done"],
    ] as const) {
      expect(within(within(list).getByRole("group", { name: group })).getByRole("option", { name: option })).toBeInTheDocument();
    }
    expect(within(list).getByRole("option", { name: "In progress" })).toHaveAttribute("aria-selected", "true");

    await user.click(within(list).getByRole("option", { name: "Done" }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Status")).toEqual({ kind: "status", value: await optionIdOf(memory, "Status", "Done") });
    });
    expect(screen.queryByRole("listbox", { name: "Status" })).not.toBeInTheDocument();
    expect(within(propertyRow(panel, "Status")).getByText("Done")).toBeInTheDocument();
  });

  it("select: typing filters, Create adds the option with the next colour and picks it, Clear removes the value", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    await user.click(within(propertyRow(panel, "Stage")).getByRole("button", { name: t("properties.value.label", { name: "Stage" }) }));
    const filter = await screen.findByRole("combobox", { name: t("properties.option.filter") });
    await user.type(filter, "rev");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["Review", t("properties.option.create", { name: "rev" })]);

    await user.clear(filter);
    await user.type(filter, "Shipped");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([t("properties.option.create", { name: "Shipped" })]);
    await user.keyboard("{Enter}");
    await waitFor(async () => {
      const stage = (await schemaOf(memory)).find((def) => def.name === "Stage");
      expect(stage?.options.map((option) => [option.name, option.color])).toEqual([
        ["Draft", "gray"],
        ["Review", "orange"],
        ["Shipped", "yellow"], // the colour after Review's in the palette
      ]);
      expect(await valueOf(memory, "Notes", "Stage")).toEqual({ kind: "select", value: stage?.options[2]?.id });
    });

    await user.click(within(propertyRow(panel, "Stage")).getByRole("button", { name: t("properties.value.label", { name: "Stage" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.clear") }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Stage")).toBeUndefined();
    });
  });

  it("multi-select: chips with remove buttons, and a picker that toggles options and stays open", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const areas = propertyRow(panel, "Areas");
    expect(within(areas).getByText("Home")).toBeInTheDocument();

    await user.click(within(areas).getByRole("button", { name: t("properties.option.add", { name: "Areas" }) }));
    const list = await screen.findByRole("listbox", { name: "Areas" });
    expect(list).toHaveAttribute("aria-multiselectable", "true");
    await user.click(within(list).getByRole("option", { name: "Work" }));
    await user.type(screen.getByRole("combobox", { name: t("properties.option.filter") }), "Garden{Enter}");
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.name === "Areas");
      expect(def?.options.map((option) => option.name)).toEqual(["Home", "Work", "Garden"]);
      expect(await valueOf(memory, "Notes", "Areas")).toEqual({
        kind: "multi_select",
        value: def?.options.map((option) => option.id),
      });
    });
    // Still open: toggling Home off.
    expect(screen.getByRole("listbox", { name: "Areas" })).toBeInTheDocument();
    await user.click(within(screen.getByRole("listbox", { name: "Areas" })).getByRole("option", { name: "Home" }));
    await waitFor(async () => {
      expect(((await valueOf(memory, "Notes", "Areas")) as { value: string[] }).value).toHaveLength(2);
    });
    await user.keyboard("{Escape}");

    await user.click(within(areas).getByRole("button", { name: t("properties.option.remove", { name: "Work" }) }));
    await user.click(within(areas).getByRole("button", { name: t("properties.option.remove", { name: "Garden" }) }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Areas")).toBeUndefined();
    });
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Areas");
    });
  });

  it("Tags: add and remove tags, and create a tag", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    const tags = propertyRow(panel, "Tags");
    await user.click(within(tags).getByRole("button", { name: t("properties.tags.add") }));
    const list = await screen.findByRole("listbox", { name: "Tags" });
    await user.click(within(list).getByRole("option", { name: "idea" }));
    await user.type(screen.getByRole("combobox", { name: t("properties.option.filter") }), "reading{Enter}");
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.id === TAGS_PROPERTY_ID);
      expect(def?.options.map((option) => option.name)).toEqual(["idea", "project", "reading"]);
      expect(await valueOf(memory, "Plain", "Tags")).toEqual({
        kind: "multi_select",
        value: [def?.options[0]?.id, def?.options[2]?.id],
      });
    });
    await user.keyboard("{Escape}");
    await user.click(within(tags).getByRole("button", { name: t("properties.option.remove", { name: "idea" }) }));
    await waitFor(async () => {
      const def = (await schemaOf(memory)).find((candidate) => candidate.id === TAGS_PROPERTY_ID);
      expect(await valueOf(memory, "Plain", "Tags")).toEqual({ kind: "multi_select", value: [def?.options[2]?.id] });
    });
  });

  it("the pencil of an option edits its name, colour and group", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    await user.click(within(propertyRow(panel, "Status")).getByRole("button", { name: t("properties.value.label", { name: "Status" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.edit", { name: "In progress" }) }));
    const form = await screen.findByRole("form", { name: t("properties.option.editTitle", { name: "In progress" }) });
    const name = within(form).getByRole("textbox", { name: t("properties.option.name") });
    await user.clear(name);
    await user.type(name, "Doing");
    await user.click(within(form).getByRole("button", { name: t("callout.color.red") }));
    await user.selectOptions(within(form).getByRole("combobox", { name: t("properties.option.group") }), "done");
    await user.click(within(form).getByRole("button", { name: t("properties.option.save") }));
    await waitFor(async () => {
      const status = (await schemaOf(memory)).find((def) => def.name === "Status");
      expect(status?.options[1]).toMatchObject({ name: "Doing", color: "red", group: "done" });
    });
    // Back to the list, which now shows it under Done.
    const list = await screen.findByRole("listbox", { name: "Status" });
    expect(within(within(list).getByRole("group", { name: "Done" })).getByRole("option", { name: "Doing" })).toBeInTheDocument();
  });
});

describe("deleting an option from the picker", () => {
  it("asks first, then takes the option out of the property and out of the pages that have it", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    await user.click(within(propertyRow(panel, "Stage")).getByRole("button", { name: t("properties.value.label", { name: "Stage" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.edit", { name: "Draft" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.delete") }));
    // The picker closed to make room for the question.
    expect(screen.queryByRole("listbox", { name: "Stage" })).not.toBeInTheDocument();
    const question = await screen.findByRole("alertdialog", { name: t("properties.option.deleteTitle", { name: "Draft" }) });
    expect(
      await within(question).findByText(tp("properties.option.deleteBodyCount", 1, { name: "Draft", property: "Stage" })),
    ).toBeInTheDocument();

    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    expect(await valueOf(memory, "Notes", "Stage")).toBeDefined();

    await user.click(within(propertyRow(panel, "Stage")).getByRole("button", { name: t("properties.value.label", { name: "Stage" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.edit", { name: "Draft" }) }));
    await user.click(await screen.findByRole("button", { name: t("properties.option.delete") }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("properties.option.deleteConfirm") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).find((def) => def.name === "Stage")?.options.map((option) => option.name)).toEqual(["Review"]);
    });
    // The page lost its value, so the row is gone.
    expect(await valueOf(memory, "Notes", "Stage")).toBeUndefined();
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Stage");
    });
  });
});

describe("adding a property to the page", () => {
  it("lists the properties without a value, and a checkbox starts unchecked", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    const menu = await screen.findByRole("menu", { name: t("properties.add") });
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "StatusStatus",
      "DueDate",
      "DoneCheckbox",
      "LinkURL",
      "Owner emailEmail",
      "PriorityNumber",
      "SummaryText",
      "PhonePhone",
      "StageSelect",
      "AreasMulti-select",
      t("properties.new.menu"),
    ]);
    await user.click(within(menu).getByRole("menuitem", { name: /^Done/ }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Plain", "Done")).toEqual({ kind: "checkbox", value: false });
    });
    expect(await within(panel).findByRole("checkbox", { name: "Done" })).not.toBeChecked();
    expect(rowNames(panel)).toEqual(["Tags", "Done"]);
  });

  it("shows the other kinds as an empty row with the editor ready, and keeps it until a value is given", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: /^Summary/ }));
    const field = await within(panel).findByRole("textbox", { name: "Summary" });
    expect(field).toHaveValue("");
    expect(field).toHaveFocus();
    await user.type(field, "First words{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Plain", "Summary")).toEqual({ kind: "text", value: "First words" });
    });
    expect(rowNames(panel)).toEqual(["Tags", "Summary"]);
  });

  it("New property... creates a property in the space and offers a value", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.new.menu") }));

    const dialog = await screen.findByRole("dialog", { name: t("properties.new.title") });
    // Every kind, each with its description; Text is the default.
    const kinds = within(dialog).getAllByRole("radio");
    expect(kinds.map((kind) => kind.getAttribute("aria-label"))).toEqual([
      "Text",
      "Number",
      "Select",
      "Multi-select",
      "Date",
      "Checkbox",
      "URL",
      "Status",
      "Email",
      "Phone",
      "Files and media",
      "Checklist",
      "Place",
      "Person",
      "Relation",
      "Rollup",
      "Created time",
      "Last edited time",
      "Created by",
      "Last edited by",
      "Formula",
    ]);
    expect(within(dialog).getByRole("radio", { name: "Text" })).toBeChecked();
    expect(within(dialog).getByText(t("properties.kind.status.hint"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("properties.kind.files.hint"))).toBeInTheDocument();
    expect(within(dialog).getByText(t("properties.kind.checklist.hint"))).toBeInTheDocument();

    // A name is needed.
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    expect(await within(dialog).findByText(t("properties.error.nameEmpty"))).toBeInTheDocument();

    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "Rating");
    await user.click(within(dialog).getByRole("radio", { name: "Number" }));
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    expect(screen.queryByRole("dialog", { name: t("properties.new.title") })).not.toBeInTheDocument();

    const field = await within(panel).findByRole("textbox", { name: "Rating" });
    await waitFor(() => {
      expect(field).toHaveFocus();
    });
    expect((await schemaOf(memory)).at(-1)).toMatchObject({ name: "Rating", kind: "number" });
    await user.type(field, "4.5{Enter}");
    await waitFor(async () => {
      expect(await valueOf(memory, "Plain", "Rating")).toEqual({ kind: "number", value: 4.5 });
    });
  });

  it("shows the core's refusal of a name in the dialog", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.new.menu") }));
    const dialog = await screen.findByRole("dialog", { name: t("properties.new.title") });
    await user.type(within(dialog).getByRole("textbox", { name: t("properties.new.name") }), "x".repeat(101));
    await user.click(within(dialog).getByRole("button", { name: t("properties.new.create") }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(/^Name /);
    expect(screen.getByRole("dialog", { name: t("properties.new.title") })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.cancel") }));
    expect(screen.queryByRole("dialog", { name: t("properties.new.title") })).not.toBeInTheDocument();
  });
});

describe("the menu of a property", () => {
  async function openMenu(user: Awaited<ReturnType<typeof renderProperties>>["user"], panel: HTMLElement, name: string) {
    await user.click(within(propertyRow(panel, name)).getByRole("button", { name }));
    return screen.findByRole("menu", { name: t("properties.menu.label", { name }) });
  }

  it("renames the property", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Priority");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.rename") }));
    const input = await within(panel).findByRole("textbox", { name: t("properties.rename.label") });
    await user.clear(input);
    await user.type(input, "Urgency{Enter}");
    await waitFor(async () => {
      expect((await schemaOf(memory)).map((def) => def.name)).toContain("Urgency");
    });
    expect(await within(panel).findByRole("textbox", { name: "Urgency" })).toBeInTheDocument();
    expect(rowNames(panel)).toContain("Urgency");
    expect(rowNames(panel)).not.toContain("Priority");
    // The name gives focus back to its button, which opens the menu again.
    expect(within(propertyRow(panel, "Urgency")).getByRole("button", { name: "Urgency" })).toHaveFocus();
  });

  it("keeps the old name when the rename is cancelled or the core refuses it", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    let menu = await openMenu(user, panel, "Priority");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.rename") }));
    await user.type(await within(panel).findByRole("textbox", { name: t("properties.rename.label") }), "Nope{Escape}");
    expect(rowNames(panel)).toContain("Priority");

    menu = await openMenu(user, panel, "Priority");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.rename") }));
    await user.type(await within(panel).findByRole("textbox", { name: t("properties.rename.label") }), "x".repeat(101));
    await user.keyboard("{Enter}");
    expect(await within(panel).findByRole("alert")).toHaveTextContent(/could not be renamed/);
    expect((await schemaOf(memory)).map((def) => def.name)).toContain("Priority");
  });

  it("hides a property after asking, which clears its value", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Link");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.hide") }));
    const question = await screen.findByRole("alertdialog", { name: t("properties.hide.title", { name: "Link" }) });
    // Cancelling keeps everything.
    await user.click(within(question).getByRole("button", { name: t("dialog.cancel") }));
    expect(await valueOf(memory, "Notes", "Link")).toBeDefined();

    await user.click(within(propertyRow(panel, "Link")).getByRole("button", { name: "Link" }));
    await user.click(await screen.findByRole("menuitem", { name: t("properties.menu.hide") }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: t("properties.hide.confirm") }));
    await waitFor(async () => {
      expect(await valueOf(memory, "Notes", "Link")).toBeUndefined();
    });
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Link");
    });
    // It can be added again, and the property is still in the schema.
    expect((await schemaOf(memory)).map((def) => def.name)).toContain("Link");
  });

  it("hides a property that has no value yet without asking", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Plain");
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    await user.click(await screen.findByRole("menuitem", { name: /^Summary/ }));
    await within(panel).findByRole("textbox", { name: "Summary" });
    const menu = await openMenu(user, panel, "Summary");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.hide") }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(rowNames(panel)).toEqual(["Tags"]);
  });

  it("deletes a property after asking, with the number of pages that have a value", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Priority");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.delete") }));
    const question = await screen.findByRole("alertdialog", { name: t("properties.delete.title", { name: "Priority" }) });
    expect(await within(question).findByText(tp("properties.delete.bodyCount", 1, { name: "Priority" }))).toBeInTheDocument();
    await user.click(within(question).getByRole("button", { name: t("properties.delete.confirm") }));
    await waitFor(async () => {
      expect((await schemaOf(memory)).map((def) => def.name)).not.toContain("Priority");
    });
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Priority");
    });
  });

  it("does not delete or hide Tags, and does not move it", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Tags");
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.delete") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.hide") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.up") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.down") })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.rename") })).toBeEnabled();
  });

  it("moves a property past the next row that shows, and up again", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const names = async () => (await schemaOf(memory)).map((def) => def.name);

    let menu = await openMenu(user, panel, "Status");
    expect(within(menu).getByRole("menuitem", { name: t("properties.menu.up") })).toBeDisabled(); // next to Tags
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.down") }));
    await waitFor(async () => {
      expect((await names()).slice(0, 4)).toEqual(["Tags", "Due", "Status", "Done"]);
    });
    await waitFor(() => {
      expect(rowNames(panel).slice(0, 3)).toEqual(["Tags", "Due", "Status"]);
    });

    menu = await openMenu(user, panel, "Status");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.up") }));
    await waitFor(async () => {
      expect((await names()).slice(0, 4)).toEqual(["Tags", "Status", "Due", "Done"]);
    });
  });

  it("opens the options of the property in the properties dialog", async () => {
    const { user } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const menu = await openMenu(user, panel, "Stage");
    await user.click(within(menu).getByRole("menuitem", { name: t("properties.menu.options") }));
    const dialog = await screen.findByRole("dialog", { name: t("properties.dialog.title") });
    expect(within(dialog).getByRole("button", { name: t("properties.option.edit", { name: "Review" }) })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: t("dialog.close") }));
    expect(screen.getByRole("button", { name: "Stage" })).toHaveFocus();
  });
});

describe("a page in the trash", () => {
  it("shows its values and offers no way to change them", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PROPERTY_SPACE] });
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    await client.pageTrash(page("Notes"));
    const info = await client.pageGet(page("Notes"));
    expect(info.trashedAt).toBeDefined();

    const store = createStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    render(
      <StoreContext value={store}>
        <PropertiesPanel page={info} />
      </StoreContext>,
    );
    const panel = await screen.findByRole("region", { name: t("properties.title") });
    expect(rowNames(panel)).toEqual(["Tags", "Status", "Due", "Done", "Link", "Owner email", "Priority", "Summary", "Phone", "Stage", "Areas"]);
    expect(within(panel).getByText("Draft", { selector: ".properties-text" })).toBeInTheDocument();
    expect(within(panel).getByText("In progress")).toBeInTheDocument();
    expect(within(panel).getByText("Home")).toBeInTheDocument();
    expect(within(panel).queryByRole("textbox")).not.toBeInTheDocument();
    expect(within(panel).getByRole("checkbox", { name: "Done" })).toBeDisabled();
    expect(within(panel).queryByRole("button", { name: t("properties.add") })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Status" })).not.toBeInTheDocument(); // the name is plain text
    expect(within(panel).queryByRole("button", { name: t("properties.option.remove", { name: "Home" }) })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: /^Due: / })).not.toBeInTheDocument();
    stop();
  });

  it("shows nothing for a trashed page that has no values", async () => {
    const memory = new MemoryTransport({ workspaceName: "Research", spaces: [PROPERTY_SPACE] });
    const client = new CoreClient(memory);
    const { page } = await idsOf(memory);
    await client.pageTrash(page("Plain"));
    const info = await client.pageGet(page("Plain"));
    const store = createStore(client, { corePollMs: 5 });
    const stop = store.getState().start();
    const view = render(
      <StoreContext value={store}>
        <PropertiesPanel page={info} />
      </StoreContext>,
    );
    await waitFor(() => {
      expect(store.getState().phase).toBe("ready");
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(view.container).toBeEmptyDOMElement();
    stop();
  });
});

describe("changes made elsewhere", () => {
  it("follows a renamed property and a value another writer set", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const client = new CoreClient(memory);
    const { page, space } = await idsOf(memory);
    const priority = (await schemaOf(memory)).find((def) => def.name === "Priority");

    await act(async () => {
      await client.propertyRename(space("Personal"), priority?.id ?? "", "Urgency");
    });
    expect(await within(panel).findByRole("textbox", { name: "Urgency" })).toHaveValue("3");

    await act(async () => {
      await client.pagePropertySet(page("Notes"), priority?.id ?? "", { kind: "number", value: 9 });
    });
    await waitFor(() => {
      expect(within(panel).getByRole("textbox", { name: "Urgency" })).toHaveValue("9");
    });
  });

  it("shows a property another writer created in the add menu, and drops a deleted one's row", async () => {
    const { user, memory } = await renderProperties();
    const panel = await openProperties(user, "Notes");
    const client = new CoreClient(memory);
    const { space } = await idsOf(memory);
    const link = (await schemaOf(memory)).find((def) => def.name === "Link");
    await act(async () => {
      await client.propertyCreate(space("Personal"), "Mood", "select");
      await client.propertyDelete(space("Personal"), link?.id ?? "");
    });
    await waitFor(() => {
      expect(rowNames(panel)).not.toContain("Link");
    });
    await user.click(within(panel).getByRole("button", { name: t("properties.add") }));
    expect(await screen.findByRole("menuitem", { name: /^Mood/ })).toBeInTheDocument();
    fireEvent.keyDown(document.body, { key: "Escape" });
  });
});
